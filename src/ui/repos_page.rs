//! The repositories page: the workspace's repositories, to filter, select
//! and act on several at once (pull, index, tag, sync, remove); an owner's
//! GitHub repositories, to clone; and the background tasks.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::app::SearchApp;
use super::hub::{IndexActivity, IndexJob, RepoView};
use super::manager::{Listing, RemoteStatus, Section};
use super::render::tag_label;
use super::tasks::TaskHub;
use crate::format;
use dowse::engine::github::{self, CloneMode, RemoteRepo};
use dowse::engine::sync::Interval;
use dowse::engine::tasks::{TaskInfo, TaskKind, TaskState};

/// Height of a row in the GitHub list, which is virtualized.
const REMOTE_ROW_HEIGHT: f32 = 56.;
const PAGE_WIDTH: f32 = 1180.;

impl SearchApp {
    pub(super) fn render_repositories_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let section = self.manager.section;
        let body = match section {
            Section::Workspace => self.render_workspace_section(cx).into_any_element(),
            Section::GitHub => self.render_github_section(cx).into_any_element(),
            Section::Tasks => self.render_tasks_section(cx).into_any_element(),
        };
        v_flex()
            .id("repositories-page")
            .flex_1()
            .min_h_0()
            .size_full()
            .child(self.render_page_header(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .max_w(px(PAGE_WIDTH))
                    .mx_auto()
                    .child(body),
            )
    }

    fn render_page_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let tasks = TaskHub::global(cx).read(cx).active_count();
        let labels = [
            format!("Workspace · {}", self.members.len()),
            "GitHub".to_string(),
            if tasks > 0 {
                format!("Tasks · {tasks}")
            } else {
                "Tasks".to_string()
            },
        ];
        let icons = [Lucide::FolderGit2, Lucide::Github, Lucide::ListChecks];
        let selected = Section::ALL
            .iter()
            .position(|section| *section == self.manager.section)
            .unwrap_or(0);
        let this = cx.entity().downgrade();
        let add_menu = {
            let app = cx.entity().downgrade();
            Button::new("add-menu")
                .primary()
                .small()
                .icon(IconName::Plus)
                .label("Add")
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    let (folder, clone) = (app.clone(), app.clone());
                    menu.item(
                        PopupMenuItem::new("Add Folders… (Ctrl+O)")
                            .icon(Icon::new(IconName::FolderOpen))
                            .on_click(move |_, window, cx| {
                                folder
                                    .update(cx, |this, cx| this.prompt_for_repositories(window, cx))
                                    .ok();
                            }),
                    )
                    .item(
                        PopupMenuItem::new("Clone from GitHub…")
                            .icon(Icon::new(Lucide::Github))
                            .on_click(move |_, _, cx| {
                                clone
                                    .update(cx, |this, cx| this.show_section(Section::GitHub, cx))
                                    .ok();
                            }),
                    )
                })
        };
        let row = h_flex()
            .w_full()
            .max_w(px(PAGE_WIDTH))
            .mx_auto()
            .px_6()
            .py_3()
            .gap_4()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(div().text_lg().font_semibold().child("Repositories"))
                    .child(
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(Icon::new(Lucide::Layers).xsmall())
                            .child(self.workspace_name()),
                    ),
            )
            .child(
                TabBar::new("repositories-sections")
                    .segmented()
                    .small()
                    .selected_index(selected)
                    .on_click(move |index: &usize, _, cx| {
                        this.update(cx, |this, cx| this.show_section(Section::ALL[*index], cx))
                            .ok();
                    })
                    .children(labels.into_iter().zip(icons).map(|(label, icon)| {
                        Tab::new()
                            .prefix(div().pl_2().child(Icon::new(icon).small()))
                            .label(label)
                    })),
            )
            .child(div().flex_1())
            .child(add_menu);
        div()
            .flex_none()
            .w_full()
            .border_b_1()
            .border_color(theme.border)
            .child(row)
    }

    // ----- the workspace's repositories ---------------------------------------------

    fn render_workspace_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let repos = self.managed_repos(cx);
        let total = self.members.len();
        let selected = self.selected_ids();
        let all_shown_selected = !repos.is_empty()
            && repos
                .iter()
                .all(|repo| self.manager.selected.contains(&repo.info.id));

        let toolbar = h_flex()
            .flex_none()
            .px_6()
            .pt_4()
            .pb_2()
            .gap_3()
            .child(
                Checkbox::new("select-all-repos")
                    .checked(all_shown_selected)
                    .tooltip("Select every repository shown")
                    .disabled(repos.is_empty())
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        this.select_all_shown(*checked, cx)
                    })),
            )
            .child(
                div().w(px(380.)).child(
                    Input::new(&self.manager.filter)
                        .small()
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).small().text_color(muted)),
                ),
            )
            .child(div().text_xs().text_color(muted).child(if repos.len() == total {
                format::plural(total, "repository", "repositories")
            } else {
                format!("{} of {total} shown", repos.len())
            }))
            .child(div().flex_1())
            .child(
                Button::new("pull-all")
                    .ghost()
                    .small()
                    .icon(Lucide::CloudDownload)
                    .label("Pull all")
                    .tooltip("Pull every repository shown: fetch, and fast-forward when that is safe")
                    .disabled(repos.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| {
                        let ids = this.managed_repos(cx).into_iter().map(|r| r.info.id.clone()).collect();
                        this.pull_repos(ids, window, cx)
                    })),
            )
            .child(
                Button::new("index-all")
                    .ghost()
                    .small()
                    .icon(Lucide::RefreshCw)
                    .label("Update indexes")
                    .tooltip("Update the index of every repository shown, reading only the files that changed")
                    .disabled(repos.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| {
                        let ids: Vec<String> =
                            this.managed_repos(cx).into_iter().map(|r| r.info.id.clone()).collect();
                        this.index_repos(&ids, IndexJob::Update, cx)
                    })),
            );

        let list =
            if total == 0 {
                empty_state(
                    Lucide::FolderGit2,
                    "No repositories in this workspace yet",
                    "Add folders from disk, or clone repositories from GitHub.",
                    cx,
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("empty-add")
                                .outline()
                                .icon(IconName::FolderOpen)
                                .label("Add Folders…")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.prompt_for_repositories(window, cx)
                                })),
                        )
                        .child(
                            Button::new("empty-clone")
                                .primary()
                                .icon(Lucide::Github)
                                .label("Clone from GitHub…")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_section(Section::GitHub, cx)
                                })),
                        ),
                )
                .into_any_element()
            } else if repos.is_empty() {
                empty_state(IconName::Search, "No repositories match the filter", "", cx)
                    .into_any_element()
            } else {
                let rows: Vec<AnyElement> = repos
                    .iter()
                    .map(|repo| self.render_repository_row(repo, cx).into_any_element())
                    .collect();
                v_flex()
                    .mx_6()
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius_lg)
                    .overflow_hidden()
                    .children(rows)
                    .into_any_element()
            };

        v_flex()
            .flex_1()
            .min_h_0()
            .child(toolbar)
            .when(!selected.is_empty(), |page| {
                page.child(self.render_selection_bar(selected.len(), cx))
            })
            .child(
                div()
                    .id("repository-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(
                        v_flex()
                            .pb_6()
                            .gap_4()
                            .child(list)
                            .child(
                                div()
                                    .px_6()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("Click a repository for its tags and pull settings. Tags such as mirror, dev or owner:alice choose what a search covers; every repository is also tagged with its branch, and sync:<interval> when dowse pulls it. Removing takes a repository out of this workspace only; its files, index and tags are kept."),
                            )
                            .children(self.render_explorer_integration(cx)),
                    ),
            )
    }

    /// Actions on the selected repositories.
    fn render_selection_bar(&self, count: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let app = cx.entity().downgrade();
        h_flex()
            .flex_none()
            .mx_6()
            .mb_2()
            .px_3()
            .py_1p5()
            .gap_2()
            .rounded(theme.radius_lg)
            .bg(theme.accent)
            .text_color(theme.accent_foreground)
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .child(format!("{count} selected")),
            )
            .child(div().w(px(1.)).h(px(18.)).bg(theme.border))
            .child(
                Button::new("bulk-pull")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::CloudDownload)
                    .label("Pull")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let ids = this.selected_ids();
                        this.pull_repos(ids, window, cx)
                    })),
            )
            .child(
                Button::new("bulk-index")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::RefreshCw)
                    .label("Update Index")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let ids = this.selected_ids();
                        this.index_repos(&ids, IndexJob::Update, cx)
                    })),
            )
            .child(
                Button::new("bulk-rebuild")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::Hammer)
                    .label("Rebuild")
                    .tooltip("Build the indexes again from every file")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let ids = this.selected_ids();
                        this.index_repos(&ids, IndexJob::Rebuild, cx)
                    })),
            )
            .child(
                Button::new("bulk-sync")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::Timer)
                    .label("Auto Pull")
                    .dropdown_caret(true)
                    .dropdown_menu(move |menu, _, _| {
                        interval_menu(menu, None, {
                            let app = app.clone();
                            move |every, window, cx| {
                                app.update(cx, |this, cx| {
                                    let ids = this.selected_ids();
                                    this.set_pull_every(&ids, every, window, cx)
                                })
                                .ok();
                            }
                        })
                    }),
            )
            .child(
                div().w(px(300.)).child(
                    Input::new(&self.manager.bulk_tags)
                        .xsmall()
                        .prefix(Icon::new(Lucide::Tag).xsmall()),
                ),
            )
            .child(
                Button::new("bulk-tag")
                    .ghost()
                    .xsmall()
                    .label("Apply Tags")
                    .on_click(cx.listener(|this, _, window, cx| this.apply_bulk_tags(window, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("bulk-remove")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::Trash)
                    .label("Remove")
                    .tooltip(
                        "Take them out of this workspace. Their files, indexes and tags are kept.",
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.remove_selected(window, cx))),
            )
            .child(
                Button::new("bulk-clear")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("Clear the selection")
                    .on_click(cx.listener(|this, _, _, cx| this.select_all_shown(false, cx))),
            )
    }

    fn render_repository_row(&self, repo: &RepoView, cx: &mut Context<Self>) -> impl IntoElement {
        let tasks = TaskHub::global(cx);
        let tasks = tasks.read(cx);
        let theme = cx.theme();
        let (muted, danger, success) = (theme.muted_foreground, theme.danger, theme.success);
        let id = repo.info.id.clone();
        let selected = self.manager.selected.contains(&id);
        let expanded = self.manager.expanded.as_deref() == Some(id.as_str());
        let is_git = Path::new(&id).join(".git").exists();
        let pulling = tasks.is_pulling(&id);
        let last_pull = tasks.last_pull(&id).cloned();
        let problem = matches!(
            repo.activity,
            IndexActivity::Failed(_) | IndexActivity::Missing
        );

        let mut chips = h_flex().gap_1().flex_wrap();
        if let Some(branch) = &repo.info.branch {
            chips = chips.child(chip(Some(Lucide::GitBranch), branch.clone(), None, cx));
        }
        if let Some(every) = repo.info.pull_every {
            chips = chips.child(chip(
                Some(Lucide::Timer),
                format!("pull every {every}"),
                Some(theme.primary),
                cx,
            ));
        }
        for tag in &repo.info.tags {
            chips = chips.child(chip(Some(Lucide::Tag), tag_label(tag), None, cx));
        }

        let status = v_flex()
            .w(px(320.))
            .flex_none()
            .items_end()
            .gap_0p5()
            .text_xs()
            .child(
                h_flex()
                    .gap_1()
                    .when(repo.is_busy(), |row| row.child(Spinner::new().xsmall()))
                    .child(
                        div()
                            .max_w(px(310.))
                            .truncate()
                            .text_color(if problem { danger } else { muted })
                            .child(repo.index_summary()),
                    ),
            )
            .when(pulling, |column| {
                column.child(
                    h_flex()
                        .gap_1()
                        .text_color(muted)
                        .child(Spinner::new().xsmall())
                        .child("Pulling…"),
                )
            })
            .when_some(last_pull.filter(|_| !pulling), |column, record| {
                column.child(
                    div()
                        .max_w(px(310.))
                        .truncate()
                        .text_color(if record.ok { success } else { danger })
                        .child(format!(
                            "{} · {}",
                            record.summary,
                            format::ago(record.at, SystemTime::now())
                        )),
                )
            });

        let menu_id = id.clone();
        let app = cx.entity().downgrade();
        // Clicks on the buttons are theirs, not the row's.
        let actions = h_flex()
            .id(SharedString::from(format!("actions:{id}")))
            .flex_none()
            .gap_0p5()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                Button::new(SharedString::from(format!("pull:{id}")))
                    .ghost()
                    .xsmall()
                    .icon(Lucide::CloudDownload)
                    .tooltip("Pull: fetch, and fast-forward the default branch when it is safe")
                    .disabled(!is_git || pulling)
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, window, cx| this.pull_repos(vec![id.clone()], window, cx)
                    })),
            )
            .child(
                Button::new(SharedString::from(format!("index:{id}")))
                    .ghost()
                    .xsmall()
                    .icon(Lucide::RefreshCw)
                    .tooltip("Update the index, reading only the files that changed")
                    .disabled(repo.is_busy() || repo.activity == IndexActivity::Missing)
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| this.queue_index(&id, cx)
                    })),
            )
            .child(
                Button::new(SharedString::from(format!("more:{id}")))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Ellipsis)
                    .dropdown_menu(move |menu, _, _| {
                        let item = |label: &str,
                                    icon: Lucide,
                                    run: fn(
                            &mut SearchApp,
                            &str,
                            &mut Window,
                            &mut Context<SearchApp>,
                        )| {
                            let (app, id) = (app.clone(), menu_id.clone());
                            PopupMenuItem::new(label.to_string())
                                .icon(Icon::new(icon))
                                .on_click(move |_, window, cx| {
                                    app.update(cx, |this, cx| run(this, &id, window, cx)).ok();
                                })
                        };
                        menu.item(item("Rebuild Index", Lucide::Hammer, |this, id, _, cx| {
                            this.index_repos(&[id.to_string()], IndexJob::Rebuild, cx)
                        }))
                        .item(item(
                            "Show in File Manager",
                            Lucide::FolderOpen,
                            |_, id, _, cx| cx.reveal_path(Path::new(id)),
                        ))
                        .item(item("Copy Path", Lucide::Copy, |this, id, window, cx| {
                            this.copy_path(id, window, cx)
                        }))
                        .separator()
                        .item(item(
                            "Remove from Workspace",
                            Lucide::Trash,
                            |this, id, window, cx| this.remove_repository(id, window, cx),
                        ))
                    }),
            )
            .child(
                Button::new(SharedString::from(format!("expand:{id}")))
                    .ghost()
                    .xsmall()
                    .icon(if expanded {
                        IconName::ChevronUp
                    } else {
                        IconName::ChevronDown
                    })
                    .tooltip("Tags and pull settings")
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| this.toggle_expanded_repo(&id, cx)
                    })),
            );

        let row = h_flex()
            .id(SharedString::from(format!("repo:{id}")))
            .gap_3()
            .px_3()
            .py_2()
            .cursor_pointer()
            .when(selected, |row| row.bg(theme.list_active))
            .when(!selected, |row| {
                row.hover(|style| style.bg(theme.list_hover))
            })
            .on_click(cx.listener({
                let id = id.clone();
                move |this, _, _, cx| this.toggle_expanded_repo(&id, cx)
            }))
            .child(
                Checkbox::new(SharedString::from(format!("select:{id}")))
                    .checked(selected)
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _: &bool, _, cx| {
                            cx.stop_propagation();
                            this.toggle_selected(&id, cx)
                        }
                    })),
            )
            .child(
                Icon::new(if is_git {
                    Lucide::FolderGit2
                } else {
                    Lucide::Folder
                })
                .text_color(if problem { danger } else { muted }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .font_semibold()
                                    .flex_none()
                                    .child(repo.info.name.clone()),
                            )
                            .child(chips),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .truncate()
                            .child(id.clone()),
                    ),
            )
            .child(status)
            .child(actions);

        v_flex()
            .border_b_1()
            .border_color(theme.table_row_border)
            .child(row)
            .when(expanded, |column| {
                column.child(self.render_repository_details(repo, cx))
            })
    }

    /// A repository's tags and pull settings, below its row.
    fn render_repository_details(
        &self,
        repo: &RepoView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let id = repo.info.id.clone();
        let is_git = Path::new(&id).join(".git").exists();
        let current = repo.info.pull_every;
        let app = cx.entity().downgrade();
        let label = |text: &str| {
            div()
                .w(px(110.))
                .flex_none()
                .text_sm()
                .text_color(muted)
                .child(text.to_string())
        };

        v_flex()
            .px_3()
            .pl(px(64.))
            .py_3()
            .gap_3()
            .bg(theme.secondary)
            .border_t_1()
            .border_color(theme.table_row_border)
            .child(
                h_flex()
                    .gap_3()
                    .child(label("Tags"))
                    .child(div().flex_1().children(self.tag_inputs.get(&id).map(|tags| {
                        Input::new(&tags.input)
                            .small()
                            .prefix(Icon::new(Lucide::Tag).small().text_color(muted))
                    }))),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(label("Pull"))
                    .child(
                        Button::new(SharedString::from(format!("sync:{id}")))
                            .outline()
                            .small()
                            .icon(Lucide::Timer)
                            .label(match current {
                                Some(every) => format!("Every {every}"),
                                None => "Never".to_string(),
                            })
                            .disabled(!is_git)
                            .dropdown_caret(true)
                            .dropdown_menu(move |menu, _, _| {
                                let (app, id) = (app.clone(), id.clone());
                                interval_menu(menu, Some(current), move |every, window, cx| {
                                    app.update(cx, |this, cx| {
                                        this.set_pull_every(std::slice::from_ref(&id), every, window, cx)
                                    })
                                    .ok();
                                })
                            }),
                    )
                    .child(div().flex_1().min_w_0().text_xs().text_color(muted).child(if is_git {
                        "Fetches origin, then fast-forwards the default branch when it is checked out, has no uncommitted changes and no commits of its own. Otherwise it only fetches, and says why."
                    } else {
                        "Only git repositories can be pulled."
                    })),
            )
    }

    /// Offer "Add to dowse" in Explorer's folder menu, and opening workspace
    /// files with a double click. Windows only.
    fn render_explorer_integration(&self, cx: &Context<Self>) -> Option<AnyElement> {
        #[cfg(windows)]
        {
            use crate::shell::{self, State};
            use gpui_kit::component::WindowExt as _;
            use gpui_kit::component::notification::Notification;
            let exe = std::env::current_exe().ok()?;
            let state = shell::state(&exe);
            let theme = cx.theme();
            let (label, detail) = match state {
                State::Missing => (
                    "Add to Explorer",
                    "Right-click a folder in Explorer and choose \"Add to dowse\" to add it to the last focused window, as dowse --add does. Workspace files open with a double click.",
                ),
                State::Installed => (
                    "Remove from Explorer",
                    "Explorer offers \"Add to dowse\" on folders, and opens workspace files with a double click.",
                ),
                State::Elsewhere => (
                    "Point Explorer here",
                    "Explorer's \"Add to dowse\" starts another copy of dowse.",
                ),
            };
            Some(
                h_flex()
                    .mx_6()
                    .gap_4()
                    .p_3()
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius_lg)
                    .child(Icon::new(Lucide::MousePointerClick).text_color(theme.muted_foreground))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(div().text_sm().font_semibold().child("Explorer"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(detail),
                            ),
                    )
                    .child(
                        Button::new("explorer-integration")
                            .outline()
                            .small()
                            .label(label)
                            .on_click(cx.listener(move |_, _, window, cx| {
                                let result = match state {
                                    State::Installed => shell::uninstall(),
                                    State::Missing | State::Elsewhere => shell::install(&exe),
                                };
                                if let Err(error) = result {
                                    window.push_notification(
                                        Notification::error(format!(
                                            "Could not change Explorer's menu: {error}"
                                        )),
                                        cx,
                                    );
                                }
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            )
        }
        #[cfg(not(windows))]
        {
            let _ = cx;
            None
        }
    }

    // ----- GitHub ---------------------------------------------------------------------

    fn render_github_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let github = &self.manager.github;
        let loading = matches!(github.listing, Listing::Loading(_));
        let listed = github.listing.repos().len();
        let shown = github.visible.len();
        let chosen = github.selected.len();

        let status: AnyElement = match &github.listing {
            Listing::NotLoaded => div().child("").into_any_element(),
            Listing::Loading(owner) => h_flex()
                .gap_1p5()
                .child(Spinner::new().xsmall())
                .child(format!("Listing the repositories of {owner}…"))
                .into_any_element(),
            Listing::Loaded { owner, .. } => div()
                .child(format!(
                    "{} of {owner}",
                    format::plural(listed, "repository", "repositories")
                ))
                .into_any_element(),
            Listing::Failed(error) => div()
                .text_color(theme.danger)
                .max_w(px(560.))
                .truncate()
                .child(error.clone())
                .into_any_element(),
        };

        let source = h_flex()
            .flex_none()
            .px_6()
            .pt_4()
            .gap_3()
            .child(
                div().w(px(340.)).child(
                    Input::new(&github.owner)
                        .small()
                        .prefix(Icon::new(Lucide::Users).small().text_color(muted)),
                ),
            )
            .child(
                Button::new("load-github")
                    .outline()
                    .small()
                    .icon(Lucide::RefreshCw)
                    .label(if listed > 0 { "Refresh" } else { "List" })
                    .loading(loading)
                    .tooltip(
                        "List the owner's repositories with the GitHub CLI (gh), 100 at a time",
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.load_github(window, cx))),
            )
            .child(div().text_xs().text_color(muted).child(status))
            .child(div().flex_1())
            .when_some(github.me.clone(), |row, me| {
                row.child(
                    h_flex()
                        .gap_1()
                        .text_xs()
                        .text_color(muted)
                        .child(Icon::new(Lucide::CircleUser).xsmall())
                        .child(format!("Signed in to gh as {me}")),
                )
            });

        let (forks, archived) = (github.forks, github.archived);
        let filters = h_flex()
            .flex_none()
            .px_6()
            .pt_3()
            .pb_2()
            .gap_3()
            .child(
                Checkbox::new("select-all-remote")
                    .checked(chosen > 0 && chosen >= shown)
                    .tooltip("Select every repository shown that is not cloned yet")
                    .disabled(shown == 0)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.select_all_remote(*checked, cx)
                    })),
            )
            .child(
                div().w(px(380.)).child(
                    Input::new(&github.filter)
                        .small()
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).small().text_color(muted)),
                ),
            )
            .child(
                Checkbox::new("show-forks")
                    .label("Forks")
                    .checked(forks)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.set_remote_option(Some(*checked), None, cx)
                    })),
            )
            .child(
                Checkbox::new("show-archived")
                    .label("Archived")
                    .checked(archived)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.set_remote_option(None, Some(*checked), cx)
                    })),
            )
            .child(div().flex_1())
            .child(div().text_xs().text_color(muted).child(if listed == 0 {
                String::new()
            } else if shown == listed {
                format!("{shown} shown")
            } else {
                format!("{shown} of {listed} shown")
            }));

        let list = if shown == 0 {
            match &github.listing {
                Listing::NotLoaded | Listing::Loading(_) => div().flex_1().into_any_element(),
                Listing::Failed(error) => {
                    empty_state(Lucide::Github, "Could not list the repositories", error, cx)
                        .into_any_element()
                }
                Listing::Loaded { .. } => empty_state(
                    IconName::Search,
                    if listed == 0 {
                        "The owner has no repositories you can see"
                    } else {
                        "No repositories match; forks and archived ones are hidden unless ticked"
                    },
                    "",
                    cx,
                )
                .into_any_element(),
            }
        } else {
            let statuses = self.remote_statuses(cx);
            let root = self.destination_root(cx);
            div()
                .flex_1()
                .min_h_0()
                .mx_6()
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius_lg)
                .overflow_hidden()
                .child(
                    uniform_list(
                        "github-repositories",
                        shown,
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            let repos = this.manager.github.listing.repos();
                            range
                                .map(|row| {
                                    let repo = &repos[this.manager.github.visible[row]];
                                    let status = this.remote_status(repo, &statuses, root.as_ref());
                                    let checked =
                                        this.manager.github.selected.contains(&repo.full_name);
                                    render_remote_row(row, repo, status, checked, cx)
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .size_full()
                    .track_scroll(&self.manager.github.scroll),
                )
                .into_any_element()
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .child(source)
            .child(filters)
            .child(list)
            .child(self.render_clone_bar(cx))
    }

    /// Where and how the chosen repositories are cloned.
    fn render_clone_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let github = &self.manager.github;
        let chosen = github.selected.len();
        let root = self.destination_root(cx);
        let (mode, pull_every) = (github.mode, github.pull_every);
        let (mode_app, sync_app) = (cx.entity().downgrade(), cx.entity().downgrade());
        let example = root.as_ref().map(|root| {
            github::clone_destination(root, "<owner>/<name>")
                .display()
                .to_string()
        });

        v_flex()
            .flex_none()
            .mx_6()
            .my_4()
            .p_3()
            .gap_2()
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .bg(theme.secondary)
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(70.)).text_sm().text_color(muted).child("Clone into"))
                    .child(
                        div().flex_1().child(
                            Input::new(&github.destination)
                                .small()
                                .prefix(Icon::new(IconName::Folder).small().text_color(muted)),
                        ),
                    )
                    .child(
                        Button::new("browse-destination")
                            .outline()
                            .small()
                            .label("Browse…")
                            .on_click(cx.listener(|this, _, window, cx| this.browse_destination(window, cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(70.)).text_sm().text_color(muted).child("Options"))
                    .child(
                        Button::new("clone-mode")
                            .outline()
                            .small()
                            .icon(Lucide::GitCommitHorizontal)
                            .label(mode.label())
                            .tooltip(mode.describe())
                            .dropdown_caret(true)
                            .dropdown_menu(move |mut menu, _, _| {
                                for option in CloneMode::ALL {
                                    let app = mode_app.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(format!("{}: {}", option.label(), option.describe()))
                                            .checked(option == mode)
                                            .on_click(move |_, _, cx| {
                                                app.update(cx, |this, cx| {
                                                    this.manager.github.mode = option;
                                                    cx.notify();
                                                })
                                                .ok();
                                            }),
                                    );
                                }
                                menu
                            }),
                    )
                    .child(
                        Button::new("clone-sync")
                            .outline()
                            .small()
                            .icon(Lucide::Timer)
                            .label(match pull_every {
                                Some(every) => format!("Pull every {every}"),
                                None => "No auto pull".to_string(),
                            })
                            .dropdown_caret(true)
                            .dropdown_menu(move |menu, _, _| {
                                let app = sync_app.clone();
                                interval_menu(menu, Some(pull_every), move |every, _, cx| {
                                    app.update(cx, |this, cx| {
                                        this.manager.github.pull_every = every;
                                        cx.notify();
                                    })
                                    .ok();
                                })
                            }),
                    )
                    .child(
                        div().w(px(260.)).child(
                            Input::new(&github.tags)
                                .small()
                                .prefix(Icon::new(Lucide::Tag).small().text_color(muted)),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("clone-selected")
                            .primary()
                            .small()
                            .icon(Lucide::Download)
                            .label(if chosen == 0 {
                                "Clone".to_string()
                            } else {
                                format!("Clone {}", format::plural(chosen, "repository", "repositories"))
                            })
                            .disabled(chosen == 0 || root.is_none())
                            .on_click(cx.listener(|this, _, window, cx| this.clone_selected(window, cx))),
                    ),
            )
            .child(div().text_xs().text_color(muted).child(match example {
                Some(example) => format!(
                    "Each goes to {example}, tagged owner:<owner>, and joins this workspace when it finishes. Clones run in the background, {} at a time; see Tasks.",
                    TaskHub::global(cx).read(cx).limit(TaskKind::Clone)
                ),
                None => "Choose the folder clones go under; each goes to <folder>/<owner>/<name>.".to_string(),
            }))
    }

    // ----- tasks ------------------------------------------------------------------------

    fn render_tasks_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let tasks = TaskHub::global(cx);
        let infos = tasks.read(cx).infos();
        let (clone_limit, pull_limit) = (
            tasks.read(cx).limit(TaskKind::Clone),
            tasks.read(cx).limit(TaskKind::Pull),
        );
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let running = infos
            .iter()
            .filter(|info| info.state == TaskState::Running)
            .count();
        let queued = infos
            .iter()
            .filter(|info| info.state == TaskState::Queued)
            .count();
        let finished = infos.len() - running - queued;

        let stepper = |id: &'static str,
                       label: &str,
                       kind: TaskKind,
                       value: usize,
                       cx: &mut Context<Self>| {
            h_flex()
                .gap_1()
                .child(div().text_xs().text_color(muted).child(label.to_string()))
                .child(
                    Button::new(SharedString::from(format!("{id}-less")))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Minus)
                        .disabled(value <= 1)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            TaskHub::global(cx)
                                .update(cx, |tasks, cx| tasks.set_limit(kind, value - 1, cx))
                        })),
                )
                .child(
                    div()
                        .w(px(20.))
                        .text_center()
                        .text_sm()
                        .child(value.to_string()),
                )
                .child(
                    Button::new(SharedString::from(format!("{id}-more")))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .disabled(value >= 16)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            TaskHub::global(cx)
                                .update(cx, |tasks, cx| tasks.set_limit(kind, value + 1, cx))
                        })),
                )
        };

        let toolbar = h_flex()
            .flex_none()
            .px_6()
            .pt_4()
            .pb_3()
            .gap_4()
            .child(div().text_sm().child(format!(
                "{running} running · {queued} queued · {finished} finished"
            )))
            .child(div().flex_1())
            .child(stepper(
                "clone-limit",
                "Clones at once",
                TaskKind::Clone,
                clone_limit,
                cx,
            ))
            .child(stepper(
                "pull-limit",
                "Pulls at once",
                TaskKind::Pull,
                pull_limit,
                cx,
            ))
            .child(
                Button::new("cancel-all")
                    .ghost()
                    .small()
                    .icon(Lucide::CircleStop)
                    .label("Cancel All")
                    .disabled(running + queued == 0)
                    .on_click(cx.listener(|_, _, _, cx| {
                        TaskHub::global(cx).update(cx, |tasks, cx| tasks.cancel_all(cx));
                    })),
            )
            .child(
                Button::new("clear-finished")
                    .ghost()
                    .small()
                    .icon(Lucide::ListX)
                    .label("Clear Finished")
                    .disabled(finished == 0)
                    .on_click(cx.listener(|_, _, _, cx| {
                        TaskHub::global(cx).update(cx, |tasks, cx| tasks.clear_finished(cx))
                    })),
            );

        let list = if infos.is_empty() {
            empty_state(
                Lucide::ListChecks,
                "No tasks",
                "Clones and pulls run here in the background, so you can keep searching.",
                cx,
            )
            .into_any_element()
        } else {
            // Running first, then queued, then the latest finished.
            let mut ordered: Vec<&TaskInfo> = infos.iter().collect();
            ordered.sort_by_key(|info| {
                let rank = match info.state {
                    TaskState::Running => 0,
                    TaskState::Queued => 1,
                    _ => 2,
                };
                let recency = match info.state {
                    TaskState::Queued => info.id as i64,
                    _ => -(info.id as i64),
                };
                (rank, recency)
            });
            let rows: Vec<AnyElement> = ordered
                .into_iter()
                .map(|info| self.render_task_row(info, cx).into_any_element())
                .collect();
            div()
                .id("task-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(
                    v_flex()
                        .mx_6()
                        .mb_6()
                        .border_1()
                        .border_color(theme.border)
                        .rounded(theme.radius_lg)
                        .overflow_hidden()
                        .children(rows),
                )
                .into_any_element()
        };

        v_flex().flex_1().min_h_0().child(toolbar).child(list)
    }

    fn render_task_row(&self, info: &TaskInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let id = info.id;
        let title = self.task_title(info, cx);
        let (icon, kind) = match info.kind {
            TaskKind::Clone => (Lucide::Download, "Clone"),
            TaskKind::Pull => (Lucide::CloudDownload, "Pull"),
        };
        let (state_text, state_color) = match &info.state {
            TaskState::Queued => ("Queued".to_string(), muted),
            TaskState::Running => (
                info.fraction
                    .map(|fraction| format!("{:.0}%", fraction * 100.0))
                    .unwrap_or_else(|| "Running".into()),
                theme.primary,
            ),
            TaskState::Done { message } => (message.clone(), theme.success),
            TaskState::Failed { error } => (error.clone(), theme.danger),
            TaskState::Cancelled => ("Cancelled".to_string(), muted),
        };
        let when = info.finished_at.unwrap_or(info.queued_at);
        let when = format::ago(
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(when),
            SystemTime::now(),
        );
        let running = info.state == TaskState::Running;
        let retryable = matches!(info.state, TaskState::Failed { .. } | TaskState::Cancelled);
        let folder: Option<PathBuf> = match (&info.kind, &info.state) {
            (TaskKind::Clone, TaskState::Done { message }) => {
                message.strip_prefix("cloned into ").map(PathBuf::from)
            }
            (TaskKind::Pull, _) => Some(PathBuf::from(&info.title)),
            _ => None,
        };

        h_flex()
            .id(("task", id as usize))
            .gap_3()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.table_row_border)
            .child(Icon::new(icon).text_color(muted))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_xs().text_color(muted).child(kind))
                            .child(div().font_semibold().truncate().child(title)),
                    )
                    .when(running, |column| {
                        column.child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div().w(px(240.)).flex_none().child(
                                        Progress::new(("task-progress", id as usize))
                                            .value(info.fraction.unwrap_or(0.0) * 100.0)
                                            .loading(info.fraction.is_none()),
                                    ),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .truncate()
                                        .child(info.detail.clone()),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .max_w(px(420.))
                    .text_xs()
                    .text_color(state_color)
                    .truncate()
                    .child(state_text),
            )
            .child(
                div()
                    .w(px(80.))
                    .flex_none()
                    .text_right()
                    .text_xs()
                    .text_color(muted)
                    .child(when),
            )
            .child(
                h_flex()
                    .flex_none()
                    .w(px(56.))
                    .justify_end()
                    .gap_0p5()
                    .when(!info.state.is_finished(), |row| {
                        row.child(
                            Button::new(("cancel-task", id as usize))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("Cancel")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    TaskHub::global(cx).update(cx, |tasks, cx| tasks.cancel(id, cx))
                                })),
                        )
                    })
                    .when(retryable, |row| {
                        row.child(
                            Button::new(("retry-task", id as usize))
                                .ghost()
                                .xsmall()
                                .icon(Lucide::RotateCw)
                                .tooltip("Try again")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    TaskHub::global(cx).update(cx, |tasks, cx| tasks.retry(id, cx))
                                })),
                        )
                    })
                    .when_some(folder.filter(|folder| folder.exists()), |row, folder| {
                        row.child(
                            Button::new(("reveal-task", id as usize))
                                .ghost()
                                .xsmall()
                                .icon(IconName::FolderOpen)
                                .tooltip("Show in file manager")
                                .on_click(move |_, _, cx| cx.reveal_path(&folder)),
                        )
                    }),
            )
    }
}

/// A GitHub repository in the clone list.
fn render_remote_row(
    row: usize,
    repo: &RemoteRepo,
    status: RemoteStatus,
    checked: bool,
    cx: &mut Context<SearchApp>,
) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let full_name = repo.full_name.clone();
    let selectable = matches!(status, RemoteStatus::Available | RemoteStatus::Failed(_));
    let mut badges = h_flex().gap_1();
    if repo.private {
        badges = badges.child(chip(Some(Lucide::Lock), "private".into(), None, cx));
    }
    if repo.fork {
        badges = badges.child(chip(Some(Lucide::GitFork), "fork".into(), None, cx));
    }
    if repo.archived {
        badges = badges.child(chip(
            Some(Lucide::Archive),
            "archived".into(),
            Some(theme.warning),
            cx,
        ));
    }
    let pushed = repo
        .pushed_at
        .as_deref()
        .and_then(github::parse_timestamp)
        .map(|at| format::ago(at, SystemTime::now()))
        .unwrap_or_default();
    let status_element: AnyElement = match &status {
        RemoteStatus::Available => div().into_any_element(),
        RemoteStatus::Cloned => h_flex()
            .gap_1()
            .text_color(theme.success)
            .child(Icon::new(IconName::Check).xsmall())
            .child("Cloned")
            .into_any_element(),
        RemoteStatus::Queued => div().text_color(muted).child("Queued").into_any_element(),
        RemoteStatus::Cloning(fraction) => h_flex()
            .gap_1p5()
            .child(
                div().w(px(70.)).child(
                    Progress::new(("clone-progress", row))
                        .value(fraction.unwrap_or(0.0) * 100.0)
                        .loading(fraction.is_none()),
                ),
            )
            .child(format!("{:.0}%", fraction.unwrap_or(0.0) * 100.0))
            .into_any_element(),
        RemoteStatus::Failed(error) => div()
            .id(("clone-failed", row))
            .text_color(theme.danger)
            .child("Failed")
            .tooltip({
                let error = error.clone();
                move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(error.clone()).build(window, cx)
                }
            })
            .into_any_element(),
    };

    h_flex()
        .id(("remote", row))
        .w_full()
        .h(px(REMOTE_ROW_HEIGHT))
        .gap_3()
        .px_3()
        .border_b_1()
        .border_color(theme.table_row_border)
        .when(checked, |row| row.bg(theme.list_active))
        .when(selectable && !checked, |row| {
            row.hover(|style| style.bg(theme.list_hover))
        })
        .when(selectable, |row| {
            row.cursor_pointer().on_click(cx.listener({
                let full_name = full_name.clone();
                move |this, _, _, cx| this.toggle_remote(&full_name, cx)
            }))
        })
        .child(
            Checkbox::new(("remote-check", row))
                .checked(checked || status == RemoteStatus::Cloned)
                .disabled(!selectable)
                .on_click(cx.listener({
                    let full_name = full_name.clone();
                    move |this, _: &bool, _, cx| {
                        cx.stop_propagation();
                        this.toggle_remote(&full_name, cx)
                    }
                })),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div().flex_none().child(
                                h_flex()
                                    .child(
                                        div().text_color(muted).child(format!("{}/", repo.owner)),
                                    )
                                    .child(div().font_semibold().child(repo.name.clone())),
                            ),
                        )
                        .child(badges),
                )
                .child(div().text_xs().text_color(muted).truncate().child(
                    if repo.description.is_empty() {
                        "No description".to_string()
                    } else {
                        repo.description.clone()
                    },
                )),
        )
        .child(
            div()
                .w(px(110.))
                .flex_none()
                .text_xs()
                .text_color(muted)
                .truncate()
                .child(repo.language.clone().unwrap_or_default()),
        )
        .child(
            div()
                .w(px(70.))
                .flex_none()
                .text_right()
                .text_xs()
                .text_color(muted)
                .child(format::kilobytes(repo.size_kb)),
        )
        .child(
            div()
                .w(px(80.))
                .flex_none()
                .text_right()
                .text_xs()
                .text_color(muted)
                .child(pushed),
        )
        .child(
            div()
                .w(px(110.))
                .flex_none()
                .flex()
                .justify_end()
                .text_xs()
                .child(status_element),
        )
        .into_any_element()
}

/// Fill `menu` with the pull intervals, calling `choose` with the one picked.
/// `current` ticks the interval in use, when there is one to show.
fn interval_menu(
    mut menu: gpui_kit::component::menu::PopupMenu,
    current: Option<Option<Interval>>,
    choose: impl Fn(Option<Interval>, &mut Window, &mut App) + 'static,
) -> gpui_kit::component::menu::PopupMenu {
    let choose = std::rc::Rc::new(choose);
    let never = choose.clone();
    menu = menu
        .item(
            PopupMenuItem::new("Never")
                .checked(current == Some(None))
                .on_click(move |_, window, cx| never(None, window, cx)),
        )
        .separator();
    for preset in Interval::PRESETS {
        let every: Interval = preset.parse().expect("presets parse");
        let choose = choose.clone();
        menu = menu.item(
            PopupMenuItem::new(format!("Every {every}"))
                .checked(current == Some(Some(every)))
                .on_click(move |_, window, cx| choose(Some(every), window, cx)),
        );
    }
    menu
}

/// A small rounded label.
fn chip(icon: Option<Lucide>, text: String, color: Option<Hsla>, cx: &App) -> Div {
    let theme = cx.theme();
    let color = color.unwrap_or(theme.muted_foreground);
    h_flex()
        .flex_none()
        .gap_1()
        .px_1p5()
        .py(px(1.))
        .rounded(px(4.))
        .border_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(color)
        .when_some(icon, |chip, icon| chip.child(Icon::new(icon).xsmall()))
        .child(text)
}

fn empty_state(icon: impl Into<Icon>, title: &str, detail: &str, cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .flex_1()
        .py_12()
        .gap_2()
        .items_center()
        .justify_center()
        .child(icon.into().size(px(36.)).text_color(theme.muted_foreground))
        .child(div().text_sm().font_semibold().child(title.to_string()))
        .when(!detail.is_empty(), |state| {
            state.child(
                div()
                    .max_w(px(560.))
                    .text_center()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(detail.to_string()),
            )
        })
        .mb_3()
}
