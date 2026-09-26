//! The preview pane: clicking a result shows its whole file beside the
//! results, coloured by language and scrolled to the line, without waiting
//! for an editor to start. From there the file opens in the editor at the
//! chosen line. Each tab has its own preview.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::app::{SearchApp, full_path};
use super::highlight::{self, LineStyles};
use super::render::{centered_message, code_text};
use super::{ClosePreview, NextMatch, PreviousMatch};
use crate::format;
use tgrep_gpui::engine::preview::{self, FilePreview};
use tgrep_gpui::engine::repo::RepoInfo;
use tgrep_gpui::engine::search::FileMatch;

const LINE_NUMBER_WIDTH: f32 = 64.;

pub(super) struct Preview {
    pub(super) repo: Arc<RepoInfo>,
    /// Relative to the repository root, `/`-separated.
    pub(super) path: String,
    pub(super) language: Option<&'static str>,
    /// The chosen line, 1-based: highlighted, and where the editor opens.
    pub(super) line: usize,
    pub(super) state: PreviewState,
    pub(super) scroll: UniformListScrollHandle,
    task: Option<Task<()>>,
}

pub(super) enum PreviewState {
    Loading,
    Ready(Arc<Loaded>),
    Failed(String),
}

pub(super) struct Loaded {
    pub(super) file: FilePreview,
    /// Syntax styles by line; empty without a grammar.
    pub(super) syntax: Vec<LineStyles>,
}

impl Preview {
    fn shows(&self, repo: &RepoInfo, path: &str) -> bool {
        self.repo.id == repo.id && self.path == path
    }

    pub(super) fn shows_file(&self, file: &FileMatch) -> bool {
        self.shows(&file.repo, &file.path)
    }

    fn loaded(&self) -> Option<&Arc<Loaded>> {
        match &self.state {
            PreviewState::Ready(loaded) => Some(loaded),
            _ => None,
        }
    }

    /// Choose `line` and bring it into view.
    fn select(&mut self, line: usize) {
        self.line = line;
        self.scroll
            .scroll_to_item(line.saturating_sub(1), ScrollStrategy::Center);
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
        let tab = self.tab_mut();
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
            scroll: UniformListScrollHandle::new(),
            task: None,
        });
        self.load_preview(self.active_tab, true, cx);
    }

    /// Read the previewed file of the tab at `index` again, with its current
    /// query and the current theme, then scroll to the chosen line when
    /// `reveal` is set.
    pub(super) fn load_preview(&mut self, index: usize, reveal: bool, cx: &mut Context<Self>) {
        let theme = cx.theme().highlight_theme.clone();
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let tab_id = tab.id;
        let matcher = tab.results.as_ref().map(|results| results.matcher.clone());
        let Some(preview) = tab.preview.as_mut() else {
            return;
        };
        let file = full_path(&preview.repo.root, &preview.path);
        let grammar = preview.language.and_then(highlight::grammar);
        let (repo, path) = (preview.repo.clone(), preview.path.clone());
        preview.task = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let file = preview::load(&file, matcher.as_ref())?;
                    let syntax = match grammar {
                        Some(grammar) => {
                            let lines: Vec<&str> =
                                file.lines.iter().map(|line| line.text.as_str()).collect();
                            highlight::highlight_once(grammar, &lines, &theme)
                        }
                        None => Vec::new(),
                    };
                    Ok::<_, String>(Loaded { file, syntax })
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
                        preview.line = preview.line.clamp(1, loaded.file.lines.len().max(1));
                        if reveal {
                            preview
                                .scroll
                                .scroll_to_item_strict(preview.line - 1, ScrollStrategy::Center);
                        }
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
        self.tab_mut().preview = None;
        cx.notify();
    }

    /// Go to the next (or previous) match: within the previewed file, then
    /// on to the neighbouring result file, wrapping around.
    pub(super) fn step_match(&mut self, forward: bool, cx: &mut Context<Self>) {
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
            let target = if forward {
                loaded.file.next_match(preview.line)
            } else {
                loaded.file.previous_match(preview.line)
            };
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

    /// Open the previewed file in the editor at the chosen line.
    pub(super) fn open_preview_in_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preview) = self.tab().preview.as_ref() else {
            return;
        };
        let (root, path, line) = (
            preview.repo.root.clone(),
            preview.path.clone(),
            preview.line,
        );
        self.open_hit(&root, &path, line, window, cx);
    }

    pub(super) fn on_close_preview(
        &mut self,
        _: &ClosePreview,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tab().preview.is_some() {
            self.close_preview(cx);
        } else {
            cx.propagate();
        }
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

    pub(super) fn render_preview(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let preview = self.tab().preview.as_ref()?;
        let theme = cx.theme();
        let (border, muted, header_bg) = (theme.border, theme.muted_foreground, theme.secondary);
        let (radius, chip_bg) = (theme.radius, theme.background);
        let (directory, name) = match preview.path.rsplit_once('/') {
            Some((directory, name)) => (format!("{directory}/"), name.to_string()),
            None => (String::new(), preview.path.clone()),
        };
        let loaded = preview.loaded().cloned();
        let line = preview.line;

        let navigation = loaded
            .as_ref()
            .filter(|loaded| !loaded.file.matches.is_empty())
            .map(|loaded| {
                let total = loaded.file.matches.len();
                let position = match loaded.file.match_ordinal(line) {
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
                    .tooltip(format!(
                        "Open at line {line}; double-click a line to open there"
                    ))
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
            PreviewState::Ready(loaded) => {
                let theme = cx.theme();
                div()
                    .id("preview-body")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .font_family(theme.mono_font_family.clone())
                    .text_size(theme.mono_font_size)
                    .child(
                        uniform_list(
                            "preview-lines",
                            loaded.file.lines.len(),
                            cx.processor(|this, range: Range<usize>, _, cx| {
                                this.render_preview_lines(range, cx)
                            }),
                        )
                        .track_scroll(&preview.scroll)
                        .with_horizontal_sizing_behavior(
                            ListHorizontalSizingBehavior::Unconstrained,
                        )
                        .with_width_from_item(Some(loaded.file.widest))
                        .size_full()
                        .py_1(),
                    )
                    .scrollbar(&preview.scroll, ScrollbarAxis::Both)
                    .into_any_element()
            }
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

    fn render_preview_lines(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(preview) = self.tab().preview.as_ref() else {
            return Vec::new();
        };
        let Some(loaded) = preview.loaded().cloned() else {
            return Vec::new();
        };
        let selected = preview.line;
        let theme = cx.theme();
        let (muted, foreground, hover) =
            (theme.muted_foreground, theme.foreground, theme.list_hover);
        let selected_bg = theme
            .yellow
            .opacity(if theme.is_dark() { 0.16 } else { 0.18 });
        range
            .filter_map(|index| {
                let line = loaded.file.lines.get(index)?;
                let number = index + 1;
                let is_match = loaded.file.matches.binary_search(&index).is_ok();
                let styled = code_text(&line.text, loaded.syntax.get(index), &line.highlights, cx);
                Some(
                    h_flex()
                        .id(("preview-line", index))
                        .w_full()
                        .cursor_pointer()
                        .when(number == selected, |row| row.bg(selected_bg))
                        .when(number != selected, |row| {
                            row.hover(move |style| style.bg(hover))
                        })
                        .child(
                            div()
                                .flex_none()
                                .w(px(LINE_NUMBER_WIDTH))
                                .pr_3()
                                .text_right()
                                .when(is_match, |gutter| {
                                    gutter.text_color(foreground).font_semibold()
                                })
                                .when(!is_match, |gutter| gutter.text_color(muted))
                                .child(number.to_string()),
                        )
                        .child(div().flex_none().whitespace_nowrap().pr_4().child(styled))
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            if let Some(preview) = this.tab_mut().preview.as_mut() {
                                preview.line = number;
                            }
                            if event.click_count() >= 2 {
                                this.open_preview_in_editor(window, cx);
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }
}
