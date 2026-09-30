use std::path::Path;
use std::sync::Arc;

use gpui_kit as gpui;
use gpui_kit::component::Root;
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};

use super::SETTLE_AFTER;
use crate::ui::app::{Page, SearchApp, TabsOpening};
use crate::ui::tasks::TaskHub;
use crate::ui::windows::{Opening, Windows};
use dowse::engine::facets::FacetKind;
use dowse::engine::repo::RepoInfo;

fn open(cx: &mut TestAppContext) -> (Entity<SearchApp>, &mut VisualTestContext) {
    let config = tempfile::tempdir().unwrap().keep();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        Windows::init(config, cx);
        TaskHub::init(cx);
    });
    let mut app = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view =
            cx.new(|cx| SearchApp::new(Opening::empty(), TabsOpening::default(), window, cx));
        app = Some(view.clone());
        Root::new(view, window, cx)
    });
    (app.unwrap(), cx)
}

/// Type `text` into the search box, as a keystroke would.
fn type_query(app: &Entity<SearchApp>, text: &str, cx: &mut VisualTestContext) {
    app.update_in(cx, |this, window, cx| {
        let input = this.tab().search_input.clone();
        input.update(cx, |input, cx| {
            input.set_value(text.to_string(), window, cx)
        });
        this.run_search(true, cx);
    });
}

/// Let the query shown stay long enough to count as a step.
fn pause(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(SETTLE_AFTER);
    cx.run_until_parked();
}

fn pattern(app: &Entity<SearchApp>, cx: &mut VisualTestContext) -> String {
    app.read_with(cx, |this, cx| this.tab().query(cx).pattern)
}

fn page(app: &Entity<SearchApp>, cx: &mut VisualTestContext) -> Page {
    app.read_with(cx, |this, _| this.page)
}

fn back(app: &Entity<SearchApp>, cx: &mut VisualTestContext) {
    app.update_in(cx, |this, window, cx| this.go_back(window, cx));
    cx.run_until_parked();
}

fn forward(app: &Entity<SearchApp>, cx: &mut VisualTestContext) {
    app.update_in(cx, |this, window, cx| this.go_forward(window, cx));
    cx.run_until_parked();
}

fn can_go(app: &Entity<SearchApp>, cx: &mut VisualTestContext) -> (bool, bool) {
    app.read_with(cx, |this, _| (this.can_go_back(), this.can_go_forward()))
}

fn show_page(app: &Entity<SearchApp>, page: Page, cx: &mut VisualTestContext) {
    app.update_in(cx, |this, window, cx| this.show_page(page, window, cx));
    cx.run_until_parked();
}

fn toggle_language(app: &Entity<SearchApp>, language: &str, cx: &mut VisualTestContext) {
    let language = language.to_string();
    app.update(cx, |this, cx| {
        this.toggle_facet(FacetKind::Language, language, cx)
    });
}

fn language(app: &Entity<SearchApp>, cx: &mut VisualTestContext) -> Option<String> {
    app.read_with(cx, |this, _| {
        this.tab()
            .facet_filter
            .get(&FacetKind::Language)
            .map(str::to_string)
    })
}

/// A repository at `root` holding `a.txt` and `b.txt`.
fn repo(root: &Path) -> Arc<RepoInfo> {
    for name in ["a.txt", "b.txt"] {
        std::fs::write(root.join(name), "one\ntwo\nthree\n").unwrap();
    }
    Arc::new(RepoInfo {
        id: "test".into(),
        name: "test".into(),
        root: root.to_path_buf(),
        branch: None,
        tags: Vec::new(),
        pull_every: None,
    })
}

/// Click line `line` of `path` in the results.
fn pick(
    app: &Entity<SearchApp>,
    repo: &Arc<RepoInfo>,
    path: &str,
    line: usize,
    cx: &mut VisualTestContext,
) {
    let (repo, path) = (repo.clone(), path.to_string());
    app.update(cx, |this, cx| this.pick_hit(repo, path, None, line, cx));
    cx.run_until_parked();
}

fn previewed(app: &Entity<SearchApp>, cx: &mut VisualTestContext) -> Option<String> {
    app.read_with(cx, |this, _| {
        this.tab()
            .preview
            .as_ref()
            .map(|preview| preview.path.clone())
    })
}

#[gpui::test]
fn typing_a_new_search_after_a_pause_is_a_step(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    type_query(&app, "foo", cx);
    pause(cx);
    type_query(&app, "bar", cx);
    assert_eq!(can_go(&app, cx), (true, false));

    back(&app, cx);
    assert_eq!(pattern(&app, cx), "foo");
    assert_eq!(can_go(&app, cx), (false, true));

    forward(&app, cx);
    assert_eq!(pattern(&app, cx), "bar");
    assert_eq!(can_go(&app, cx), (true, false));
}

#[gpui::test]
fn repositories_page_is_a_step_both_ways(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    type_query(&app, "foo", cx);
    show_page(&app, Page::Repositories, cx);
    assert_eq!(can_go(&app, cx), (true, false));

    back(&app, cx);
    assert_eq!(page(&app, cx), Page::Search);
    assert_eq!(pattern(&app, cx), "foo");
    assert_eq!(can_go(&app, cx), (false, true));

    forward(&app, cx);
    assert_eq!(page(&app, cx), Page::Repositories);
    assert_eq!(can_go(&app, cx), (true, false));
}

#[gpui::test]
fn back_from_repositories_passes_through_earlier_searches(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    type_query(&app, "foo", cx);
    pause(cx);
    type_query(&app, "bar", cx);
    show_page(&app, Page::Repositories, cx);

    back(&app, cx);
    assert_eq!(
        (page(&app, cx), pattern(&app, cx)),
        (Page::Search, "bar".into())
    );
    back(&app, cx);
    assert_eq!(
        (page(&app, cx), pattern(&app, cx)),
        (Page::Search, "foo".into())
    );
    forward(&app, cx);
    forward(&app, cx);
    assert_eq!(page(&app, cx), Page::Repositories);
}

#[gpui::test]
fn leaving_repositories_other_than_back_is_a_new_step(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    type_query(&app, "foo", cx);
    show_page(&app, Page::Repositories, cx);
    show_page(&app, Page::Search, cx);
    assert_eq!(can_go(&app, cx), (true, false));

    back(&app, cx);
    assert_eq!(page(&app, cx), Page::Repositories);
    back(&app, cx);
    assert_eq!(
        (page(&app, cx), pattern(&app, cx)),
        (Page::Search, "foo".into())
    );
}

#[gpui::test]
fn new_step_from_repositories_drops_forward(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    type_query(&app, "foo", cx);
    pause(cx);
    type_query(&app, "bar", cx);
    pause(cx);
    back(&app, cx);
    assert_eq!(can_go(&app, cx), (false, true));

    show_page(&app, Page::Repositories, cx);
    assert_eq!(can_go(&app, cx), (true, false));
}

#[gpui::test]
fn switching_tabs_leaves_repositories_as_a_step_of_the_old_tab(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    type_query(&app, "foo", cx);
    show_page(&app, Page::Repositories, cx);
    app.update_in(cx, |this, window, cx| this.open_tab(window, cx));
    cx.run_until_parked();
    assert_eq!(page(&app, cx), Page::Search);
    // The new tab has been nowhere yet.
    assert_eq!(can_go(&app, cx), (false, false));

    app.update_in(cx, |this, window, cx| this.activate_tab(0, window, cx));
    cx.run_until_parked();
    assert_eq!(
        (page(&app, cx), pattern(&app, cx)),
        (Page::Search, "foo".into())
    );
    back(&app, cx);
    assert_eq!(page(&app, cx), Page::Repositories);
}

#[gpui::test]
fn back_still_leaves_repositories_after_the_workspace_changes(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    type_query(&app, "foo", cx);
    pause(cx);
    type_query(&app, "bar", cx);
    show_page(&app, Page::Repositories, cx);
    app.update(cx, |this, cx| this.reset_search(cx));
    assert_eq!(can_go(&app, cx), (true, false));

    back(&app, cx);
    assert_eq!(
        (page(&app, cx), pattern(&app, cx)),
        (Page::Search, "bar".into())
    );
    assert_eq!(can_go(&app, cx), (false, true));
}

#[gpui::test]
fn filtering_a_settled_search_is_a_step(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    type_query(&app, "foo", cx);
    pause(cx);
    toggle_language(&app, "Rust", cx);
    assert_eq!(can_go(&app, cx), (true, false));

    back(&app, cx);
    assert_eq!(
        (pattern(&app, cx), language(&app, cx)),
        ("foo".into(), None)
    );
    forward(&app, cx);
    assert_eq!(language(&app, cx).as_deref(), Some("Rust"));

    app.update(cx, |this, cx| this.clear_facet(&FacetKind::Language, cx));
    back(&app, cx);
    assert_eq!(language(&app, cx).as_deref(), Some("Rust"));
}

#[gpui::test]
fn filtering_while_typing_is_part_of_the_same_step(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    type_query(&app, "foo", cx);
    toggle_language(&app, "Rust", cx);
    assert_eq!(can_go(&app, cx), (false, false));

    type_query(&app, "bar", cx);
    back(&app, cx);
    assert_eq!(
        (pattern(&app, cx), language(&app, cx)),
        ("foo".into(), Some("Rust".into()))
    );
}

#[gpui::test]
fn clearing_a_filter_that_is_not_set_is_no_step(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    type_query(&app, "foo", cx);
    pause(cx);
    app.update(cx, |this, cx| this.clear_facet(&FacetKind::Language, cx));
    assert_eq!(can_go(&app, cx), (false, false));
}

#[gpui::test]
fn previewing_another_file_is_a_step(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo(dir.path());
    let (app, cx) = open(cx);
    type_query(&app, "foo", cx);
    pause(cx);
    pick(&app, &repo, "a.txt", 1, cx);
    pick(&app, &repo, "a.txt", 3, cx);
    pick(&app, &repo, "b.txt", 2, cx);

    back(&app, cx);
    assert_eq!(previewed(&app, cx).as_deref(), Some("a.txt"));
    back(&app, cx);
    assert_eq!(previewed(&app, cx), None);
    assert_eq!(can_go(&app, cx), (false, true));
    forward(&app, cx);
    forward(&app, cx);
    assert_eq!(previewed(&app, cx).as_deref(), Some("b.txt"));
}

#[gpui::test]
fn previewing_while_typing_is_part_of_the_same_step(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo(dir.path());
    let (app, cx) = open(cx);
    type_query(&app, "foo", cx);
    pick(&app, &repo, "a.txt", 1, cx);
    assert_eq!(can_go(&app, cx), (false, false));
}
