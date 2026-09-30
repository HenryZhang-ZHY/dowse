//! Keeping dowse up to date, for every window: while the app runs it asks
//! GitHub for the latest release a day after the last answer (unless turned
//! off, and only in builds the release workflow made), offers a newer one in
//! the status bar, installs it over the running version, and restarts into
//! it. The work is in [`dowse::engine::update`].

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::progress::Progress as ProgressBar;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use semver::Version;

use super::app::SearchApp;
use super::tasks::TaskHub;
use super::windows::Windows;
use crate::format;
use dowse::engine::update::client::Client;
use dowse::engine::update::install::{self, Installation, Progress};
use dowse::engine::update::release::{Release, Source, current_version};
use dowse::engine::update::state::UpdateState;
use dowse::launch;

/// The first scheduled look after starting, once windows have opened.
const FIRST_CHECK: Duration = Duration::from_secs(15);
/// How often the schedule sees whether a look is due.
const SCHEDULE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Whether the release workflow built this program, telling it so with
/// `DOWSE_RELEASE`. Only those look for updates on their own: a build from
/// source is as new as its source.
const RELEASED_BUILD: bool = option_env!("DOWSE_RELEASE").is_some();

/// How installing the newer release goes.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Install {
    Idle,
    Running(Progress),
    Failed(String),
    /// Installed; a restart runs it.
    Done(Version),
}

pub(super) struct UpdateHub {
    file: PathBuf,
    /// Where downloads are unpacked.
    work_dir: PathBuf,
    state: UpdateState,
    /// Set when the update file could not be read, so it is never overwritten.
    locked: bool,
    current: Version,
    /// Where the running program is installed, or why it cannot be updated.
    installation: Result<Installation, String>,
    checking: bool,
    /// Why the last look failed.
    check_error: Option<String>,
    install: Install,
    cancel: Arc<AtomicBool>,
    /// The installed version is being started, and this one quits once it has.
    restarting: bool,
    _schedule: Task<()>,
}

struct GlobalUpdates(Entity<UpdateHub>);

impl Global for GlobalUpdates {}

impl UpdateHub {
    pub(super) fn init(cx: &mut App) {
        let config = Windows::config(cx);
        let (state, locked) = match UpdateState::load(&config.update_file()) {
            Ok(state) => (state, false),
            Err(error) => {
                log::warn!("not looking for updates: {error:#}");
                (UpdateState::default(), true)
            }
        };
        let installation = std::env::current_exe()
            .map_err(|error| format!("cannot tell where dowse is installed: {error}"))
            .and_then(|exe| Installation::detect(&exe).map_err(|error| format!("{error:#}")));
        match &installation {
            Ok(installation) => installation.remove_leftovers(),
            Err(error) => log::info!("updates cannot be installed: {error}"),
        }
        std::fs::remove_dir_all(config.updates_dir()).ok();
        let hub = cx.new(|cx| Self {
            file: config.update_file(),
            work_dir: config.updates_dir(),
            state,
            locked,
            current: current_version(),
            installation,
            checking: false,
            check_error: None,
            install: Install::Idle,
            cancel: Arc::default(),
            restarting: false,
            _schedule: Self::schedule(cx),
        });
        cx.set_global(GlobalUpdates(hub));
    }

    pub(super) fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalUpdates>().0.clone()
    }

    // ----- reading ---------------------------------------------------------------

    /// The release to suggest: newer than this one, and not skipped.
    pub(super) fn offer(&self) -> Option<&Release> {
        self.state.offer(&self.current)
    }

    /// The latest release when it is newer than this one, skipped or not.
    pub(super) fn newer(&self) -> Option<&Release> {
        self.state.newer_than(&self.current)
    }

    pub(super) fn current(&self) -> &Version {
        &self.current
    }

    pub(super) fn automatic(&self) -> bool {
        self.state.automatic
    }

    pub(super) fn checking(&self) -> bool {
        self.checking
    }

    pub(super) fn check_error(&self) -> Option<&str> {
        self.check_error.as_deref()
    }

    pub(super) fn checked_at(&self) -> Option<SystemTime> {
        self.state.checked_at()
    }

    pub(super) fn install_state(&self) -> &Install {
        &self.install
    }

    /// Why an update cannot be installed here, if it cannot.
    pub(super) fn cannot_install(&self) -> Option<&str> {
        self.installation.as_ref().err().map(String::as_str)
    }

    // ----- changing ----------------------------------------------------------------

    pub(super) fn set_automatic(&mut self, automatic: bool, cx: &mut Context<Self>) {
        self.state.automatic = automatic;
        self.save();
        if self.is_due() {
            self.check(cx);
        }
        cx.notify();
    }

    /// Stop offering the newer release.
    pub(super) fn skip(&mut self, cx: &mut Context<Self>) {
        if let Some(release) = self.newer() {
            log::info!("skipping dowse {}", release.version);
            self.state.skipped = Some(release.version.clone());
            self.save();
            cx.notify();
        }
    }

    fn save(&self) {
        if self.locked {
            return;
        }
        if let Err(error) = self.state.save(&self.file) {
            log::warn!("could not save the update file: {error:#}");
        }
    }

    // ----- looking -------------------------------------------------------------------

    fn schedule(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FIRST_CHECK).await;
            loop {
                let alive = this.update(cx, |this, cx| {
                    if this.is_due() {
                        this.check(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(SCHEDULE_INTERVAL).await;
            }
        })
    }

    /// Whether a scheduled look is due. A failed one is tried again on the
    /// next round of the schedule.
    fn is_due(&self) -> bool {
        RELEASED_BUILD && !self.locked && !self.checking && self.state.is_due(SystemTime::now())
    }

    /// Ask GitHub for the latest release, on a thread of its own.
    pub(super) fn check(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        self.check_error = None;
        cx.notify();
        let mut state = self.state.clone();
        let (sender, receiver) = async_channel::bounded(1);
        std::thread::Builder::new()
            .name("update check".into())
            .spawn(move || {
                let client = Client::new(Source::github());
                let result = dowse::engine::update::check(&client, &mut state, SystemTime::now())
                    .map(|()| state)
                    .map_err(|error| format!("{error:#}"));
                sender.send_blocking(result).ok();
            })
            .expect("a thread for the update check");
        cx.spawn(async move |this, cx| {
            let result = receiver
                .recv()
                .await
                .unwrap_or_else(|_| Err("the check stopped unexpectedly".into()));
            this.update(cx, |this, cx| this.checked(result, cx)).ok();
        })
        .detach();
    }

    fn checked(&mut self, result: Result<UpdateState, String>, cx: &mut Context<Self>) {
        self.checking = false;
        match result {
            Ok(answer) => {
                // What the user chose while GitHub was being asked stands.
                self.state = UpdateState {
                    automatic: self.state.automatic,
                    skipped: self.state.skipped.clone(),
                    ..answer
                };
                self.save();
                match self.newer() {
                    Some(release) => log::info!("dowse {} is available", release.version),
                    None => log::info!("dowse {} is up to date", self.current),
                }
            }
            Err(error) => {
                log::warn!("could not look for updates: {error}");
                self.check_error = Some(error);
            }
        }
        cx.notify();
    }

    // ----- installing ------------------------------------------------------------------

    /// Download the newer release and install it over this one, on a thread
    /// of its own.
    pub(super) fn install(&mut self, cx: &mut Context<Self>) {
        if matches!(self.install, Install::Running(_) | Install::Done(_)) {
            return;
        }
        let Some(release) = self.newer().cloned() else {
            return;
        };
        let installation = match &self.installation {
            Ok(installation) => installation.clone(),
            Err(error) => {
                self.install = Install::Failed(error.clone());
                cx.notify();
                return;
            }
        };
        log::info!("installing dowse {} over {}", release.version, self.current);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        self.install = Install::Running(Progress::Downloading {
            done: 0,
            total: None,
        });
        cx.notify();

        enum Message {
            Progress(Progress),
            Finished(Result<(), String>),
        }
        let (sender, receiver) = async_channel::unbounded();
        let work_dir = self.work_dir.join(&release.tag);
        let version = release.version.clone();
        std::thread::Builder::new()
            .name("update install".into())
            .spawn(move || {
                let client = Client::new(Source::github());
                let result = install::install(
                    &client,
                    &release,
                    &installation,
                    &work_dir,
                    &cancel,
                    |progress| {
                        sender.send_blocking(Message::Progress(progress)).ok();
                    },
                )
                .map_err(|error| format!("{error:#}"));
                sender.send_blocking(Message::Finished(result)).ok();
            })
            .expect("a thread for the update");
        cx.spawn(async move |this, cx| {
            let result = loop {
                match receiver.recv().await {
                    Ok(Message::Progress(progress)) => {
                        let alive = this.update(cx, |this, cx| {
                            this.install = Install::Running(progress);
                            cx.notify();
                        });
                        if alive.is_err() {
                            return;
                        }
                    }
                    Ok(Message::Finished(result)) => break result,
                    Err(_) => break Err("the update stopped unexpectedly".into()),
                }
            };
            this.update(cx, |this, cx| {
                this.install = match result {
                    Ok(()) => {
                        log::info!("installed dowse {version}; it runs after a restart");
                        Install::Done(version)
                    }
                    Err(error) if this.cancel.load(Ordering::Relaxed) => {
                        log::info!("the update was cancelled ({error})");
                        Install::Idle
                    }
                    Err(error) => {
                        log::warn!("could not install dowse {version}: {error}");
                        Install::Failed(error)
                    }
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn cancel_install(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub(super) fn restarting(&self) -> bool {
        self.restarting
    }

    /// Quit, starting the installed version to take over with the same
    /// windows. Refused while clones or pulls run, which quitting would cut off.
    pub(super) fn restart(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if self.restarting {
            return Ok(());
        }
        let program = match &self.installation {
            Ok(installation) => installation.program(),
            Err(error) => return Err(error.clone()),
        };
        if TaskHub::keeps_app_running(cx) {
            return Err(
                "Clones or pulls are running. Restart once they finish, or cancel them first."
                    .into(),
            );
        }
        Windows::save_now(cx);
        let background = Windows::count(cx) == 0;
        log::info!("restarting into {}", program.display());
        self.restarting = true;
        cx.notify();
        // Starting a program just written can take seconds on Windows while
        // it is scanned for viruses; the windows stay responsive meanwhile.
        let start =
            cx.background_spawn(async move { launch::start_taking_over(&program, background) });
        cx.spawn(async move |this, cx| match start.await {
            Ok(()) => cx.update(|cx| cx.quit()),
            Err(error) => {
                log::warn!("could not start the new version: {error}");
                this.update(cx, |this, cx| {
                    this.restarting = false;
                    this.install =
                        Install::Failed(format!("Could not start the new version: {error}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        Ok(())
    }
}

impl SearchApp {
    /// Look for an update now, showing what is found.
    pub(super) fn check_for_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        UpdateHub::global(cx).update(cx, |hub, cx| hub.check(cx));
        self.open_updates(window, cx);
    }

    /// The dialog about updates: the newer release and its notes, installing
    /// it and restarting, or that dowse is up to date.
    pub(super) fn open_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        window.open_dialog(cx, |dialog, _, cx| {
            let hub = UpdateHub::global(cx);
            let hub = hub.read(cx);
            let (body, footer) = update_dialog(hub, cx);
            dialog.w(px(560.)).child(body).footer(footer)
        });
    }

    /// The status bar's note of an update: available, installing, installed.
    pub(super) fn render_update_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let hub = UpdateHub::global(cx);
        let hub = hub.read(cx);
        let theme = cx.theme();
        let (icon, text, color, tooltip): (Option<Lucide>, String, Hsla, &'static str) =
            match hub.install_state() {
                Install::Running(progress) => (
                    None,
                    format!("Updating dowse… {}", progress_text(progress)),
                    theme.muted_foreground,
                    "Show the update",
                ),
                Install::Done(version) => (
                    Some(Lucide::RotateCw),
                    format!("Restart to run dowse {version}"),
                    theme.primary,
                    "Restart dowse into the new version",
                ),
                Install::Failed(_) => (
                    Some(Lucide::Download),
                    "The update failed".into(),
                    theme.danger,
                    "Show what went wrong",
                ),
                Install::Idle => {
                    let release = hub.offer()?;
                    (
                        Some(Lucide::Download),
                        format!("dowse {} is available", release.version),
                        theme.primary,
                        "Show the new version",
                    )
                }
            };
        let running = matches!(hub.install_state(), Install::Running(_));
        Some(
            h_flex()
                .id("update-status")
                .gap_1()
                .cursor_pointer()
                .text_color(color)
                .hover(|style| style.underline())
                .when(running, |row| row.child(Spinner::new().xsmall()))
                .when_some(icon, |row, icon| row.child(Icon::new(icon).xsmall()))
                .child(text)
                .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
                .on_click(cx.listener(|this, _, window, cx| this.open_updates(window, cx)))
                .into_any_element(),
        )
    }
}

fn progress_text(progress: &Progress) -> String {
    match progress {
        Progress::Downloading {
            done,
            total: Some(total),
        } if *total > 0 => format!("{}%", done * 100 / total),
        Progress::Downloading { done, .. } => format::kilobytes(done / 1024),
        Progress::Unpacking => "unpacking".into(),
        Progress::Installing => "installing".into(),
    }
}

/// The update dialog's body and buttons, from how things stand.
fn update_dialog(hub: &UpdateHub, cx: &App) -> (AnyElement, AnyElement) {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let current = hub.current().clone();
    let buttons = h_flex().gap_2().justify_end();
    let close = Button::new("update-close")
        .ghost()
        .label("Close")
        .on_click(|_, window, cx| window.close_dialog(cx));
    let heading = |text: String| div().text_lg().font_semibold().child(text);
    let note = |text: String| div().text_sm().text_color(muted).child(text);

    let Some(release) = hub.newer().cloned() else {
        let (title, detail) = if hub.checking() {
            ("Looking for updates…".to_string(), None)
        } else if let Some(error) = hub.check_error() {
            (
                "Could not look for updates".to_string(),
                Some(error.to_string()),
            )
        } else {
            let checked = hub
                .checked_at()
                .map(|at| format!("Checked {}.", format::ago(at, SystemTime::now())));
            (format!("dowse {current} is up to date"), checked)
        };
        let body = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .when(hub.checking(), |row| row.child(Spinner::new().small()))
                    .child(heading(title)),
            )
            .when_some(detail, |body, detail| body.child(note(detail)))
            .child(automatic_note(hub));
        let footer = buttons
            .when(!hub.checking(), |row| {
                row.child(
                    Button::new("update-check")
                        .ghost()
                        .icon(Lucide::RefreshCw)
                        .label("Check Again")
                        .on_click(|_, _, cx| {
                            UpdateHub::global(cx).update(cx, |hub, cx| hub.check(cx));
                        }),
                )
            })
            .child(close);
        return (body.into_any_element(), footer.into_any_element());
    };

    let page = release.page.clone();
    let open_page = Button::new("update-page")
        .ghost()
        .icon(Lucide::Github)
        .label("Release Page")
        .on_click(move |_, _, cx| cx.open_url(&page));
    let notes = if release.notes.trim().is_empty() {
        None
    } else {
        Some(
            div()
                .id("release-notes")
                .max_h(px(280.))
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .text_sm()
                .child(TextView::markdown(
                    "release-notes-text",
                    release.notes.clone(),
                ))
                .overflow_y_scrollbar(),
        )
    };

    let mut body = v_flex().gap_3();
    let mut footer = buttons;
    match hub.install_state() {
        Install::Done(version) => {
            body = body
                .child(heading(format!("dowse {version} is installed")))
                .child(note(format!(
                    "Restart dowse to run it. Your windows and searches come back. Until then \
                     dowse {current} keeps running."
                )));
            footer = footer
                .child(
                    Button::new("update-later")
                        .ghost()
                        .label("Later")
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                )
                .child(
                    Button::new("update-restart")
                        .primary()
                        .icon(Lucide::RotateCw)
                        .label(if hub.restarting() {
                            "Restarting…"
                        } else {
                            "Restart Now"
                        })
                        .loading(hub.restarting())
                        .on_click(|_, window, cx| {
                            let restarted =
                                UpdateHub::global(cx).update(cx, |hub, cx| hub.restart(cx));
                            if let Err(error) = restarted {
                                window.push_notification(Notification::error(error), cx);
                            }
                        }),
                );
        }
        Install::Running(progress) => {
            let (value, label) = match progress {
                Progress::Downloading {
                    done,
                    total: Some(total),
                } if *total > 0 => (
                    (*done as f32 / *total as f32) * 100.0,
                    format!(
                        "Downloading… {} of {}",
                        format::kilobytes(done / 1024),
                        format::kilobytes(total / 1024)
                    ),
                ),
                Progress::Downloading { done, .. } => (
                    0.0,
                    format!("Downloading… {}", format::kilobytes(done / 1024)),
                ),
                Progress::Unpacking => (100.0, "Unpacking…".into()),
                Progress::Installing => (100.0, "Installing…".into()),
            };
            body = body
                .child(heading(format!("Updating to dowse {}", release.version)))
                .child(ProgressBar::new("update-progress").value(value))
                .child(note(label));
            footer = footer.child(
                Button::new("update-cancel")
                    .ghost()
                    .label("Cancel")
                    .on_click(|_, _, cx| {
                        UpdateHub::global(cx).update(cx, |hub, _| hub.cancel_install());
                    }),
            );
        }
        state => {
            body = body
                .child(heading(format!("dowse {} is available", release.version)))
                .child(note(format!("You have {current}.")))
                .children(notes);
            if let Install::Failed(error) = state {
                body = body.child(
                    div()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(format!("The update failed: {error}")),
                );
            }
            let skipped = hub.offer().is_none();
            footer = footer
                .child(open_page)
                .when(!skipped, |row| {
                    row.child(
                        Button::new("update-skip")
                            .ghost()
                            .label("Skip This Version")
                            .on_click(|_, window, cx| {
                                UpdateHub::global(cx).update(cx, |hub, cx| hub.skip(cx));
                                window.close_dialog(cx);
                            }),
                    )
                })
                .child(close);
            footer = match hub.cannot_install() {
                Some(reason) => {
                    body = body.child(note(format!(
                        "dowse cannot install it here: {reason}. Download it from the release \
                         page."
                    )));
                    footer
                }
                None => footer.child(
                    Button::new("update-install")
                        .primary()
                        .icon(Lucide::Download)
                        .label(if matches!(state, Install::Failed(_)) {
                            "Try Again"
                        } else {
                            "Install"
                        })
                        .on_click(|_, _, cx| {
                            UpdateHub::global(cx).update(cx, |hub, cx| hub.install(cx));
                        }),
                ),
            };
        }
    }
    (body.into_any_element(), footer.into_any_element())
}

/// Whether dowse looks on its own, and where to change it.
fn automatic_note(hub: &UpdateHub) -> impl IntoElement {
    let text = if !RELEASED_BUILD {
        "This build is from source, so dowse does not look for updates on its own."
    } else if hub.automatic() {
        "dowse looks for updates once a day. Turn it off in Help > Check for Updates Automatically."
    } else {
        "dowse does not look for updates on its own. Turn it on in Help > Check for Updates Automatically."
    };
    div().text_xs().child(text)
}
