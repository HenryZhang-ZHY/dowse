//! Keeping searches fresh between index builds.
//!
//! The on-disk index only knows the files as they were when it was built. A
//! [`ChangeTracker`] watches the folder and remembers every file created,
//! modified or deleted since then; searches read those files directly on top
//! of the index's candidates, so edits show up without re-indexing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use tgrep_core::gitignore::{self, IgnoreMatcher};
use tgrep_core::walker::{self, WalkOptions};

pub struct ChangeTracker {
    state: Arc<State>,
    _watcher: RecommendedWatcher,
}

struct State {
    root: PathBuf,
    /// Changed path -> its latest change.
    changed: Mutex<BTreeMap<String, Stamp>>,
    epoch: AtomicU64,
    /// Numbers every change, so a newer change of a file is told apart.
    version: AtomicU64,
    /// Built on a background thread at start; events wait for it.
    ignore: OnceLock<Option<IgnoreMatcher>>,
}

/// A point in the change history, taken when an index build starts.
#[derive(Clone, Copy, Debug)]
pub struct ChangeMark(u64);

/// A file changed since the last completed index build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Repository-relative, `/`-separated.
    pub path: String,
    /// Grows with every change recorded, so a file whose version moved on
    /// has changed again.
    pub version: u64,
}

#[derive(Clone, Copy, Debug)]
struct Stamp {
    /// The build epoch the change was recorded in.
    epoch: u64,
    version: u64,
}

impl ChangeTracker {
    /// Watch `root`, which must be canonical (as [`super::index::RepoIndex`] keeps it).
    pub fn start(root: &Path) -> notify::Result<Self> {
        let state = Arc::new(State {
            root: root.to_path_buf(),
            changed: Mutex::new(BTreeMap::new()),
            epoch: AtomicU64::new(0),
            version: AtomicU64::new(0),
            ignore: OnceLock::new(),
        });
        let handler_state = state.clone();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            if let Ok(event) = event {
                handler_state.record(&event);
            }
        })?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        let ignore_state = state.clone();
        std::thread::spawn(move || {
            ignore_state.ignore_matcher();
        });
        Ok(Self {
            state,
            _watcher: watcher,
        })
    }

    /// The files changed since the last completed index build, by path.
    pub fn changes(&self) -> Vec<Change> {
        self.state
            .changed
            .lock()
            .unwrap()
            .iter()
            .map(|(path, stamp)| Change {
                path: path.clone(),
                version: stamp.version,
            })
            .collect()
    }

    /// Count these repository-relative paths as changed, as if the watcher
    /// had seen them: files that changed while nothing watched.
    pub fn note(&self, paths: Vec<String>) {
        self.state.insert(paths);
    }

    pub fn changed_count(&self) -> usize {
        self.state.changed.lock().unwrap().len()
    }

    /// Call before building the index; pass the mark to [`Self::forget_before`]
    /// once the build succeeds.
    pub fn mark(&self) -> ChangeMark {
        ChangeMark(self.state.epoch.fetch_add(1, Ordering::SeqCst))
    }

    /// Drop changes the new index already covers: those recorded before `mark`.
    /// Changes made while the build ran are kept.
    pub fn forget_before(&self, mark: ChangeMark) {
        self.state
            .changed
            .lock()
            .unwrap()
            .retain(|_, stamp| stamp.epoch > mark.0);
    }
}

impl State {
    fn ignore_matcher(&self) -> Option<&IgnoreMatcher> {
        self.ignore
            .get_or_init(|| gitignore::build_matcher(&self.root))
            .as_ref()
    }

    fn record(&self, event: &Event) {
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        // Only a directory that appeared, or was renamed in, brings files of
        // its own. Other events on a directory, such as Windows reporting one
        // whose entries changed, would otherwise mark all of its files.
        let arrived = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
        );
        let mut found = Vec::new();
        for path in &event.paths {
            if path.is_dir() {
                if arrived {
                    let walk = walker::walk_dir(path, &WalkOptions::default());
                    found.extend(walk.files.iter().filter_map(|file| self.accept(file)));
                }
            } else if let Some(relative) = self.accept(path) {
                found.push(relative);
            }
        }
        self.insert(found);
    }

    fn insert(&self, paths: Vec<String>) {
        if paths.is_empty() {
            return;
        }
        // A change recorded before `mark()` carries that mark's epoch and is
        // forgotten once the build finishes; the build started after it.
        let epoch = self.epoch.load(Ordering::SeqCst);
        let mut changed = self.changed.lock().unwrap();
        for path in paths {
            let version = self.version.fetch_add(1, Ordering::SeqCst);
            changed.insert(path, Stamp { epoch, version });
        }
    }

    /// The repository-relative path of a file a default search would read, or
    /// `None` for hidden, ignored and binary files.
    fn accept(&self, path: &Path) -> Option<String> {
        let relative = relative_path(&self.root, path)?;
        if relative.split('/').any(|part| part.starts_with('.')) {
            return None;
        }
        if walker::is_binary_extension(path) {
            return None;
        }
        if self
            .ignore_matcher()
            .is_some_and(|matcher| matcher.is_ignored(Path::new(&relative), false))
        {
            return None;
        }
        Some(relative)
    }
}

fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let text = relative.to_string_lossy().replace('\\', "/");
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn changed_paths(tracker: &ChangeTracker) -> Vec<String> {
        tracker.changes().into_iter().map(|c| c.path).collect()
    }

    fn wait_for(tracker: &ChangeTracker, expected: &[&str]) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let paths = changed_paths(tracker);
            if expected.iter().all(|path| paths.iter().any(|p| p == path))
                || Instant::now() > deadline
            {
                return paths;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn noted_changes_count_until_the_next_build_covers_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let tracker = ChangeTracker::start(&root).unwrap();
        tracker.note(vec!["src/a.rs".into(), "b.md".into()]);
        assert_eq!(changed_paths(&tracker), ["b.md", "src/a.rs"]);
        let mark = tracker.mark();
        tracker.forget_before(mark);
        assert_eq!(tracker.changed_count(), 0);
    }

    #[test]
    fn each_change_of_a_file_gets_a_newer_version() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let tracker = ChangeTracker::start(&root).unwrap();
        tracker.note(vec!["b.md".into(), "a.rs".into()]);
        let first = tracker.changes();
        assert_eq!(
            first.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(),
            ["a.rs", "b.md"]
        );
        tracker.note(vec!["a.rs".into()]);
        let second = tracker.changes();
        assert!(second[0].version > first[0].version);
        assert_eq!(second[1], first[1]);
    }

    #[test]
    fn a_change_inside_a_directory_does_not_mark_its_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/old.rs"), "fn old() {}\n").unwrap();
        let tracker = ChangeTracker::start(&root).unwrap();
        tracker.state.ignore_matcher();
        std::thread::sleep(Duration::from_millis(200));

        std::fs::write(root.join("src/new.rs"), "fn new() {}\n").unwrap();
        std::fs::create_dir_all(root.join("moved/deep")).unwrap();
        std::fs::write(root.join("moved/deep/a.rs"), "fn a() {}\n").unwrap();
        wait_for(&tracker, &["src/new.rs", "moved/deep/a.rs"]);
        // Give the directories' own events time to arrive too.
        std::thread::sleep(Duration::from_millis(300));
        let paths = changed_paths(&tracker);
        assert!(paths.contains(&"src/new.rs".to_string()), "{paths:?}");
        assert!(paths.contains(&"moved/deep/a.rs".to_string()), "{paths:?}");
        assert!(!paths.contains(&"src/old.rs".to_string()), "{paths:?}");
    }

    #[test]
    fn records_changes_but_not_hidden_ignored_or_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        let tracker = ChangeTracker::start(&root).unwrap();
        // Let the ignore matcher and the watcher settle before changing files.
        tracker.state.ignore_matcher();
        std::thread::sleep(Duration::from_millis(200));

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/new.rs"), "fn fresh() {}\n").unwrap();
        std::fs::write(root.join("debug.log"), "noise\n").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(root.join("image.png"), [0u8, 1, 2]).unwrap();

        let paths = wait_for(&tracker, &["src/new.rs"]);
        assert!(paths.contains(&"src/new.rs".to_string()), "{paths:?}");
        assert!(
            !paths
                .iter()
                .any(|p| p == "debug.log" || p == ".env" || p == "image.png"),
            "{paths:?}"
        );

        let mark = tracker.mark();
        tracker.forget_before(mark);
        assert_eq!(tracker.changed_count(), 0);
    }
}
