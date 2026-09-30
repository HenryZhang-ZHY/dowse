//! Back and Forward: each tab remembers what it showed, as a browser tab
//! remembers pages: its searches, with their filters and the file being
//! previewed, and the repositories page.
//! Typing changes the search being shown; it becomes a step of its own once
//! it settles: after a pause, on Enter, or once a result is previewed or
//! filtered. A different query after that starts the next step. Going to the
//! repositories page and leaving it, other than by Back or Forward, is a step
//! too. That page shows no tabs, so switching tabs leaves it for the search.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;

use super::app::{Page, SearchApp};
use super::preview::PreviewState;
use super::tabs::SearchTab;
use super::{GoBack, GoForward};
use dowse::engine::facets::FacetFilter;
use dowse::engine::query::SearchQuery;
use dowse::engine::repo::RepoInfo;

/// A query that stays this long counts as a step.
const SETTLE_AFTER: Duration = Duration::from_millis(1000);
/// Steps kept each way.
const MAX_STEPS: usize = 100;

/// Something a tab showed, to go back or forward to.
enum Entry {
    Search(Visit),
    Repositories,
}

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
    offset: Option<usize>,
}

impl Visit {
    /// `tab` as it is now, for `query`.
    fn of(tab: &SearchTab, query: SearchQuery, cx: &App) -> Self {
        Self {
            query,
            facet_filter: tab.facet_filter.clone(),
            preview: tab.preview.as_ref().map(|preview| PreviewSpot {
                repo: preview.repo.clone(),
                path: preview.path.clone(),
                language: preview.language,
                line: preview.current_line(cx),
                offset: matches!(&preview.state, PreviewState::Ready(_))
                    .then(|| preview.current_offset(cx)),
            }),
            preview_open: tab.preview_open,
        }
    }
}

#[derive(Default)]
pub(super) struct History {
    back: Vec<Entry>,
    forward: Vec<Entry>,
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

fn push(steps: &mut Vec<Entry>, entry: Entry) {
    if steps.len() == MAX_STEPS {
        steps.remove(0);
    }
    steps.push(entry);
}

impl SearchApp {
    pub(super) fn can_go_back(&self) -> bool {
        self.tab().history.can_go_back()
    }

    pub(super) fn can_go_forward(&self) -> bool {
        self.tab().history.can_go_forward()
    }

    pub(super) fn back_tooltip(&self) -> &'static str {
        match self.tab().history.back.last() {
            Some(Entry::Repositories) => "Back to the repositories (Alt+Left)",
            _ => "Back to the previous search (Alt+Left)",
        }
    }

    pub(super) fn forward_tooltip(&self) -> &'static str {
        match self.tab().history.forward.last() {
            Some(Entry::Repositories) => "Forward to the repositories (Alt+Right)",
            _ => "Forward to the next search (Alt+Right)",
        }
    }

    /// What the current tab shows, as a step to come back to.
    fn here(&self, cx: &App) -> Entry {
        match self.page {
            Page::Repositories => Entry::Repositories,
            Page::Search => Entry::Search(Visit::of(self.tab(), self.tab().query(cx), cx)),
        }
    }

    /// The current tab is about to show `page` instead of another: what it
    /// shows now becomes the step to go back to.
    pub(super) fn note_page(&mut self, page: Page, cx: &App) {
        if page == self.page {
            return;
        }
        let here = self.here(cx);
        let history = &mut self.tab_mut().history;
        push(&mut history.back, here);
        history.forward.clear();
    }

    /// Forget every tab's steps, as when the workspace changes. Back still
    /// leaves the repositories page for the search.
    pub(super) fn forget_history(&mut self, cx: &App) {
        for tab in &mut self.tabs {
            tab.history = History::default();
        }
        if self.page == Page::Repositories {
            let visit = Visit::of(self.tab(), self.tab().query(cx), cx);
            self.tab_mut().history.back.push(Entry::Search(visit));
        }
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
                let visit = Visit::of(tab, settled, cx);
                push(&mut tab.history.back, Entry::Search(visit));
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
        let here = self.here(cx);
        let history = &mut self.tab_mut().history;
        let (from, to) = if back {
            (&mut history.back, &mut history.forward)
        } else {
            (&mut history.forward, &mut history.back)
        };
        let Some(target) = from.pop() else {
            return;
        };
        push(to, here);
        match target {
            Entry::Search(visit) => self.restore(visit, window, cx),
            Entry::Repositories => self.set_page(Page::Repositories, window, cx),
        }
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
            if let Some(offset) = spot.offset
                && let Some(preview) = self.tab_mut().preview.as_mut()
            {
                preview.select_match(offset);
            }
        }
        self.tab_mut().preview_open = visit.preview_open;
        self.run_search(false, cx);
        self.set_page(Page::Search, window, cx);
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

#[cfg(test)]
mod tests;
