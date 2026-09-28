//! How the search page looks: header with the search bar, the scope bar,
//! facet sidebar, result cards and the status bar.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::table::DataTable;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Root, Selectable as _, Side, Sizable as _,
    StyledExt as _, TitleBar, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::CONTEXT;
use super::app::{AppCommand, Page, SearchApp};
use super::highlight::LineStyles;
use super::hub::{IndexActivity, IndexJob, RepoView};
use super::manager::Section;
use super::table::ResultsView;
use super::tabs::file_key;
use super::tasks::TaskHub;
use super::windows::Windows;
use super::{CommandPalette, ToggleTheme};
use crate::assets::BRAND_MARK;
use crate::format;
use dowse::engine::facets::FacetKind;
use dowse::engine::repo::{self, BRANCH_GROUP};
use dowse::engine::search::{FileMatch, Snippet, SnippetLine};
use dowse::engine::table::ExportFormat;
use dowse::engine::tasks::{TaskKind, TaskState};
use dowse::engine::workspace;

/// Matching lines shown per file before "Show more".
const COLLAPSED_MATCH_LINES: usize = 6;
/// Facet values listed per section.
const FACET_ROWS: usize = 30;
const SIDEBAR_WIDTH: f32 = 240.;
const LINE_NUMBER_WIDTH: f32 = 60.;
/// The preview pane's starting width.
const PREVIEW_WIDTH: f32 = 720.;
/// The header, which is also the window's title bar.
const TITLE_BAR_HEIGHT: f32 = 46.;
/// Empty title bar kept beside the search box for dragging the window.
const DRAG_GAP: f32 = 40.;

impl Render for SearchApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.page {
            Page::Repositories => self.render_repositories_page(cx).into_any_element(),
            Page::Search if self.members.is_empty() => self.render_welcome(cx).into_any_element(),
            Page::Search => v_flex()
                .flex_1()
                .min_h_0()
                .child(self.render_tab_strip(cx))
                .child(self.render_scope_bar(cx))
                .child(
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .items_start()
                        .child(self.render_sidebar(cx))
                        .child(self.render_results_and_preview(window, cx)),
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
            .on_action(cx.listener(Self::on_update_index))
            .on_action(cx.listener(Self::on_rebuild_index))
            .on_action(cx.listener(Self::on_toggle_theme))
            .on_action(cx.listener(Self::on_new_workspace))
            .on_action(cx.listener(Self::on_open_workspace))
            .on_action(cx.listener(Self::on_save_workspace_as))
            .on_action(cx.listener(Self::on_new_tab))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_previous_tab))
            .on_action(cx.listener(Self::on_close_preview))
            .on_action(cx.listener(Self::on_toggle_preview))
            .on_action(cx.listener(Self::on_next_match))
            .on_action(cx.listener(Self::on_previous_match))
            .on_action(cx.listener(Self::on_toggle_results_view))
            .on_action(cx.listener(Self::on_export_results))
            .on_action(cx.listener(Self::on_copy_results_as_tsv))
            .on_action(cx.listener(Self::on_copy_results_as_markdown))
            .on_action(cx.listener(Self::on_command_palette))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.drop_paths(paths.paths(), window, cx)
            }))
            .drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().drop_target))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_header(cx))
            .child(body)
            .child(self.render_status_bar(cx))
            // `Root` keeps them, but each view draws them over itself.
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

impl SearchApp {
    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let searchable = !self.members.is_empty();
        let path_filter_shown = self.path_filter_shown(cx);

        let toggles = h_flex()
            .gap_0p5()
            .child(
                option_toggle(
                    "case-sensitive",
                    IconName::CaseSensitive,
                    "Match case (Alt+C)",
                    self.tab().case_sensitive,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_case_sensitive(!this.tab().case_sensitive, cx)
                })),
            )
            .child(
                option_toggle(
                    "whole-word",
                    Lucide::WholeWord,
                    "Match whole word (Alt+W)",
                    self.tab().whole_word,
                )
                .on_click(
                    cx.listener(|this, _, _, cx| this.set_whole_word(!this.tab().whole_word, cx)),
                ),
            )
            .child(
                option_toggle(
                    "regex",
                    Lucide::Regex,
                    "Whole query is one regular expression (Alt+R); use /…/ for a regex term",
                    self.tab().regex,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_regex(!this.tab().regex, cx))),
            )
            .child(
                option_toggle(
                    "path-filter",
                    Lucide::Funnel,
                    "Filter paths (Ctrl+P); path: in the query does the same",
                    path_filter_shown,
                )
                .on_click(cx.listener(|this, _, window, cx| this.toggle_path_filter(window, cx))),
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

        let preview_open = self.tab().preview_open;
        let previewable = self.page == Page::Search && searchable;
        let preview_button = Button::new("toggle-preview")
            .ghost()
            .small()
            .icon(IconName::PanelRight)
            .selected(previewable && preview_open)
            .disabled(!previewable)
            .tooltip(if preview_open {
                "Hide the preview (Ctrl+Alt+B)"
            } else {
                "Show the preview (Ctrl+Alt+B)"
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_preview(cx)));

        // The header is the window's title bar: dragging it moves the window
        // and a double click maximizes it, except over the controls, which
        // occlude it so that clicks reach them.
        TitleBar::new()
            .h(px(TITLE_BAR_HEIGHT))
            .bg(theme.title_bar)
            .border_color(theme.border)
            .child(
                h_flex()
                    .flex_1()
                    .h_full()
                    .gap_3()
                    .pr_3()
                    // Where the app is: its home, the workspace and its repositories.
                    .child(
                        h_flex()
                            .id("home")
                            .occlude()
                            .flex_none()
                            .gap_1p5()
                            .cursor_pointer()
                            .child(brand_mark().text_color(theme.foreground))
                            .child(div().font_semibold().child("dowse"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_page(Page::Search, window, cx)
                            })),
                    )
                    .child(
                        h_flex()
                            .occlude()
                            .flex_none()
                            .gap_0p5()
                            .child(self.render_workspace_menu(cx))
                            .child(repositories_button),
                    )
                    .child(
                        div().occlude().flex_1().max_w(px(960.)).child(
                            Input::new(&self.tab().search_input)
                                .prefix(Icon::new(IconName::Search).small().text_color(muted))
                                .suffix(toggles)
                                .cleanable(true)
                                .disabled(!searchable),
                        ),
                    )
                    .when(path_filter_shown, |header| {
                        header.child(
                            div().occlude().w(px(280.)).flex_none().child(
                                Input::new(&self.tab().path_input)
                                    .prefix(Icon::new(Lucide::Funnel).small().text_color(muted))
                                    .cleanable(true)
                                    .disabled(!searchable),
                            ),
                        )
                    })
                    // Somewhere to take hold of the window, however narrow.
                    .child(div().flex_none().w(px(DRAG_GAP)))
                    // Tools, beside the window's own buttons.
                    .child(
                        h_flex()
                            .occlude()
                            .flex_none()
                            .ml_auto()
                            .gap_1()
                            .child(preview_button)
                            .child(self.render_more_menu(cx)),
                    ),
            )
    }

    /// What is seldom needed from the title bar: the command palette, which
    /// Ctrl+K opens anyway, and the theme.
    fn render_more_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app = cx.entity().downgrade();
        Button::new("more-menu")
            .ghost()
            .small()
            .icon(IconName::EllipsisVertical)
            .tooltip("More")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, cx| {
                let dark = cx.theme().is_dark();
                let (palette, theme) = (app.clone(), app.clone());
                menu.min_w(px(240.))
                    .check_side(Side::Right)
                    .item(
                        PopupMenuItem::new("Command Palette")
                            .icon(Icon::new(Lucide::Command))
                            // For its shortcut; the click runs the handler.
                            .action(Box::new(CommandPalette))
                            .on_click(move |_, window, cx| {
                                palette
                                    .update(cx, |this, cx| this.open_palette(window, cx))
                                    .ok();
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("Dark Theme")
                            .icon(Icon::new(IconName::Moon))
                            .checked(dark)
                            .on_click(move |_, window, cx| {
                                theme
                                    .update(cx, |this, cx| {
                                        this.on_toggle_theme(&ToggleTheme, window, cx)
                                    })
                                    .ok();
                            }),
                    )
            })
    }

    /// The workspace's name, opening a menu to switch, save or open another.
    fn render_workspace_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app = cx.entity().downgrade();
        let saved = self.workspace.is_some();
        let current = self.workspace.clone();
        let recent: Vec<PathBuf> = Windows::recent(cx)
            .into_iter()
            .filter(|file| Some(file) != current.as_ref())
            .collect();
        let item =
            move |label: &str,
                  icon: Lucide,
                  run: fn(&mut SearchApp, &mut Window, &mut Context<SearchApp>)| {
                let app = app.clone();
                PopupMenuItem::new(label.to_string())
                    .icon(Icon::new(icon))
                    .on_click(move |_, window, cx| {
                        app.update(cx, |this, cx| run(this, window, cx)).ok();
                    })
            };
        let open_recent = {
            let app = cx.entity().downgrade();
            move |file: PathBuf| {
                let app = app.clone();
                let label = format!("{}  ·  {}", workspace::name(&file), short_dir(&file));
                PopupMenuItem::new(label).on_click(move |_, window, cx| {
                    let file = file.clone();
                    app.update(cx, |this, cx| this.open_workspace_file(file, window, cx))
                        .ok();
                })
            }
        };

        Button::new("workspace-menu")
            .ghost()
            .small()
            .icon(Lucide::Layers)
            .label(self.workspace_name())
            .tooltip("Switch, save or open a workspace")
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu
                    .item(item(
                        "New Window (Ctrl+Shift+N)",
                        Lucide::AppWindow,
                        |_, _, cx| {
                            Windows::new_window(cx);
                        },
                    ))
                    .item(item(
                        "New Workspace",
                        Lucide::FilePlus,
                        |this, window, cx| this.new_workspace(window, cx),
                    ))
                    .item(item(
                        "Open Workspace… (Ctrl+Shift+O)",
                        Lucide::FolderOpen,
                        |this, window, cx| this.prompt_open_workspace(window, cx),
                    ))
                    .item(item(
                        if saved {
                            "Save Workspace As… (Ctrl+Shift+S)"
                        } else {
                            "Save Workspace… (Ctrl+Shift+S)"
                        },
                        Lucide::Save,
                        |this, window, cx| this.save_workspace_as(window, cx).detach(),
                    ));
                if !recent.is_empty() {
                    menu = menu.separator().label("Recent");
                    for file in &recent {
                        menu = menu.item(open_recent(file.clone()));
                    }
                }
                menu
            })
    }

    // ----- tabs -----------------------------------------------------------------

    /// One tab per open search, each labelled with its query and how much it
    /// found, and a button for another.
    fn render_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, hover) = (theme.muted_foreground, theme.border, theme.list_hover);
        let (active_bg, active_fg, bar_bg) = (theme.background, theme.foreground, theme.tab_bar);
        let closable = self.tabs.len() > 1;

        let tabs = self.tabs.iter().enumerate().map(|(index, tab)| {
            let active = index == self.active_tab;
            // Lines found, or files for a query of only qualifiers.
            let count = tab.results.as_ref().map(|results| {
                let outcome = &results.outcome;
                format::count(if outcome.matched_lines > 0 {
                    outcome.matched_lines
                } else {
                    outcome.files.len()
                })
            });
            h_flex()
                .id(("search-tab", tab.id))
                .flex_none()
                .max_w(px(260.))
                .gap_1p5()
                .pl_3()
                .pr_1()
                .h(px(30.))
                .border_1()
                .border_b_0()
                .rounded_t(theme.radius)
                .text_sm()
                .cursor_pointer()
                .when(active, |tab| {
                    tab.bg(active_bg)
                        .border_color(border)
                        .text_color(active_fg)
                        // Cover the strip's bottom border, joining the page below.
                        .mb(px(-1.))
                })
                .when(!active, |tab| {
                    tab.border_color(gpui_kit::transparent_black())
                        .text_color(muted)
                        .hover(move |style| style.bg(hover))
                })
                .child(Icon::new(IconName::Search).xsmall())
                .child(div().min_w_0().truncate().child(tab.label(cx)))
                .when(tab.searching, |row| row.child(Spinner::new().xsmall()))
                .when_some(count.filter(|_| !tab.searching), |row, count| {
                    row.child(div().flex_none().text_xs().text_color(muted).child(count))
                })
                .child(div().flex_none().w(px(20.)).when(closable, |slot| {
                    slot.child(
                        Button::new(("close-tab", tab.id))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Close).xsmall())
                            .tooltip("Close tab (Ctrl+W)")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(index, window, cx)
                            })),
                    )
                }))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.activate_tab(index, window, cx)),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| this.close_tab(index, window, cx)),
                )
        });

        h_flex()
            .id("search-tabs")
            .flex_none()
            .items_end()
            .gap_0p5()
            .px_2()
            .pt_1p5()
            .border_b_1()
            .border_color(border)
            .bg(bar_bg)
            .overflow_x_scroll()
            .children(tabs)
            .child(
                div().pb_1().pl_1().child(
                    Button::new("new-tab")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .tooltip("New search tab (Ctrl+T)")
                        .on_click(cx.listener(|this, _, window, cx| this.open_tab(window, cx))),
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
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_scope_tag(&target, cx)))
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
                            .on_click(move |_, _, cx| {
                                app.update(cx, |this, cx| this.toggle_scope_tag(&target, cx))
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
                        .on_click(cx.listener(|this, _, _, cx| this.clear_scope(cx))),
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

        let Some(results) = self.tab().results.as_ref() else {
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
        let selected = self.tab().facet_filter.get(kind);
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

    /// The results, and beside them the file being previewed, if any, with a
    /// divider to resize them.
    fn render_results_and_preview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let results = self.render_results(window, cx).into_any_element();
        match self.render_preview(cx) {
            Some(preview) => div()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(
                    h_resizable("results-preview")
                        .child(
                            resizable_panel()
                                .size_range(px(320.)..Pixels::MAX)
                                .child(results),
                        )
                        .child(
                            resizable_panel()
                                .size(px(PREVIEW_WIDTH))
                                .size_range(px(360.)..Pixels::MAX)
                                .child(preview),
                        ),
                )
                .into_any_element(),
            None => results,
        }
    }

    fn render_results(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let table = if self.tab().view == ResultsView::Table {
            self.ensure_table(window, cx)
        } else {
            None
        };
        let controls = self
            .tab()
            .results
            .is_some()
            .then(|| self.render_view_controls(cx));
        let theme = cx.theme();
        let muted = theme.muted_foreground;

        let summary = h_flex()
            .flex_none()
            .gap_2()
            .px_4()
            .py_1p5()
            .min_h(px(36.))
            .text_sm()
            .text_color(muted)
            .when(self.tab().searching, |row| {
                row.child(Spinner::new().small())
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(self.summary_text()),
            )
            .children(controls);

        let repos = self.repo_views(cx);
        let content = if let Some(error) = self.tab().query_error.clone() {
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
        } else if self.tab().results.is_none() {
            if self.tab().searching {
                div().into_any_element()
            } else {
                self.render_tips(cx).into_any_element()
            }
        } else if self
            .tab()
            .results
            .as_ref()
            .is_some_and(|results| results.visible.is_empty())
        {
            centered_message(
                cx,
                Icon::new(IconName::Search).text_color(muted),
                "No results",
                if !self.tab().facet_filter.is_empty() {
                    "Nothing matches with the current filters. Try clearing them.".to_string()
                } else {
                    "Try a shorter query, turn off whole word or case matching, widen the path filter, or widen the scope."
                        .to_string()
                },
            )
            .into_any_element()
        } else if let Some(table) = table {
            div()
                .flex_1()
                .min_h_0()
                .size_full()
                .px_4()
                .pb_3()
                .child(DataTable::new(&table).stripe(true).small())
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
                        self.tab().list_state.clone(),
                        cx.processor(|this, index, _window, cx| this.render_file(index, cx)),
                    )
                    .size_full(),
                )
                .vertical_scrollbar(&self.tab().list_state)
                .into_any_element()
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(summary)
            .child(content)
    }

    /// Switch between snippets and the table, and export or copy the rows.
    fn render_view_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = self.tab().view;
        let app = cx.entity().downgrade();
        let item = move |label: String, icon: Icon, run: AppCommand| {
            let app = app.clone();
            PopupMenuItem::new(label)
                .icon(icon)
                .on_click(move |_, window, cx| {
                    app.update(cx, |this, cx| run(this, window, cx)).ok();
                })
        };
        let view_button = |id: &'static str, target: ResultsView, icon: Lucide, label, tooltip| {
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(icon)
                .label(label)
                .selected(view == target)
                .tooltip(tooltip)
                .on_click(cx.listener(move |this, _, window, cx| this.set_view(target, window, cx)))
        };

        h_flex()
            .flex_none()
            .gap_0p5()
            .child(view_button(
                "view-snippets",
                ResultsView::Snippets,
                Lucide::Rows3,
                "Snippets",
                "Matches in their code (Alt+T switches)",
            ))
            .child(view_button(
                "view-table",
                ResultsView::Table,
                Lucide::Table,
                "Table",
                "One row per matching line, sortable (Alt+T switches)",
            ))
            .child(
                Button::new("export-results")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::Download)
                    .label("Export")
                    .dropdown_caret(true)
                    .tooltip("Save or copy the matching lines as a table")
                    .dropdown_menu(move |menu, _, _| {
                        let mut menu = menu;
                        for format in ExportFormat::ALL {
                            let shortcut = if format == ExportFormat::Csv {
                                " (Ctrl+Shift+E)"
                            } else {
                                ""
                            };
                            menu = menu.item(item(
                                format!("Export as {}…{shortcut}", format.label()),
                                Icon::new(Lucide::Download),
                                Rc::new(move |this, window, cx| {
                                    this.export_results(format, window, cx)
                                }),
                            ));
                        }
                        menu.separator()
                            .item(item(
                                "Copy as TSV, for spreadsheets".into(),
                                Icon::new(IconName::Copy),
                                Rc::new(|this, window, cx| {
                                    this.copy_results(ExportFormat::Tsv, window, cx)
                                }),
                            ))
                            .item(item(
                                "Copy as Markdown".into(),
                                Icon::new(IconName::Copy),
                                Rc::new(|this, window, cx| {
                                    this.copy_results(ExportFormat::Markdown, window, cx)
                                }),
                            ))
                    }),
            )
            .into_any_element()
    }

    fn summary_text(&self) -> String {
        let Some(results) = self.tab().results.as_ref() else {
            return if self.tab().searching {
                "Searching…".into()
            } else {
                String::new()
            };
        };
        let outcome = &results.outcome;
        let files = format::plural(outcome.files.len(), "file", "files");
        let mut parts = vec![if outcome.matched_lines == 0 && !outcome.files.is_empty() {
            files
        } else {
            format!(
                "{} in {files}",
                format::plural(outcome.matched_lines, "result", "results"),
            )
        }];
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
        if self.tab().searching {
            parts.push("some repositories are still loading".into());
        }
        if outcome.truncated {
            parts.push("result limit reached, refine the query".into());
        }
        parts.join(" · ")
    }

    fn render_file(&mut self, visible_index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some((file, multi_repo)) = self.tab().results.as_ref().and_then(|results| {
            Some((
                results.file(visible_index)?.clone(),
                results.outcome.repos > 1,
            ))
        }) else {
            return div().into_any_element();
        };
        let key = file_key(&file);
        let expanded = self.tab().expanded.contains(&key);
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
        let previewed_line = self
            .tab()
            .preview
            .as_ref()
            .filter(|preview| preview.shows_file(&file))
            .map(|preview| preview.line);

        let header = {
            let (open_repo, open_path, language) =
                (file.repo.clone(), file.path.clone(), file.language);
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
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            if event.modifiers().secondary() {
                                this.open_hit(&open_repo.root, &open_path, first_line, window, cx)
                            } else {
                                let repo = open_repo.clone();
                                this.preview_hit(repo, open_path.clone(), language, first_line, cx)
                            }
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
                .when(file.matched_lines > 0, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(muted)
                            .child(format::plural(file.matched_lines, "match", "matches")),
                    )
                })
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
        let syntax = self.snippet_syntax(visible_index, cx);
        let mut body = v_flex()
            .py_1()
            .font_family(mono_family)
            .text_size(mono_size);
        for (index, snippet) in shown.iter().enumerate() {
            if index > 0 {
                body = body.child(div().h_px().mx_3().my_1().bg(border));
            }
            let snippet_syntax = syntax.as_ref().and_then(|syntax| syntax.get(index));
            for (line_index, line) in snippet.lines.iter().enumerate() {
                let line_syntax = snippet_syntax.and_then(|styles| styles.get(line_index));
                let previewed = previewed_line == Some(line.number);
                body = body.child(self.render_line(&file, &key, line, line_syntax, previewed, cx));
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
                    // A file found by its qualifiers alone has no lines.
                    .when(!file.snippets.is_empty(), |card| card.child(body))
                    .children(footer),
            )
            .into_any_element()
    }

    fn render_line(
        &self,
        file: &FileMatch,
        key: &str,
        line: &SnippetLine,
        syntax: Option<&LineStyles>,
        previewed: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let styled = code_text(&line.text, syntax, &line.highlights, cx);
        let (hover, muted) = (theme.list_hover, theme.muted_foreground);
        let previewed_bg = theme
            .yellow
            .opacity(if theme.is_dark() { 0.16 } else { 0.18 });
        let number = line.number;
        let (repo, path, language) = (file.repo.clone(), file.path.clone(), file.language);

        h_flex()
            .id(SharedString::from(format!("line:{key}:{number}")))
            .cursor_pointer()
            .when(previewed, |row| row.bg(previewed_bg))
            .when(!previewed, |row| row.hover(move |style| style.bg(hover)))
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
                    .when(!line.is_match, |text| text.text_color(muted).opacity(0.8))
                    .child(styled),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.modifiers().secondary() {
                    this.open_hit(&repo.root, &path, number, window, cx)
                } else {
                    this.preview_hit(repo.clone(), path.clone(), language, number, cx)
                }
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
                        .w(px(190.))
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
            .child(brand_mark().large().text_color(muted))
            .child(div().text_lg().font_semibold().child(format!(
                "Search {}",
                format::plural(in_scope, "repository", "repositories")
            )))
            .child(
                v_flex()
                    .gap_1p5()
                    .text_sm()
                    .child(tip("parse config", "Files containing both, on any lines"))
                    .child(tip("a OR b  NOT c", "Either term; files without c"))
                    .child(tip("\"a b\"  /a\\d+/", "Exact phrase; regular expression"))
                    .child(tip(
                        "lang:rust  -path:test",
                        "Qualifiers: path, lang, repo, branch, tag",
                    ))
                    .child(tip("Alt+C  Alt+W", "Match case, whole word"))
                    .child(tip("Alt+R  .*", "Whole query as one regular expression"))
                    .child(tip(
                        "path:src/*.rs",
                        "Only matching paths; -path: drops them",
                    ))
                    .child(tip("Ctrl+P", "A path filter box: src  *.rs  !tests"))
                    .child(tip("Narrow by tag", "Pick which repositories to search"))
                    .child(tip("Ctrl+T  Ctrl+Tab", "New search tab, next tab"))
                    .child(tip("Click a line", "Preview the file there"))
                    .child(tip("F4  Shift+F4", "Next and previous match"))
                    .child(tip("Ctrl+Click", "Open in your editor directly")),
            )
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, link) = (theme.muted_foreground, theme.primary);
        let current = self.workspace.clone();
        let recent: Vec<PathBuf> = Windows::recent(cx)
            .into_iter()
            .filter(|file| Some(file) != current.as_ref())
            .take(5)
            .collect();
        v_flex()
            .flex_1()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(brand_mark().size(px(48.)).text_color(theme.foreground))
            .child(div().text_2xl().font_semibold().child("Search across your repositories"))
            .child(
                div()
                    .max_w(px(560.))
                    .text_center()
                    .text_color(muted)
                    .child("Add the repositories you work with. Each gets a trigram index in its .tgrep directory, shared with the tgrep command-line tool. Tag them, e.g. mirror, dev or owner:alice, to choose which ones a search covers."),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("welcome-add")
                            .primary()
                            .icon(IconName::Plus)
                            .label("Add Repositories…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.prompt_for_repositories(window, cx)
                            })),
                    )
                    .child(
                        Button::new("welcome-clone")
                            .outline()
                            .icon(Lucide::Github)
                            .label("Clone from GitHub…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_section(Section::GitHub, window, cx)
                            })),
                    )
                    .child(
                        Button::new("welcome-open")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label("Open Workspace…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.prompt_open_workspace(window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Choosing a folder that holds several git repositories adds each of them."),
            )
            .when(!recent.is_empty(), |page| {
                page.child(
                    v_flex()
                        .pt_4()
                        .gap_1()
                        .items_center()
                        .text_sm()
                        .child(div().text_color(muted).child("Recent workspaces"))
                        .children(recent.into_iter().enumerate().map(|(index, file)| {
                            let label = workspace::name(&file);
                            let dir = short_dir(&file);
                            h_flex()
                                .id(("welcome-recent", index))
                                .gap_2()
                                .cursor_pointer()
                                .child(div().text_color(link).child(label))
                                .child(div().text_xs().text_color(muted).child(dir))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_workspace_file(file.clone(), window, cx)
                                }))
                        })),
                )
            })
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
        let tasks = self.render_task_status(cx);
        let repos = self.repo_views(cx);
        if repos.is_empty() {
            return bar
                .child("No repositories yet")
                .child(div().flex_1())
                .children(tasks);
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
        .children(tasks)
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
                .label("Update indexes")
                .tooltip(
                    "Bring the indexes in scope up to date, reading only the files that changed (Ctrl+Shift+R)",
                )
                .disabled(in_scope.is_empty())
                .on_click(
                    cx.listener(|this, _, _, cx| this.queue_scope_indexes(IndexJob::Update, cx)),
                ),
        )
    }
}

impl SearchApp {
    /// Clones and pulls in the background, opening the task list.
    fn render_task_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let infos = TaskHub::global(cx).read(cx).infos();
        let running: Vec<_> = infos
            .iter()
            .filter(|info| info.state == TaskState::Running)
            .collect();
        let queued = infos
            .iter()
            .filter(|info| info.state == TaskState::Queued)
            .count();
        if running.is_empty() && queued == 0 {
            return None;
        }
        let verb = |kind: TaskKind| match kind {
            TaskKind::Clone => "Cloning",
            TaskKind::Pull => "Pulling",
        };
        let text = match running.as_slice() {
            [one] => format!("{} {}", verb(one.kind), one.title),
            many => format!("{} tasks running", many.len()),
        };
        let text = if queued > 0 {
            format!("{text} · {queued} queued")
        } else {
            text
        };
        Some(
            h_flex()
                .id("task-status")
                .gap_1()
                .cursor_pointer()
                .hover(|style| style.underline())
                .child(Spinner::new().xsmall())
                .child(text)
                .tooltip(|window, cx| Tooltip::new("Show the background tasks").build(window, cx))
                .on_click(
                    cx.listener(|this, _, window, cx| {
                        this.open_section(Section::Tasks, window, cx)
                    }),
                )
                .into_any_element(),
        )
    }
}

/// The folder holding `file`, shortened to its last two parts.
pub(super) fn short_dir(file: &std::path::Path) -> String {
    let Some(dir) = file.parent() else {
        return String::new();
    };
    let parts: Vec<_> = dir.components().collect();
    if parts.len() <= 3 {
        return dir.display().to_string();
    }
    let tail: std::path::PathBuf = parts[parts.len() - 2..].iter().collect();
    format!("…{}{}", std::path::MAIN_SEPARATOR, tail.display())
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

/// How search matches stand out over the syntax colours.
pub(super) fn match_style(cx: &App) -> HighlightStyle {
    let theme = cx.theme();
    HighlightStyle {
        background_color: Some(
            theme
                .yellow
                .opacity(if theme.is_dark() { 0.4 } else { 0.55 }),
        ),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    }
}

/// A line of code with its syntax colours and search matches.
pub(super) fn code_text(
    text: &str,
    syntax: Option<&LineStyles>,
    matches: &[std::ops::Range<usize>],
    cx: &App,
) -> StyledText {
    if text.is_empty() {
        return StyledText::new(" ");
    }
    let highlight = match_style(cx);
    let highlights = combine_highlights(
        syntax.into_iter().flatten().cloned(),
        matches.iter().map(|range| (range.clone(), highlight)),
    );
    StyledText::new(SharedString::from(text.to_string())).with_highlights(highlights)
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

/// dowse's mark, as an icon: drawn in the text colour unless given another.
pub(super) fn brand_mark() -> Icon {
    Icon::empty().path(BRAND_MARK)
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
