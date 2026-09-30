//! The preview pane: clicking a result shows its whole file beside the
//! results, coloured by language and scrolled to the line, without waiting
//! for an editor to start. From there the file opens in the editor at the
//! cursor's line. Each tab has its own preview, and its pane can be hidden
//! and shown again from the title bar.

use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, TextDecoration, TextDecorationCollection};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::app::{SearchApp, full_path};
use super::highlight;
use super::render::{centered_message, match_style};
use super::{ClosePreview, NextMatch, PreviousMatch, TogglePreview};
use crate::format;
use dowse::engine::preview::{self, FilePreview};
use dowse::engine::repo::RepoInfo;
use dowse::engine::search::FileMatch;

pub(super) struct Preview {
    pub(super) repo: Arc<RepoInfo>,
    /// Relative to the repository root, `/`-separated.
    pub(super) path: String,
    pub(super) language: Option<&'static str>,
    /// The requested line until the editor's native cursor takes over.
    line: usize,
    pub(super) state: PreviewState,
    editor: Option<PreviewEditor>,
    reveal: bool,
    task: Option<Task<()>>,
}

pub(super) enum PreviewState {
    Loading,
    Ready(Arc<FilePreview>),
    Failed(String),
}

struct PreviewEditor {
    state: Entity<EditorState>,
    file: Arc<FilePreview>,
    decorations: TextDecorationCollection,
    style: HighlightStyle,
    _subscription: Subscription,
}

impl Preview {
    fn shows(&self, repo: &RepoInfo, path: &str) -> bool {
        self.repo.id == repo.id && self.path == path
    }

    pub(super) fn shows_file(&self, file: &FileMatch) -> bool {
        self.shows(&file.repo, &file.path)
    }

    fn loaded(&self) -> Option<&Arc<FilePreview>> {
        match &self.state {
            PreviewState::Ready(loaded) => Some(loaded),
            _ => None,
        }
    }

    pub(super) fn current_line(&self, cx: &App) -> usize {
        match &self.editor {
            Some(editor) if !self.reveal => {
                editor.state.read(cx).cursor_position().line as usize + 1
            }
            _ => self.line,
        }
    }

    /// Let the editor reveal `line` using its native cursor scrolling.
    fn select(&mut self, line: usize) {
        self.line = line;
        self.reveal = true;
    }

    fn ensure_editor<T: 'static>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        let style = match_style(cx);
        let Some(file) = self.loaded().cloned() else {
            return;
        };
        if self.editor.is_none() {
            let language = self.language.and_then(highlight::grammar).unwrap_or("text");
            let state = cx.new(|cx| {
                let mut state = EditorState::new(window, cx)
                    .language(language)
                    .searchable(false)
                    .default_value(SharedString::new(file.text.clone()));
                state.set_soft_wrap(false, window, cx);
                state.set_line_number(true, window, cx);
                state.set_folding(false, window, cx);
                state.set_readonly(true, cx);
                state
            });
            let decorations = state.update(cx, |state, cx| {
                state.create_decorations_collection(Vec::new(), cx)
            });
            let subscription = cx.observe(&state, |_, _, cx| cx.notify());
            self.editor = Some(PreviewEditor {
                state,
                file: file.clone(),
                decorations,
                style,
                _subscription: subscription,
            });
            self.reveal = true;
        }
        let editor = self
            .editor
            .as_mut()
            .expect("preview editor was initialized");
        if !Arc::ptr_eq(&editor.file.text, &file.text) {
            if !self.reveal {
                self.line = editor.state.read(cx).cursor_position().line as usize + 1;
            }
            editor.state.update(cx, |state, cx| {
                state.set_value(SharedString::new(file.text.clone()), window, cx);
            });
            self.reveal = true;
        }
        if !Arc::ptr_eq(&editor.file, &file) || editor.style != style || self.reveal {
            editor.decorations.set(
                file.highlights
                    .iter()
                    .map(|range| TextDecoration::new(range.clone(), style))
                    .collect(),
                cx,
            );
            editor.file = file.clone();
            editor.style = style;
        }
        if self.reveal && editor.state.read(cx).line_height().is_some() {
            self.line = self.line.clamp(1, file.line_starts.len());
            let offset = file.line_offset(self.line);
            editor.state.update(cx, |state, cx| {
                state.set_selected_range(offset..offset, cx);
            });
            self.reveal = false;
        }
    }
}

impl SearchApp {
    /// Show `path` of `repo` in the current tab's preview, at `line`.
    pub(super) fn preview_hit(
        &mut self,
        repo: Arc<RepoInfo>,
        path: String,
        language: Option<&'static str>,
        line: usize,
        cx: &mut Context<Self>,
    ) {
        // Looking at a result makes its search a step to come back to.
        self.settle_search(cx);
        let tab = self.tab_mut();
        tab.preview_open = true;
        if let Some(preview) = tab.preview.as_mut()
            && preview.shows(&repo, &path)
        {
            preview.select(line);
            cx.notify();
            return;
        }
        tab.preview = Some(Preview {
            repo,
            path,
            language,
            line,
            state: PreviewState::Loading,
            editor: None,
            reveal: true,
            task: None,
        });
        self.load_preview(self.active_tab, true, cx);
    }

    /// Read the file and mark the current query without replacing an unchanged
    /// editor buffer, so its selection and scroll survive search refreshes.
    pub(super) fn load_preview(&mut self, index: usize, reveal: bool, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let tab_id = tab.id;
        let matcher = tab.results.as_ref().map(|results| results.matcher.clone());
        let Some(preview) = tab.preview.as_mut() else {
            return;
        };
        // This load replaces one that may still be on its way to the line.
        preview.reveal |= reveal || matches!(preview.state, PreviewState::Loading);
        let file = full_path(&preview.repo.root, &preview.path);
        let previous = preview.loaded().cloned();
        let (repo, path) = (preview.repo.clone(), preview.path.clone());
        preview.task = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let file = preview::load(&file, None)?;
                    let source = previous
                        .as_deref()
                        .filter(|previous| previous.text == file.text)
                        .unwrap_or(&file);
                    Ok::<_, String>(source.with_matches(matcher.as_ref()))
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(preview) = this
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == tab_id)
                    .and_then(|tab| tab.preview.as_mut())
                    .filter(|preview| preview.shows(&repo, &path))
                else {
                    return;
                };
                preview.state = match loaded {
                    Ok(loaded) => {
                        preview.line = preview.line.clamp(1, loaded.line_starts.len());
                        PreviewState::Ready(Arc::new(loaded))
                    }
                    Err(error) => PreviewState::Failed(error),
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(super) fn close_preview(&mut self, cx: &mut Context<Self>) {
        let tab = self.tab_mut();
        tab.preview = None;
        tab.preview_open = false;
        cx.notify();
    }

    /// Hide the preview pane, keeping its file, or show it again.
    pub(super) fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        let tab = self.tab_mut();
        tab.preview_open = !tab.preview_open;
        cx.notify();
    }

    /// Go to the next (or previous) match: within the previewed file, then
    /// on to the neighbouring result file, wrapping around.
    pub(super) fn step_match(&mut self, forward: bool, cx: &mut Context<Self>) {
        let line = self
            .tab()
            .preview
            .as_ref()
            .map(|preview| preview.current_line(cx));
        let tab = self.tab_mut();
        let Some(results) = tab.results.as_ref() else {
            return;
        };
        let count = results.visible.len();
        if count == 0 {
            return;
        }
        let current = tab.preview.as_ref().and_then(|preview| {
            (0..count).find(|&i| results.file(i).is_some_and(|file| preview.shows_file(file)))
        });
        if let (Some(preview), Some(_)) = (tab.preview.as_mut(), current) {
            let Some(loaded) = preview.loaded().cloned() else {
                // Still loading; its matches are not known yet.
                return;
            };
            let target = line.and_then(|line| {
                if forward {
                    loaded.next_match(line)
                } else {
                    loaded.previous_match(line)
                }
            });
            if let Some(line) = target {
                preview.select(line);
                cx.notify();
                return;
            }
        }
        let next = match current {
            Some(current) if forward => (current + 1) % count,
            Some(current) => (current + count - 1) % count,
            None if forward => 0,
            None => count - 1,
        };
        let Some(file) = results.file(next) else {
            return;
        };
        let line = if forward {
            file.first_match_line()
        } else {
            file.snippets
                .iter()
                .rev()
                .flat_map(|snippet| snippet.lines.iter().rev())
                .find(|line| line.is_match)
                .map(|line| line.number)
        }
        .unwrap_or(1);
        let (repo, path, language) = (file.repo.clone(), file.path.clone(), file.language);
        tab.list_state.scroll_to_reveal_item(next);
        self.preview_hit(repo, path, language, line, cx);
    }

    /// Open the previewed file in the external editor at the native cursor.
    pub(super) fn open_preview_in_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preview) = self.tab().preview.as_ref() else {
            return;
        };
        let (root, path, line) = (
            preview.repo.root.clone(),
            preview.path.clone(),
            preview.current_line(cx),
        );
        self.open_hit(&root, &path, line, window, cx);
    }

    pub(super) fn on_close_preview(
        &mut self,
        _: &ClosePreview,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tab().preview_open {
            self.close_preview(cx);
        } else {
            cx.propagate();
        }
    }

    pub(super) fn on_toggle_preview(
        &mut self,
        _: &TogglePreview,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_preview(cx);
    }

    pub(super) fn on_next_match(&mut self, _: &NextMatch, _: &mut Window, cx: &mut Context<Self>) {
        self.step_match(true, cx);
    }

    pub(super) fn on_previous_match(
        &mut self,
        _: &PreviousMatch,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_match(false, cx);
    }

    // ----- rendering -------------------------------------------------------------

    pub(super) fn render_preview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.tab().preview_open
            && let Some(preview) = self.tab_mut().preview.as_mut()
        {
            preview.ensure_editor(window, cx);
        }
        let tab = self.tab();
        if !tab.preview_open {
            return None;
        }
        let Some(preview) = tab.preview.as_ref() else {
            return Some(
                v_flex()
                    .id("preview")
                    .size_full()
                    .min_w_0()
                    .border_l_1()
                    .border_color(cx.theme().border)
                    .child(centered_message(
                        cx,
                        Icon::new(IconName::PanelRight).text_color(cx.theme().muted_foreground),
                        "Nothing to preview",
                        "Click a line in the results to show its file here.".to_string(),
                    ))
                    .into_any_element(),
            );
        };
        let theme = cx.theme();
        let (border, muted, header_bg) = (theme.border, theme.muted_foreground, theme.secondary);
        let (radius, chip_bg) = (theme.radius, theme.background);
        let (directory, name) = match preview.path.rsplit_once('/') {
            Some((directory, name)) => (format!("{directory}/"), name.to_string()),
            None => (String::new(), preview.path.clone()),
        };
        let loaded = preview.loaded().cloned();
        let line = preview.current_line(cx);

        let navigation = loaded
            .as_ref()
            .filter(|loaded| !loaded.matches.is_empty())
            .map(|loaded| {
                let total = loaded.matches.len();
                let position = match loaded.match_ordinal(line) {
                    Some(ordinal) => format!("{ordinal} of {}", format::count(total)),
                    None => format::plural(total, "match", "matches"),
                };
                h_flex()
                    .flex_none()
                    .gap_0p5()
                    .child(div().text_xs().text_color(muted).pr_1().child(position))
                    .child(
                        Button::new("preview-previous")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ChevronUp)
                            .tooltip("Previous match (Shift+F4)")
                            .on_click(cx.listener(|this, _, _, cx| this.step_match(false, cx))),
                    )
                    .child(
                        Button::new("preview-next")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ChevronDown)
                            .tooltip("Next match (F4)")
                            .on_click(cx.listener(|this, _, _, cx| this.step_match(true, cx))),
                    )
            });

        let copy_path = full_path(&preview.repo.root, &preview.path)
            .to_string_lossy()
            .into_owned();
        let (reveal_root, reveal_path) = (preview.repo.root.clone(), preview.path.clone());
        let header = h_flex()
            .flex_none()
            .gap_2()
            .px_3()
            .py_1p5()
            .bg(header_bg)
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .px_1p5()
                    .py_0p5()
                    .rounded(radius)
                    .border_1()
                    .border_color(border)
                    .bg(chip_bg)
                    .text_xs()
                    .child(Icon::new(Lucide::FolderGit2).xsmall().text_color(muted))
                    .child(preview.repo.name.clone())
                    .when_some(preview.repo.branch.clone(), |chip, branch| {
                        chip.child(Icon::new(Lucide::GitBranch).xsmall().text_color(muted))
                            .child(div().text_color(muted).child(branch))
                    }),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_sm()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(muted)
                            .child(directory),
                    )
                    .child(div().flex_none().font_semibold().child(name)),
            )
            .children(navigation)
            .child(
                Button::new("preview-open")
                    .primary()
                    .xsmall()
                    .icon(Lucide::SquareArrowOutUpRight)
                    .label("Open in Editor")
                    .tooltip(format!("Open at cursor line {line}"))
                    .disabled(loaded.is_none())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_preview_in_editor(window, cx)),
                    ),
            )
            .child(
                Button::new("preview-copy")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Copy)
                    .tooltip("Copy path")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.copy_path(&copy_path, window, cx)
                    })),
            )
            .child(
                Button::new("preview-reveal")
                    .ghost()
                    .xsmall()
                    .icon(IconName::FolderOpen)
                    .tooltip("Reveal in file manager")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.reveal(&reveal_root, &reveal_path, cx)
                    })),
            )
            .child(
                Button::new("preview-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("Close preview (Esc)")
                    .on_click(cx.listener(|this, _, _, cx| this.close_preview(cx))),
            );

        let body = match &preview.state {
            PreviewState::Loading => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element(),
            PreviewState::Failed(error) => centered_message(
                cx,
                Icon::new(IconName::TriangleAlert).text_color(muted),
                "Can't preview this file",
                error.clone(),
            )
            .into_any_element(),
            PreviewState::Ready(_) => div()
                .id("preview-body")
                .flex_1()
                .min_h_0()
                .child(
                    Editor::new(
                        &preview
                            .editor
                            .as_ref()
                            .expect("ready preview has an editor")
                            .state,
                    )
                    .readonly(true)
                    .bordered(false)
                    .size_full(),
                )
                .into_any_element(),
        };

        Some(
            v_flex()
                .id("preview")
                .size_full()
                .min_w_0()
                .border_l_1()
                .border_color(border)
                .child(header)
                .child(body)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests;
