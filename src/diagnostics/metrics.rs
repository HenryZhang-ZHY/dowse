//! The app's key numbers: how long searches, index loads and builds take,
//! how much each search had to read, what the command line asks for, and
//! the process's memory. The developer tools window and `dowse dev metrics`
//! show them, to see where time goes while working on the app.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::log::now_ms;
use crate::engine::search::SearchOutcome;

/// Samples kept per timing, for its percentiles.
const SAMPLES: usize = 512;
/// Searches kept for the searches list.
const RECENT_SEARCHES: usize = 200;

/// Where a search came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Window,
    Cli,
}

impl Origin {
    pub fn name(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Cli => "cli",
        }
    }
}

/// One search, as the searches list shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchRecord {
    /// When it finished, in milliseconds since the Unix epoch.
    pub time_ms: u64,
    pub origin: Origin,
    pub query: String,
    pub repos: usize,
    pub corpus_files: usize,
    /// Files read after the indexes narrowed the candidates.
    pub searched_files: usize,
    pub files: usize,
    pub matched_lines: usize,
    pub truncated: bool,
    pub elapsed_ms: f64,
    pub candidates_ms: f64,
    pub bytes_read: u64,
}

impl SearchRecord {
    pub fn new(origin: Origin, query: &str, outcome: &SearchOutcome) -> Self {
        Self {
            time_ms: now_ms(),
            origin,
            query: query.to_string(),
            repos: outcome.repos,
            corpus_files: outcome.corpus_files,
            searched_files: outcome.searched_files,
            files: outcome.files.len(),
            matched_lines: outcome.matched_lines,
            truncated: outcome.truncated,
            elapsed_ms: millis(outcome.elapsed),
            candidates_ms: millis(outcome.candidates_elapsed),
            bytes_read: outcome.bytes_read,
        }
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// A timing's distribution: every sample counts toward `count`, `mean_ms`
/// and `max_ms`; the percentiles cover the latest samples.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TimingSummary {
    pub count: u64,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

#[derive(Debug, Default)]
struct Timing {
    count: u64,
    total_ms: f64,
    max_ms: f64,
    recent: VecDeque<f64>,
}

impl Timing {
    fn record(&mut self, ms: f64) {
        self.count += 1;
        self.total_ms += ms;
        self.max_ms = self.max_ms.max(ms);
        if self.recent.len() == SAMPLES {
            self.recent.pop_front();
        }
        self.recent.push_back(ms);
    }

    fn summary(&self) -> TimingSummary {
        let mut sorted: Vec<f64> = self.recent.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        let percentile = |p: f64| -> f64 {
            if sorted.is_empty() {
                return 0.0;
            }
            // Nearest rank.
            let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
            sorted[rank - 1]
        };
        TimingSummary {
            count: self.count,
            mean_ms: if self.count == 0 {
                0.0
            } else {
                self.total_ms / self.count as f64
            },
            p50_ms: percentile(0.50),
            p95_ms: percentile(0.95),
            max_ms: self.max_ms,
        }
    }
}

/// Everything at one moment, for display.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub uptime_ms: u64,
    /// The process's resident memory, where the platform tells.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    pub window_searches: TimingSummary,
    pub cli_searches: TimingSummary,
    /// Of every search, the share of candidate files the indexes ruled out,
    /// from 0 to 1.
    pub index_savings: f64,
    pub index_loads: TimingSummary,
    /// Whole indexes built.
    pub index_builds: TimingSummary,
    /// Indexes brought up to date by reading only the files that changed,
    /// including those found current.
    #[serde(default)]
    pub index_updates: TimingSummary,
    pub index_build_failures: u64,
    /// Command-line requests by kind.
    pub requests: BTreeMap<String, u64>,
    /// Log records kept, by level, most severe first.
    pub log_counts: [usize; 5],
    /// The latest searches, the most recent first.
    pub recent_searches: Vec<SearchRecord>,
}

#[derive(Debug)]
pub struct Metrics {
    started: Instant,
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    window_searches: Timing,
    cli_searches: Timing,
    searched_files: u64,
    corpus_files: u64,
    index_loads: Timing,
    index_builds: Timing,
    index_updates: Timing,
    index_build_failures: u64,
    requests: BTreeMap<String, u64>,
    recent_searches: VecDeque<SearchRecord>,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            inner: Mutex::default(),
        }
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn record_search(&self, record: SearchRecord) {
        let mut inner = self.inner();
        match record.origin {
            Origin::Window => inner.window_searches.record(record.elapsed_ms),
            Origin::Cli => inner.cli_searches.record(record.elapsed_ms),
        }
        inner.searched_files += record.searched_files as u64;
        inner.corpus_files += record.corpus_files as u64;
        if inner.recent_searches.len() == RECENT_SEARCHES {
            inner.recent_searches.pop_back();
        }
        inner.recent_searches.push_front(record);
    }

    pub fn record_index_load(&self, elapsed: Duration) {
        self.inner().index_loads.record(millis(elapsed));
    }

    pub fn record_index_build(&self, elapsed: Duration, succeeded: bool) {
        let mut inner = self.inner();
        if succeeded {
            inner.index_builds.record(millis(elapsed));
        } else {
            inner.index_build_failures += 1;
        }
    }

    pub fn record_index_update(&self, elapsed: Duration) {
        self.inner().index_updates.record(millis(elapsed));
    }

    pub fn count_request(&self, kind: &str) {
        *self.inner().requests.entry(kind.to_string()).or_default() += 1;
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let inner = self.inner();
        MetricsSnapshot {
            uptime_ms: self.started.elapsed().as_millis() as u64,
            memory_bytes: resident_memory(),
            window_searches: inner.window_searches.summary(),
            cli_searches: inner.cli_searches.summary(),
            index_savings: if inner.corpus_files == 0 {
                0.0
            } else {
                1.0 - inner.searched_files as f64 / inner.corpus_files as f64
            },
            index_loads: inner.index_loads.summary(),
            index_builds: inner.index_builds.summary(),
            index_updates: inner.index_updates.summary(),
            index_build_failures: inner.index_build_failures,
            requests: inner.requests.clone(),
            log_counts: super::log::buffer().counts(),
            recent_searches: inner.recent_searches.iter().cloned().collect(),
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

static METRICS: OnceLock<Metrics> = OnceLock::new();

/// The app's metrics, counted from the first use.
pub fn metrics() -> &'static Metrics {
    METRICS.get_or_init(Metrics::new)
}

/// The process's resident memory (working set on Windows).
pub fn resident_memory() -> Option<u64> {
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> isize;
            fn K32GetProcessMemoryInfo(
                process: isize,
                counters: *mut ProcessMemoryCounters,
                size: u32,
            ) -> i32;
        }
        let mut counters = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            ..Default::default()
        };
        // SAFETY: `counters` is a correctly sized, writable struct.
        let ok =
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
        (ok != 0).then_some(counters.working_set_size as u64)
    }
    #[cfg(target_os = "linux")]
    {
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        Some(pages * 4096)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(origin: Origin, query: &str, elapsed_ms: f64, searched: usize) -> SearchRecord {
        SearchRecord {
            time_ms: 0,
            origin,
            query: query.into(),
            repos: 1,
            corpus_files: 100,
            searched_files: searched,
            files: 1,
            matched_lines: 1,
            truncated: false,
            elapsed_ms,
            candidates_ms: 0.0,
            bytes_read: 0,
        }
    }

    #[test]
    fn timings_summarise_their_samples() {
        let mut timing = Timing::default();
        for ms in 1..=100 {
            timing.record(ms as f64);
        }
        let summary = timing.summary();
        assert_eq!(summary.count, 100);
        assert_eq!(summary.mean_ms, 50.5);
        assert_eq!(summary.p50_ms, 50.0);
        assert_eq!(summary.p95_ms, 95.0);
        assert_eq!(summary.max_ms, 100.0);
        assert_eq!(Timing::default().summary(), TimingSummary::default());
    }

    #[test]
    fn searches_count_by_origin_and_list_the_latest_first() {
        let metrics = Metrics::new();
        metrics.record_search(record(Origin::Window, "a", 10.0, 10));
        metrics.record_search(record(Origin::Cli, "b", 30.0, 30));
        metrics.count_request("search");
        metrics.count_request("search");
        metrics.record_index_build(Duration::from_secs(2), true);
        metrics.record_index_build(Duration::from_secs(1), false);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.window_searches.count, 1);
        assert_eq!(snapshot.cli_searches.mean_ms, 30.0);
        assert_eq!(snapshot.recent_searches[0].query, "b");
        assert_eq!(snapshot.index_savings, 0.8);
        assert_eq!(snapshot.requests.get("search"), Some(&2));
        assert_eq!(snapshot.index_builds.max_ms, 2000.0);
        assert_eq!(snapshot.index_build_failures, 1);
    }

    #[test]
    fn memory_is_known_where_the_platform_tells() {
        if cfg!(any(windows, target_os = "linux")) {
            assert!(resident_memory().is_some_and(|bytes| bytes > 0));
        }
    }
}
