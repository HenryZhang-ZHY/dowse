//! The window's main menu, at the left of the title bar: File, Workspace,
//! Repositories, View and Help, each command with its shortcut. Also the About dialog and the
//! filters sidebar, which the title bar shows and hides.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Side, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::*;

use std::path::PathBuf;

use super::app::{Page, SearchApp};
use super::manager::Section;
use super::render::{brand_mark, short_dir};
use super::table::ResultsView;
use super::windows::Windows;
use super::*;
use dowse::engine::workspace;

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// A menu item running `run` on the window, showing `action`'s shortcut.
fn command(
    app: &WeakEntity<SearchApp>,
    label: &'static str,
    action: Option<Box<dyn Action>>,
    run: impl Fn(&mut SearchApp, &mut Window, &mut Context<SearchApp>) + 'static,
) -> PopupMenuItem {
    let app = app.clone();
    let item = PopupMenuItem::new(label).on_click(move |_, window, cx| {
        app.update(cx, |this, cx| run(this, window, cx)).ok();
    });
    match action {
        Some(action) => item.action(action),
        None => item,
    }
}

/// What the View menu shows as checked or unavailable, read when the menu
/// opens.
#[derive(Clone, Copy)]
struct ViewState {
    can_go_back: bool,
    can_go_forward: bool,
    sidebar_open: bool,
    preview_open: bool,
    table: bool,
}

fn file_menu(menu: PopupMenu, app: &WeakEntity<SearchApp>) -> PopupMenu {
    menu.item(command(
        app,
        "New Window",
        Some(Box::new(NewWindow)),
        |_, _, cx| {
            Windows::new_window(cx);
        },
    ))
    .item(command(
        app,
        "New Tab",
        Some(Box::new(NewTab)),
        |this, w, cx| this.open_tab(w, cx),
    ))
    .item(command(
        app,
        "Close Tab",
        Some(Box::new(CloseTab)),
        |this, w, cx| this.close_tab(this.active_tab, w, cx),
    ))
    .separator()
    .item(
        PopupMenuItem::new("Quit")
            .action(Box::new(Quit))
            .on_click(|_, _, cx| cx.quit()),
    )
}

/// The window's workspace, read when the menu opens.
#[derive(Clone)]
struct WorkspaceState {
    name: String,
    file: Option<PathBuf>,
}

/// The workspace commands, then the recent workspaces, each with a button
/// that takes it off the list and draws the menu again without it.
fn workspace_menu(
    menu: PopupMenu,
    app: &WeakEntity<SearchApp>,
    state: &WorkspaceState,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let recent: Vec<PathBuf> = Windows::recent(cx)
        .into_iter()
        .filter(|file| Some(file) != state.file.as_ref())
        .collect();
    let mut menu = menu
        .min_w(px(320.))
        .label(state.name.clone())
        .item(command(app, "New Workspace", None, |this, w, cx| {
            this.on_new_workspace(&NewWorkspace, w, cx)
        }))
        .item(command(
            app,
            "Open Workspace…",
            Some(Box::new(OpenWorkspace)),
            |this, w, cx| this.on_open_workspace(&OpenWorkspace, w, cx),
        ))
        .item(command(
            app,
            if state.file.is_some() {
                "Save Workspace As…"
            } else {
                "Save Workspace…"
            },
            Some(Box::new(SaveWorkspaceAs)),
            |this, w, cx| this.on_save_workspace_as(&SaveWorkspaceAs, w, cx),
        ));
    if recent.is_empty() {
        return menu;
    }
    menu = menu.separator().label("Recent");
    let this_menu = cx.entity().downgrade();
    for (index, file) in recent.into_iter().enumerate() {
        let open = {
            let (app, file) = (app.clone(), file.clone());
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                let file = file.clone();
                app.update(cx, |this, cx| this.open_workspace_file(file, window, cx))
                    .ok();
            }
        };
        let (app, state, this_menu) = (app.clone(), state.clone(), this_menu.clone());
        menu = menu.item(
            PopupMenuItem::element(move |_, cx| {
                let muted = cx.theme().muted_foreground;
                let forget = {
                    let (app, state, this_menu, file) =
                        (app.clone(), state.clone(), this_menu.clone(), file.clone());
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        // Not the row's click, which would open the workspace.
                        cx.stop_propagation();
                        Windows::forget_recent(&file, cx);
                        let (app, state) = (app.clone(), state.clone());
                        this_menu
                            .update(cx, |menu, cx| {
                                menu.rebuild(window, cx, |menu, _, cx| {
                                    workspace_menu(menu, &app, &state, cx)
                                })
                            })
                            .ok();
                    }
                };
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(div().flex_none().child(workspace::name(&file)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(muted)
                            .child(short_dir(&file)),
                    )
                    .child(
                        Button::new(("forget-recent", index))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip("Remove from recent workspaces")
                            .on_click(forget),
                    )
            })
            .on_click(open),
        );
    }
    menu
}

fn repositories_menu(menu: PopupMenu, app: &WeakEntity<SearchApp>) -> PopupMenu {
    menu.item(command(
        app,
        "Manage Repositories",
        Some(Box::new(ShowRepositories)),
        |this, w, cx| this.show_page(Page::Repositories, w, cx),
    ))
    .separator()
    .item(command(
        app,
        "Add Repositories…",
        Some(Box::new(AddRepository)),
        |this, w, cx| this.on_add_repository(&AddRepository, w, cx),
    ))
    .item(command(app, "Clone from GitHub…", None, |this, w, cx| {
        this.open_section(Section::GitHub, w, cx)
    }))
    .separator()
    .item(command(
        app,
        "Pull Repositories in Scope",
        None,
        |this, w, cx| this.pull_scope(w, cx),
    ))
    .item(command(
        app,
        "Update Indexes in Scope",
        Some(Box::new(UpdateIndex)),
        |this, w, cx| this.on_update_index(&UpdateIndex, w, cx),
    ))
    .item(command(
        app,
        "Rebuild Indexes in Scope",
        None,
        |this, w, cx| this.on_rebuild_index(&RebuildIndex, w, cx),
    ))
    .separator()
    .item(command(app, "Background Tasks", None, |this, w, cx| {
        this.open_section(Section::Tasks, w, cx)
    }))
}

fn view_menu(
    menu: PopupMenu,
    app: &WeakEntity<SearchApp>,
    state: ViewState,
    dark: bool,
) -> PopupMenu {
    menu.check_side(Side::Right)
        .item(
            command(app, "Back", Some(Box::new(GoBack)), |this, w, cx| {
                this.go_back(w, cx)
            })
            .disabled(!state.can_go_back),
        )
        .item(
            command(app, "Forward", Some(Box::new(GoForward)), |this, w, cx| {
                this.go_forward(w, cx)
            })
            .disabled(!state.can_go_forward),
        )
        .separator()
        .item(
            command(
                app,
                "Filters Sidebar",
                Some(Box::new(ToggleSidebar)),
                |this, _, cx| this.toggle_sidebar(cx),
            )
            .checked(state.sidebar_open),
        )
        .item(
            command(
                app,
                "Preview Pane",
                Some(Box::new(TogglePreview)),
                |this, _, cx| this.toggle_preview(cx),
            )
            .checked(state.preview_open),
        )
        .item(
            command(
                app,
                "Results as Table",
                Some(Box::new(ToggleResultsView)),
                |this, w, cx| this.on_toggle_results_view(&ToggleResultsView, w, cx),
            )
            .checked(state.table),
        )
        .separator()
        .item(command(
            app,
            "Command Palette",
            Some(Box::new(CommandPalette)),
            |this, w, cx| this.open_palette(w, cx),
        ))
        .item(
            command(app, "Dark Theme", None, |this, w, cx| {
                this.on_toggle_theme(&ToggleTheme, w, cx)
            })
            .checked(dark),
        )
        .separator()
        .item(
            PopupMenuItem::new("Developer Tools")
                .action(Box::new(ToggleDevTools))
                .on_click(|_, _, cx| super::devtools::open(cx)),
        )
}

fn help_menu(menu: PopupMenu, app: &WeakEntity<SearchApp>) -> PopupMenu {
    menu.link("dowse on GitHub", REPOSITORY)
        .link("Release Notes", format!("{REPOSITORY}/releases"))
        .link("Report an Issue", format!("{REPOSITORY}/issues/new"))
        .separator()
        .item(command(app, "About dowse", None, |this, w, cx| {
            this.open_about(w, cx)
        }))
}

impl SearchApp {
    pub(super) fn render_main_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app = cx.entity().downgrade();
        let tab = self.tab();
        let workspace = WorkspaceState {
            name: self.workspace_name(),
            file: self.workspace.clone(),
        };
        let state = ViewState {
            can_go_back: self.can_go_back(),
            can_go_forward: self.can_go_forward(),
            sidebar_open: self.sidebar_open,
            preview_open: tab.preview_open,
            table: tab.view == ResultsView::Table,
        };
        Button::new("main-menu")
            .ghost()
            .small()
            .icon(IconName::Menu)
            .tooltip("Menu")
            .dropdown_menu(move |menu, window, cx| {
                let dark = cx.theme().is_dark();
                let (file, repos, view, help) =
                    (app.clone(), app.clone(), app.clone(), app.clone());
                let (workspace_app, workspace) = (app.clone(), workspace.clone());
                menu.min_w(px(200.))
                    .submenu("File", window, cx, move |menu, _, _| file_menu(menu, &file))
                    .submenu("Workspace", window, cx, move |menu, _, cx| {
                        workspace_menu(menu, &workspace_app, &workspace, cx)
                    })
                    .submenu("Repositories", window, cx, move |menu, _, _| {
                        repositories_menu(menu, &repos)
                    })
                    .submenu("View", window, cx, move |menu, _, _| {
                        view_menu(menu, &view, state, dark)
                    })
                    .submenu("Help", window, cx, move |menu, _, _| help_menu(menu, &help))
            })
    }

    /// Show or hide the filters beside the results.
    pub(super) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        cx.notify();
    }

    pub(super) fn on_toggle_sidebar(
        &mut self,
        _: &ToggleSidebar,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_sidebar(cx);
    }

    pub(super) fn open_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        window.open_dialog(cx, |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            dialog.w(px(400.)).child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .pt_2()
                    .pb_4()
                    .child(brand_mark().size(px(56.)).text_color(cx.theme().foreground))
                    .child(div().pt_2().text_xl().font_semibold().child("dowse"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                    )
                    .child(
                        div()
                            .max_w(px(300.))
                            .text_center()
                            .text_sm()
                            .text_color(muted)
                            .child(env!("CARGO_PKG_DESCRIPTION")),
                    )
                    .child(
                        div().pt_2().child(
                            Button::new("about-github")
                                .ghost()
                                .small()
                                .icon(Lucide::Github)
                                .label("dowse on GitHub")
                                .on_click(|_, _, cx| cx.open_url(REPOSITORY)),
                        ),
                    ),
            )
        });
    }

    pub(super) fn on_about(&mut self, _: &About, window: &mut Window, cx: &mut Context<Self>) {
        self.open_about(window, cx);
    }
}
