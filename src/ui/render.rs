//! How the main view looks: header with the search bar, facet sidebar,
//! result cards and the status bar.

use std::path::PathBuf;
use std::time::SystemTime;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::CONTEXT;
use super::app::{IndexActivity, SearchApp};
use crate::format;
use tgrep_gpui::engine::search::{FileMatch, Snippet, SnippetLine};
use tgrep_gpui::engine::workspace::{IndexStatus, display_path};

/// Matching lines shown per file before "Show more".
const COLLAPSED_MATCH_LINES: usize = 6;
/// Facet values listed per section.
const FACET_ROWS: usize = 30;
const SIDEBAR_WIDTH: f32 = 240.;
const LINE_NUMBER_WIDTH: f32 = 60.;

impl Render for SearchApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.folder.is_some() {
            h_flex()
                .flex_1()
                .min_h_0()
                .items_start()
                .child(self.render_sidebar(cx))
                .child(self.render_results(window, cx))
                .into_any_element()
        } else {
            self.render_welcome(cx).into_any_element()
        };

        v_flex()
            .id("search-app")
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::on_open_folder))
            .on_action(cx.listener(Self::on_focus_search))
            .on_action(cx.listener(Self::on_focus_path_filter))
            .on_action(cx.listener(Self::on_toggle_case_sensitive))
            .on_action(cx.listener(Self::on_toggle_whole_word))
            .on_action(cx.listener(Self::on_toggle_regex))
            .on_action(cx.listener(Self::on_rebuild_index))
            .on_action(cx.listener(Self::on_toggle_theme))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_header(cx))
            .child(body)
            .child(self.render_status_bar(cx))
    }
}

impl SearchApp {
    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let has_folder = self.folder.is_some();

        let toggles = h_flex()
            .gap_0p5()
            .child(
                option_toggle(
                    "case-sensitive",
                    IconName::CaseSensitive,
                    "Match case (Alt+C)",
                    self.case_sensitive,
                )
                .on_click(
                    cx.listener(|this, _, _, cx| this.set_case_sensitive(!this.case_sensitive, cx)),
                ),
            )
            .child(
                option_toggle(
                    "whole-word",
                    Lucide::WholeWord,
                    "Match whole word (Alt+W)",
                    self.whole_word,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_whole_word(!this.whole_word, cx))),
            )
            .child(
                option_toggle(
                    "regex",
                    Lucide::Regex,
                    "Use regular expression (Alt+R)",
                    self.regex,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_regex(!this.regex, cx))),
            );

        let folder_button = match self.folder.as_ref() {
            Some(folder) => Button::new("open-folder")
                .ghost()
                .small()
                .icon(IconName::FolderOpen)
                .label(folder.workspace.name())
                .tooltip(format!(
                    "{} — open another folder (Ctrl+O)",
                    folder.workspace.display_root()
                )),
            None => Button::new("open-folder")
                .ghost()
                .small()
                .icon(IconName::FolderOpen)
                .label("Open Folder")
                .tooltip("Open a folder (Ctrl+O)"),
        }
        .on_click(cx.listener(|this, _, window, cx| this.prompt_for_folder(window, cx)));

        let theme_icon = if theme.is_dark() {
            IconName::Sun
        } else {
            IconName::Moon
        };

        h_flex()
            .flex_none()
            .gap_3()
            .px_4()
            .py_2p5()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.title_bar)
            .child(
                h_flex()
                    .flex_none()
                    .gap_1p5()
                    .child(Icon::new(Lucide::TextSearch).text_color(theme.primary))
                    .child(div().font_semibold().child("tgrep")),
            )
            .child(
                div().flex_1().max_w(px(960.)).child(
                    Input::new(&self.search_input)
                        .prefix(Icon::new(IconName::Search).small().text_color(muted))
                        .suffix(toggles)
                        .cleanable(true)
                        .disabled(!has_folder),
                ),
            )
            .child(
                div().w(px(300.)).flex_none().child(
                    Input::new(&self.path_input)
                        .prefix(Icon::new(Lucide::Funnel).small().text_color(muted))
                        .cleanable(true)
                        .disabled(!has_folder),
                ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .ml_auto()
                    .gap_1()
                    .child(folder_button)
                    .child(
                        Button::new("toggle-theme")
                            .ghost()
                            .small()
                            .icon(theme_icon)
                            .tooltip("Toggle light and dark theme")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.on_toggle_theme(&super::ToggleTheme, window, cx)
                            })),
                    ),
            )
    }

    // ----- sidebar ------------------------------------------------------------

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let sidebar = v_flex()
            .id("sidebar")
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .gap_5()
            .px_3()
            .py_3()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground);

        let Some(results) = self.results.as_ref() else {
            return sidebar
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Filters for languages and directories appear here once a search finds matches."),
                )
                .overflow_y_scrollbar();
        };

        let languages = self.facet_section(
            "Language",
            &results.facets.languages,
            self.facet_filter.language.as_deref(),
            FacetHandlers {
                toggle: |this, value, cx| this.toggle_language(value, cx),
                clear: |this, cx| this.clear_language(cx),
            },
            cx,
        );
        let directories = self.facet_section(
            "Directory",
            &results.facets.directories,
            self.facet_filter.directory.as_deref(),
            FacetHandlers {
                toggle: |this, value, cx| this.toggle_directory(value, cx),
                clear: |this, cx| this.clear_directory(cx),
            },
            cx,
        );
        sidebar
            .child(languages)
            .child(directories)
            .overflow_y_scrollbar()
    }

    fn facet_section(
        &self,
        title: &'static str,
        entries: &[(String, usize)],
        selected: Option<&str>,
        handlers: FacetHandlers,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (hover, active, muted) = (theme.list_hover, theme.accent, theme.muted_foreground);
        let (active_foreground, link) = (theme.accent_foreground, theme.link);
        let radius = theme.radius;
        let on_toggle = handlers.toggle;
        let on_clear = handlers.clear;

        let rows = entries.iter().take(FACET_ROWS).map(|(name, count)| {
            let is_selected = selected == Some(name.as_str());
            let value = name.clone();
            h_flex()
                .id(SharedString::from(format!("facet:{title}:{name}")))
                .gap_2()
                .px_2()
                .py_1()
                .rounded(radius)
                .cursor_pointer()
                .text_sm()
                .hover(move |style| style.bg(hover))
                .when(is_selected, |row| {
                    row.bg(active).text_color(active_foreground).font_semibold()
                })
                .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format::count(*count)),
                )
                .on_click(cx.listener(move |this, _, _, cx| on_toggle(this, value.clone(), cx)))
        });
        let hidden = entries.len().saturating_sub(FACET_ROWS);
        let empty = entries.is_empty();

        v_flex()
            .gap_0p5()
            .when(empty, |section| section.hidden())
            .child(
                h_flex()
                    .px_2()
                    .pb_1()
                    .text_xs()
                    .child(
                        div()
                            .flex_1()
                            .font_semibold()
                            .text_color(muted)
                            .child(title.to_uppercase()),
                    )
                    .when(selected.is_some(), |header| {
                        header.child(
                            div()
                                .id(SharedString::from(format!("clear:{title}")))
                                .cursor_pointer()
                                .text_color(link)
                                .hover(|style| style.underline())
                                .child("Clear")
                                .on_click(cx.listener(move |this, _, _, cx| on_clear(this, cx))),
                        )
                    }),
            )
            .children(rows)
            .when(hidden > 0, |section| {
                section.child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("+{} more", format::count(hidden))),
                )
            })
    }

    // ----- results ------------------------------------------------------------

    fn render_results(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let theme = cx.theme();
        let muted = theme.muted_foreground;

        let summary = h_flex()
            .flex_none()
            .gap_2()
            .px_4()
            .py_2()
            .text_sm()
            .text_color(muted)
            .when(self.searching, |row| row.child(Spinner::new().small()))
            .child(self.summary_text());

        let content = if let Some(error) = self.query_error.clone() {
            centered_message(
                cx,
                Icon::new(IconName::TriangleAlert).text_color(theme.danger),
                "Invalid query",
                error,
            )
            .into_any_element()
        } else if self.results.is_none() {
            if self.searching {
                div().into_any_element()
            } else {
                self.render_tips(cx).into_any_element()
            }
        } else if self
            .results
            .as_ref()
            .is_some_and(|results| results.visible.is_empty())
        {
            centered_message(
                cx,
                Icon::new(IconName::Search).text_color(muted),
                "No results",
                if !self.facet_filter.is_empty() {
                    "Nothing matches with the current filters. Try clearing them.".to_string()
                } else {
                    "Try a shorter query, turn off whole word or case matching, or widen the path filter."
                        .to_string()
                },
            )
            .into_any_element()
        } else {
            div()
                .id("results")
                .relative()
                .flex_1()
                .min_h_0()
                .size_full()
                .child(
                    list(
                        self.list_state.clone(),
                        cx.processor(|this, index, _window, cx| this.render_file(index, cx)),
                    )
                    .size_full(),
                )
                .vertical_scrollbar(&self.list_state)
                .into_any_element()
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(summary)
            .child(content)
    }

    fn summary_text(&self) -> String {
        let Some(results) = self.results.as_ref() else {
            return if self.searching {
                "Searching…".into()
            } else {
                String::new()
            };
        };
        let outcome = &results.outcome;
        let mut parts = vec![format!(
            "{} in {}",
            format::plural(outcome.matched_lines, "result", "results"),
            format::plural(outcome.files.len(), "file", "files"),
        )];
        if results.visible.len() != outcome.files.len() {
            parts.push(format!(
                "showing {}",
                format::plural(results.visible.len(), "file", "files")
            ));
        }
        parts.push(format!(
            "searched {} of {}",
            format::count(outcome.searched_files),
            format::plural(outcome.corpus_files, "file", "files"),
        ));
        parts.push(format::duration(outcome.elapsed));
        if !outcome.indexed {
            parts.push("no index, scanned the folder".into());
        }
        if outcome.truncated {
            parts.push("result limit reached, refine the query".into());
        }
        parts.join(" · ")
    }

    fn render_file(&mut self, visible_index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(file) = self
            .results
            .as_ref()
            .and_then(|results| results.file(visible_index))
            .cloned()
        else {
            return div().into_any_element();
        };
        let expanded = self.expanded.contains(&file.path);
        let theme = cx.theme();
        let (border, muted, header_bg) = (theme.border, theme.muted_foreground, theme.secondary);
        let radius = theme.radius_lg;
        let mono_family = theme.mono_font_family.clone();
        let mono_size = theme.mono_font_size;

        let (directory, name) = match file.path.rsplit_once('/') {
            Some((directory, name)) => (format!("{directory}/"), name.to_string()),
            None => (String::new(), file.path.clone()),
        };
        let first_line = file.first_match_line().unwrap_or(1);

        let header = {
            let (open_path, copy_path, reveal_path) =
                (file.path.clone(), file.path.clone(), file.path.clone());
            h_flex()
                .gap_2()
                .px_3()
                .py_1p5()
                .bg(header_bg)
                .border_b_1()
                .border_color(border)
                .child(Icon::new(Lucide::FileCode).small().text_color(muted))
                .child(
                    h_flex()
                        .id(SharedString::from(format!("open:{}", file.path)))
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_sm()
                        .cursor_pointer()
                        .hover(|style| style.underline())
                        .child(div().flex_none().text_color(muted).child(directory))
                        .child(div().flex_none().font_semibold().child(name))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_hit(&open_path, first_line, window, cx)
                        })),
                )
                .when_some(file.language, |row, language| {
                    row.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(muted)
                            .child(language),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(format::plural(file.matched_lines, "match", "matches")),
                )
                .child(
                    Button::new(SharedString::from(format!("copy:{}", file.path)))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Copy)
                        .tooltip("Copy path")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.copy_path(&copy_path, window, cx)
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("reveal:{}", file.path)))
                        .ghost()
                        .xsmall()
                        .icon(IconName::FolderOpen)
                        .tooltip("Reveal in file manager")
                        .on_click(cx.listener(move |this, _, _, cx| this.reveal(&reveal_path, cx))),
                )
        };

        let (shown, hidden_lines) = visible_snippets(&file, expanded);
        let mut body = v_flex()
            .py_1()
            .font_family(mono_family)
            .text_size(mono_size);
        for (index, snippet) in shown.iter().enumerate() {
            if index > 0 {
                body = body.child(div().h_px().mx_3().my_1().bg(border));
            }
            for line in &snippet.lines {
                body = body.child(self.render_line(&file.path, line, cx));
            }
        }

        let unkept = file.matched_lines - file.kept_lines();
        let footer = (hidden_lines > 0 || expanded || unkept > 0).then(|| {
            let path = file.path.clone();
            let link = cx.theme().primary;
            h_flex()
                .gap_3()
                .px_3()
                .py_1()
                .border_t_1()
                .border_color(border)
                .text_xs()
                .when(hidden_lines > 0 || expanded, |row| {
                    row.child(
                        div()
                            .id(SharedString::from(format!("more:{path}")))
                            .cursor_pointer()
                            .text_color(link)
                            .hover(|style| style.underline())
                            .child(if expanded {
                                "Show fewer".to_string()
                            } else if hidden_lines == 1 {
                                "Show 1 more match".to_string()
                            } else {
                                format!("Show {} more matches", format::count(hidden_lines))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_expanded(visible_index, path.clone(), cx)
                            })),
                    )
                })
                .when(unkept > 0 && (expanded || hidden_lines == 0), |row| {
                    row.child(div().text_color(muted).child(format!(
                        "{} not shown",
                        format::plural(unkept, "more match", "more matches")
                    )))
                })
        });

        div()
            .px_4()
            .pb_3()
            .child(
                v_flex()
                    .border_1()
                    .border_color(border)
                    .rounded(radius)
                    .overflow_hidden()
                    .child(header)
                    .child(body)
                    .children(footer),
            )
            .into_any_element()
    }

    fn render_line(&self, path: &str, line: &SnippetLine, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let highlight = HighlightStyle {
            background_color: Some(
                theme
                    .yellow
                    .opacity(if theme.is_dark() { 0.4 } else { 0.55 }),
            ),
            font_weight: Some(FontWeight::SEMIBOLD),
            ..Default::default()
        };
        let text: SharedString = if line.text.is_empty() {
            " ".into()
        } else {
            line.text.clone().into()
        };
        let styled = StyledText::new(text).with_highlights(
            line.highlights
                .iter()
                .map(|range| (range.clone(), highlight)),
        );
        let (hover, muted) = (theme.list_hover, theme.muted_foreground);
        let number = line.number;
        let open_path = path.to_string();

        h_flex()
            .id(SharedString::from(format!("line:{path}:{number}")))
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .child(
                div()
                    .flex_none()
                    .w(px(LINE_NUMBER_WIDTH))
                    .pr_3()
                    .text_right()
                    .text_color(muted)
                    .child(number.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .pr_3()
                    .when(!line.is_match, |text| text.text_color(muted))
                    .child(styled),
            )
            .on_click(
                cx.listener(move |this, _, window, cx| {
                    this.open_hit(&open_path, number, window, cx)
                }),
            )
    }

    // ----- empty states ---------------------------------------------------------

    fn render_tips(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let name = self
            .folder
            .as_ref()
            .map(|folder| folder.workspace.name())
            .unwrap_or_default();
        let tip = |key: &'static str, text: &'static str| {
            h_flex()
                .gap_3()
                .child(
                    div()
                        .w(px(150.))
                        .flex_none()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.foreground)
                        .child(key),
                )
                .child(div().text_color(muted).child(text))
        };
        v_flex()
            .flex_1()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(Icon::new(Lucide::TextSearch).large().text_color(muted))
            .child(
                div()
                    .text_lg()
                    .font_semibold()
                    .child(format!("Search {name}")),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .text_sm()
                    .child(tip("Alt+C  Aa", "Match case"))
                    .child(tip("Alt+W  ab", "Match whole word"))
                    .child(tip("Alt+R  .*", "Regular expression"))
                    .child(tip("src  *.rs", "Path filter keeps matching paths"))
                    .child(tip("!tests  -*.md", "Path filter drops matching paths"))
                    .child(tip("Click a line", "Open it in your editor")),
            )
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, hover, radius) = (theme.muted_foreground, theme.list_hover, theme.radius);

        let recent = self.recent.iter().map(|path: &PathBuf| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| display_path(path));
            let target = path.clone();
            h_flex()
                .id(SharedString::from(format!("recent:{}", path.display())))
                .gap_3()
                .px_3()
                .py_2()
                .rounded(radius)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(Icon::new(IconName::Folder).small().text_color(muted))
                .child(div().flex_none().font_semibold().child(name))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(muted)
                        .child(display_path(path)),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_folder(target.clone(), window, cx)
                }))
        });

        v_flex()
            .flex_1()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(Icon::new(Lucide::TextSearch).size(px(48.)).text_color(theme.primary))
            .child(div().text_2xl().font_semibold().child("tgrep"))
            .child(
                div()
                    .max_w(px(520.))
                    .text_center()
                    .text_color(muted)
                    .child("Fast code search backed by a trigram index. Each folder keeps its index in a .tgrep directory, shared with the tgrep CLI."),
            )
            .child(
                Button::new("welcome-open")
                    .primary()
                    .icon(IconName::FolderOpen)
                    .label("Open Folder…")
                    .on_click(cx.listener(|this, _, window, cx| this.prompt_for_folder(window, cx))),
            )
            .when(!self.recent.is_empty(), |welcome| {
                welcome.child(
                    v_flex()
                        .w(px(560.))
                        .mt_4()
                        .gap_0p5()
                        .child(div().px_3().pb_1().text_xs().font_semibold().text_color(muted).child("RECENT"))
                        .children(recent),
                )
            })
    }

    // ----- status bar -------------------------------------------------------------

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let mut bar = h_flex()
            .flex_none()
            .gap_3()
            .px_3()
            .py_1()
            .border_t_1()
            .border_color(theme.status_bar_border)
            .bg(theme.status_bar)
            .text_xs()
            .text_color(muted);

        let Some(folder) = self.folder.as_ref() else {
            return bar.child("No folder open");
        };
        bar = bar
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(folder.workspace.display_root()),
            )
            .child(div().flex_1());

        let building = matches!(folder.index, IndexActivity::Building);
        let status = match &folder.index {
            IndexActivity::Loading => "Loading index…".to_string(),
            IndexActivity::Building => "Building index…".to_string(),
            IndexActivity::Failed(error) => format!("Indexing failed: {error}"),
            IndexActivity::Idle(IndexStatus::Missing) => "No index yet".to_string(),
            IndexActivity::Idle(IndexStatus::Unusable) => {
                "Index unusable, scanning files".to_string()
            }
            IndexActivity::Idle(IndexStatus::Ready { files, updated_at }) => format!(
                "Indexed {} · updated {}",
                format::plural(*files as usize, "file", "files"),
                format::ago(*updated_at, SystemTime::now())
            ),
        };
        let changed = folder.changed_files;
        bar.when(building || matches!(folder.index, IndexActivity::Loading), |bar| {
            bar.child(Spinner::new().xsmall())
        })
        .child(status)
        .when(changed > 0, |bar| {
            bar.child(
                div()
                    .id("changed-files")
                    .child(format!("· {} changed", format::plural(changed, "file", "files")))
                    .tooltip(|window, cx| {
                        Tooltip::new(
                            "Files changed since the index was built. Searches read them directly, so results stay current.",
                        )
                        .build(window, cx)
                    }),
            )
        })
        .child(
            Button::new("rebuild-index")
                .ghost()
                .xsmall()
                .icon(Lucide::RefreshCw)
                .label("Rebuild index")
                .tooltip("Re-index the folder to pick up changes (Ctrl+Shift+R)")
                .disabled(building)
                .on_click(cx.listener(|this, _, _, cx| this.build_index(cx))),
        )
    }
}

/// What clicking a facet row, and its section's "Clear", does.
struct FacetHandlers {
    toggle: fn(&mut SearchApp, String, &mut Context<SearchApp>),
    clear: fn(&mut SearchApp, &mut Context<SearchApp>),
}

/// An icon toggle inside the search input.
fn option_toggle(
    id: &'static str,
    icon: impl Into<Icon>,
    tooltip: &'static str,
    selected: bool,
) -> Button {
    // A ghost button's selected state is too faint to read at a glance.
    let button = Button::new(id);
    let button = if selected {
        button.primary()
    } else {
        button.ghost()
    };
    button
        .xsmall()
        .icon(icon)
        .selected(selected)
        .toggled(selected)
        .tooltip(tooltip)
}

fn centered_message(cx: &App, icon: Icon, title: &'static str, detail: String) -> impl IntoElement {
    v_flex()
        .flex_1()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .px_8()
        .child(icon.large())
        .child(div().font_semibold().child(title))
        .child(
            div()
                .max_w(px(560.))
                .text_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(detail),
        )
}

/// The snippets to render for a file, and how many matching lines they leave
/// out when collapsed.
fn visible_snippets(file: &FileMatch, expanded: bool) -> (&[Snippet], usize) {
    let kept = file.kept_lines();
    if expanded {
        return (&file.snippets, 0);
    }
    let mut shown_matches = 0;
    let mut count = 0;
    for snippet in &file.snippets {
        if count > 0 && shown_matches >= COLLAPSED_MATCH_LINES {
            break;
        }
        shown_matches += snippet.lines.iter().filter(|line| line.is_match).count();
        count += 1;
    }
    (&file.snippets[..count], kept - shown_matches)
}
