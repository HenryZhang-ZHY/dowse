//! How the search page looks: header with the search bar, the scope bar,
//! facet sidebar, result cards and the status bar.

use std::path::PathBuf;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::popover::Popover;
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
use super::app::{Page, SearchApp, file_key};
use super::hub::{IndexActivity, RepoView};
use crate::format;
use tgrep_gpui::engine::facets::FacetKind;
use tgrep_gpui::engine::repo::{self, BRANCH_GROUP};
use tgrep_gpui::engine::search::{FileMatch, Snippet, SnippetLine};

/// Matching lines shown per file before "Show more".
const COLLAPSED_MATCH_LINES: usize = 6;
/// Facet values listed per section.
const FACET_ROWS: usize = 30;
const SIDEBAR_WIDTH: f32 = 240.;
const LINE_NUMBER_WIDTH: f32 = 60.;

impl Render for SearchApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.page {
            Page::Repositories => self.render_repositories_page(cx).into_any_element(),
            Page::Search if self.members.is_empty() => self.render_welcome(cx).into_any_element(),
            Page::Search => v_flex()
                .flex_1()
                .min_h_0()
                .child(self.render_scope_bar(cx))
                .child(
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .items_start()
                        .child(self.render_sidebar(cx))
                        .child(self.render_results(window, cx)),
                )
                .into_any_element(),
        };

        v_flex()
            .id("search-app")
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::on_add_repository))
            .on_action(cx.listener(Self::on_show_repositories))
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
        let searchable = !self.members.is_empty();

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

        let on_repositories = self.page == Page::Repositories;
        let repositories_button = Button::new("repositories")
            .ghost()
            .small()
            .icon(Lucide::FolderGit2)
            .label(format!("Repositories · {}", self.members.len()))
            .selected(on_repositories)
            .tooltip("Add, tag and index repositories (Ctrl+,)")
            .on_click(cx.listener(move |this, _, window, cx| {
                let page = if on_repositories {
                    Page::Search
                } else {
                    Page::Repositories
                };
                this.show_page(page, window, cx)
            }));

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
                    .id("home")
                    .flex_none()
                    .gap_1p5()
                    .cursor_pointer()
                    .child(Icon::new(Lucide::TextSearch).text_color(theme.primary))
                    .child(div().font_semibold().child("tgrep"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_page(Page::Search, window, cx)),
                    ),
            )
            .child(
                div().flex_1().max_w(px(960.)).child(
                    Input::new(&self.search_input)
                        .prefix(Icon::new(IconName::Search).small().text_color(muted))
                        .suffix(toggles)
                        .cleanable(true)
                        .disabled(!searchable),
                ),
            )
            .child(
                div().w(px(300.)).flex_none().child(
                    Input::new(&self.path_input)
                        .prefix(Icon::new(Lucide::Funnel).small().text_color(muted))
                        .cleanable(true)
                        .disabled(!searchable),
                ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .ml_auto()
                    .gap_1()
                    .child(repositories_button)
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

    // ----- scope bar ------------------------------------------------------------

    /// Which repositories are searched: the selected tags as removable chips,
    /// and a picker listing every tag.
    fn render_scope_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, accent, accent_foreground, link) = (
            theme.muted_foreground,
            theme.accent,
            theme.accent_foreground,
            theme.primary,
        );
        let repos = self.repo_views(cx);
        let total = repos.len();
        let in_scope = self.in_scope(&repos).count();
        let selected: Vec<String> = self.scope.tags().map(str::to_string).collect();

        let chips = selected.iter().map(|tag| {
            let target = tag.clone();
            h_flex()
                .id(SharedString::from(format!("scope-chip:{tag}")))
                .gap_1()
                .pl_2()
                .pr_1()
                .py_0p5()
                .rounded_full()
                .bg(accent)
                .text_color(accent_foreground)
                .text_xs()
                .cursor_pointer()
                .child(tag_label(tag))
                .child(Icon::new(IconName::Close).xsmall())
                .tooltip(|window, cx| Tooltip::new("Remove from scope").build(window, cx))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_scope_tag(&target, window, cx)
                }))
        });

        let catalog = repo::tag_catalog(repos.iter().map(|repo| &*repo.info));
        let app = cx.entity();
        let scope = self.scope.clone();
        let picker = Popover::new("scope-picker")
            .trigger(
                Button::new("scope-add")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::Tag)
                    .label(if selected.is_empty() {
                        "Narrow by tag"
                    } else {
                        "Tags"
                    }),
            )
            .content(move |_, _, cx| {
                let theme = cx.theme();
                let (muted, hover, radius) = (theme.muted_foreground, theme.list_hover, theme.radius);
                if catalog.is_empty() {
                    return v_flex()
                        .w(px(280.))
                        .gap_2()
                        .text_sm()
                        .child("No tags yet.")
                        .child(
                            div()
                                .text_color(muted)
                                .child("Tag repositories on the Repositories page, e.g. mirror, dev or owner:alice."),
                        )
                        .into_any_element();
                }
                let groups = catalog.iter().map(|(group, tags)| {
                    let rows = tags.iter().map(|(tag, count)| {
                        let checked = scope.contains(tag);
                        let (app, target) = (app.clone(), tag.clone());
                        h_flex()
                            .id(SharedString::from(format!("scope-option:{tag}")))
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded(radius)
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover))
                            .child(
                                div()
                                    .w(px(16.))
                                    .flex_none()
                                    .when(checked, |slot| slot.child(Icon::new(IconName::Check).small())),
                            )
                            .child(div().flex_1().min_w_0().truncate().child(tag_value(tag).to_string()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(format::plural(*count, "repo", "repos")),
                            )
                            .on_click(move |_, window, cx| {
                                app.update(cx, |this, cx| this.toggle_scope_tag(&target, window, cx))
                            })
                    });
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .px_2()
                                .pt_1()
                                .text_xs()
                                .font_semibold()
                                .text_color(muted)
                                .child(group_title(group).to_uppercase()),
                        )
                        .children(rows)
                });
                v_flex()
                    .id("scope-options")
                    .w(px(300.))
                    .max_h(px(440.))
                    .gap_2()
                    .text_sm()
                    .children(groups)
                    .child(
                        div()
                            .px_2()
                            .text_xs()
                            .text_color(muted)
                            .child("Tags in one group are alternatives; groups narrow each other."),
                    )
                    .overflow_y_scrollbar()
                    .into_any_element()
            });

        h_flex()
            .flex_none()
            .gap_2()
            .px_4()
            .py_1p5()
            .border_b_1()
            .border_color(theme.border)
            .text_sm()
            .child(div().text_color(muted).child(if selected.is_empty() {
                format!(
                    "Searching all {}",
                    format::plural(total, "repository", "repositories")
                )
            } else {
                format!(
                    "Searching {in_scope} of {}",
                    format::plural(total, "repository", "repositories")
                )
            }))
            .children(chips)
            .child(picker)
            .when(!selected.is_empty(), |bar| {
                bar.child(
                    div()
                        .id("scope-clear")
                        .text_xs()
                        .cursor_pointer()
                        .text_color(link)
                        .hover(|style| style.underline())
                        .child("Clear")
                        .on_click(cx.listener(|this, _, window, cx| this.clear_scope(window, cx))),
                )
            })
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
                        .child("Filters appear here once a search finds matches. They narrow the results without changing which repositories are searched."),
                )
                .overflow_y_scrollbar();
        };
        let sections: Vec<AnyElement> = results
            .facets
            .sections
            .iter()
            .filter(|(_, entries)| !entries.is_empty())
            .map(|(kind, entries)| self.facet_section(kind, entries, cx).into_any_element())
            .collect();
        sidebar.children(sections).overflow_y_scrollbar()
    }

    fn facet_section(
        &self,
        kind: &FacetKind,
        entries: &[(String, usize)],
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (hover, active, muted) = (theme.list_hover, theme.accent, theme.muted_foreground);
        let (active_foreground, link) = (theme.accent_foreground, theme.primary);
        let radius = theme.radius;
        let selected = self.facet_filter.get(kind);
        let title = kind.title();

        let rows = entries.iter().take(FACET_ROWS).map(|(value, count)| {
            let is_selected = selected == Some(value.as_str());
            let (kind, target) = (kind.clone(), value.clone());
            h_flex()
                .id(SharedString::from(format!("facet:{title}:{value}")))
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
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(kind.display(value).to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format::count(*count)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_facet(kind.clone(), target.clone(), cx)
                }))
        });
        let hidden = entries.len().saturating_sub(FACET_ROWS);
        let clear_kind = kind.clone();

        v_flex()
            .gap_0p5()
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
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.clear_facet(&clear_kind, cx)
                                })),
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

        let repos = self.repo_views(cx);
        let content = if let Some(error) = self.query_error.clone() {
            centered_message(
                cx,
                Icon::new(IconName::TriangleAlert).text_color(theme.danger),
                "Invalid query",
                error,
            )
            .into_any_element()
        } else if self.in_scope(&repos).next().is_none() {
            centered_message(
                cx,
                Icon::new(Lucide::Tag).text_color(muted),
                "No repositories in scope",
                format!(
                    "The selected tags match none of your {}. Change the scope above.",
                    format::plural(repos.len(), "repository", "repositories")
                ),
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
                    "Try a shorter query, turn off whole word or case matching, widen the path filter, or widen the scope."
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
        if outcome.repos > 1 {
            let with_hits = {
                let mut ids: Vec<&str> = outcome
                    .files
                    .iter()
                    .map(|file| file.repo.id.as_str())
                    .collect();
                ids.dedup();
                ids.len()
            };
            parts[0].push_str(&format!(
                " across {}",
                format::plural(with_hits, "repository", "repositories")
            ));
        }
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
        if outcome.unindexed_repos > 0 {
            parts.push(format!(
                "{} without an index, scanned",
                format::plural(outcome.unindexed_repos, "repository", "repositories")
            ));
        }
        if self.searching {
            parts.push("some repositories are still loading".into());
        }
        if outcome.truncated {
            parts.push("result limit reached, refine the query".into());
        }
        parts.join(" · ")
    }

    fn render_file(&mut self, visible_index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some((file, multi_repo)) = self.results.as_ref().and_then(|results| {
            Some((
                results.file(visible_index)?.clone(),
                results.outcome.repos > 1,
            ))
        }) else {
            return div().into_any_element();
        };
        let key = file_key(&file);
        let expanded = self.expanded.contains(&key);
        let theme = cx.theme();
        let (border, muted, header_bg) = (theme.border, theme.muted_foreground, theme.secondary);
        let (radius, chip_bg) = (theme.radius, theme.background);
        let radius_lg = theme.radius_lg;
        let mono_family = theme.mono_font_family.clone();
        let mono_size = theme.mono_font_size;

        let (directory, name) = match file.path.rsplit_once('/') {
            Some((directory, name)) => (format!("{directory}/"), name.to_string()),
            None => (String::new(), file.path.clone()),
        };
        let first_line = file.first_match_line().unwrap_or(1);
        let root: PathBuf = file.repo.root.clone();

        let header = {
            let (open_root, open_path) = (root.clone(), file.path.clone());
            let (reveal_root, reveal_path) = (root.clone(), file.path.clone());
            let copy_path = full_display_path(&root, &file.path);
            h_flex()
                .gap_2()
                .px_3()
                .py_1p5()
                .bg(header_bg)
                .border_b_1()
                .border_color(border)
                .when(multi_repo, |row| {
                    row.child(
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
                            .child(file.repo.name.clone())
                            .when_some(file.repo.branch.clone(), |chip, branch| {
                                chip.child(Icon::new(Lucide::GitBranch).xsmall().text_color(muted))
                                    .child(div().text_color(muted).child(branch))
                            }),
                    )
                })
                .when(!multi_repo, |row| {
                    row.child(Icon::new(Lucide::FileCode).small().text_color(muted))
                })
                .child(
                    h_flex()
                        .id(SharedString::from(format!("open:{key}")))
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_sm()
                        .cursor_pointer()
                        .hover(|style| style.underline())
                        .child(div().flex_none().text_color(muted).child(directory))
                        .child(div().flex_none().font_semibold().child(name))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_hit(&open_root, &open_path, first_line, window, cx)
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
                    Button::new(SharedString::from(format!("copy:{key}")))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Copy)
                        .tooltip("Copy path")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.copy_path(&copy_path, window, cx)
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("reveal:{key}")))
                        .ghost()
                        .xsmall()
                        .icon(IconName::FolderOpen)
                        .tooltip("Reveal in file manager")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.reveal(&reveal_root, &reveal_path, cx)
                        })),
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
                body = body.child(self.render_line(&file, &key, line, cx));
            }
        }

        let unkept = file.matched_lines - file.kept_lines();
        let footer = (hidden_lines > 0 || expanded || unkept > 0).then(|| {
            let link = cx.theme().primary;
            let toggle_key = key.clone();
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
                            .id(SharedString::from(format!("more:{key}")))
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
                                this.toggle_expanded(visible_index, toggle_key.clone(), cx)
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
                    .rounded(radius_lg)
                    .overflow_hidden()
                    .child(header)
                    .child(body)
                    .children(footer),
            )
            .into_any_element()
    }

    fn render_line(
        &self,
        file: &FileMatch,
        key: &str,
        line: &SnippetLine,
        cx: &Context<Self>,
    ) -> impl IntoElement {
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
        let (root, path) = (file.repo.root.clone(), file.path.clone());

        h_flex()
            .id(SharedString::from(format!("line:{key}:{number}")))
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
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_hit(&root, &path, number, window, cx)
            }))
    }

    // ----- empty states ---------------------------------------------------------

    fn render_tips(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let in_scope = self.in_scope(&self.repo_views(cx)).count();
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
            .child(div().text_lg().font_semibold().child(format!(
                "Search {}",
                format::plural(in_scope, "repository", "repositories")
            )))
            .child(
                v_flex()
                    .gap_1p5()
                    .text_sm()
                    .child(tip("Alt+C  Aa", "Match case"))
                    .child(tip("Alt+W  ab", "Match whole word"))
                    .child(tip("Alt+R  .*", "Regular expression"))
                    .child(tip("src  *.rs", "Path filter keeps matching paths"))
                    .child(tip("!tests  -*.md", "Path filter drops matching paths"))
                    .child(tip("Narrow by tag", "Pick which repositories to search"))
                    .child(tip("Click a line", "Open it in your editor")),
            )
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        v_flex()
            .flex_1()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(Icon::new(Lucide::TextSearch).size(px(48.)).text_color(theme.primary))
            .child(div().text_2xl().font_semibold().child("Search across your repositories"))
            .child(
                div()
                    .max_w(px(560.))
                    .text_center()
                    .text_color(muted)
                    .child("Add the repositories you work with. Each gets a trigram index in its .tgrep directory, shared with the tgrep CLI. Tag them, e.g. mirror, dev or owner:alice, to choose which ones a search covers."),
            )
            .child(
                Button::new("welcome-add")
                    .primary()
                    .icon(IconName::FolderOpen)
                    .label("Add Repositories…")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.prompt_for_repositories(window, cx)
                    })),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Choosing a folder that holds several git repositories adds each of them."),
            )
    }

    // ----- status bar -------------------------------------------------------------

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let bar = h_flex()
            .flex_none()
            .gap_3()
            .px_3()
            .py_1()
            .border_t_1()
            .border_color(theme.status_bar_border)
            .bg(theme.status_bar)
            .text_xs()
            .text_color(muted);
        let repos = self.repo_views(cx);
        if repos.is_empty() {
            return bar.child("No repositories yet");
        }

        let in_scope: Vec<&RepoView> = self.in_scope(&repos).collect();
        let building = repos
            .iter()
            .find(|repo| repo.activity == IndexActivity::Building);
        let queued = repos
            .iter()
            .filter(|repo| repo.activity == IndexActivity::Queued)
            .count();
        let loading = repos
            .iter()
            .filter(|repo| repo.activity == IndexActivity::Loading)
            .count();
        let failed = repos
            .iter()
            .filter(|repo| {
                matches!(
                    repo.activity,
                    IndexActivity::Failed(_) | IndexActivity::Missing
                )
            })
            .count();
        let changed: usize = in_scope.iter().map(|repo| repo.changed_files).sum();

        let activity = if let Some(repo) = building {
            Some(if queued > 0 {
                format!("Indexing {} ({queued} queued)…", repo.info.name)
            } else {
                format!("Indexing {}…", repo.info.name)
            })
        } else if loading > 0 {
            Some(format!(
                "Loading {}…",
                format::plural(loading, "index", "indexes")
            ))
        } else {
            None
        };

        bar.child(format!(
            "{} · {} in scope",
            format::plural(repos.len(), "repository", "repositories"),
            in_scope.len()
        ))
        .child(div().flex_1())
        .when_some(activity, |bar, activity| {
            bar.child(Spinner::new().xsmall()).child(activity)
        })
        .when(failed > 0, |bar| {
            bar.child(
                div()
                    .id("status-failed")
                    .cursor_pointer()
                    .text_color(danger)
                    .hover(|style| style.underline())
                    .child(format!("{} need attention", format::plural(failed, "repository", "repositories")))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_page(Page::Repositories, window, cx)
                    })),
            )
        })
        .when(changed > 0, |bar| {
            bar.child(
                div()
                    .id("changed-files")
                    .child(format!("{} changed since indexing", format::plural(changed, "file", "files")))
                    .tooltip(|window, cx| {
                        Tooltip::new(
                            "Files changed since their repository was indexed. Searches read them directly, so results stay current.",
                        )
                        .build(window, cx)
                    }),
            )
        })
        .child(
            Button::new("rebuild-scope")
                .ghost()
                .xsmall()
                .icon(Lucide::RefreshCw)
                .label("Rebuild indexes")
                .tooltip("Re-index the repositories in scope (Ctrl+Shift+R)")
                .disabled(in_scope.is_empty())
                .on_click(cx.listener(|this, _, _, cx| this.queue_scope_indexes(cx))),
        )
    }
}

/// How a tag reads on a chip: `owner: alice`, `branch: main`, `mirror`.
pub(super) fn tag_label(tag: &str) -> String {
    match repo::tag_group(tag) {
        "" => tag.to_string(),
        group => format!("{group}: {}", tag_value(tag)),
    }
}

/// The part of a tag after its group key.
fn tag_value(tag: &str) -> &str {
    match repo::tag_group(tag) {
        "" => tag,
        group => &tag[group.len() + 1..],
    }
}

fn group_title(group: &str) -> String {
    match group {
        "" => "Tags".into(),
        BRANCH_GROUP => "Branch".into(),
        other => FacetKind::Tags(other.to_string()).title(),
    }
}

fn full_display_path(root: &std::path::Path, path: &str) -> String {
    root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR))
        .to_string_lossy()
        .into_owned()
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

pub(super) fn centered_message(
    cx: &App,
    icon: Icon,
    title: &'static str,
    detail: String,
) -> impl IntoElement {
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
