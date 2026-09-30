use std::sync::Arc;

use super::{Preview, PreviewState, RevealTarget};
use crate::ui::render::match_style;
use crate::ui::{CONTEXT, ClosePreview, FocusSearch, NextMatch, PreviousMatch};
use dowse::engine::preview::{self, FilePreview};
use dowse::engine::repo::RepoInfo;
use gpui_kit as gpui;
use gpui_kit::component::input::Editor;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    Context, Entity, InteractiveElement as _, IntoElement, KeyBinding, MouseButton,
    ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window, div, point,
    px,
};
use regex::Regex;

struct Harness {
    preview: Preview,
    actions: Vec<&'static str>,
}

impl Render for Harness {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.preview.ensure_editor(window, cx);
        div()
            .size_full()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &ClosePreview, _, _| this.actions.push("close")))
            .on_action(cx.listener(|this, _: &FocusSearch, _, _| this.actions.push("search")))
            .on_action(cx.listener(|this, _: &NextMatch, _, _| this.actions.push("next")))
            .on_action(cx.listener(|this, _: &PreviousMatch, _, _| this.actions.push("previous")))
            .child(
                Editor::new(&self.preview.editor.as_ref().unwrap().state)
                    .readonly(true)
                    .bordered(false)
                    .size_full(),
            )
    }
}

fn file(text: &str) -> Arc<FilePreview> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preview.txt");
    std::fs::write(&path, text).unwrap();
    Arc::new(preview::load(&path, None).unwrap())
}

fn open<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
    line: usize,
) -> (Entity<Harness>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.bind_keys([
            KeyBinding::new("secondary-f", FocusSearch, Some(CONTEXT)),
            KeyBinding::new("f4", NextMatch, Some(CONTEXT)),
            KeyBinding::new("shift-f4", PreviousMatch, Some(CONTEXT)),
            KeyBinding::new("escape", ClosePreview, Some(CONTEXT)),
        ]);
    });
    let preview = Preview {
        repo: Arc::new(RepoInfo {
            id: "test".into(),
            name: "test".into(),
            root: Default::default(),
            branch: None,
            tags: Vec::new(),
            pull_every: None,
        }),
        path: "preview.txt".into(),
        language: None,
        state: PreviewState::Ready(file(text)),
        editor: None,
        target: Some(RevealTarget::Line(line)),
        task: None,
    };
    let (view, cx) = cx.add_window_view(move |_, _| Harness {
        preview,
        actions: Vec::new(),
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (view, cx)
}

#[gpui::test]
fn native_copy_preserves_source_and_rejects_edits(cx: &mut TestAppContext) {
    let text = "\tfoo  \r\nbar\t \n";
    let (view, cx) = open(cx, text, 1);
    let state = cx.read(|cx| view.read(cx).preview.editor.as_ref().unwrap().state.clone());
    cx.update(|window, cx| {
        state.update(cx, |state, cx| {
            assert!(!state.is_editable());
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        window.draw(cx).clear(cx);
    });
    #[cfg(target_os = "macos")]
    cx.simulate_keystrokes("cmd-c");
    #[cfg(not(target_os = "macos"))]
    cx.simulate_keystrokes("ctrl-c");
    cx.read(|cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(text.into())
        );
    });
    cx.simulate_keystrokes("x backspace enter");
    cx.read(|cx| assert_eq!(state.read(cx).text().to_string(), text));
}

#[gpui::test]
fn native_drag_selects_across_lines_and_moves_navigation_cursor(cx: &mut TestAppContext) {
    let text = "first\nsecond\nthird";
    let (view, cx) = open(cx, text, 1);
    let state = cx.read(|cx| view.read(cx).preview.editor.as_ref().unwrap().state.clone());
    let (start, end) = cx.read(|cx| {
        let state = state.read(cx);
        let first = state.range_to_bounds(&(0..1)).unwrap();
        let last = state.range_to_bounds(&(17..18)).unwrap();
        (
            point(first.left(), first.top() + first.size.height / 2.),
            point(last.right(), last.top() + last.size.height / 2.),
        )
    });
    cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
    cx.simulate_mouse_move(end, MouseButton::Left, Default::default());
    cx.simulate_mouse_up(end, MouseButton::Left, Default::default());
    cx.read(|cx| {
        assert_eq!(state.read(cx).selected_text().to_string(), text);
        assert_eq!(view.read(cx).preview.current_line(cx), 3);
    });
    #[cfg(target_os = "macos")]
    cx.simulate_keystrokes("cmd-c");
    #[cfg(not(target_os = "macos"))]
    cx.simulate_keystrokes("ctrl-c");
    cx.read(|cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(text.into())
        );
    });
}

#[gpui::test]
fn query_and_theme_refresh_preserve_selection_and_scroll(cx: &mut TestAppContext) {
    let text = format!("{}\n", "foo bar ".repeat(40)).repeat(2_000);
    let (view, cx) = open(cx, &text, 500);
    let state = cx.read(|cx| view.read(cx).preview.editor.as_ref().unwrap().state.clone());
    cx.update(|window, cx| {
        state.update(cx, |state, cx| {
            let offset = state.cursor();
            state.set_selected_range(offset..offset + 7, cx);
            state.set_scroll_offset(point(px(-80.), px(-8_000.)), cx);
        });
        window.draw(cx).clear(cx);
    });
    let (selection, scroll) = cx.read(|cx| {
        (
            state.read(cx).selected_range(),
            state.read(cx).scroll_offset(),
        )
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let refreshed = view
                .preview
                .loaded()
                .unwrap()
                .with_matches(Some(&Regex::new("bar").unwrap()));
            view.preview.state = PreviewState::Ready(Arc::new(refreshed));
            cx.notify();
        });
        Theme::change(ThemeMode::Dark, Some(window), cx);
        window.draw(cx).clear(cx);
    });
    cx.read(|cx| {
        let editor = view.read(cx).preview.editor.as_ref().unwrap();
        assert_eq!(editor.state, state);
        assert_eq!(state.read(cx).selected_range(), selection);
        assert_eq!(state.read(cx).scroll_offset(), scroll);
        assert_eq!(editor.decorations.get_ranges(cx), editor.file.highlights);
        assert_eq!(editor.style, match_style(cx));
    });
}

#[gpui::test]
fn native_cursor_reveals_lines_beyond_ten_thousand(cx: &mut TestAppContext) {
    let text = "source line\n".repeat(10_050);
    let (view, cx) = open(cx, &text, 10_020);
    cx.read(|cx| {
        let preview = &view.read(cx).preview;
        let state = preview.editor.as_ref().unwrap().state.read(cx);
        assert_eq!(state.text().to_string(), text);
        assert_eq!(preview.current_line(cx), 10_020);
        assert!(state.visible_row_range().unwrap().contains(&10_019));
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.preview.select(10_030);
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    cx.read(|cx| {
        let preview = &view.read(cx).preview;
        assert_eq!(preview.current_line(cx), 10_030);
        assert!(
            preview
                .editor
                .as_ref()
                .unwrap()
                .state
                .read(cx)
                .visible_row_range()
                .unwrap()
                .contains(&10_029)
        );
    });
}

#[gpui::test]
fn changed_source_reuses_editor_and_clamps_cursor(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "first\nsecond\nthird\n", 3);
    let state = cx.read(|cx| view.read(cx).preview.editor.as_ref().unwrap().state.clone());
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.preview.state = PreviewState::Ready(file("changed"));
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    cx.read(|cx| {
        let preview = &view.read(cx).preview;
        assert_eq!(preview.editor.as_ref().unwrap().state, state);
        assert_eq!(state.read(cx).text().to_string(), "changed");
        assert_eq!(preview.current_line(cx), 1);
    });
}

#[gpui::test]
fn native_editor_wraps_long_lines_without_changing_source(cx: &mut TestAppContext) {
    let text = format!("{}\tfoo  \n", "a".repeat(20_000));
    let (view, cx) = open(cx, &text, 1);
    let state = cx.read(|cx| view.read(cx).preview.editor.as_ref().unwrap().state.clone());
    cx.update(|window, cx| {
        state.update(cx, |state, cx| {
            state.set_selected_range(20_000..20_006, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.read(|cx| {
        assert_eq!(state.read(cx).text().to_string(), text);
        assert_eq!(state.read(cx).selected_text().to_string(), "\tfoo  ");
        assert_eq!(state.read(cx).scroll_offset().x, px(0.));
        assert!(state.read(cx).scroll_offset().y < px(0.));
        assert!(state.read(cx).range_to_bounds(&(20_000..20_006)).is_some());
        assert_eq!(view.read(cx).preview.current_line(cx), 1);
    });
}

#[gpui::test]
fn focused_editor_keeps_application_shortcuts(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "source", 1);
    let state = cx.read(|cx| view.read(cx).preview.editor.as_ref().unwrap().state.clone());
    cx.update(|window, cx| {
        state.update(cx, |state, cx| state.focus(window, cx));
        window.draw(cx).clear(cx);
    });
    cx.simulate_keystrokes("f4 shift-f4 escape");
    #[cfg(target_os = "macos")]
    cx.simulate_keystrokes("cmd-f");
    #[cfg(not(target_os = "macos"))]
    cx.simulate_keystrokes("ctrl-f");
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).actions,
            vec!["next", "previous", "close", "search"]
        );
    });
}

#[gpui::test]
fn cursor_reveals_each_occurrence_inside_wrapped_source(cx: &mut TestAppContext) {
    let text = format!("{}needle needle\n\tneedle", "prefix ".repeat(3_000));
    let (view, cx) = open(cx, &text, 1);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let file = view
                .preview
                .loaded()
                .unwrap()
                .with_matches(Some(&Regex::new("needle").unwrap()));
            view.preview.state = PreviewState::Ready(Arc::new(file));
            view.preview.select(1);
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    let matches = cx.read(|cx| view.read(cx).preview.loaded().unwrap().matches.clone());
    assert_eq!(matches.len(), 3);
    for (index, &offset) in matches.iter().enumerate() {
        cx.read(|cx| {
            let preview = &view.read(cx).preview;
            let state = preview.editor.as_ref().unwrap().state.read(cx);
            assert_eq!(state.cursor(), offset);
            assert_eq!(state.selected_range(), offset..offset);
            assert!(state.range_to_bounds(&(offset..offset + 6)).is_some());
            assert_eq!(state.scroll_offset().x, px(0.));
            assert_eq!(
                preview.loaded().unwrap().match_ordinal(offset),
                Some(index + 1)
            );
        });
        if index + 1 < matches.len() {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    let offset = view
                        .preview
                        .loaded()
                        .unwrap()
                        .next_match(view.preview.current_offset(cx))
                        .unwrap();
                    view.preview.select_match(offset);
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
        }
    }
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let offset = view
                .preview
                .loaded()
                .unwrap()
                .previous_match(view.preview.current_offset(cx))
                .unwrap();
            view.preview.select_match(offset);
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    cx.read(|cx| {
        assert_eq!(view.read(cx).preview.current_offset(cx), matches[1]);
        assert_eq!(view.read(cx).preview.current_line(cx), 1);
    });
}

#[gpui::test]
fn file_edges_reveal_first_and_last_occurrences(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "start foo foo\nlast foo foo", 1);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let file = view
                .preview
                .loaded()
                .unwrap()
                .with_matches(Some(&Regex::new("foo").unwrap()));
            view.preview.state = PreviewState::Ready(Arc::new(file));
            view.preview.select_edge(false);
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    cx.read(|cx| {
        let preview = &view.read(cx).preview;
        assert_eq!(
            preview.current_offset(cx),
            *preview.loaded().unwrap().matches.last().unwrap()
        );
        assert_eq!(preview.current_line(cx), 2);
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.preview.select_edge(true);
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    cx.read(|cx| {
        let preview = &view.read(cx).preview;
        assert_eq!(
            preview.current_offset(cx),
            preview.loaded().unwrap().matches[0]
        );
        assert_eq!(preview.current_line(cx), 1);
    });
}

#[gpui::test]
fn restored_occurrence_offset_survives_initial_editor_layout(cx: &mut TestAppContext) {
    let (view, cx) = open(cx, "start foo foo\nlast foo", 1);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let file = view
                .preview
                .loaded()
                .unwrap()
                .with_matches(Some(&Regex::new("foo").unwrap()));
            let offset = file.matches[1];
            view.preview.state = PreviewState::Ready(Arc::new(file));
            view.preview.editor = None;
            view.preview.select_match(offset);
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.read(|cx| {
        let preview = &view.read(cx).preview;
        let offset = preview.loaded().unwrap().matches[1];
        assert_eq!(
            preview.editor.as_ref().unwrap().state.read(cx).cursor(),
            offset
        );
        assert_eq!(preview.current_offset(cx), offset);
        assert_eq!(preview.current_line(cx), 1);
    });
}
