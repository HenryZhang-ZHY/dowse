//! The registered repositories: loading each one's index, building indexes
//! one at a time, watching for changes, tags, and the search scope.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::*;

use super::app::SearchApp;
use tgrep_gpui::engine::registry::{Registry, RepoEntry};
use tgrep_gpui::engine::repo::{self, RepoInfo, Scope};
use tgrep_gpui::engine::search::SearchSource;
use tgrep_gpui::engine::watch::ChangeTracker;
use tgrep_gpui::engine::workspace::{Corpus, IndexStatus, Workspace, display_path};

/// How often change counts and checked-out branches are refreshed.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Past this many changed files, reading them all on every search costs more
/// than re-indexing, so the index is rebuilt in the background.
const AUTO_REINDEX_CHANGES: usize = 2_000;

/// Overrides where settings are kept, e.g. for a portable install or tests.
const CONFIG_DIR_ENV: &str = "TGREP_GPUI_CONFIG_DIR";

/// Where the repository list is saved.
pub(super) fn registry_file() -> Option<PathBuf> {
    let dir = match std::env::var_os(CONFIG_DIR_ENV) {
        Some(dir) => PathBuf::from(dir),
        None => dirs::config_dir()?.join("tgrep-gpui"),
    };
    Some(dir.join("repos.json"))
}

/// One registered repository and what its index is doing.
pub(super) struct RepoState {
    pub(super) info: Arc<RepoInfo>,
    /// `None` when the folder no longer exists.
    workspace: Option<Workspace>,
    pub(super) corpus: Option<Arc<Corpus>>,
    pub(super) index: IndexActivity,
    tracker: Option<Arc<ChangeTracker>>,
    pub(super) changed_files: usize,
    pub(super) tags_input: Entity<InputState>,
    load_task: Option<Task<()>>,
    index_task: Option<Task<()>>,
    _tags_subscription: Subscription,
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

impl RepoState {
    pub(super) fn is_busy(&self) -> bool {
        matches!(
            self.index,
            IndexActivity::Loading | IndexActivity::Queued | IndexActivity::Building
        )
    }

    fn changed_paths(&self) -> Vec<String> {
        self.tracker
            .as_ref()
            .map(|tracker| tracker.changed_paths())
            .unwrap_or_default()
    }
}

impl SearchApp {
    // ----- registry -----------------------------------------------------------

    /// Load the saved repositories and start loading their indexes.
    pub(super) fn restore_registry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = registry_file() else {
            return;
        };
        match Registry::load(&file) {
            Ok(registry) => {
                self.scope = Scope::new(registry.scope.iter().cloned());
                for entry in &registry.repos {
                    let state = self.new_repo_state(entry, window, cx);
                    self.repos.push(state);
                }
                self.registry = registry;
                self.sort_repos();
                for id in self.repo_ids() {
                    self.load_repo(&id, cx);
                }
            }
            Err(error) => {
                // Never overwrite a file we could not read.
                self.registry_locked = true;
                window.push_notification(
                    Notification::error(format!(
                        "{error:#}. Repository changes will not be saved until it is fixed."
                    )),
                    cx,
                );
            }
        }
    }

    fn save_registry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.registry_locked {
            return;
        }
        self.registry.scope = self.scope.tags().map(str::to_string).collect();
        let saved = registry_file()
            .ok_or_else(|| anyhow::anyhow!("no configuration directory"))
            .and_then(|file| self.registry.save(&file));
        if let Err(error) = saved {
            window.push_notification(
                Notification::error(format!("Could not save repositories: {error:#}")),
                cx,
            );
        }
    }

    /// Register every repository found at `paths` (see [`repo::discover`]).
    pub(super) fn add_repositories(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut added = Vec::new();
        for path in paths.iter().flat_map(|path| repo::discover(path)) {
            let Ok(canonical) = std::fs::canonicalize(&path) else {
                continue;
            };
            let path = PathBuf::from(display_path(&canonical));
            if self.registry.add(path.clone(), Vec::new()) {
                added.push(path);
            }
        }
        if added.is_empty() {
            window.push_notification("Those repositories are already added.", cx);
            return;
        }
        for path in &added {
            let entry = self
                .registry
                .repos
                .iter()
                .find(|entry| &entry.path == path)
                .cloned()
                .expect("just added");
            let state = self.new_repo_state(&entry, window, cx);
            self.repos.push(state);
        }
        self.sort_repos();
        self.save_registry(window, cx);
        for path in &added {
            self.load_repo(&path.to_string_lossy(), cx);
        }
        let message = match added.as_slice() {
            [one] => format!("Added {}", one.display()),
            many => format!("Added {} repositories", many.len()),
        };
        window.push_notification(message, cx);
        cx.notify();
    }

    pub(super) fn prompt_for_repositories(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.add_repositories(paths, window, cx)
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn remove_repository(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.repos.retain(|repo| repo.info.id != id);
        self.registry.remove(Path::new(id));
        self.save_registry(window, cx);
        self.schedule_search(false, cx);
    }

    fn new_repo_state(
        &mut self,
        entry: &RepoEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> RepoState {
        let workspace = Workspace::open(&entry.path).ok();
        let info = Arc::new(RepoInfo {
            id: entry.path.to_string_lossy().into_owned(),
            name: entry.name.clone(),
            root: entry.path.clone(),
            branch: repo::current_branch(&entry.path),
            tags: entry.tags.clone(),
        });
        let tracker = workspace
            .as_ref()
            .and_then(|workspace| ChangeTracker::start(workspace.root()).ok())
            .map(Arc::new);
        let tags_text = entry.tags.join(" ");
        let tags_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Add tags: mirror dev owner:alice project:billing")
                .default_value(tags_text)
        });
        let id = info.id.clone();
        let subscription = cx.subscribe_in(
            &tags_input,
            window,
            move |this, input, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let text = input.read(cx).value().to_string();
                    this.set_repository_tags(&id, &text, window, cx);
                }
            },
        );
        RepoState {
            index: if workspace.is_some() {
                IndexActivity::Loading
            } else {
                IndexActivity::Missing
            },
            info,
            workspace,
            corpus: None,
            tracker,
            changed_files: 0,
            tags_input,
            load_task: None,
            index_task: None,
            _tags_subscription: subscription,
        }
    }

    pub(super) fn set_repository_tags(
        &mut self,
        id: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tags = repo::parse_tags(text);
        let Some(state) = self.repo_mut(id) else {
            return;
        };
        if state.info.tags == tags {
            return;
        }
        let mut info = (*state.info).clone();
        info.tags = tags.clone();
        state.info = Arc::new(info);
        let normalized = tags.join(" ");
        state
            .tags_input
            .update(cx, |input, cx| input.set_value(normalized, window, cx));
        self.registry.set_tags(Path::new(id), tags);
        self.save_registry(window, cx);
        self.schedule_search(false, cx);
    }

    fn sort_repos(&mut self) {
        self.repos.sort_by_key(|repo| repo.info.name.to_lowercase());
    }

    fn repo_ids(&self) -> Vec<String> {
        self.repos.iter().map(|repo| repo.info.id.clone()).collect()
    }

    pub(super) fn repo_mut(&mut self, id: &str) -> Option<&mut RepoState> {
        self.repos.iter_mut().find(|repo| repo.info.id == id)
    }

    // ----- scope ----------------------------------------------------------------

    pub(super) fn in_scope(&self) -> impl Iterator<Item = &RepoState> {
        self.repos
            .iter()
            .filter(|repo| self.scope.includes(&repo.info))
    }

    pub(super) fn toggle_scope_tag(
        &mut self,
        tag: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scope.toggle(tag);
        self.save_registry(window, cx);
        self.schedule_search(false, cx);
    }

    pub(super) fn clear_scope(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.scope.clear();
        self.save_registry(window, cx);
        self.schedule_search(false, cx);
    }

    /// What a search over the current scope reads, in repository order, and
    /// whether some repository in scope is still loading.
    pub(super) fn search_sources(&self) -> (Vec<SearchSource>, bool) {
        let mut waiting = false;
        let sources = self
            .in_scope()
            .filter_map(|repo| {
                if repo.corpus.is_none() && repo.is_busy() {
                    waiting = true;
                }
                Some(SearchSource {
                    repo: repo.info.clone(),
                    corpus: repo.corpus.clone()?,
                    changed: repo.changed_paths(),
                })
            })
            .collect();
        (sources, waiting)
    }

    // ----- indexes ----------------------------------------------------------------

    /// Load the repository's index (or walk it when there is none), then
    /// queue a build if it has no usable index.
    fn load_repo(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(state) = self.repo_mut(id) else {
            return;
        };
        let Some(workspace) = state.workspace.clone() else {
            return;
        };
        state.index = IndexActivity::Loading;
        let id = id.to_string();
        state.load_task = Some(cx.spawn(async move |this, cx| {
            let (status, corpus) = cx
                .background_spawn(
                    async move { (workspace.index_status(), workspace.load_corpus()) },
                )
                .await;
            this.update(cx, |this, cx| {
                let Some(state) = this.repo_mut(&id) else {
                    return;
                };
                state.corpus = Some(Arc::new(corpus));
                let needs_index = !matches!(status, IndexStatus::Ready { .. });
                state.index = IndexActivity::Idle(status);
                if needs_index {
                    this.queue_index(&id, cx);
                }
                let in_scope = this
                    .repos
                    .iter()
                    .any(|repo| repo.info.id == id && this.scope.includes(&repo.info));
                if in_scope {
                    this.schedule_search(true, cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Ask for the repository's index to be rebuilt once no other build runs.
    pub(super) fn queue_index(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(state) = self.repo_mut(id)
            && state.workspace.is_some()
            && !matches!(state.index, IndexActivity::Queued | IndexActivity::Building)
        {
            state.index = IndexActivity::Queued;
        }
        self.start_next_build(cx);
    }

    /// Rebuild every repository in scope.
    pub(super) fn queue_scope_indexes(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.in_scope().map(|repo| repo.info.id.clone()).collect();
        for id in ids {
            self.queue_index(&id, cx);
        }
    }

    /// Builds run one at a time: each already uses every core, and several at
    /// once would only compete for memory and disk. Repositories in scope go first.
    fn start_next_build(&mut self, cx: &mut Context<Self>) {
        if self
            .repos
            .iter()
            .any(|repo| repo.index == IndexActivity::Building)
        {
            return;
        }
        let next = self
            .repos
            .iter()
            .filter(|repo| repo.index == IndexActivity::Queued)
            .max_by_key(|repo| self.scope.includes(&repo.info))
            .map(|repo| repo.info.id.clone());
        if let Some(id) = next {
            self.build_repo_index(&id, cx);
        }
        cx.notify();
    }

    fn build_repo_index(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(state) = self.repo_mut(id) else {
            return;
        };
        let Some(workspace) = state.workspace.clone() else {
            return;
        };
        let tracker = state.tracker.clone();
        let mark = tracker.as_ref().map(|tracker| tracker.mark());
        state.index = IndexActivity::Building;
        let id = id.to_string();
        state.index_task = Some(cx.spawn(async move |this, cx| {
            let builder = workspace.clone();
            let staged = cx
                .background_spawn(async move { builder.build_index() })
                .await;
            let staged = match staged {
                Ok(staged) => staged,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        if let Some(state) = this.repo_mut(&id) {
                            state.index = IndexActivity::Failed(format!("{error:#}"));
                        }
                        this.start_next_build(cx);
                    })
                    .ok();
                    return;
                }
            };

            // Stop searching and let go of the old index so its files can be
            // replaced; a search queued meanwhile runs once the new one loads.
            let Ok(Some(old)) = this.update(cx, |this, _| {
                this.search_cancel.store(true, Ordering::Relaxed);
                this.search_task = None;
                this.repo_mut(&id).map(|state| state.corpus.take())
            }) else {
                return;
            };
            let (published, status, corpus) = cx
                .background_spawn(async move {
                    if let Some(old) = old {
                        release(old);
                    }
                    let published = staged.publish();
                    (published, workspace.index_status(), workspace.load_corpus())
                })
                .await;

            this.update(cx, |this, cx| {
                if let Some(state) = this.repo_mut(&id) {
                    state.corpus = Some(Arc::new(corpus));
                    match published {
                        Ok(()) => {
                            if let (Some(tracker), Some(mark)) = (tracker, mark) {
                                tracker.forget_before(mark);
                                state.changed_files = tracker.changed_count();
                            }
                            state.index = IndexActivity::Idle(status);
                        }
                        Err(error) => state.index = IndexActivity::Failed(format!("{error:#}")),
                    }
                }
                this.schedule_search(false, cx);
                this.start_next_build(cx);
            })
            .ok();
        }));
    }

    /// Keep change counts and branches current, and re-index repositories
    /// that changed a lot.
    pub(super) fn poll_repositories(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let alive = this.update(cx, |this, cx| this.refresh_repositories(cx));
                if alive.is_err() {
                    break;
                }
            }
        })
    }

    fn refresh_repositories(&mut self, cx: &mut Context<Self>) {
        let mut changed_view = false;
        let mut branch_moved = false;
        let mut reindex = Vec::new();
        for state in &mut self.repos {
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
                let mut info = (*state.info).clone();
                info.branch = branch;
                state.info = Arc::new(info);
                branch_moved = true;
            }
            let ready = matches!(state.index, IndexActivity::Idle(IndexStatus::Ready { .. }));
            if ready && count >= AUTO_REINDEX_CHANGES {
                reindex.push(state.info.id.clone());
            }
        }
        for id in reindex {
            self.queue_index(&id, cx);
        }
        if branch_moved {
            // A branch switch can move a repository in or out of scope.
            self.schedule_search(true, cx);
        }
        if changed_view || branch_moved {
            cx.notify();
        }
    }
}

/// Close an index once in-flight searches drop their handles to it. Each
/// search checks its cancel flag between files, so this is quick.
fn release(corpus: Arc<Corpus>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while Arc::strong_count(&corpus) > 1 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
}
