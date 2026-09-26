//! The desktop UI: windows, each showing a workspace, with a search page
//! (scope bar, facet sidebar, results) and a repositories page for adding
//! and tagging the workspace's repositories.

mod app;
mod hub;
mod render;
mod repos;
mod repos_page;
mod windows;
mod workspace;

use gpui_kit::{App, KeyBinding, actions};

use crate::cli::Command;

/// Key context of the main view, which the bindings below are scoped to.
pub(crate) const CONTEXT: &str = "SearchApp";

actions!(
    tgrep,
    [
        AddRepository,
        ShowRepositories,
        FocusSearch,
        FocusPathFilter,
        ToggleCaseSensitive,
        ToggleWholeWord,
        ToggleRegex,
        RebuildIndex,
        ToggleTheme,
        NewWindow,
        NewWorkspace,
        OpenWorkspace,
        SaveWorkspaceAs,
        Quit,
    ]
);

pub fn init(cx: &mut App) {
    windows::Windows::init(cx);
    cx.on_window_closed(|cx, _| windows::Windows::closed(cx))
        .detach();
    cx.bind_keys([
        KeyBinding::new("secondary-o", AddRepository, Some(CONTEXT)),
        KeyBinding::new("secondary-,", ShowRepositories, Some(CONTEXT)),
        KeyBinding::new("secondary-f", FocusSearch, Some(CONTEXT)),
        KeyBinding::new("secondary-k", FocusSearch, Some(CONTEXT)),
        KeyBinding::new("secondary-p", FocusPathFilter, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-r", RebuildIndex, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-n", NewWindow, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-o", OpenWorkspace, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-s", SaveWorkspaceAs, Some(CONTEXT)),
    ]);
    // VS Code's search toggles. On macOS plain alt-letter types a character.
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-alt-c", ToggleCaseSensitive, Some(CONTEXT)),
        KeyBinding::new("cmd-alt-w", ToggleWholeWord, Some(CONTEXT)),
        KeyBinding::new("cmd-alt-r", ToggleRegex, Some(CONTEXT)),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("alt-c", ToggleCaseSensitive, Some(CONTEXT)),
        KeyBinding::new("alt-w", ToggleWholeWord, Some(CONTEXT)),
        KeyBinding::new("alt-r", ToggleRegex, Some(CONTEXT)),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &NewWindow, cx| {
        windows::Windows::new_window(cx);
    });
}

/// Restore the last session's windows, then carry out `command`.
pub fn start(command: Command, cx: &mut App) {
    windows::Windows::start(command, cx);
}
