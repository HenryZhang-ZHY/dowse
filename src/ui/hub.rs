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
use dowse::diagnostics::metrics::metrics;
use dowse::engine::config::ConfigDir;
use dowse::engine::index::{self, Corpus, IndexMove, IndexStatus, IndexUpdate, RepoIndex};
use dowse::engine::library::{Library, LibraryEntry};
use dowse::engine::repo::{self, RepoInfo};
use dowse::engine::search::SearchSource;
use dowse::engine::settings::{
    IndexLocation, IndexPlace, IndexSettings, Settings, TaskSettings, UpdateSettings,
};
use dowse::engine::sync::Interval;
use dowse::engine::watch::ChangeTracker;

/// How often change counts and checked-out branches are refreshed.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Past this many changed files, reading them all on every search costs more
/// than bringing the index up to date, which reads only those files and
/// merges them in, so that is done in the background.
const AUTO_UPDATE_CHANGES: usize = 500;
/// Files modified this long before an index was finished may have changed
/// after the build read them.
const STALE_SLACK: Duration = Duration::from_secs(10);

pub(super) struct RepoHub {
    library: Library,
    library_file: PathBuf,
    /// Set when the library could not be read, so it is never overwritten.
    library_locked: bool,
    settings: Settings,
    settings_file: PathBuf,
    /// Set when the settings could not be read, so they are never overwritten.
    settings_locked: bool,
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
    /// What the queued or running build does.
    job: IndexJob,
    tracker: Option<Arc<ChangeTracker>>,
    changed_files: usize,
    /// How many windows use the repository.
    users: usize,
    /// Loading the index, or moving it.
    load_task: Option<Task<()>>,
    index_task: Option<Task<()>>,
    /// The index is to move here from where this opens it, once no load or
    /// build uses it.
    pending_move: Option<RepoIndex>,
    /// Something the user should know about the index, such as why it was
    /// built again.
    note: Option<String>,
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

/// What a build does to a repository's index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum IndexJob {
    /// Read the files that changed and merge them in; build the whole index
    /// only when there is none to start from or most files changed.
    #[default]
    Update,
    /// Read every file again.
    Rebuild,
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
    /// Where the index is, or will be once built; `None` when the folder is
    /// gone.
    pub(super) index_dir: Option<PathBuf>,
    pub(super) note: Option<String>,
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
    /// Create the hub every window shares, over the library and settings in
    /// `config`. Returns why they could not be read, if so.
    pub(super) fn init(config: &ConfigDir, cx: &mut App) -> Vec<String> {
        let mut errors = Vec::new();
        let library_file = config.library_file();
        let (library, library_locked) = match Library::load(&library_file) {
            Ok(library) => (library, false),
            Err(error) => {
                log::error!("could not read the library: {error:#}");
                errors.push(format!(
                    "{error:#}. Tags will not be saved until it is fixed."
                ));
                (Library::default(), true)
            }
        };
        let settings_file = config.settings_file();
        let (settings, settings_locked) = match Settings::load(&settings_file) {
            Ok(settings) => (settings, false),
            Err(error) => {
                log::error!("could not read the settings: {error:#}");
                errors.push(format!(
                    "{error:#}. The defaults apply, and settings will not be saved until it is fixed."
                ));
                (Settings::default(), true)
            }
        };
        let external_dir = settings.index.external_dir();
        cx.background_spawn(async move {
            for removed in index::remove_orphaned_indexes(&external_dir) {
                log::info!(
                    "removed {}, the index of a folder that is gone",
                    index::display_path(&removed)
                );
            }
        })
        .detach();
        let hub = cx.new(|cx| Self {
            library,
            library_file,
            library_locked,
            settings,
            settings_file,
            settings_locked,
            open: HashMap::new(),
            preferred: HashSet::new(),
            _poll_task: Self::poll(cx),
        });
        cx.set_global(GlobalHub(hub));
        errors
    }

    pub(super) fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalHub>().0.clone()
    }

    pub(super) fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalHub>().map(|hub| hub.0.clone())
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

    /// Pull the repository every `every`, or never.
    pub(super) fn set_pull_every(
        &mut self,
        id: &str,
        every: Option<Interval>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if !self.library.set_pull_every(Path::new(id), every) {
            return Ok(());
        }
        if let Some(state) = self.open.get_mut(id) {
            let mut info = (*state.info).clone();
            info.pull_every = every;
            state.info = Arc::new(info);
        }
        cx.emit(HubEvent::MetadataChanged);
        cx.notify();
        self.save()
    }

    // ----- where indexes live ---------------------------------------------------

    pub(super) fn index_settings(&self) -> &IndexSettings {
        &self.settings.index
    }

    /// The repository's own index location, `None` when it follows the app's.
    pub(super) fn own_index_location(&self, id: &str) -> Option<IndexLocation> {
        self.library
            .get(Path::new(id))
            .and_then(|entry| entry.index_location)
    }

    /// Where the repository's index lives, by its setting and the app's.
    pub(super) fn index_place(&self, id: &str) -> IndexPlace {
        self.settings.index.place(self.own_index_location(id))
    }

    /// Keep the indexes of these repositories at `location`, or where the
    /// app's setting says with `None`, moving those that change place.
    pub(super) fn set_index_location(
        &mut self,
        ids: &[String],
        location: Option<IndexLocation>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let before = self.index_places();
        let mut changed = false;
        for id in ids {
            changed |= self.library.set_index_location(Path::new(id), location);
        }
        if !changed {
            return Ok(());
        }
        self.save()?;
        self.move_indexes(before, cx);
        cx.notify();
        Ok(())
    }

    /// Change where indexes live, moving those that change place.
    pub(super) fn set_index_settings(
        &mut self,
        settings: IndexSettings,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if self.settings.index == settings {
            return Ok(());
        }
        if self.settings_locked {
            anyhow::bail!(
                "{} could not be read, so it is not overwritten; fix or remove it first",
                self.settings_file.display()
            );
        }
        let before = self.index_places();
        self.settings.index = settings;
        self.settings.save(&self.settings_file)?;
        self.move_indexes(before, cx);
        cx.notify();
        Ok(())
    }

    pub(super) fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Change whether dowse looks for updates.
    pub(super) fn set_update_settings(
        &mut self,
        settings: UpdateSettings,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if self.settings.updates == settings {
            return Ok(());
        }
        if self.settings_locked {
            anyhow::bail!(
                "{} could not be read, so it is not overwritten; fix or remove it first",
                self.settings_file.display()
            );
        }
        self.settings.updates = settings;
        self.settings.save(&self.settings_file)?;
        cx.notify();
        Ok(())
    }

    /// Change how many clones and pulls run at once.
    pub(super) fn set_task_settings(
        &mut self,
        settings: TaskSettings,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if self.settings.tasks == settings {
            return Ok(());
        }
        if self.settings_locked {
            anyhow::bail!(
                "{} could not be read, so it is not overwritten; fix or remove it first",
                self.settings_file.display()
            );
        }
        self.settings.tasks = settings;
        self.settings.save(&self.settings_file)?;
        cx.notify();
        Ok(())
    }

    /// Where every repository's index lives now.
    fn index_places(&self) -> HashMap<String, IndexPlace> {
        self.library
            .repos
            .iter()
            .map(|entry| {
                let id = entry.path.to_string_lossy().into_owned();
                let place = self.index_place(&id);
                (id, place)
            })
            .collect()
    }

    /// Move the indexes whose place changed from `before`. Open repositories
    /// move theirs once no load or build uses it; the others at once, in the
    /// background.
    fn move_indexes(&mut self, before: HashMap<String, IndexPlace>, cx: &mut Context<Self>) {
        for (id, old_place) in before {
            let new_place = self.index_place(&id);
            if new_place == old_place {
                continue;
            }
            let root = PathBuf::from(&id);
            let (Ok(old), Ok(new)) = (
                RepoIndex::open_in(&root, &old_place),
                RepoIndex::open_in(&root, &new_place),
            ) else {
                continue;
            };
            match self.open.get_mut(&id) {
                Some(state) => {
                    // A move still waiting knows where the index really is.
                    state.pending_move.get_or_insert(old);
                    state.index = Some(new);
                }
                None => {
                    cx.background_spawn(async move {
                        log_move(&id, &new, new.move_from(&old));
                    })
                    .detach();
                }
            }
        }
        self.start_pending_moves(cx);
    }

    /// Move the indexes of open repositories that wait to move and that no
    /// load or build uses, then load them from their new place.
    fn start_pending_moves(&mut self, cx: &mut Context<Self>) {
        let ready: Vec<String> = self
            .open
            .iter()
            .filter(|(_, state)| {
                state.pending_move.is_some()
                    && !matches!(
                        state.activity,
                        IndexActivity::Loading | IndexActivity::Building
                    )
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ready {
            let Some(state) = self.open.get_mut(&id) else {
                continue;
            };
            let (Some(old), Some(new)) = (state.pending_move.take(), state.index.clone()) else {
                continue;
            };
            state.activity = IndexActivity::Loading;
            let corpus = state.corpus.take();
            cx.emit(HubEvent::ReleaseCorpus(id.clone()));
            state.load_task = Some(cx.spawn(async move |this, cx| {
                let moved = cx
                    .background_spawn(async move {
                        if let Some(corpus) = corpus {
                            wait_for_readers(corpus);
                        }
                        let moved = new.move_from(&old);
                        log_move(&id, &new, moved.clone());
                        (id, moved)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    let (id, moved) = moved;
                    if let Some(state) = this.open.get_mut(&id) {
                        state.note = match moved {
                            IndexMove::Kept(reason) => Some(format!(
                                "The index could not be moved ({reason}), so it was built again here; the old one was left where it was."
                            )),
                            _ => None,
                        };
                    }
                    this.load(&id, cx);
                })
                .ok();
            }));
        }
    }

    /// Note that dowse tried to pull the repository at `time`.
    pub(super) fn note_pulled(&mut self, id: &str, time: SystemTime) {
        self.library.set_pulled_at(Path::new(id), time);
        self.save().ok();
    }

    /// A clone that just finished: known to the library from now on, with
    /// `tags` added to any it has and pulled every `pull_every` when given.
    /// Returns its id.
    pub(super) fn adopt(
        &mut self,
        folder: &Path,
        tags: &[String],
        pull_every: Option<Interval>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<String> {
        let id = self
            .register(&[folder.to_path_buf()], false)?
            .into_iter()
            .next()
            .expect("one folder registers one repository");
        let mut merged = self.library_tags(&id);
        for tag in tags {
            if !merged.contains(tag) {
                merged.push(tag.clone());
            }
        }
        self.set_tags(&id, merged, cx)?;
        if pull_every.is_some() {
            self.set_pull_every(&id, pull_every, cx)?;
        }
        Ok(id)
    }

    /// Whether some window or request has the repository open.
    pub(super) fn is_open(&self, id: &str) -> bool {
        self.open.contains_key(id)
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
        let mut state = RepoState::open(&entry, &self.index_place(id));
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
            .map(RepoState::view)
            .collect();
        views.sort_by_key(|view| view.info.name.to_lowercase());
        views
    }

    /// A snapshot of the repository, when it is open.
    pub(super) fn view(&self, id: &str) -> Option<RepoView> {
        self.open.get(id).map(RepoState::view)
    }

    /// Snapshots of every open repository, sorted by name.
    pub(super) fn open_views(&self) -> Vec<RepoView> {
        self.views(self.open.keys())
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
                        .map(|tracker| tracker.changes())
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
    /// queue a build if it has no usable index. Files that differ from the
    /// index, changed while nothing watched them, count as changed, so
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
                            // The index's file stamps also reveal deleted
                            // files, and moved ones that kept their times.
                            let stale = index
                                .stale_files()
                                .unwrap_or_else(|| index.modified_since(*updated_at - STALE_SLACK));
                            let count = stale.len();
                            tracker.note(stale);
                            count
                        }
                        _ => 0,
                    };
                    (status, corpus, stale, started.elapsed())
                })
                .await;
            metrics().record_index_load(elapsed);
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
                    this.queue_index(&id, IndexJob::Update, cx);
                } else {
                    this.start_pending_moves(cx);
                }
                cx.emit(HubEvent::CorpusChanged(id));
                cx.notify();
            })
            .ok();
        }));
    }

    /// Ask for `job` to be done to the repository's index once no other
    /// build runs. A rebuild asked for while an update waits replaces it.
    pub(super) fn queue_index(&mut self, id: &str, job: IndexJob, cx: &mut Context<Self>) {
        if let Some(state) = self.open.get_mut(id)
            && state.index.is_some()
        {
            match state.activity {
                IndexActivity::Building => {}
                IndexActivity::Queued => state.job = state.job.max(job),
                _ => {
                    state.activity = IndexActivity::Queued;
                    state.job = job;
                }
            }
        }
        self.start_next_build(cx);
    }

    /// Builds run one at a time: each already uses every core, and several at
    /// once would only compete for memory and disk. Preferred repositories go
    /// first.
    fn start_next_build(&mut self, cx: &mut Context<Self>) {
        self.start_pending_moves(cx);
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
        // Taken before the build walks the folder: changes made after it may
        // not be in the new index, so they are kept.
        let mark = tracker.as_ref().map(|tracker| tracker.mark());
        state.activity = IndexActivity::Building;
        let job = state.job;
        let id = id.to_string();
        match job {
            IndexJob::Update => log::info!("updating the index of {id}"),
            IndexJob::Rebuild => log::info!("rebuilding the index of {id}"),
        }
        let started = Instant::now();
        state.index_task = Some(cx.spawn(async move |this, cx| {
            let builder = index.clone();
            let update = cx
                .background_spawn(async move {
                    match job {
                        IndexJob::Update => builder.update_index(),
                        IndexJob::Rebuild => {
                            builder.build_index().map(|staged| IndexUpdate::Rebuilt {
                                staged,
                                reason: "a rebuild was asked for".into(),
                            })
                        }
                    }
                })
                .await;
            let update = match update {
                Ok(update) => update,
                Err(error) => {
                    log::error!("indexing {id} failed: {error:#}");
                    metrics().record_index_build(started.elapsed(), false);
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
            let (staged, merged) = match update {
                IndexUpdate::UpToDate => {
                    log::info!(
                        "the index of {id} is up to date, checked in {:.1?}",
                        started.elapsed()
                    );
                    metrics().record_index_update(started.elapsed());
                    let status = cx
                        .background_spawn(async move { index.index_status() })
                        .await;
                    this.update(cx, |this, cx| {
                        if let Some(state) = this.open.get_mut(&id) {
                            if let (Some(tracker), Some(mark)) = (tracker, mark) {
                                tracker.forget_before(mark);
                                state.changed_files = tracker.changed_count();
                            }
                            state.activity = IndexActivity::Idle(status);
                        }
                        this.start_next_build(cx);
                    })
                    .ok();
                    return;
                }
                IndexUpdate::Merged { staged, changes } => {
                    log::info!(
                        "merging into the index of {id}: {} modified, {} added, {} deleted",
                        changes.modified,
                        changes.added,
                        changes.deleted
                    );
                    (staged, true)
                }
                IndexUpdate::Rebuilt { staged, reason } => {
                    log::info!("built the whole index of {id}: {reason}");
                    (staged, false)
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
                            if merged {
                                metrics().record_index_update(started.elapsed());
                            } else {
                                metrics().record_index_build(started.elapsed(), true);
                            }
                            log::info!(
                                "{} {id} in {:.1?}: {} files",
                                if merged { "updated" } else { "indexed" },
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
                            metrics().record_index_build(started.elapsed(), false);
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

    /// Keep change counts and branches current, and bring the indexes of
    /// repositories that changed a lot up to date.
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
        let mut outdated = Vec::new();
        let mut lost = Vec::new();
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
            if ready
                && state
                    .index
                    .as_ref()
                    .is_some_and(|index| !index.index_exists())
            {
                log::warn!("the index of {id} was deleted; building it again");
                state.note = Some(deleted_note(state.index_dir_is_in_repo()));
                lost.push(id.clone());
            } else if ready && count >= AUTO_UPDATE_CHANGES {
                log::info!("{count} files changed in {id}; updating its index");
                outdated.push(id.clone());
            }
        }
        for id in outdated {
            self.queue_index(&id, IndexJob::Update, cx);
        }
        for id in lost {
            // Searches go back to reading the folder until the new index is
            // built; what is left of the old one is let go so it can be
            // replaced.
            if let Some(state) = self.open.get_mut(&id) {
                state.corpus = None;
            }
            cx.emit(HubEvent::ReleaseCorpus(id.clone()));
            self.load(&id, cx);
            changed_view = true;
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
    fn open(entry: &LibraryEntry, place: &IndexPlace) -> Self {
        let index = RepoIndex::open_in(&entry.path, place).ok();
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
                pull_every: entry.pull_every,
            }),
            activity: if index.is_some() {
                IndexActivity::Loading
            } else {
                IndexActivity::Missing
            },
            index,
            corpus: None,
            job: IndexJob::default(),
            tracker,
            changed_files: 0,
            users: 0,
            load_task: None,
            index_task: None,
            pending_move: None,
            note: None,
        }
    }

    /// Whether the index lives in the repository's own `.tgrep`.
    fn index_dir_is_in_repo(&self) -> bool {
        self.index
            .as_ref()
            .is_some_and(|index| index.index_dir().starts_with(index.root()))
    }

    fn view(&self) -> RepoView {
        RepoView {
            info: self.info.clone(),
            activity: self.activity.clone(),
            changed_files: self.changed_files,
            index_dir: self
                .index
                .as_ref()
                .map(|index| index.index_dir().to_path_buf()),
            note: self.note.clone(),
        }
    }
}

/// Why a repository's index was built again after it vanished.
fn deleted_note(in_repo: bool) -> String {
    let mut note = "The index was deleted outside dowse, so it was built again.".to_string();
    if in_repo {
        note.push_str(
            " If cleaning the repository deletes it, keep the index outside the repository instead.",
        );
    }
    note
}

fn log_move(id: &str, new: &RepoIndex, moved: IndexMove) {
    let to = index::display_path(new.index_dir());
    match moved {
        IndexMove::NothingToMove => {
            log::info!("{id} has no index to move; it will be built in {to}")
        }
        IndexMove::Moved => log::info!("moved the index of {id} to {to}"),
        IndexMove::AlreadyThere => {
            log::info!("{id} already had an index in {to}; removed the old one")
        }
        IndexMove::Kept(reason) => {
            log::warn!("kept the index of {id} where it was: {reason}; it will be built in {to}")
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
