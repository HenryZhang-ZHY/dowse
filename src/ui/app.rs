//! State and behaviour of the main view: running each tab's query over the
//! repositories in scope, and facet filtering of the results. Tabs live in
//! `tabs.rs`, repository management in `repos.rs`; rendering in `render.rs`
//! and `repos_page.rs`.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use dowse::diagnostics::metrics::{Origin, SearchRecord, metrics};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, WindowExt as _};
use gpui_kit::*;

use super::highlight::{self, Highlighters, LineStyles};
use super::history::History;
use super::hub::{IndexJob, RepoHub};
use super::manager::Manager;
use super::repos::TagInputs;
use super::tabs::SearchTab;
use super::tasks::TaskHub;
use super::windows::{Opening, Windows};
use super::{
    AddRepository, CloseTab, FocusPathFilter, FocusSearch, NewTab, NewWorkspace, NextTab,
    OpenWorkspace, PreviousTab, RebuildIndex, SaveWorkspaceAs, ShowRepositories,
    ToggleCaseSensitive, ToggleRegex, ToggleTheme, ToggleWholeWord, UpdateIndex,
};
use crate::editor::{self, Launch};
use dowse::engine::facets::{FacetFilter, FacetKind};
use dowse::engine::query::{CompiledQuery, SearchQuery};
use dowse::engine::repo::Scope;
use dowse::engine::search::{self, SearchLimits};

/// Pause after a keystroke before searching, so typing a word runs one search.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Page {
    Search,
    Repositories,
}

pub struct SearchApp {
    pub(super) page: Page,
    pub(super) hub: Entity<RepoHub>,
    /// The saved workspace file, or `None` for an untitled workspace.
    pub(super) workspace: Option<PathBuf>,
    /// The user agreed to close the window; don't ask again.
    pub(super) close_confirmed: bool,
    /// Ids of the repositories this window uses, each acquired from the hub.
    pub(super) members: Vec<String>,
    pub(super) tag_inputs: TagInputs,
    /// The repositories page.
    pub(super) manager: Manager,
    pub(super) scope: Scope,
    /// Open searches; never empty.
    pub(super) tabs: Vec<SearchTab>,
    pub(super) active_tab: usize,
    pub(super) next_tab_id: usize,
    pub(super) highlighters: Highlighters,
    /// The path filter box is shown, though the search box's `path:` does
    /// the same. It shows anyway while a tab has a path filter.
    pub(super) path_filter_open: bool,
    /// The filters sidebar is shown beside the results.
    pub(super) sidebar_open: bool,
    _subscriptions: Vec<Subscription>,
}

/// Styles for every line of every snippet of a file.
pub(super) type SnippetSyntax = Vec<Vec<LineStyles>>;

/// Something to do to a window, as a menu item or palette command does.
pub(super) type AppCommand = Rc<dyn Fn(&mut SearchApp, &mut Window, &mut Context<SearchApp>)>;

/// The search tabs a window starts with.
#[derive(Clone, Debug, Default)]
pub(super) struct TabsOpening {
    pub(super) queries: Vec<SearchQuery>,
    pub(super) active: usize,
}

impl SearchApp {
    /// A window showing `opening`, with the search tabs of `tabs`.
    pub(super) fn new(
        opening: Opening,
        tabs: TabsOpening,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let hub = RepoHub::global(cx);
        let tasks = TaskHub::global(cx);
        let subscriptions = vec![
            cx.subscribe_in(&hub, window, Self::on_hub_event),
            cx.observe(&hub, |_, _, cx| cx.notify()),
            cx.observe(&tasks, |_, _, cx| cx.notify()),
            cx.observe_global::<Theme>(|this, cx| this.theme_changed(cx)),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    Windows::activated(window.window_handle(), cx);
                    this.prefer_scope(cx);
                }
            }),
        ];
        cx.on_release(|this, cx| this.release_repositories(cx))
            .detach();
        let app = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            app.update(cx, |this, cx| this.should_close(window, cx))
                .unwrap_or(true)
        });

        let mut this = Self {
            page: Page::Search,
            hub,
            workspace: None,
            close_confirmed: false,
            members: Vec::new(),
            tag_inputs: TagInputs::default(),
            manager: Manager::new(window, cx),
            scope: Scope::default(),
            tabs: Vec::new(),
            active_tab: 0,
            next_tab_id: 0,
            highlighters: Highlighters::default(),
            path_filter_open: false,
            sidebar_open: true,
            _subscriptions: subscriptions,
        };
        let mut queries = tabs.queries;
        if queries.is_empty() {
            queries.push(SearchQuery::default());
        }
        for query in queries {
            let tab = this.create_tab(query, window, cx);
            this.tabs.push(tab);
        }
        this.active_tab = tabs.active.min(this.tabs.len() - 1);
        this.tab()
            .search_input
            .update(cx, |input, cx| input.focus(window, cx));

        // Load the workspace now, so requests right after opening (such as
        // `--add`) see its repositories.
        let mut errors = Windows::take_startup_errors(cx);
        errors.extend(this.apply_workspace(opening, window, cx).err());
        // Notifications wait for the window's `Root`, which hosts them.
        cx.defer_in(window, move |_, window, cx| {
            for error in errors {
                window.push_notification(Notification::error(error), cx);
            }
        });
        this
    }

    /// Forget every tab's results, as when the workspace changes.
    pub(super) fn reset_search(&mut self, cx: &mut Context<Self>) {
        for tab in &mut self.tabs {
            tab.cancel();
            tab.searching = false;
            tab.facet_filter = FacetFilter::default();
            tab.set_results(None);
            tab.preview = None;
            tab.history = History::default();
            tab.stale = true;
        }
        cx.notify();
    }

    pub(super) fn on_input_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = self.tab();
        if &tab.search_input != input && &tab.path_input != input {
            return;
        }
        match event {
            InputEvent::Change => self.run_search(true, cx),
            InputEvent::PressEnter { .. } => {
                self.run_search(false, cx);
                self.settle_search(cx);
            }
            InputEvent::Focus | InputEvent::Blur => {}
        }
    }

    pub(super) fn show_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if page == Page::Repositories && self.page != page {
            self.manager.forget_disk_state();
        }
        self.page = page;
        if page == Page::Search {
            self.tab()
                .search_input
                .update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    /// Show the search page with the current tab's search box focused.
    pub(super) fn show_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_page(Page::Search, window, cx);
    }

    // ----- searching --------------------------------------------------------

    /// What is searched changed (repositories, their indexes or the scope):
    /// search again in the current tab, and in the others once shown.
    pub(super) fn schedule_search(&mut self, debounce: bool, cx: &mut Context<Self>) {
        let active = self.active_tab;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if index != active {
                tab.cancel();
                tab.stale = true;
            }
        }
        self.run_search(debounce, cx);
    }

    /// Search the repositories in scope for the current tab's query,
    /// replacing any search in flight.
    pub(super) fn run_search(&mut self, debounce: bool, cx: &mut Context<Self>) {
        let (sources, waiting) = self.search_sources(cx);
        let query = self.tab().query(cx);
        self.note_search(&query, cx);
        let tab = self.tab_mut();
        tab.cancel();
        tab.stale = false;

        if query.is_empty() {
            tab.query_error = None;
            tab.searching = false;
            tab.set_results(None);
            tab.preview = None;
            Windows::save(cx);
            cx.notify();
            return;
        }
        let compiled = match CompiledQuery::new(&query) {
            Ok(compiled) => compiled,
            Err(error) => {
                tab.query_error = Some(error);
                tab.searching = false;
                cx.notify();
                return;
            }
        };
        tab.query_error = None;
        // Repositories still loading trigger another search when they finish.
        tab.searching = true;
        if sources.is_empty() && waiting {
            cx.notify();
            return;
        }

        let cancel = Arc::new(AtomicBool::new(false));
        tab.search_cancel = cancel.clone();
        let tab_id = tab.id;
        let matcher = compiled.matcher.clone();
        tab.search_task = Some(cx.spawn(async move |this, cx| {
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
            metrics().record_search(SearchRecord::new(Origin::Window, &query.pattern, &outcome));
            log::debug!(
                "searched {:?}: {} lines in {} files, read {} of {} files in {} repositories, {:.1?}",
                query.pattern,
                outcome.matched_lines,
                outcome.files.len(),
                outcome.searched_files,
                outcome.corpus_files,
                outcome.repos,
                outcome.elapsed
            );
            this.update(cx, |this, cx| {
                if let Some(index) = this.tabs.iter().position(|tab| tab.id == tab_id) {
                    let tab = &mut this.tabs[index];
                    tab.searching = waiting;
                    tab.set_results(Some((outcome, matcher)));
                    // Mark the new query's matches in the file being previewed.
                    if tab.preview.is_some() {
                        this.load_preview(index, false, cx);
                    }
                }
                // Each tab's query is part of the session.
                Windows::save(cx);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Syntax styles for the snippets of the file shown at `visible_index`,
    /// or `None` when its language has no grammar.
    pub(super) fn snippet_syntax(
        &mut self,
        visible_index: usize,
        cx: &App,
    ) -> Option<Rc<SnippetSyntax>> {
        let results = self.tabs[self.active_tab].results.as_mut()?;
        let index = *results.visible.get(visible_index)?;
        if let Some(syntax) = results.syntax.get(&index) {
            return Some(syntax.clone());
        }
        let file = &results.outcome.files[index];
        let grammar = highlight::grammar(file.language?)?;
        let theme = &cx.theme().highlight_theme;
        let syntax: SnippetSyntax = file
            .snippets
            .iter()
            .map(|snippet| {
                let lines: Vec<&str> = snippet
                    .lines
                    .iter()
                    .map(|line| line.text.as_str())
                    .collect();
                self.highlighters.highlight(grammar, &lines, theme)
            })
            .collect();
        let syntax = Rc::new(syntax);
        results.syntax.insert(index, syntax.clone());
        Some(syntax)
    }

    /// Syntax colours follow the theme.
    fn theme_changed(&mut self, cx: &mut Context<Self>) {
        for index in 0..self.tabs.len() {
            if let Some(results) = self.tabs[index].results.as_mut() {
                results.syntax.clear();
            }
        }
        cx.notify();
    }

    pub(super) fn toggle_facet(&mut self, kind: FacetKind, value: String, cx: &mut Context<Self>) {
        let tab = self.tab_mut();
        tab.facet_filter.toggle(kind, value);
        tab.refresh_visible();
        self.settle_search(cx);
        cx.notify();
    }

    pub(super) fn clear_facet(&mut self, kind: &FacetKind, cx: &mut Context<Self>) {
        let tab = self.tab_mut();
        tab.facet_filter.clear(kind);
        tab.refresh_visible();
        self.settle_search(cx);
        cx.notify();
    }

    pub(super) fn toggle_expanded(
        &mut self,
        visible_index: usize,
        key: String,
        cx: &mut Context<Self>,
    ) {
        let tab = self.tab_mut();
        if !tab.expanded.remove(&key) {
            tab.expanded.insert(key);
        }
        // The card changed height; have the list measure it again.
        tab.list_state.splice(visible_index..visible_index + 1, 1);
        cx.notify();
    }

    pub(super) fn set_case_sensitive(&mut self, value: bool, cx: &mut Context<Self>) {
        self.tab_mut().case_sensitive = value;
        self.run_search(false, cx);
        self.settle_search(cx);
    }

    pub(super) fn set_whole_word(&mut self, value: bool, cx: &mut Context<Self>) {
        self.tab_mut().whole_word = value;
        self.run_search(false, cx);
        self.settle_search(cx);
    }

    pub(super) fn set_regex(&mut self, value: bool, cx: &mut Context<Self>) {
        self.tab_mut().regex = value;
        self.run_search(false, cx);
        self.settle_search(cx);
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
        self.show_search(window, cx);
    }

    pub(super) fn on_focus_path_filter(
        &mut self,
        _: &FocusPathFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.page = Page::Search;
        self.path_filter_open = true;
        self.tab()
            .path_input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Whether the path filter box is shown.
    pub(super) fn path_filter_shown(&self, cx: &App) -> bool {
        self.path_filter_open || !self.tab().path_input.read(cx).value().trim().is_empty()
    }

    /// Show the path filter box and focus it, or hide it and drop the
    /// current tab's path filter.
    pub(super) fn toggle_path_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.path_filter_shown(cx) {
            self.path_filter_open = false;
            let input = self.tab().path_input.clone();
            if !input.read(cx).value().is_empty() {
                input.update(cx, |input, cx| input.set_value("", window, cx));
                self.run_search(false, cx);
            }
            self.tab()
                .search_input
                .update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
        } else {
            self.on_focus_path_filter(&FocusPathFilter, window, cx);
        }
    }

    pub(super) fn on_toggle_case_sensitive(
        &mut self,
        _: &ToggleCaseSensitive,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_case_sensitive(!self.tab().case_sensitive, cx);
    }

    pub(super) fn on_toggle_whole_word(
        &mut self,
        _: &ToggleWholeWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_whole_word(!self.tab().whole_word, cx);
    }

    pub(super) fn on_toggle_regex(
        &mut self,
        _: &ToggleRegex,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_regex(!self.tab().regex, cx);
    }

    pub(super) fn on_new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(window, cx);
    }

    pub(super) fn on_close_tab(
        &mut self,
        _: &CloseTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_tab(self.active_tab, window, cx);
    }

    pub(super) fn on_next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(true, window, cx);
    }

    pub(super) fn on_previous_tab(
        &mut self,
        _: &PreviousTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_tab(false, window, cx);
    }

    pub(super) fn on_update_index(
        &mut self,
        _: &UpdateIndex,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.queue_scope_indexes(IndexJob::Update, cx);
    }

    pub(super) fn on_rebuild_index(
        &mut self,
        _: &RebuildIndex,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.queue_scope_indexes(IndexJob::Rebuild, cx);
    }

    pub(super) fn on_new_workspace(
        &mut self,
        _: &NewWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_workspace(window, cx);
    }

    pub(super) fn on_open_workspace(
        &mut self,
        _: &OpenWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prompt_open_workspace(window, cx);
    }

    pub(super) fn on_save_workspace_as(
        &mut self,
        _: &SaveWorkspaceAs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_workspace_as(window, cx).detach();
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

pub(super) fn full_path(root: &Path, path: &str) -> PathBuf {
    root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR))
}
