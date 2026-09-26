//! The repositories page: add, tag, re-index and remove repositories.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::app::SearchApp;
use super::hub::{IndexActivity, RepoView};

impl SearchApp {
    pub(super) fn render_repositories_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;

        let header = h_flex()
            .gap_4()
            .items_start()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(div().text_xl().font_semibold().child(format!("Repositories in {}", self.workspace_name())))
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child("The repositories this workspace searches. Tag them to choose which ones a search covers. Use plain tags such as mirror or dev, or key:value tags such as owner:alice or project:billing, which get their own group. Every repository is also tagged with its current branch. Tags belong to the repository, so every workspace sees them."),
                    ),
            )
            .child(
                Button::new("add-repositories")
                    .primary()
                    .icon(IconName::Plus)
                    .label("Add Repositories…")
                    .tooltip("Choose a repository, or a folder holding several (Ctrl+O)")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.prompt_for_repositories(window, cx)
                    })),
            );

        let rows: Vec<AnyElement> = self
            .repo_views(cx)
            .iter()
            .map(|repo| self.render_repository_row(repo, cx).into_any_element())
            .collect();

        let list = if rows.is_empty() {
            div()
                .py_10()
                .text_center()
                .text_color(muted)
                .child("No repositories in this workspace yet. Add one, or a folder that holds several git repositories.")
                .into_any_element()
        } else {
            v_flex().gap_3().children(rows).into_any_element()
        };

        div()
            .id("repositories-page")
            .flex_1()
            .min_h_0()
            .size_full()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(1000.))
                    .mx_auto()
                    .px_6()
                    .py_5()
                    .gap_5()
                    .child(header)
                    .child(list)
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Press Enter or leave the field to save tags. Removing a repository takes it out of this workspace only; its index, tags and files are kept."),
                    )
                    .children(self.render_explorer_integration(cx)),
            )
            .overflow_y_scrollbar()
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
                    .gap_4()
                    .p_3()
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius_lg)
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

    fn render_repository_row(&self, repo: &RepoView, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, danger) = (theme.muted_foreground, theme.border, theme.danger);
        let radius = theme.radius_lg;
        let id = repo.info.id.clone();
        let problem = matches!(
            repo.activity,
            IndexActivity::Failed(_) | IndexActivity::Missing
        );
        let (rebuild_id, remove_id) = (id.clone(), id.clone());

        v_flex()
            .id(SharedString::from(format!("repo:{id}")))
            .gap_2()
            .p_3()
            .border_1()
            .border_color(border)
            .rounded(radius)
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(Lucide::FolderGit2).text_color(muted))
                    .child(div().font_semibold().child(repo.info.name.clone()))
                    .when_some(repo.info.branch.clone(), |row, branch| {
                        row.child(
                            h_flex()
                                .gap_1()
                                .text_sm()
                                .text_color(muted)
                                .child(Icon::new(Lucide::GitBranch).xsmall())
                                .child(branch),
                        )
                    })
                    .child(div().flex_1())
                    .when(repo.is_busy(), |row| row.child(Spinner::new().xsmall()))
                    .child(
                        div()
                            .max_w(px(420.))
                            .truncate()
                            .text_xs()
                            .text_color(if problem { danger } else { muted })
                            .child(repo.index_summary()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("rebuild:{id}")))
                            .ghost()
                            .xsmall()
                            .icon(Lucide::RefreshCw)
                            .tooltip("Update this index, reading only the files that changed")
                            .disabled(repo.is_busy() || repo.activity == IndexActivity::Missing)
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.queue_index(&rebuild_id, cx)
                                }),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("remove:{id}")))
                            .ghost()
                            .xsmall()
                            .icon(Lucide::Trash)
                            .tooltip(
                                "Remove from this workspace. Its index, tags and files are kept.",
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_repository(&remove_id, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .truncate()
                    .child(id.clone()),
            )
            .when_some(self.tag_inputs.get(&id), |row, tags| {
                row.child(
                    Input::new(&tags.input)
                        .small()
                        .prefix(Icon::new(Lucide::Tag).small().text_color(muted)),
                )
            })
    }
}
