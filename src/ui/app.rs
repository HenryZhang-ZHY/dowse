//! State and behaviour of the main view: the query, running searches over the
//! repositories in scope, and facet filtering of the results. Repository
//! management lives in `repos.rs`; rendering in `render.rs` and `repos_page.rs`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, WindowExt as _};
use gpui_kit::*;

use super::repos::RepoState;
use super::{
    AddRepository, FocusPathFilter, FocusSearch, RebuildIndex, ShowRepositories,
    ToggleCaseSensitive, ToggleRegex, ToggleTheme, ToggleWholeWord,
};
use crate::editor::{self, Launch};
use tgrep_gpui::engine::facets::{FacetFilter, FacetKind, Facets};
use tgrep_gpui::engine::query::{CompiledQuery, SearchQuery};
use tgrep_gpui::engine::registry::Registry;
use tgrep_gpui::engine::repo::Scope;
use tgrep_gpui::engine::search::{self, FileMatch, SearchLimits, SearchOutcome};

/// Pause after a keystroke before searching, so typing a word runs one search.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Page {
    Search,
    Repositories,
}

pub struct SearchApp {
    pub(super) page: Page,
    pub(super) registry: Registry,
    /// Set when the saved registry could not be read, so it is never overwritten.
    pub(super) registry_locked: bool,
    /// Sorted by name.
    pub(super) repos: Vec<RepoState>,
    pub(super) scope: Scope,
    pub(super) search_input: Entity<InputState>,
    pub(super) path_input: Entity<InputState>,
    pub(super) case_sensitive: bool,
    pub(super) whole_word: bool,
    pub(super) regex: bool,
    pub(super) results: Option<Results>,
    pub(super) query_error: Option<String>,
    /// A search is running (or waiting for repositories to load).
    pub(super) searching: bool,
    pub(super) facet_filter: FacetFilter,
    /// Files whose every kept line is shown, keyed by [`file_key`].
    pub(super) expanded: HashSet<String>,
    pub(super) list_state: ListState,
    pub(super) search_task: Option<Task<()>>,
    pub(super) search_cancel: Arc<AtomicBool>,
    _poll_task: Task<()>,
    _subscriptions: Vec<Subscription>,
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

/// Identifies a result file across repositories.
pub(super) fn file_key(file: &FileMatch) -> String {
    format!("{}\u{0}{}", file.repo.id, file.path)
}

impl SearchApp {
    /// `folders` are added to the saved repositories, as if chosen with
    /// "Add repository".
    pub fn new(folders: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search code…"));
        let path_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter paths: src *.rs !test"));
        let subscriptions = vec![
            cx.subscribe_in(&search_input, window, Self::on_input_event),
            cx.subscribe_in(&path_input, window, Self::on_input_event),
        ];
        search_input.update(cx, |input, cx| input.focus(window, cx));

        let this = Self {
            page: Page::Search,
            registry: Registry::default(),
            registry_locked: false,
            repos: Vec::new(),
            scope: Scope::default(),
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
            _poll_task: Self::poll_repositories(cx),
            _subscriptions: subscriptions,
        };
        // After construction, once the window's `Root` exists to show notifications.
        cx.defer_in(window, move |this, window, cx| {
            this.restore_registry(window, cx);
            if !folders.is_empty() {
                this.add_repositories(folders, window, cx);
            }
        });
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

    pub(super) fn show_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        if page == Page::Search {
            self.search_input
                .update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
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

    /// Search the repositories in scope for the current query, replacing any
    /// search in flight.
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
        let (sources, waiting) = self.search_sources();
        // Repositories still loading trigger another search when they finish.
        self.searching = true;
        if sources.is_empty() && waiting {
            cx.notify();
            return;
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.search_cancel = cancel.clone();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            }
            let outcome = cx
                .background_spawn(async move {
                    search::search(&sources, &compiled, &SearchLimits::default(), &cancel)
                })
                .await;
            if outcome.cancelled {
                return;
            }
            this.update(cx, |this, cx| {
                this.searching = waiting;
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

    pub(super) fn toggle_facet(&mut self, kind: FacetKind, value: String, cx: &mut Context<Self>) {
        self.facet_filter.toggle(kind, value);
        self.refresh_visible(cx);
    }

    pub(super) fn clear_facet(&mut self, kind: &FacetKind, cx: &mut Context<Self>) {
        self.facet_filter.clear(kind);
        self.refresh_visible(cx);
    }

    pub(super) fn toggle_expanded(
        &mut self,
        visible_index: usize,
        key: String,
        cx: &mut Context<Self>,
    ) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
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
        root: &Path,
        path: &str,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let file = full_path(root, path);
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

    pub(super) fn reveal(&mut self, root: &Path, path: &str, cx: &mut Context<Self>) {
        cx.reveal_path(&full_path(root, path));
    }

    pub(super) fn copy_path(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(path.to_string()));
        window.push_notification(format!("Copied {path}"), cx);
    }

    // ----- actions ------------------------------------------------------------

    pub(super) fn on_add_repository(
        &mut self,
        _: &AddRepository,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prompt_for_repositories(window, cx);
    }

    pub(super) fn on_show_repositories(
        &mut self,
        _: &ShowRepositories,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = if self.page == Page::Repositories {
            Page::Search
        } else {
            Page::Repositories
        };
        self.show_page(page, window, cx);
    }

    pub(super) fn on_focus_search(
        &mut self,
        _: &FocusSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_page(Page::Search, window, cx);
    }

    pub(super) fn on_focus_path_filter(
        &mut self,
        _: &FocusPathFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.page = Page::Search;
        self.path_input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
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
        self.queue_scope_indexes(cx);
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

fn full_path(root: &Path, path: &str) -> PathBuf {
    root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR))
}
