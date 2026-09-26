//! Search tabs: each holds a query, its options and results, so several
//! searches can stay open in a window and be switched between. The
//! repositories and scope belong to the window and are shared by every tab.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::input::InputState;
use gpui_kit::*;
use regex::Regex;

use super::app::{SearchApp, SnippetSyntax};
use super::preview::Preview;
use super::windows::Windows;
use tgrep_gpui::engine::facets::{FacetFilter, Facets};
use tgrep_gpui::engine::query::SearchQuery;
use tgrep_gpui::engine::search::{FileMatch, SearchOutcome};

/// Characters of the query shown on a tab.
const TAB_LABEL_CHARS: usize = 24;

pub(super) struct SearchTab {
    /// Stays the same while tabs open and close around it.
    pub(super) id: usize,
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
    /// What is searched changed while the tab was in the background, so it
    /// searches again when shown.
    pub(super) stale: bool,
    /// The result file shown beside the results.
    pub(super) preview: Option<Preview>,
    _subscriptions: Vec<Subscription>,
}

/// A finished search and the facet view over it.
pub(super) struct Results {
    pub(super) outcome: Arc<SearchOutcome>,
    /// The line matcher that found them, which the preview marks matches with.
    pub(super) matcher: Regex,
    pub(super) facets: Facets,
    /// Indexes into `outcome.files` that pass the facet filters.
    pub(super) visible: Vec<usize>,
    /// Syntax styles of each file's snippets, by index into `outcome.files`,
    /// computed as files are first shown.
    pub(super) syntax: HashMap<usize, Rc<SnippetSyntax>>,
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

impl SearchTab {
    pub(super) fn query(&self, cx: &App) -> SearchQuery {
        SearchQuery {
            pattern: self.search_input.read(cx).value().to_string(),
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
            path_filter: self.path_input.read(cx).value().trim().to_string(),
        }
    }

    /// What the tab strip shows: the query, shortened.
    pub(super) fn label(&self, cx: &App) -> String {
        let pattern = self.search_input.read(cx).value();
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return "New search".into();
        }
        let mut label: String = pattern.chars().take(TAB_LABEL_CHARS).collect();
        if pattern.chars().count() > TAB_LABEL_CHARS {
            label.push('…');
        }
        label
    }

    /// Stop the search in flight, if any.
    pub(super) fn cancel(&mut self) {
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_task = None;
    }

    pub(super) fn set_results(&mut self, outcome: Option<(SearchOutcome, Regex)>) {
        self.expanded.clear();
        self.results = outcome.map(|(outcome, matcher)| Results {
            outcome: Arc::new(outcome),
            matcher,
            facets: Facets::default(),
            visible: Vec::new(),
            syntax: HashMap::new(),
        });
        self.refresh_visible();
    }

    /// Re-apply the facet filters and reset the list to the top.
    pub(super) fn refresh_visible(&mut self) {
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
    }
}

impl SearchApp {
    pub(super) fn tab(&self) -> &SearchTab {
        &self.tabs[self.active_tab]
    }

    pub(super) fn tab_mut(&mut self) -> &mut SearchTab {
        &mut self.tabs[self.active_tab]
    }

    /// A tab showing `query`, not yet searched.
    pub(super) fn create_tab(
        &mut self,
        query: SearchQuery,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SearchTab {
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search code…  e.g. parse config lang:rust -path:tests")
                .default_value(query.pattern)
        });
        let path_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter paths: src *.rs !test")
                .default_value(query.path_filter)
        });
        let subscriptions = vec![
            cx.subscribe_in(&search_input, window, Self::on_input_event),
            cx.subscribe_in(&path_input, window, Self::on_input_event),
        ];
        self.next_tab_id += 1;
        SearchTab {
            id: self.next_tab_id,
            search_input,
            path_input,
            case_sensitive: query.case_sensitive,
            whole_word: query.whole_word,
            regex: query.regex,
            results: None,
            query_error: None,
            searching: false,
            facet_filter: FacetFilter::default(),
            expanded: HashSet::new(),
            list_state: ListState::new(0, ListAlignment::Top, px(800.)),
            search_task: None,
            search_cancel: Arc::new(AtomicBool::new(false)),
            stale: true,
            preview: None,
            _subscriptions: subscriptions,
        }
    }

    /// Open an empty tab after the current one and show it.
    pub(super) fn open_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.create_tab(SearchQuery::default(), window, cx);
        self.tabs.insert(self.active_tab + 1, tab);
        self.activate_tab(self.active_tab + 1, window, cx);
    }

    /// Close a tab. The last one is emptied instead, so a window always has
    /// somewhere to type.
    pub(super) fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            let tab = self.tab_mut();
            tab.cancel();
            let (search_input, path_input) = (tab.search_input.clone(), tab.path_input.clone());
            search_input.update(cx, |input, cx| input.set_value("", window, cx));
            path_input.update(cx, |input, cx| input.set_value("", window, cx));
            self.run_search(false, cx);
            self.show_search(window, cx);
            return;
        }
        let mut tab = self.tabs.remove(index);
        tab.cancel();
        let active = if index < self.active_tab || self.active_tab == self.tabs.len() {
            self.active_tab - 1
        } else {
            self.active_tab
        };
        // Show the new active tab even if it kept its index.
        self.active_tab = active;
        self.activate_tab(active, window, cx);
    }

    pub(super) fn activate_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.tabs.len() {
            return;
        }
        self.active_tab = index;
        if self.tab().stale {
            self.run_search(false, cx);
        }
        self.show_search(window, cx);
        Windows::save(cx);
    }

    /// Step through the tabs, wrapping around.
    pub(super) fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.tabs.len();
        let index = if forward {
            (self.active_tab + 1) % count
        } else {
            (self.active_tab + count - 1) % count
        };
        self.activate_tab(index, window, cx);
    }

    /// The queries of every tab, for the session.
    pub(super) fn tab_queries(&self, cx: &App) -> Vec<SearchQuery> {
        self.tabs.iter().map(|tab| tab.query(cx)).collect()
    }
}
