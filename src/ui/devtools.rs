//! The developer tools window (`Ctrl+Shift+I` or `F12`, or
//! `dowse dev open`): the app's log as it is written, its key metrics, the
//! latest searches with what each cost, and the repositories the app has
//! open. One window serves the whole app, whichever window opened it.

use std::ops::Range;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Root, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::hub::{RepoHub, RepoView};
use super::windows::Windows;
use dowse::cli::output;
use dowse::diagnostics::log::{self as app_log, BUFFER_CAPACITY, LogEntry, LogLevel, format_time};
use dowse::diagnostics::metrics::{MetricsSnapshot, TimingSummary, metrics};

/// How often the window picks up new log records and metrics.
const REFRESH_INTERVAL: Duration = Duration::from_millis(500);
/// Log records copied with the diagnostics.
const COPIED_RECORDS: usize = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Logs,
    Metrics,
    Searches,
}

impl Page {
    const ALL: [Self; 3] = [Self::Logs, Self::Metrics, Self::Searches];

    fn label(self) -> &'static str {
        match self {
            Self::Logs => "Logs",
            Self::Metrics => "Metrics",
            Self::Searches => "Searches",
        }
    }
}

pub(super) struct DevTools {
    page: Page,
    /// The least severe level shown.
    level: LogLevel,
    filter: Entity<InputState>,
    /// Every record kept, oldest first, and those shown, by index.
    entries: Vec<LogEntry>,
    shown: Vec<usize>,
    /// Keep the newest record in view.
    follow: bool,
    log_scroll: UniformListScrollHandle,
    snapshot: MetricsSnapshot,
    repos: Vec<RepoView>,
    _filter_changed: Subscription,
    _refresh: Task<()>,
}

/// The open developer tools window, if any.
struct DevToolsWindow(Option<AnyWindowHandle>);

impl Global for DevToolsWindow {}

/// Focus the developer tools window, opening it first when need be.
pub(super) fn open(cx: &mut App) {
    if let Some(handle) = window(cx)
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Developer Tools — dowse".into()),
            ..Default::default()
        }),
        window_min_size: Some(size(px(640.), px(400.))),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| DevTools::new(window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    });
    match opened {
        Ok(handle) => {
            log::info!("opened the developer tools");
            cx.set_global(DevToolsWindow(Some(handle.into())));
            cx.activate(true);
        }
        Err(error) => log::error!("could not open the developer tools: {error:#}"),
    }
}

/// Close the developer tools window, if open, once the current update is
/// over: this runs while another window is being closed.
pub(super) fn close(cx: &mut App) {
    if let Some(handle) = window(cx) {
        cx.set_global(DevToolsWindow(None));
        cx.defer(move |cx| {
            handle
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        });
    }
}

pub(super) fn is_open(cx: &App) -> bool {
    window(cx).is_some()
}

/// Forget the window once it is gone from `open`, the windows still open.
pub(super) fn forget_if_closed(open: &[AnyWindowHandle], cx: &mut App) {
    if window(cx).is_some_and(|handle| !open.contains(&handle)) {
        cx.set_global(DevToolsWindow(None));
    }
}

fn window(cx: &App) -> Option<AnyWindowHandle> {
    cx.try_global::<DevToolsWindow>()
        .and_then(|global| global.0)
}

impl DevTools {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter by text or module"));
        let filter_changed = cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.refilter(cx);
            }
        });
        let refresh = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH_INTERVAL).await;
            }
        });
        Self {
            page: Page::Logs,
            level: LogLevel::Info,
            filter,
            entries: Vec::new(),
            shown: Vec::new(),
            follow: true,
            log_scroll: UniformListScrollHandle::new(),
            snapshot: MetricsSnapshot::default(),
            repos: Vec::new(),
            _filter_changed: filter_changed,
            _refresh: refresh,
        }
    }

    /// Pick up records logged since the last look, and the latest metrics.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let after = self.entries.last().map_or(0, |entry| entry.seq);
        let new = app_log::buffer().since(after, LogLevel::Trace, BUFFER_CAPACITY);
        let logs_changed = !new.is_empty();
        if logs_changed {
            self.entries.extend(new);
            let excess = self.entries.len().saturating_sub(BUFFER_CAPACITY);
            self.entries.drain(..excess);
            self.refilter(cx);
        }
        self.snapshot = metrics().snapshot();
        let hub = RepoHub::global(cx);
        self.repos = hub.read(cx).open_views();
        if logs_changed || self.page != Page::Logs {
            cx.notify();
        }
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let needle = self.filter.read(cx).value().to_lowercase();
        self.shown = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.level <= self.level)
            .filter(|(_, entry)| {
                needle.is_empty()
                    || entry.message.to_lowercase().contains(&needle)
                    || entry.target.to_lowercase().contains(&needle)
            })
            .map(|(index, _)| index)
            .collect();
        if self.follow && !self.shown.is_empty() {
            self.log_scroll
                .scroll_to_item(self.shown.len() - 1, ScrollStrategy::Bottom);
        }
        cx.notify();
    }

    fn set_level(&mut self, level: LogLevel, cx: &mut Context<Self>) {
        self.level = level;
        self.refilter(cx);
    }

    /// Status, metrics and the latest log records as text, for a bug report
    /// or an agent to read.
    fn diagnostics(&self, cx: &App) -> String {
        let config = Windows::config(cx);
        let mut text = format!(
            "dowse {} on {} ({})\nsettings: {}\nlog: {}\nwindows: {}\n\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            config.root().display(),
            app_log::log_file(&config.logs_dir()).display(),
            Windows::count(cx),
        );
        text.push_str(&output::metrics_text(&self.snapshot));
        text.push_str("\nrepositories:\n");
        for repo in &self.repos {
            text.push_str(&format!("  {}: {}\n", repo.info.name, repo.index_summary()));
        }
        text.push_str("\nlatest log records:\n");
        let skip = self.entries.len().saturating_sub(COPIED_RECORDS);
        for entry in &self.entries[skip..] {
            text.push_str(&output::log_line(entry, output::Style::default()));
        }
        text
    }

    fn copy_diagnostics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.diagnostics(cx)));
        window.push_notification(Notification::success("Copied the diagnostics"), cx);
    }

    fn open_log_folder(&mut self, cx: &mut Context<Self>) {
        let config = Windows::config(cx);
        cx.reveal_path(&app_log::log_file(&config.logs_dir()));
    }

    // ----- rendering ------------------------------------------------------------

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let selected = Page::ALL
            .iter()
            .position(|page| *page == self.page)
            .unwrap_or(0);
        h_flex()
            .flex_none()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                TabBar::new("devtools-pages")
                    .segmented()
                    .small()
                    .selected_index(selected)
                    .on_click(move |index: &usize, _, cx| {
                        this.update(cx, |this, cx| {
                            this.page = Page::ALL[*index];
                            cx.notify();
                        })
                        .ok();
                    })
                    .children(Page::ALL.map(|page| Tab::new().label(page.label()))),
            )
            .child(div().flex_1())
            .child(
                Button::new("copy-diagnostics")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::ClipboardCopy))
                    .label("Copy Diagnostics")
                    .tooltip("Copy the status, metrics and latest log records")
                    .on_click(cx.listener(|this, _, window, cx| this.copy_diagnostics(window, cx))),
            )
            .child(
                Button::new("open-log-folder")
                    .ghost()
                    .small()
                    .icon(IconName::FolderOpen)
                    .label("Log File")
                    .tooltip("Show the log file in the file manager")
                    .on_click(cx.listener(|this, _, _, cx| this.open_log_folder(cx))),
            )
    }

    fn render_logs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let this = cx.entity().downgrade();
        let levels = [
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ];
        let selected = levels
            .iter()
            .position(|level| *level == self.level)
            .unwrap_or(2);
        let follow_this = this.clone();
        let toolbar = h_flex()
            .flex_none()
            .gap_3()
            .px_3()
            .py_2()
            .child(
                TabBar::new("log-level")
                    .segmented()
                    .small()
                    .selected_index(selected)
                    .on_click(move |index: &usize, _, cx| {
                        this.update(cx, |this, cx| this.set_level(levels[*index], cx))
                            .ok();
                    })
                    .children(levels.map(|level| Tab::new().label(capitalized(level.name())))),
            )
            .child(
                div()
                    .w(px(280.))
                    .child(Input::new(&self.filter).small().cleanable(true)),
            )
            .child(
                Checkbox::new("follow")
                    .label("Follow")
                    .checked(self.follow)
                    .on_click(move |checked: &bool, _, cx| {
                        follow_this
                            .update(cx, |this, cx| {
                                this.follow = *checked;
                                this.refilter(cx);
                            })
                            .ok();
                    }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "{} of {} records",
                        output::number(self.shown.len()),
                        output::number(self.entries.len())
                    )),
            );
        let list = div()
            .id("log-body")
            .relative()
            .flex_1()
            .min_h_0()
            .font_family(theme.mono_font_family.clone())
            .text_size(theme.mono_font_size)
            .child(
                uniform_list(
                    "log-lines",
                    self.shown.len(),
                    cx.processor(|this, range: Range<usize>, _, cx| {
                        this.render_log_lines(range, cx)
                    }),
                )
                .track_scroll(&self.log_scroll)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .size_full(),
            )
            .scrollbar(&self.log_scroll, ScrollbarAxis::Both);
        v_flex().flex_1().min_h_0().child(toolbar).child(list)
    }

    fn render_log_lines(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        range
            .filter_map(|row| {
                let entry = self.entries.get(*self.shown.get(row)?)?;
                let level_color = match entry.level {
                    LogLevel::Error => theme.danger,
                    LogLevel::Warn => theme.warning,
                    LogLevel::Info => theme.info,
                    LogLevel::Debug | LogLevel::Trace => theme.muted_foreground,
                };
                Some(
                    h_flex()
                        .min_w_full()
                        .px_3()
                        .gap_3()
                        .whitespace_nowrap()
                        .when(row % 2 == 1, |row| row.bg(theme.list_even))
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(format_time(entry.time_ms)[11..23].to_string()),
                        )
                        .child(
                            div()
                                .w(px(44.))
                                .text_color(level_color)
                                .child(entry.level.name().to_uppercase()),
                        )
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(entry.target.clone()),
                        )
                        .child(div().child(entry.message.clone()))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_metrics(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let snapshot = &self.snapshot;
        let muted = theme.muted_foreground;
        let (border, radius) = (theme.border, theme.radius_lg);
        let tile = |title: &str, value: String, detail: String| {
            v_flex()
                .w(px(250.))
                .gap_1()
                .p_3()
                .border_1()
                .border_color(border)
                .rounded(radius)
                .child(div().text_xs().text_color(muted).child(title.to_string()))
                .child(div().text_xl().font_semibold().child(value))
                .child(div().text_xs().text_color(muted).child(detail))
        };
        let timing = |timing: &TimingSummary| {
            if timing.count == 0 {
                "none yet".to_string()
            } else {
                format!(
                    "p50 {} · p95 {} · max {}",
                    output::ms(timing.p50_ms),
                    output::ms(timing.p95_ms),
                    output::ms(timing.max_ms)
                )
            }
        };
        let [errors, warnings, ..] = snapshot.log_counts;
        let tiles = h_flex()
            .flex_wrap()
            .gap_3()
            .child(tile(
                "Memory",
                snapshot.memory_bytes.map_or_else(
                    || "unknown".into(),
                    |bytes| format!("{:.0} MB", bytes as f64 / 1_048_576.0),
                ),
                format!("up {}", uptime(snapshot.uptime_ms)),
            ))
            .child(tile(
                "Searches from windows",
                output::number(snapshot.window_searches.count as usize),
                timing(&snapshot.window_searches),
            ))
            .child(tile(
                "Searches from the command line",
                output::number(snapshot.cli_searches.count as usize),
                timing(&snapshot.cli_searches),
            ))
            .child(tile(
                "File reads the indexes spared",
                format!("{:.1}%", snapshot.index_savings * 100.0),
                "of the files in scope, over every search".into(),
            ))
            .child(tile(
                "Index loads",
                output::number(snapshot.index_loads.count as usize),
                timing(&snapshot.index_loads),
            ))
            .child(tile(
                "Index builds",
                output::number(snapshot.index_builds.count as usize),
                if snapshot.index_build_failures > 0 {
                    format!(
                        "{} failed · {}",
                        snapshot.index_build_failures,
                        timing(&snapshot.index_builds)
                    )
                } else {
                    timing(&snapshot.index_builds)
                },
            ))
            .child(tile(
                "Index updates",
                output::number(snapshot.index_updates.count as usize),
                timing(&snapshot.index_updates),
            ))
            .child(tile(
                "Log problems",
                format!("{errors} errors"),
                format!("{warnings} warnings among the records kept"),
            ));

        let requests: Vec<String> = snapshot
            .requests
            .iter()
            .map(|(kind, count)| format!("{kind}: {count}"))
            .collect();
        let repos = self.repos.iter().map(|repo| {
            h_flex()
                .gap_3()
                .py_1()
                .border_b_1()
                .border_color(border)
                .child(
                    div()
                        .w(px(200.))
                        .truncate()
                        .font_semibold()
                        .child(repo.info.name.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .truncate()
                        .text_color(muted)
                        .child(repo.index_summary()),
                )
        });

        div()
            .id("metrics-page")
            .flex_1()
            .min_h_0()
            .child(
                v_flex()
                    .p_4()
                    .gap_4()
                    .child(tiles)
                    .child(section_title("Command-line requests", muted))
                    .child(div().text_sm().child(if requests.is_empty() {
                        "none yet".to_string()
                    } else {
                        requests.join(" · ")
                    }))
                    .child(section_title("Open repositories", muted))
                    .child(if self.repos.is_empty() {
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child("none")
                            .into_any_element()
                    } else {
                        v_flex().text_sm().children(repos).into_any_element()
                    }),
            )
            .overflow_y_scrollbar()
    }

    fn render_searches(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let columns: [(&str, f32); 8] = [
            ("Time", 70.),
            ("From", 56.),
            ("Took", 72.),
            ("Choosing", 72.),
            ("Read", 110.),
            ("Found", 150.),
            ("Bytes", 72.),
            ("Query", 0.),
        ];
        let cell = |text: String, width: f32| {
            let cell = div().truncate().child(text);
            if width > 0. {
                cell.w(px(width)).flex_none()
            } else {
                cell.flex_1().min_w_0()
            }
        };
        let header = h_flex()
            .gap_3()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(border)
            .text_xs()
            .text_color(muted)
            .children(columns.map(|(name, width)| cell(name.to_string(), width)));
        let rows = self
            .snapshot
            .recent_searches
            .iter()
            .enumerate()
            .map(|(index, search)| {
                let values = [
                    format_time(search.time_ms)[11..19].to_string(),
                    search.origin.name().to_string(),
                    output::ms(search.elapsed_ms),
                    output::ms(search.candidates_ms),
                    format!(
                        "{} / {}",
                        output::number(search.searched_files),
                        output::number(search.corpus_files)
                    ),
                    format!(
                        "{} lines, {} files{}",
                        output::number(search.matched_lines),
                        output::number(search.files),
                        if search.truncated { " (cut)" } else { "" }
                    ),
                    format!("{:.0} KB", search.bytes_read as f64 / 1024.0),
                    search.query.clone(),
                ];
                h_flex()
                    .gap_3()
                    .px_3()
                    .py_1()
                    .text_sm()
                    .when(index % 2 == 1, |row| row.bg(theme.list_even))
                    .children(
                        values
                            .into_iter()
                            .zip(columns)
                            .map(|(value, (_, width))| cell(value, width)),
                    )
            });
        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(
                div()
                    .id("searches-body")
                    .flex_1()
                    .min_h_0()
                    .child(if self.snapshot.recent_searches.is_empty() {
                        div()
                            .p_4()
                            .text_sm()
                            .text_color(muted)
                            .child("No searches yet. Searches from windows and from the command line show here, newest first.")
                            .into_any_element()
                    } else {
                        v_flex().children(rows).into_any_element()
                    })
                    .overflow_y_scrollbar(),
            )
    }
}

impl Render for DevTools {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.page {
            Page::Logs => self.render_logs(cx).into_any_element(),
            Page::Metrics => self.render_metrics(cx).into_any_element(),
            Page::Searches => self.render_searches(cx).into_any_element(),
        };
        let theme = cx.theme();
        v_flex()
            .key_context(super::DEVTOOLS_CONTEXT)
            .on_action(cx.listener(|_, _: &super::ToggleDevTools, window, _| {
                window.remove_window();
            }))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_header(cx))
            .child(body)
            .children(Root::render_notification_layer(window, cx))
    }
}

fn section_title(title: &str, color: Hsla) -> impl IntoElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(color)
        .child(title.to_uppercase())
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn uptime(ms: u64) -> String {
    let minutes = ms / 60_000;
    match minutes {
        0 => format!("{}s", ms / 1000),
        1..60 => format!("{minutes}m"),
        _ => format!("{}h {}m", minutes / 60, minutes % 60),
    }
}
