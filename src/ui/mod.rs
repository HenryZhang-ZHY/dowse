//! The desktop UI: windows, each showing a workspace, with a search page
//! (scope bar, facet sidebar, results) and a repositories page for adding
//! and tagging the workspace's repositories.

mod app;
mod devtools;
mod highlight;
mod history;
mod hub;
mod main_menu;
mod manager;
mod palette;
mod preview;
mod remote;
mod render;
mod repos;
mod repos_page;
mod table;
mod tabs;
mod tasks;
mod windows;
mod workspace;

use std::path::PathBuf;

use gpui_kit::{App, KeyBinding, QuitMode, actions};

use dowse::ipc::Incoming;
use dowse::launch::Command;

/// Key context of the main view, which the bindings below are scoped to.
pub(crate) const CONTEXT: &str = "SearchApp";
/// Key context of the developer tools window.
pub(crate) const DEVTOOLS_CONTEXT: &str = "DevTools";

actions!(
    dowse,
    [
        AddRepository,
        ShowRepositories,
        FocusSearch,
        FocusPathFilter,
        ToggleCaseSensitive,
        ToggleWholeWord,
        ToggleRegex,
        UpdateIndex,
        RebuildIndex,
        ToggleTheme,
        NewWindow,
        NewWorkspace,
        OpenWorkspace,
        SaveWorkspaceAs,
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        ClosePreview,
        TogglePreview,
        ToggleSidebar,
        GoBack,
        GoForward,
        About,
        NextMatch,
        PreviousMatch,
        ToggleResultsView,
        ExportResults,
        CopyResultsAsTsv,
        CopyResultsAsMarkdown,
        CommandPalette,
        ToggleDevTools,
        Quit,
    ]
);

pub fn init(config_root: PathBuf, cx: &mut App) {
    // Windows decides when the app quits: with no windows open it may keep
    // running for the command line.
    cx.set_quit_mode(QuitMode::Explicit);
    windows::Windows::init(config_root, cx);
    tasks::TaskHub::init(cx);
    remote::Remote::init(cx);
    cx.on_window_closed(|cx, _| windows::Windows::closed(cx))
        .detach();
    cx.bind_keys([
        KeyBinding::new("secondary-o", AddRepository, Some(CONTEXT)),
        KeyBinding::new("secondary-,", ShowRepositories, Some(CONTEXT)),
        KeyBinding::new("secondary-f", FocusSearch, Some(CONTEXT)),
        // Menus show the last binding of an action: Ctrl+K, as the tooltips say.
        KeyBinding::new("secondary-shift-p", CommandPalette, Some(CONTEXT)),
        KeyBinding::new("secondary-k", CommandPalette, Some(CONTEXT)),
        KeyBinding::new("secondary-p", FocusPathFilter, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-r", UpdateIndex, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-n", NewWindow, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-o", OpenWorkspace, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-s", SaveWorkspaceAs, Some(CONTEXT)),
        KeyBinding::new("secondary-t", NewTab, Some(CONTEXT)),
        KeyBinding::new("secondary-w", CloseTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-tab", NextTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-pagedown", NextTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-pageup", PreviousTab, Some(CONTEXT)),
        KeyBinding::new("escape", ClosePreview, Some(CONTEXT)),
        // VS Code's secondary side bar.
        KeyBinding::new("secondary-alt-b", TogglePreview, Some(CONTEXT)),
        KeyBinding::new("secondary-b", ToggleSidebar, Some(CONTEXT)),
        KeyBinding::new("f4", NextMatch, Some(CONTEXT)),
        KeyBinding::new("shift-f4", PreviousMatch, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-e", ExportResults, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-i", ToggleDevTools, Some(CONTEXT)),
        KeyBinding::new("f12", ToggleDevTools, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-i", ToggleDevTools, Some(DEVTOOLS_CONTEXT)),
        KeyBinding::new("f12", ToggleDevTools, Some(DEVTOOLS_CONTEXT)),
    ]);
    // VS Code's search toggles. On macOS plain alt-letter types a character.
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-alt-c", ToggleCaseSensitive, Some(CONTEXT)),
        KeyBinding::new("cmd-alt-w", ToggleWholeWord, Some(CONTEXT)),
        KeyBinding::new("cmd-alt-r", ToggleRegex, Some(CONTEXT)),
        KeyBinding::new("cmd-alt-t", ToggleResultsView, Some(CONTEXT)),
        // VS Code's; Alt+Left moves by word in the search box here.
        KeyBinding::new("ctrl--", GoBack, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift--", GoForward, Some(CONTEXT)),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("alt-c", ToggleCaseSensitive, Some(CONTEXT)),
        KeyBinding::new("alt-w", ToggleWholeWord, Some(CONTEXT)),
        KeyBinding::new("alt-r", ToggleRegex, Some(CONTEXT)),
        KeyBinding::new("alt-t", ToggleResultsView, Some(CONTEXT)),
        KeyBinding::new("alt-left", GoBack, Some(CONTEXT)),
        KeyBinding::new("alt-right", GoForward, Some(CONTEXT)),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &NewWindow, cx| {
        windows::Windows::new_window(cx);
    });
    cx.on_action(|_: &ToggleDevTools, cx| devtools::open(cx));
}

/// Restore the last session's windows and carry out `command`, then carry
/// out the commands later launches forward and answer the command line.
pub fn start(command: Command, incoming: async_channel::Receiver<Incoming>, cx: &mut App) {
    windows::Windows::start(command, cx);
    cx.spawn(async move |cx| {
        while let Ok(message) = incoming.recv().await {
            cx.update(|cx| match message {
                Incoming::Launch(command) => windows::Windows::forwarded(command, cx),
                Incoming::Request(request, reply) => remote::handle(request, reply, cx),
            });
        }
    })
    .detach();
}
