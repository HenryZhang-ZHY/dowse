//! State and behaviour of the main view. Rendering lives in `render.rs`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, WindowExt as _};
use gpui_kit::*;

use super::{
    FocusPathFilter, FocusSearch, OpenFolder, RebuildIndex, ToggleCaseSensitive, ToggleRegex,
    ToggleTheme, ToggleWholeWord,
};
use crate::editor::{self, Launch};
use crate::recent;
use tgrep_gpui::engine::query::{CompiledQuery, SearchQuery};
use tgrep_gpui::engine::search::{
    self, FacetFilter, Facets, FileMatch, SearchLimits, SearchOutcome,
};
use tgrep_gpui::engine::watch::ChangeTracker;
use tgrep_gpui::engine::workspace::{Corpus, IndexStatus, Workspace};

/// Pause after a keystroke before searching, so typing a word runs one search.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);
/// How often the status bar picks up the watcher's changed-file count.
const CHANGE_POLL: Duration = Duration::from_secs(2);
/// Past this many changed files, reading them all on every search costs more
/// than re-indexing, so the index is rebuilt in the background.
const AUTO_REINDEX_CHANGES: usize = 2_000;

pub struct SearchApp {
    pub(super) folder: Option<Folder>,
    pub(super) recent: Vec<PathBuf>,
    pub(super) search_input: Entity<InputState>,
    pub(super) path_input: Entity<InputState>,
    pub(super) case_sensitive: bool,
    pub(super) whole_word: bool,
    pub(super) regex: bool,
    pub(super) results: Option<Results>,
    pub(super) query_error: Option<String>,
    /// A search is running (or waiting for the corpus) for the current query.
    pub(super) searching: bool,
    pub(super) facet_filter: FacetFilter,
    /// Files whose every kept line is shown, not just the first few.
    pub(super) expanded: HashSet<String>,
    pub(super) list_state: ListState,
    search_task: Option<Task<()>>,
    search_cancel: Arc<AtomicBool>,
    _subscriptions: Vec<Subscription>,
}

/// The open folder and what its index is doing.
pub(super) struct Folder {
    pub(super) workspace: Workspace,
    pub(super) corpus: Option<Arc<Corpus>>,
    pub(super) index: IndexActivity,
    /// Files changed since the index was built; `None` if watching failed.
    tracker: Option<Arc<ChangeTracker>>,
    pub(super) changed_files: usize,
    load_task: Option<Task<()>>,
    index_task: Option<Task<()>>,
    _poll_task: Task<()>,
}

pub(super) enum IndexActivity {
    /// Reading the index, or walking the folder when there is none.
    Loading,
    Building,
    Idle(IndexStatus),
    Failed(String),
}

/// A finished search and the facet view over it.
pub(super) struct Results {
    pub(super) outcome: Arc<SearchOutcome>,
    pub(super) facets: Facets,
    /// Indexes into `outcome.files` that pass the facet filters.
    pub(super) visible: Vec<usize>,
}

impl Results {
    pub(super) fn file(&self, visible_index: usize) -> Option<&FileMatch> {
        let index = *self.visible.get(visible_index)?;
        self.outcome.files.get(index)
    }
}

impl SearchApp {
    pub fn new(
        initial_folder: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search code…"));
        let path_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter paths: src *.rs !test"));
        let subscriptions = vec![
            cx.subscribe_in(&search_input, window, Self::on_input_event),
            cx.subscribe_in(&path_input, window, Self::on_input_event),
        ];
        search_input.update(cx, |input, cx| input.focus(window, cx));

        let mut this = Self {
            folder: None,
            recent: recent::load(),
            search_input,
            path_input,
            case_sensitive: false,
            whole_word: false,
            regex: false,
            results: None,
            query_error: None,
            searching: false,
            facet_filter: FacetFilter::default(),
            expanded: HashSet::new(),
            list_state: ListState::new(0, ListAlignment::Top, px(800.)),
            search_task: None,
            search_cancel: Arc::new(AtomicBool::new(false)),
            _subscriptions: subscriptions,
        };
        if let Some(folder) = initial_folder {
            this.open_folder(folder, window, cx);
        }
        this
    }

    fn on_input_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => self.schedule_search(true, cx),
            InputEvent::PressEnter { .. } => self.schedule_search(false, cx),
            InputEvent::Focus | InputEvent::Blur => {}
        }
    }

    // ----- folders and the index -------------------------------------------

    pub(super) fn open_folder(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = match Workspace::open(&path) {
            Ok(workspace) => workspace,
            Err(error) => {
                window.push_notification(Notification::error(format!("{error:#}")), cx);
                return;
            }
        };
        self.recent = recent::remember(Path::new(&workspace.display_root()));
        window.set_window_title(&format!("{} — tgrep", workspace.name()));
        let tracker = ChangeTracker::start(workspace.root()).ok().map(Arc::new);
        let poll_task = self.poll_changes(workspace.root().to_path_buf(), cx);
        self.folder = Some(Folder {
            workspace,
            corpus: None,
            index: IndexActivity::Loading,
            tracker,
            changed_files: 0,
            load_task: None,
            index_task: None,
            _poll_task: poll_task,
        });
        self.facet_filter = FacetFilter::default();
        self.set_results(None, cx);
        self.load_corpus(cx);
    }

    pub(super) fn prompt_for_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.open_folder(path, window, cx))
                .ok();
        })
        .detach();
    }

    /// Load the index (or walk the folder), then search. Builds the index in
    /// the background when there is no usable one.
    fn load_corpus(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.folder.as_mut() else {
            return;
        };
        let workspace = folder.workspace.clone();
        folder.index = IndexActivity::Loading;
        folder.load_task = Some(cx.spawn(async move |this, cx| {
            let root = workspace.root().to_path_buf();
            let (status, corpus) = cx
                .background_spawn(async move {
                    let status = workspace.index_status();
                    (status, workspace.load_corpus())
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(folder) = this.folder_at(&root) else {
                    return;
                };
                folder.corpus = Some(Arc::new(corpus));
                let needs_index = !matches!(status, IndexStatus::Ready { .. });
                folder.index = IndexActivity::Idle(status);
                if needs_index {
                    this.build_index(cx);
                }
                this.schedule_search(false, cx);
            })
            .ok();
        }));
        cx.notify();
    }

    /// Refresh the changed-file count now and then, and re-index once enough
    /// files have changed.
    fn poll_changes(&self, root: PathBuf, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CHANGE_POLL).await;
                let alive = this.update(cx, |this, cx| {
                    let Some(folder) = this.folder_at(&root) else {
                        return;
                    };
                    let count = folder
                        .tracker
                        .as_ref()
                        .map_or(0, |tracker| tracker.changed_count());
                    if count != folder.changed_files {
                        folder.changed_files = count;
                        cx.notify();
                    }
                    let idle =
                        matches!(folder.index, IndexActivity::Idle(IndexStatus::Ready { .. }));
                    if idle && count >= AUTO_REINDEX_CHANGES {
                        this.build_index(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
    }

    pub(super) fn build_index(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.folder.as_mut() else {
            return;
        };
        if matches!(folder.index, IndexActivity::Building) {
            return;
        }
        let workspace = folder.workspace.clone();
        let tracker = folder.tracker.clone();
        let mark = tracker.as_ref().map(|tracker| tracker.mark());
        folder.index = IndexActivity::Building;
        folder.index_task = Some(cx.spawn(async move |this, cx| {
            let root = workspace.root().to_path_buf();
            let builder = workspace.clone();
            let staged = cx
                .background_spawn(async move { builder.build_index() })
                .await;
            let staged = match staged {
                Ok(staged) => staged,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        if let Some(folder) = this.folder_at(&root) {
                            folder.index = IndexActivity::Failed(format!("{error:#}"));
                            cx.notify();
                        }
                    })
                    .ok();
                    return;
                }
            };

            // Stop searching and let go of the old index so its files can be
            // replaced; searches queued meanwhile run once the new one loads.
            let Ok(Some(old)) = this.update(cx, |this, _| {
                this.search_cancel.store(true, Ordering::Relaxed);
                this.search_task = None;
                this.folder_at(&root).map(|folder| folder.corpus.take())
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
                let Some(folder) = this.folder_at(&root) else {
                    return;
                };
                folder.corpus = Some(Arc::new(corpus));
                match published {
                    Ok(()) => {
                        if let (Some(tracker), Some(mark)) = (tracker, mark) {
                            tracker.forget_before(mark);
                            folder.changed_files = tracker.changed_count();
                        }
                        folder.index = IndexActivity::Idle(status);
                    }
                    Err(error) => folder.index = IndexActivity::Failed(format!("{error:#}")),
                }
                this.schedule_search(false, cx);
            })
            .ok();
        }));
        cx.notify();
    }

    /// The open folder, if it is still the one at `root`.
    fn folder_at(&mut self, root: &Path) -> Option<&mut Folder> {
        self.folder
            .as_mut()
            .filter(|folder| folder.workspace.root() == root)
    }

    // ----- searching --------------------------------------------------------

    fn current_query(&self, cx: &App) -> SearchQuery {
        SearchQuery {
            pattern: self.search_input.read(cx).value().to_string(),
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
            path_filter: self.path_input.read(cx).value().trim().to_string(),
        }
    }

    /// Search for the current query, replacing any search in flight.
    pub(super) fn schedule_search(&mut self, debounce: bool, cx: &mut Context<Self>) {
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_task = None;

        let query = self.current_query(cx);
        if query.is_empty() {
            self.query_error = None;
            self.searching = false;
            self.set_results(None, cx);
            return;
        }
        let compiled = match CompiledQuery::new(&query) {
            Ok(compiled) => compiled,
            Err(error) => {
                self.query_error = Some(error);
                self.searching = false;
                cx.notify();
                return;
            }
        };
        self.query_error = None;
        self.searching = true;
        // Without a corpus yet, the search runs once loading finishes.
        let Some(folder) = self.folder.as_ref() else {
            cx.notify();
            return;
        };
        let Some(corpus) = folder.corpus.clone() else {
            cx.notify();
            return;
        };
        let changed = folder
            .tracker
            .as_ref()
            .map(|tracker| tracker.changed_paths())
            .unwrap_or_default();

        let cancel = Arc::new(AtomicBool::new(false));
        self.search_cancel = cancel.clone();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            }
            let outcome = cx
                .background_spawn(async move {
                    search::search(
                        &corpus,
                        &changed,
                        &compiled,
                        &SearchLimits::default(),
                        &cancel,
                    )
                })
                .await;
            if outcome.cancelled {
                return;
            }
            this.update(cx, |this, cx| {
                this.searching = false;
                this.set_results(Some(outcome), cx);
            })
            .ok();
        }));
        cx.notify();
    }

    fn set_results(&mut self, outcome: Option<SearchOutcome>, cx: &mut Context<Self>) {
        self.expanded.clear();
        self.results = outcome.map(|outcome| Results {
            outcome: Arc::new(outcome),
            facets: Facets::default(),
            visible: Vec::new(),
        });
        self.refresh_visible(cx);
    }

    /// Re-apply the facet filters and reset the list to the top.
    fn refresh_visible(&mut self, cx: &mut Context<Self>) {
        let count = match self.results.as_mut() {
            Some(results) => {
                let files = &results.outcome.files;
                results.facets = Facets::new(files, &self.facet_filter);
                results.visible = (0..files.len())
                    .filter(|&index| self.facet_filter.matches(&files[index]))
                    .collect();
                results.visible.len()
            }
            None => 0,
        };
        self.list_state.reset(count);
        cx.notify();
    }

    pub(super) fn toggle_language(&mut self, language: String, cx: &mut Context<Self>) {
        self.facet_filter.language = toggled(self.facet_filter.language.take(), language);
        self.refresh_visible(cx);
    }

    pub(super) fn toggle_directory(&mut self, directory: String, cx: &mut Context<Self>) {
        self.facet_filter.directory = toggled(self.facet_filter.directory.take(), directory);
        self.refresh_visible(cx);
    }

    pub(super) fn clear_language(&mut self, cx: &mut Context<Self>) {
        self.facet_filter.language = None;
        self.refresh_visible(cx);
    }

    pub(super) fn clear_directory(&mut self, cx: &mut Context<Self>) {
        self.facet_filter.directory = None;
        self.refresh_visible(cx);
    }

    pub(super) fn toggle_expanded(
        &mut self,
        visible_index: usize,
        path: String,
        cx: &mut Context<Self>,
    ) {
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        // The card changed height; have the list measure it again.
        self.list_state.splice(visible_index..visible_index + 1, 1);
        cx.notify();
    }

    pub(super) fn set_case_sensitive(&mut self, value: bool, cx: &mut Context<Self>) {
        self.case_sensitive = value;
        self.schedule_search(false, cx);
    }

    pub(super) fn set_whole_word(&mut self, value: bool, cx: &mut Context<Self>) {
        self.whole_word = value;
        self.schedule_search(false, cx);
    }

    pub(super) fn set_regex(&mut self, value: bool, cx: &mut Context<Self>) {
        self.regex = value;
        self.schedule_search(false, cx);
    }

    // ----- opening hits -----------------------------------------------------

    pub(super) fn open_hit(
        &mut self,
        path: &str,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(folder) = self.folder.as_ref() else {
            return;
        };
        let file = folder
            .workspace
            .root()
            .join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
        match editor::resolve(&file, line) {
            Launch::Command { program, args } => {
                if let Err(error) = editor::spawn(&program, &args) {
                    window.push_notification(
                        Notification::error(format!(
                            "Could not start {}: {error}. Set {} to choose an editor.",
                            program.display(),
                            editor::EDITOR_ENV
                        )),
                        cx,
                    );
                }
            }
            Launch::System => cx.open_with_system(&file),
        }
    }

    pub(super) fn reveal(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some(folder) = self.folder.as_ref() {
            let file = folder
                .workspace
                .root()
                .join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            cx.reveal_path(&file);
        }
    }

    pub(super) fn copy_path(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(path.to_string()));
        window.push_notification(format!("Copied {path}"), cx);
    }

    // ----- actions ------------------------------------------------------------

    pub(super) fn on_open_folder(
        &mut self,
        _: &OpenFolder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prompt_for_folder(window, cx);
    }

    pub(super) fn on_focus_search(
        &mut self,
        _: &FocusSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    pub(super) fn on_focus_path_filter(
        &mut self,
        _: &FocusPathFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.path_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    pub(super) fn on_toggle_case_sensitive(
        &mut self,
        _: &ToggleCaseSensitive,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_case_sensitive(!self.case_sensitive, cx);
    }

    pub(super) fn on_toggle_whole_word(
        &mut self,
        _: &ToggleWholeWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_whole_word(!self.whole_word, cx);
    }

    pub(super) fn on_toggle_regex(
        &mut self,
        _: &ToggleRegex,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_regex(!self.regex, cx);
    }

    pub(super) fn on_rebuild_index(
        &mut self,
        _: &RebuildIndex,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.build_index(cx);
    }

    pub(super) fn on_toggle_theme(
        &mut self,
        _: &ToggleTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mode = if cx.theme().is_dark() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        Theme::change(mode, Some(window), cx);
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

/// Select `value`, or clear the selection when it is already selected.
fn toggled(current: Option<String>, value: String) -> Option<String> {
    if current.as_deref() == Some(value.as_str()) {
        None
    } else {
        Some(value)
    }
}
