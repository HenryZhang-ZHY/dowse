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

use std::path::{Path, PathBuf};

use gpui_kit::{App, KeyBinding, actions};

use crate::cli::Command;
use tgrep_gpui::engine::repo;

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

pub fn init(config_root: PathBuf, cx: &mut App) {
    windows::Windows::init(config_root, cx);
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

/// Overrides where settings are kept, e.g. for a portable install or tests.
const CONFIG_DIR_ENV: &str = "TGREP_GPUI_CONFIG_DIR";

/// Where settings are kept.
pub fn config_root() -> PathBuf {
    match std::env::var_os(CONFIG_DIR_ENV) {
        Some(dir) => repo::identity(Path::new(&dir)),
        None => dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("tgrep-gpui"),
    }
}

/// Restore the last session's windows and carry out `command`, then carry
/// out the commands later launches forward.
pub fn start(command: Command, forwarded: async_channel::Receiver<Command>, cx: &mut App) {
    windows::Windows::start(command, cx);
    cx.spawn(async move |cx| {
        while let Ok(command) = forwarded.recv().await {
            cx.update(|cx| windows::Windows::forwarded(command, cx));
        }
    })
    .detach();
}
