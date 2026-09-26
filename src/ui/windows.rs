//! The app's windows: opening them on a workspace, restoring them from the
//! last session, carrying out command-line requests, and remembering them
//! for next time.

use std::path::{Path, PathBuf};

use gpui_kit::component::Root;
use gpui_kit::*;

use super::app::{SearchApp, TabsOpening};
use super::devtools;
use super::hub::RepoHub;
use super::remote::Remote;
use dowse::engine::config::ConfigDir;
use dowse::engine::repo;
use dowse::engine::session::{Session, WindowSession};
use dowse::launch::Command;

/// How far each new window is offset from the previous one.
const CASCADE: f32 = 28.;

/// What a new window, or a window switching workspaces, opens.
#[derive(Clone, Debug)]
pub(super) enum Opening {
    /// An untitled workspace over these repository ids.
    Untitled {
        repos: Vec<String>,
        scope: Vec<String>,
    },
    /// A saved workspace file.
    Saved(PathBuf),
}

impl Opening {
    pub(super) fn empty() -> Self {
        Self::Untitled {
            repos: Vec::new(),
            scope: Vec::new(),
        }
    }
}

/// An open window.
#[derive(Clone)]
struct OpenWindow {
    handle: AnyWindowHandle,
    app: WeakEntity<SearchApp>,
    /// The saved workspace it shows. Kept here because a window asking which
    /// window shows a file may be the one being updated, which GPUI does not
    /// let anyone read.
    workspace: Option<PathBuf>,
}

pub(super) struct Windows {
    config: ConfigDir,
    /// Recent workspaces and their scopes. Its windows are rebuilt from the
    /// open windows on every save.
    session: Session,
    /// Set when the session could not be read, so it is never overwritten.
    session_locked: bool,
    /// A save is scheduled.
    save_pending: bool,
    /// Problems found while starting, for the first window to show.
    startup_errors: Vec<String>,
    /// Open windows, the most recently focused last.
    open: Vec<OpenWindow>,
}

impl Global for Windows {}

impl Windows {
    /// Read the settings (migrating an earlier version's) and create the
    /// shared repository hub.
    pub(super) fn init(root: PathBuf, cx: &mut App) {
        let config = ConfigDir::new(root);
        let mut startup_errors = Vec::new();
        if let Some(legacy) = dowse::engine::config::legacy_root()
            && let Err(error) = config.adopt_legacy(&legacy)
        {
            startup_errors.push(format!(
                "Could not carry over tgrep-gpui's settings: {error:#}"
            ));
        }
        if let Err(error) = config.migrate() {
            startup_errors.push(format!("Could not carry over your repositories: {error:#}"));
        }
        let (session, session_locked) = match Session::load(&config.session_file()) {
            Ok(session) => (session, false),
            Err(error) => {
                startup_errors.push(format!(
                    "{error:#}. Open windows will not be remembered until it is fixed."
                ));
                (Session::default(), true)
            }
        };
        startup_errors.extend(RepoHub::init(config.library_file(), cx));
        cx.set_global(Self {
            config,
            session,
            session_locked,
            save_pending: false,
            startup_errors,
            open: Vec::new(),
        });
    }

    // ----- starting ------------------------------------------------------------

    /// Bring back the last session's windows, then carry out `command`. In
    /// the background, for the command line, open none.
    pub(super) fn start(command: Command, cx: &mut App) {
        if command == Command::Background {
            log::info!("running in the background for the command line");
            Remote::started_in_background(cx);
            return;
        }
        Self::restore(cx);
        Self::run(command, cx);
        if cx.global::<Self>().open.is_empty() {
            Self::open_window(Opening::empty(), cx);
        }
    }

    /// Open the windows the session remembers.
    fn restore(cx: &mut App) {
        let restored: Vec<WindowSession> = cx.global::<Self>().session.windows.clone();
        for window in restored {
            let tabs = TabsOpening {
                queries: window.tabs,
                active: window.active_tab,
            };
            let opening = match window.workspace {
                Some(file) => Opening::Saved(file),
                None => Opening::Untitled {
                    repos: window
                        .repos
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect(),
                    scope: window.scope,
                },
            };
            Self::open_window_with_tabs(opening, tabs, cx);
        }
    }

    /// Carry out a command a later launch forwarded. Launching without
    /// arguments opens a new window, as `code` does, or brings back the last
    /// session when the app was running without windows.
    pub(super) fn forwarded(command: Command, cx: &mut App) {
        log::info!("a later launch asked for {command:?}");
        match command {
            Command::Start if Self::count(cx) == 0 => {
                Self::restore(cx);
                if Self::count(cx) == 0 {
                    Self::new_window(cx);
                }
            }
            Command::Start => Self::new_window(cx),
            command => Self::run(command, cx),
        }
    }

    /// Carry out a command-line request in the running app.
    pub(super) fn run(command: Command, cx: &mut App) {
        match command {
            Command::Background => {}
            Command::Start | Command::Help => {
                if let Some(target) = Self::last(cx) {
                    target
                        .handle
                        .update(cx, |_, window, _| window.activate_window())
                        .ok();
                }
            }
            Command::Open {
                folders,
                workspaces,
            } => {
                if !folders.is_empty() {
                    let hub = RepoHub::global(cx);
                    let repos = hub
                        .update(cx, |hub, _| hub.register(&folders, true))
                        .unwrap_or_default();
                    Self::open_window(
                        Opening::Untitled {
                            repos,
                            scope: Vec::new(),
                        },
                        cx,
                    );
                }
                for file in workspaces {
                    Self::open_workspace(&file, cx);
                }
            }
            Command::Add(folders) => {
                let target = match Self::last(cx) {
                    Some(target) => target,
                    None => match Self::open_window(Opening::empty(), cx) {
                        Some(target) => target,
                        None => return,
                    },
                };
                let app = target.app.clone();
                target
                    .handle
                    .update(cx, |_, window, cx| {
                        app.update(cx, |app, cx| app.add_repositories(folders, window, cx))
                            .ok();
                        window.activate_window();
                    })
                    .ok();
            }
            Command::Remove(folders) => {
                if let Some(target) = Self::last(cx) {
                    let app = target.app.clone();
                    target
                        .handle
                        .update(cx, |_, window, cx| {
                            app.update(cx, |app, cx| app.remove_folders(&folders, window, cx))
                                .ok();
                            window.activate_window();
                        })
                        .ok();
                }
            }
        }
    }

    /// Focus the window showing `file`, or open a new one for it.
    pub(super) fn open_workspace(file: &Path, cx: &mut App) {
        let file = repo::identity(file);
        match Self::window_for(&file, cx) {
            Some(handle) => {
                handle
                    .update(cx, |_, window, _| window.activate_window())
                    .ok();
            }
            None => {
                Self::open_window(Opening::Saved(file), cx);
            }
        }
    }

    // ----- windows -------------------------------------------------------------

    /// Open a window on `opening`.
    fn open_window(opening: Opening, cx: &mut App) -> Option<OpenWindow> {
        Self::open_window_with_tabs(opening, TabsOpening::default(), cx)
    }

    /// Open a window on `opening` with the search tabs of `tabs`.
    fn open_window_with_tabs(
        opening: Opening,
        tabs: TabsOpening,
        cx: &mut App,
    ) -> Option<OpenWindow> {
        let offset = px(CASCADE * (cx.global::<Self>().open.len() % 8) as f32);
        let mut bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        bounds.origin.x += offset;
        bounds.origin.y += offset;
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("dowse".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(720.), px(480.))),
            ..Default::default()
        };
        let mut app = None;
        let handle = cx
            .open_window(options, |window, cx| {
                let view = cx.new(|cx| SearchApp::new(opening, tabs, window, cx));
                app = Some(view.downgrade());
                // `Root` hosts notifications, dialogs and tooltips.
                cx.new(|cx| Root::new(view, window, cx))
            })
            .ok()?;
        let app = app?;
        log::info!("opened a window");
        let workspace = app
            .upgrade()
            .and_then(|app| app.read(cx).workspace_file().map(Path::to_path_buf));
        let window = OpenWindow {
            handle: handle.into(),
            app,
            workspace,
        };
        cx.global_mut::<Self>().open.push(window.clone());
        cx.activate(true);
        Self::save(cx);
        Some(window)
    }

    /// Open a new window with an empty untitled workspace.
    pub(super) fn new_window(cx: &mut App) {
        Self::open_window(Opening::empty(), cx);
    }

    /// The most recently focused window.
    fn last(cx: &App) -> Option<OpenWindow> {
        cx.global::<Self>()
            .open
            .iter()
            .rev()
            .find(|window| window.app.upgrade().is_some())
            .cloned()
    }

    /// The window showing the saved workspace `file`.
    pub(super) fn window_for(file: &Path, cx: &App) -> Option<AnyWindowHandle> {
        cx.global::<Self>()
            .open
            .iter()
            .find(|window| window.workspace.as_deref() == Some(file))
            .map(|window| window.handle)
    }

    /// How many windows are open.
    pub(super) fn count(cx: &App) -> usize {
        cx.global::<Self>()
            .open
            .iter()
            .filter(|window| window.app.upgrade().is_some())
            .count()
    }

    /// A window now shows `workspace`.
    pub(super) fn set_workspace(handle: AnyWindowHandle, workspace: Option<PathBuf>, cx: &mut App) {
        if let Some(window) = cx
            .global_mut::<Self>()
            .open
            .iter_mut()
            .find(|window| window.handle == handle)
        {
            window.workspace = workspace;
        }
        Self::save(cx);
    }

    /// Note that `handle` was focused, so it is the target of `--add`.
    pub(super) fn activated(handle: AnyWindowHandle, cx: &mut App) {
        let windows = cx.global_mut::<Self>();
        let Some(index) = windows.open.iter().position(|w| w.handle == handle) else {
            return;
        };
        if index + 1 == windows.open.len() {
            return;
        }
        let window = windows.open.remove(index);
        windows.open.push(window);
        Self::save(cx);
    }

    /// A window closed. When it was the last search window, the developer
    /// tools close with it and the app quits, unless the command line is
    /// using it; the session keeps the window, to open it again next time.
    pub(super) fn closed(cx: &mut App) {
        let open = cx.windows();
        devtools::forget_if_closed(&open, cx);
        let windows = cx.global_mut::<Self>();
        let before = windows.open.len();
        windows.open.retain(|window| open.contains(&window.handle));
        let left = windows.open.len();
        log::info!("closed a window; {left} search windows left");
        if left > 0 {
            Self::save(cx);
            return;
        }
        if before > 0 {
            devtools::close(cx);
        }
        if Remote::keeps_app_running(cx) {
            log::info!("staying in the background for the command line");
        } else if !devtools::is_open(cx) {
            cx.quit();
        }
    }

    /// Problems found while starting, once.
    pub(super) fn take_startup_errors(cx: &mut App) -> Vec<String> {
        std::mem::take(&mut cx.global_mut::<Self>().startup_errors)
    }

    // ----- the session ---------------------------------------------------------

    pub(super) fn config(cx: &App) -> ConfigDir {
        cx.global::<Self>().config.clone()
    }

    pub(super) fn workspaces_dir(cx: &App) -> PathBuf {
        cx.global::<Self>().config.workspaces_dir()
    }

    pub(super) fn recent(cx: &App) -> Vec<PathBuf> {
        cx.global::<Self>().session.recent.clone()
    }

    /// The scope last used with the saved workspace `file`.
    pub(super) fn saved_scope(file: &Path, cx: &App) -> Vec<String> {
        cx.global::<Self>()
            .session
            .scopes
            .get(file)
            .cloned()
            .unwrap_or_default()
    }

    /// Put `file` at the top of the recent list.
    pub(super) fn remember(file: &Path, cx: &mut App) {
        cx.global_mut::<Self>().session.remember(file);
        Self::save(cx);
    }

    /// Drop a workspace file that can no longer be opened.
    pub(super) fn forget(file: &Path, cx: &mut App) {
        cx.global_mut::<Self>().session.forget(file);
        Self::save(cx);
    }

    /// Write down the open windows, once the current update is over: windows
    /// call this while they change, when they cannot be read.
    pub(super) fn save(cx: &mut App) {
        let windows = cx.global_mut::<Self>();
        if windows.save_pending {
            return;
        }
        windows.save_pending = true;
        cx.defer(Self::save_now);
    }

    fn save_now(cx: &mut App) {
        let states: Vec<WindowSession> = cx
            .global::<Self>()
            .open
            .iter()
            .filter_map(|window| window.app.upgrade())
            .map(|app| app.read(cx).session_state(cx))
            .collect();
        let quitting = states.is_empty();
        let windows = cx.global_mut::<Self>();
        windows.save_pending = false;
        if quitting {
            // The last window closed: keep the session that still lists it.
            return;
        }
        for state in &states {
            if let Some(file) = &state.workspace {
                windows
                    .session
                    .scopes
                    .insert(file.clone(), state.scope.clone());
            }
        }
        windows.session.windows = states;
        if windows.session_locked {
            return;
        }
        // Failing to remember windows is not worth interrupting anyone for.
        if let Err(error) = windows.session.save(&windows.config.session_file()) {
            log::warn!("could not save the session: {error:#}");
        }
    }
}
