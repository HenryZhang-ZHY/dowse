//! Back and Forward: each tab remembers the searches it showed, with their
//! filters and the file being previewed, as a browser remembers pages.
//! Typing changes the search being shown; it becomes a step of its own once
//! it settles: after a pause, on Enter, or once a result is previewed or
//! filtered. A different query after that starts the next step. From the
//! repositories page, Back returns to the search.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;

use super::app::{Page, SearchApp};
use super::tabs::SearchTab;
use super::{GoBack, GoForward};
use dowse::engine::facets::FacetFilter;
use dowse::engine::query::SearchQuery;
use dowse::engine::repo::RepoInfo;

/// A query that stays this long counts as a step.
const SETTLE_AFTER: Duration = Duration::from_millis(1000);
/// Steps kept each way.
const MAX_STEPS: usize = 100;

/// One search as a tab showed it.
struct Visit {
    query: SearchQuery,
    facet_filter: FacetFilter,
    preview: Option<PreviewSpot>,
    preview_open: bool,
}

/// The file a preview showed, and where.
struct PreviewSpot {
    repo: Arc<RepoInfo>,
    path: String,
    language: Option<&'static str>,
    line: usize,
}

impl Visit {
    /// `tab` as it is now, for `query`.
    fn of(tab: &SearchTab, query: SearchQuery) -> Self {
        Self {
            query,
            facet_filter: tab.facet_filter.clone(),
            preview: tab.preview.as_ref().map(|preview| PreviewSpot {
                repo: preview.repo.clone(),
                path: preview.path.clone(),
                language: preview.language,
                line: preview.line,
            }),
            preview_open: tab.preview_open,
        }
    }
}

#[derive(Default)]
pub(super) struct History {
    back: Vec<Visit>,
    forward: Vec<Visit>,
    /// The query shown, once it has settled.
    settled: Option<SearchQuery>,
    settle_task: Option<Task<()>>,
}

impl History {
    pub(super) fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub(super) fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

fn push(steps: &mut Vec<Visit>, visit: Visit) {
    if steps.len() == MAX_STEPS {
        steps.remove(0);
    }
    steps.push(visit);
}

impl SearchApp {
    pub(super) fn can_go_back(&self) -> bool {
        self.page == Page::Repositories || self.tab().history.can_go_back()
    }

    /// The current tab is about to search for `query`: when it replaces a
    /// settled search, that search becomes the step to go back to.
    pub(super) fn note_search(&mut self, query: &SearchQuery, cx: &mut Context<Self>) {
        let tab = self.tab_mut();
        if tab
            .history
            .settled
            .as_ref()
            .is_some_and(|settled| settled != query)
        {
            let settled = tab.history.settled.take().unwrap_or_default();
            if !settled.is_empty() {
                let visit = Visit::of(tab, settled);
                push(&mut tab.history.back, visit);
                tab.history.forward.clear();
            }
        }
        if tab.history.settled.is_some() {
            return;
        }
        if query.is_empty() {
            tab.history.settle_task = None;
            return;
        }
        let (tab_id, query) = (tab.id, query.clone());
        tab.history.settle_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE_AFTER).await;
            this.update(cx, |this, cx| {
                let Some(index) = this.tabs.iter().position(|tab| tab.id == tab_id) else {
                    return;
                };
                if this.tabs[index].query(cx) == query {
                    this.tabs[index].history.settled = Some(query);
                }
            })
            .ok();
        }));
    }

    /// The current search counts as a step now, without waiting for a pause.
    pub(super) fn settle_search(&mut self, cx: &mut Context<Self>) {
        let query = self.tab().query(cx);
        let history = &mut self.tab_mut().history;
        history.settle_task = None;
        if !query.is_empty() {
            history.settled = Some(query);
        }
    }

    pub(super) fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step_history(true, window, cx);
    }

    pub(super) fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step_history(false, window, cx);
    }

    fn step_history(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if back && self.page == Page::Repositories {
            self.show_search(window, cx);
            return;
        }
        let query = self.tab().query(cx);
        let tab = self.tab_mut();
        let target = if back {
            tab.history.back.pop()
        } else {
            tab.history.forward.pop()
        };
        let Some(target) = target else {
            return;
        };
        if !query.is_empty() {
            let here = Visit::of(tab, query);
            let other = if back {
                &mut tab.history.forward
            } else {
                &mut tab.history.back
            };
            push(other, here);
        }
        self.restore(target, window, cx);
    }

    /// Show `visit` in the current tab again.
    fn restore(&mut self, visit: Visit, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.tab_mut();
        tab.case_sensitive = visit.query.case_sensitive;
        tab.whole_word = visit.query.whole_word;
        tab.regex = visit.query.regex;
        tab.facet_filter = visit.facet_filter;
        tab.preview = None;
        tab.preview_open = false;
        // Settled already, so searching for it again is no new step.
        tab.history.settled = Some(visit.query.clone());
        tab.history.settle_task = None;
        let (search_input, path_input) = (tab.search_input.clone(), tab.path_input.clone());
        search_input.update(cx, |input, cx| {
            input.set_value(visit.query.pattern.clone(), window, cx)
        });
        path_input.update(cx, |input, cx| {
            input.set_value(visit.query.path_filter.clone(), window, cx)
        });
        if let Some(spot) = visit.preview {
            self.preview_hit(spot.repo, spot.path, spot.language, spot.line, cx);
        }
        self.tab_mut().preview_open = visit.preview_open;
        self.run_search(false, cx);
        self.show_search(window, cx);
    }

    pub(super) fn on_go_back(&mut self, _: &GoBack, window: &mut Window, cx: &mut Context<Self>) {
        self.go_back(window, cx);
    }

    pub(super) fn on_go_forward(
        &mut self,
        _: &GoForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.go_forward(window, cx);
    }
}
