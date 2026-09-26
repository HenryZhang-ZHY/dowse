//! Repositories shared by every window: the library of names and tags, their
//! indexes and change trackers, and the build queue.
//!
//! A repository is opened when the first window starts using it and closed
//! when the last one lets go, so two windows over the same repository share
//! one index, one file watcher and one build.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::*;

use crate::format;
use dowse::engine::index::{Corpus, IndexStatus, RepoIndex};
use dowse::engine::library::{Library, LibraryEntry};
use dowse::engine::repo::{self, RepoInfo};
use dowse::engine::search::SearchSource;
use dowse::engine::watch::ChangeTracker;

/// How often change counts and checked-out branches are refreshed.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Past this many changed files, reading them all on every search costs more
/// than re-indexing, so the index is rebuilt in the background.
const AUTO_REINDEX_CHANGES: usize = 2_000;
/// Files modified this long before an index was finished may have changed
/// after the build read them.
const STALE_SLACK: Duration = Duration::from_secs(10);

pub(super) struct RepoHub {
    library: Library,
    library_file: PathBuf,
    /// Set when the library could not be read, so it is never overwritten.
    library_locked: bool,
    /// Repositories some window uses, by id.
    open: HashMap<String, RepoState>,
    /// Repositories whose builds go first: those the focused window searches.
    preferred: HashSet<String>,
    _poll_task: Task<()>,
}

/// What windows react to. Everything else they learn by observing the hub.
#[derive(Clone, Debug)]
pub(super) enum HubEvent {
    /// The repository's index is about to be replaced: stop searches reading
    /// it, since Windows refuses to replace memory-mapped files.
    ReleaseCorpus(String),
    /// The repository's files were loaded or its index replaced: searches
    /// over it should run again.
    CorpusChanged(String),
    /// Tags or checked-out branches changed, which can move repositories in or
    /// out of a scope.
    MetadataChanged,
}

impl EventEmitter<HubEvent> for RepoHub {}

struct GlobalHub(Entity<RepoHub>);

impl Global for GlobalHub {}

/// One open repository and what its index is doing.
struct RepoState {
    info: Arc<RepoInfo>,
    /// `None` when the folder no longer exists.
    index: Option<RepoIndex>,
    corpus: Option<Arc<Corpus>>,
    activity: IndexActivity,
    tracker: Option<Arc<ChangeTracker>>,
    changed_files: usize,
    /// How many windows use the repository.
    users: usize,
    load_task: Option<Task<()>>,
    index_task: Option<Task<()>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum IndexActivity {
    /// Reading the index, or walking the folder when there is none.
    Loading,
    /// Waiting for another repository's build to finish.
    Queued,
    Building,
    Idle(IndexStatus),
    Failed(String),
    /// The folder is gone; it stays registered until removed.
    Missing,
}

impl IndexActivity {
    pub(super) fn is_busy(&self) -> bool {
        matches!(self, Self::Loading | Self::Queued | Self::Building)
    }
}

/// A snapshot of an open repository, for rendering.
#[derive(Clone, Debug)]
pub(super) struct RepoView {
    pub(super) info: Arc<RepoInfo>,
    pub(super) activity: IndexActivity,
    pub(super) changed_files: usize,
}

impl RepoView {
    pub(super) fn is_busy(&self) -> bool {
        self.activity.is_busy()
    }

    /// One-line description of the index, for the status bar and the
    /// repositories page.
    pub(super) fn index_summary(&self) -> String {
        let changed = match self.changed_files {
            0 => String::new(),
            n => format!(" · {} changed", format::plural(n, "file", "files")),
        };
        match &self.activity {
            IndexActivity::Loading => "Loading index…".into(),
            IndexActivity::Queued => "Waiting to index…".into(),
            IndexActivity::Building => "Indexing…".into(),
            IndexActivity::Missing => "Folder not found".into(),
            IndexActivity::Failed(error) => format!("Indexing failed: {error}"),
            IndexActivity::Idle(IndexStatus::Missing) => "No index yet".into(),
            IndexActivity::Idle(IndexStatus::Unusable) => "Index unusable, scanning files".into(),
            IndexActivity::Idle(IndexStatus::Ready { files, updated_at }) => format!(
                "{} indexed · updated {}{changed}",
                format::plural(*files as usize, "file", "files"),
                format::ago(*updated_at, SystemTime::now())
            ),
        }
    }
}

impl RepoHub {
    /// Create the hub every window shares, over the library in
    /// `library_file`. Returns why the library could not be read, if so.
    pub(super) fn init(library_file: PathBuf, cx: &mut App) -> Option<String> {
        let (library, error) = match Library::load(&library_file) {
            Ok(library) => (library, None),
            Err(error) => {
                log::error!("could not read the library: {error:#}");
                (
                    Library::default(),
                    Some(format!(
                        "{error:#}. Tags will not be saved until it is fixed."
                    )),
                )
            }
        };
        let hub = cx.new(|cx| Self {
            library,
            library_file,
            library_locked: error.is_some(),
            open: HashMap::new(),
            preferred: HashSet::new(),
            _poll_task: Self::poll(cx),
        });
        cx.set_global(GlobalHub(hub));
        error
    }

    pub(super) fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalHub>().0.clone()
    }

    // ----- the library ----------------------------------------------------------

    /// The repositories at `paths`, known to the library from now on. With
    /// `discover`, a folder holding git repositories stands for them (see
    /// [`repo::discover`]); otherwise each path is one repository. Folders
    /// that do not exist are skipped when discovering. Returns their ids
    /// without duplicates.
    pub(super) fn register(
        &mut self,
        paths: &[PathBuf],
        discover: bool,
    ) -> anyhow::Result<Vec<String>> {
        let folders: Vec<PathBuf> = if discover {
            paths
                .iter()
                .filter(|path| path.is_dir())
                .flat_map(|path| repo::discover(path))
                .collect()
        } else {
            paths.to_vec()
        };
        let mut ids = Vec::new();
        let mut added = false;
        for folder in folders {
            let path = repo::identity(&folder);
            added |= self.library.ensure(&path);
            let id = path.to_string_lossy().into_owned();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        if added {
            self.save()?;
        }
        Ok(ids)
    }

    pub(super) fn set_tags(
        &mut self,
        id: &str,
        tags: Vec<String>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if self.library_tags(id) == tags {
            return Ok(());
        }
        if let Some(state) = self.open.get_mut(id) {
            let mut info = (*state.info).clone();
            info.tags = tags.clone();
            state.info = Arc::new(info);
        }
        self.library.set_tags(Path::new(id), tags);
        cx.emit(HubEvent::MetadataChanged);
        cx.notify();
        self.save()
    }

    /// Every repository known, open or not.
    pub(super) fn library(&self) -> &Library {
        &self.library
    }

    pub(super) fn library_tags(&self, id: &str) -> Vec<String> {
        self.library
            .get(Path::new(id))
            .map(|entry| entry.tags.clone())
            .unwrap_or_default()
    }

    pub(super) fn library_name(&self, id: &str) -> Option<String> {
        self.library
            .get(Path::new(id))
            .map(|entry| entry.name.clone())
    }

    fn save(&self) -> anyhow::Result<()> {
        if self.library_locked {
            return Ok(());
        }
        self.library.save(&self.library_file)
    }

    // ----- opening and closing --------------------------------------------------

    /// Start using a repository, opening it for the first user. Ids come from
    /// [`Self::register`].
    pub(super) fn acquire(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(state) = self.open.get_mut(id) {
            state.users += 1;
            return;
        }
        let path = Path::new(id);
        if self.library.ensure(path) {
            self.save().ok();
        }
        let Some(entry) = self.library.get(path).cloned() else {
            return;
        };
        log::info!("opening {}", entry.path.display());
        let mut state = RepoState::open(&entry);
        state.users = 1;
        let loadable = state.index.is_some();
        self.open.insert(id.to_string(), state);
        if loadable {
            self.load(id, cx);
        }
        cx.notify();
    }

    /// Stop using a repository, closing it when no window uses it any more.
    pub(super) fn release(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(state) = self.open.get_mut(id) else {
            return;
        };
        state.users = state.users.saturating_sub(1);
        if state.users == 0 {
            // Dropping the state drops its watcher and any running build.
            log::info!("closing {id}");
            self.open.remove(id);
            self.start_next_build(cx);
        }
    }

    // ----- reading -------------------------------------------------------------

    pub(super) fn info(&self, id: &str) -> Option<Arc<RepoInfo>> {
        self.open.get(id).map(|state| state.info.clone())
    }

    /// Snapshots of the open repositories among `ids`, sorted by name.
    pub(super) fn views<'a>(&self, ids: impl IntoIterator<Item = &'a String>) -> Vec<RepoView> {
        let mut views: Vec<RepoView> = ids
            .into_iter()
            .filter_map(|id| self.open.get(id))
            .map(|state| RepoView {
                info: state.info.clone(),
                activity: state.activity.clone(),
                changed_files: state.changed_files,
            })
            .collect();
        views.sort_by_key(|view| view.info.name.to_lowercase());
        views
    }

    /// A snapshot of the repository, when it is open.
    pub(super) fn view(&self, id: &str) -> Option<RepoView> {
        self.open.get(id).map(|state| RepoView {
            info: state.info.clone(),
            activity: state.activity.clone(),
            changed_files: state.changed_files,
        })
    }

    pub(super) fn open_count(&self) -> usize {
        self.open.len()
    }

    /// The names of the repositories being indexed and of those waiting to be.
    pub(super) fn build_queue(&self) -> (Vec<String>, Vec<String>) {
        let names = |activity: IndexActivity| -> Vec<String> {
            let mut names: Vec<String> = self
                .open
                .values()
                .filter(|state| state.activity == activity)
                .map(|state| state.info.name.clone())
                .collect();
            names.sort();
            names
        };
        (names(IndexActivity::Building), names(IndexActivity::Queued))
    }

    /// What a search over `ids` reads, and whether one of them is still
    /// loading and will announce its files later.
    pub(super) fn search_sources<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a String>,
    ) -> (Vec<SearchSource>, bool) {
        let mut waiting = false;
        let mut states: Vec<&RepoState> =
            ids.into_iter().filter_map(|id| self.open.get(id)).collect();
        states.sort_by_key(|state| state.info.name.to_lowercase());
        let sources = states
            .into_iter()
            .filter_map(|state| {
                if state.corpus.is_none() && state.activity.is_busy() {
                    waiting = true;
                }
                Some(SearchSource {
                    repo: state.info.clone(),
                    corpus: state.corpus.clone()?,
                    changed: state
                        .tracker
                        .as_ref()
                        .map(|tracker| tracker.changed_paths())
                        .unwrap_or_default(),
                })
            })
            .collect();
        (sources, waiting)
    }

    // ----- indexes -------------------------------------------------------------

    /// Build these repositories first when several are waiting.
    pub(super) fn prefer(&mut self, ids: impl IntoIterator<Item = String>) {
        self.preferred = ids.into_iter().collect();
    }

    /// Load the repository's index (or walk it when there is none), then
    /// queue a build if it has no usable index. Files modified since the
    /// index was built, while nothing watched them, count as changed, so
    /// searches read them.
    fn load(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(state) = self.open.get_mut(id) else {
            return;
        };
        let Some(index) = state.index.clone() else {
            return;
        };
        state.activity = IndexActivity::Loading;
        let tracker = state.tracker.clone();
        let id = id.to_string();
        state.load_task = Some(cx.spawn(async move |this, cx| {
            let (status, corpus, stale, elapsed) = cx
                .background_spawn(async move {
                    let started = Instant::now();
                    let status = index.index_status();
                    let corpus = index.load_corpus();
                    let stale = match (&status, &tracker) {
                        (IndexStatus::Ready { updated_at, .. }, Some(tracker))
                            if corpus.is_indexed() =>
                        {
                            let stale = index.modified_since(*updated_at - STALE_SLACK);
                            tracker.note(stale.clone());
                            stale.len()
                        }
                        _ => 0,
                    };
                    (status, corpus, stale, started.elapsed())
                })
                .await;
            log::info!(
                "loaded {id} in {elapsed:.0?}: {} files, {}",
                corpus.file_count(),
                if corpus.is_indexed() {
                    format!("indexed, {stale} modified since")
                } else {
                    "scanned, no usable index".into()
                }
            );
            this.update(cx, |this, cx| {
                let Some(state) = this.open.get_mut(&id) else {
                    return;
                };
                state.corpus = Some(Arc::new(corpus));
                state.changed_files = state
                    .tracker
                    .as_ref()
                    .map_or(0, |tracker| tracker.changed_count());
                let needs_index = !matches!(status, IndexStatus::Ready { .. });
                state.activity = IndexActivity::Idle(status);
                if needs_index {
                    this.queue_index(&id, cx);
                }
                cx.emit(HubEvent::CorpusChanged(id));
                cx.notify();
            })
            .ok();
        }));
    }

    /// Ask for the repository's index to be rebuilt once no other build runs.
    pub(super) fn queue_index(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(state) = self.open.get_mut(id)
            && state.index.is_some()
            && !matches!(
                state.activity,
                IndexActivity::Queued | IndexActivity::Building
            )
        {
            state.activity = IndexActivity::Queued;
        }
        self.start_next_build(cx);
    }

    /// Builds run one at a time: each already uses every core, and several at
    /// once would only compete for memory and disk. Preferred repositories go
    /// first.
    fn start_next_build(&mut self, cx: &mut Context<Self>) {
        if self
            .open
            .values()
            .any(|state| state.activity == IndexActivity::Building)
        {
            return;
        }
        let next = self
            .open
            .iter()
            .filter(|(_, state)| state.activity == IndexActivity::Queued)
            .max_by_key(|(id, state)| {
                (
                    self.preferred.contains(*id),
                    std::cmp::Reverse(state.info.name.to_lowercase()),
                )
            })
            .map(|(id, _)| id.clone());
        if let Some(id) = next {
            self.build(&id, cx);
        }
        cx.notify();
    }

    fn build(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(state) = self.open.get_mut(id) else {
            return;
        };
        let Some(index) = state.index.clone() else {
            return;
        };
        let tracker = state.tracker.clone();
        let mark = tracker.as_ref().map(|tracker| tracker.mark());
        state.activity = IndexActivity::Building;
        let id = id.to_string();
        log::info!("building the index of {id}");
        let started = Instant::now();
        state.index_task = Some(cx.spawn(async move |this, cx| {
            let builder = index.clone();
            let staged = cx
                .background_spawn(async move { builder.build_index() })
                .await;
            let staged = match staged {
                Ok(staged) => staged,
                Err(error) => {
                    log::error!("indexing {id} failed: {error:#}");
                    this.update(cx, |this, cx| {
                        if let Some(state) = this.open.get_mut(&id) {
                            state.activity = IndexActivity::Failed(format!("{error:#}"));
                        }
                        this.start_next_build(cx);
                    })
                    .ok();
                    return;
                }
            };

            // Let go of the old index so its files can be replaced; windows
            // stop the searches reading it, and search again once the new one
            // loads.
            let Ok(Some(old)) = this.update(cx, |this, cx| {
                let old = this.open.get_mut(&id).map(|state| state.corpus.take());
                cx.emit(HubEvent::ReleaseCorpus(id.clone()));
                old
            }) else {
                return;
            };
            let (published, status, corpus) = cx
                .background_spawn(async move {
                    if let Some(old) = old {
                        wait_for_readers(old);
                    }
                    let published = staged.publish();
                    (published, index.index_status(), index.load_corpus())
                })
                .await;

            this.update(cx, |this, cx| {
                if let Some(state) = this.open.get_mut(&id) {
                    let files = corpus.file_count();
                    state.corpus = Some(Arc::new(corpus));
                    match &published {
                        Ok(()) => {
                            log::info!(
                                "indexed {id} in {:.1?}: {} files",
                                started.elapsed(),
                                files
                            );
                            if let (Some(tracker), Some(mark)) = (tracker, mark) {
                                tracker.forget_before(mark);
                                state.changed_files = tracker.changed_count();
                            }
                            state.activity = IndexActivity::Idle(status);
                        }
                        Err(error) => {
                            log::error!("publishing the index of {id} failed: {error:#}");
                            state.activity = IndexActivity::Failed(format!("{error:#}"));
                        }
                    }
                }
                cx.emit(HubEvent::CorpusChanged(id));
                this.start_next_build(cx);
            })
            .ok();
        }));
    }

    // ----- keeping current -----------------------------------------------------

    /// Keep change counts and branches current, and re-index repositories
    /// that changed a lot.
    fn poll(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        })
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let mut changed_view = false;
        let mut branch_moved = false;
        let mut reindex = Vec::new();
        for (id, state) in &mut self.open {
            let count = state
                .tracker
                .as_ref()
                .map_or(0, |tracker| tracker.changed_count());
            if count != state.changed_files {
                state.changed_files = count;
                changed_view = true;
            }
            let branch = repo::current_branch(&state.info.root);
            if branch != state.info.branch {
                log::info!(
                    "{id} switched to branch {}",
                    branch.as_deref().unwrap_or("(none)")
                );
                let mut info = (*state.info).clone();
                info.branch = branch;
                state.info = Arc::new(info);
                branch_moved = true;
            }
            let ready = matches!(
                state.activity,
                IndexActivity::Idle(IndexStatus::Ready { .. })
            );
            if ready && count >= AUTO_REINDEX_CHANGES {
                log::info!("{count} files changed in {id}; re-indexing");
                reindex.push(id.clone());
            }
        }
        for id in reindex {
            self.queue_index(&id, cx);
        }
        if branch_moved {
            cx.emit(HubEvent::MetadataChanged);
        }
        if changed_view || branch_moved {
            cx.notify();
        }
    }
}

impl RepoState {
    fn open(entry: &LibraryEntry) -> Self {
        let index = RepoIndex::open(&entry.path).ok();
        let tracker = index
            .as_ref()
            .and_then(|index| {
                ChangeTracker::start(index.root())
                    .inspect_err(|error| {
                        log::warn!("not watching {}: {error}", entry.path.display());
                    })
                    .ok()
            })
            .map(Arc::new);
        if index.is_none() {
            log::warn!("{} does not exist", entry.path.display());
        }
        Self {
            info: Arc::new(RepoInfo {
                id: entry.path.to_string_lossy().into_owned(),
                name: entry.name.clone(),
                root: entry.path.clone(),
                branch: repo::current_branch(&entry.path),
                tags: entry.tags.clone(),
            }),
            activity: if index.is_some() {
                IndexActivity::Loading
            } else {
                IndexActivity::Missing
            },
            index,
            corpus: None,
            tracker,
            changed_files: 0,
            users: 0,
            load_task: None,
            index_task: None,
        }
    }
}

/// Wait until in-flight searches drop their handles to an index, so its files
/// can be replaced. Each search checks its cancel flag between files, so this
/// is quick.
fn wait_for_readers(corpus: Arc<Corpus>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while Arc::strong_count(&corpus) > 1 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
}
