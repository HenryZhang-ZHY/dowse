//! The app's log: every record goes to a ring buffer that the developer
//! tools and `dowse dev logs` read, and to a log file that survives the
//! process. Records come through the `log` facade, so GPUI's own records are
//! caught too.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use log::{Level, LevelFilter, Log, Metadata, Record};
use serde::{Deserialize, Serialize};

/// Records the ring buffer keeps.
pub const BUFFER_CAPACITY: usize = 5_000;
/// Past this size the log file is renamed to `dowse.old.log` and started
/// again, so the two together stay under twice this.
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;
/// Chooses the level, like `RUST_LOG`: `error`, `warn`, `info`, `debug` or
/// `trace`.
pub const LEVEL_ENV: &str = "DOWSE_LOG";

/// One log record, as kept and as sent to the command line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// Increases by one per record, so a reader can ask for what came after
    /// the last record it saw.
    pub seq: u64,
    /// Milliseconds since the Unix epoch.
    pub time_ms: u64,
    pub level: LogLevel,
    /// The module that logged it, such as `dowse::ui::hub`.
    pub target: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub const ALL: [Self; 5] = [
        Self::Error,
        Self::Warn,
        Self::Info,
        Self::Debug,
        Self::Trace,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|level| level.name().eq_ignore_ascii_case(text.trim()))
    }

    fn filter(self) -> LevelFilter {
        match self {
            Self::Error => LevelFilter::Error,
            Self::Warn => LevelFilter::Warn,
            Self::Info => LevelFilter::Info,
            Self::Debug => LevelFilter::Debug,
            Self::Trace => LevelFilter::Trace,
        }
    }
}

impl From<Level> for LogLevel {
    fn from(level: Level) -> Self {
        match level {
            Level::Error => Self::Error,
            Level::Warn => Self::Warn,
            Level::Info => Self::Info,
            Level::Debug => Self::Debug,
            Level::Trace => Self::Trace,
        }
    }
}

/// The most recent records, oldest first.
#[derive(Debug)]
pub struct LogBuffer {
    inner: Mutex<BufferInner>,
}

#[derive(Debug)]
struct BufferInner {
    entries: VecDeque<LogEntry>,
    capacity: usize,
    next_seq: u64,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(BufferInner {
                entries: VecDeque::with_capacity(capacity.min(1024)),
                capacity,
                next_seq: 1,
            }),
        }
    }

    /// Keep a record, dropping the oldest when full. Returns its sequence
    /// number.
    pub fn push(&self, level: LogLevel, target: &str, message: String) -> u64 {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let seq = inner.next_seq;
        inner.next_seq += 1;
        if inner.entries.len() == inner.capacity {
            inner.entries.pop_front();
        }
        inner.entries.push_back(LogEntry {
            seq,
            time_ms: now_ms(),
            level,
            target: target.to_string(),
            message,
        });
        seq
    }

    /// Records after sequence number `after` at `level` or more severe, at
    /// most the last `limit` of them.
    pub fn since(&self, after: u64, level: LogLevel, limit: usize) -> Vec<LogEntry> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let matching: Vec<&LogEntry> = inner
            .entries
            .iter()
            .filter(|entry| entry.seq > after && entry.level <= level)
            .collect();
        let skip = matching.len().saturating_sub(limit);
        matching.into_iter().skip(skip).cloned().collect()
    }

    /// The sequence number the next record gets.
    pub fn next_seq(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .next_seq
    }

    /// How many kept records are at each level, most severe first.
    pub fn counts(&self) -> [usize; 5] {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut counts = [0; 5];
        for entry in &inner.entries {
            counts[entry.level as usize] += 1;
        }
        counts
    }
}

/// Appends lines to a file, starting it over past a size limit.
struct LogFile {
    path: PathBuf,
    file: File,
    size: u64,
    max_size: u64,
}

impl LogFile {
    fn open(path: PathBuf, max_size: u64) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path,
            file,
            size,
            max_size,
        })
    }

    fn write_line(&mut self, line: &str) {
        if self.size + line.len() as u64 > self.max_size && self.size > 0 {
            let old = old_log_path(&self.path);
            std::fs::remove_file(&old).ok();
            if std::fs::rename(&self.path, &old).is_ok()
                && let Ok(file) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
            {
                self.file = file;
                self.size = 0;
            }
        }
        if self.file.write_all(line.as_bytes()).is_ok() {
            self.size += line.len() as u64;
        }
    }
}

/// Where the previous log file goes when a new one starts.
pub fn old_log_path(path: &Path) -> PathBuf {
    path.with_extension("old.log")
}

struct Logger {
    buffer: &'static LogBuffer,
    file: Option<Mutex<LogFile>>,
    level: LevelFilter,
    echo: bool,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let message = record.args().to_string();
        let level = LogLevel::from(record.level());
        self.buffer.push(level, record.target(), message.clone());
        let line = format!(
            "{} {:5} {}: {}\n",
            format_time(now_ms()),
            level.name().to_uppercase(),
            record.target(),
            message
        );
        if let Some(file) = &self.file {
            file.lock()
                .unwrap_or_else(|e| e.into_inner())
                .write_line(&line);
        }
        if self.echo {
            eprint!("{line}");
        }
    }

    fn flush(&self) {
        if let Some(file) = &self.file {
            file.lock()
                .unwrap_or_else(|e| e.into_inner())
                .file
                .flush()
                .ok();
        }
    }
}

static BUFFER: OnceLock<LogBuffer> = OnceLock::new();

/// The records kept in memory. Empty until [`init`] runs.
pub fn buffer() -> &'static LogBuffer {
    BUFFER.get_or_init(|| LogBuffer::new(BUFFER_CAPACITY))
}

/// The log file within `log_dir`.
pub fn log_file(log_dir: &Path) -> PathBuf {
    log_dir.join("dowse.log")
}

/// Send records to the ring buffer and to the log file in `log_dir` (and to
/// stderr in debug builds). The level comes from `DOWSE_LOG`, `info` when
/// unset. Only the first call has an effect.
pub fn init(log_dir: &Path) {
    let level = std::env::var(LEVEL_ENV)
        .ok()
        .and_then(|text| LogLevel::parse(&text))
        .unwrap_or(LogLevel::Info);
    let file = LogFile::open(log_file(log_dir), MAX_FILE_SIZE)
        .ok()
        .map(Mutex::new);
    let logger = Logger {
        buffer: buffer(),
        file,
        level: level.filter(),
        echo: cfg!(debug_assertions),
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(level.filter());
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// `2026-09-26T08:15:42.123Z`: UTC, so logs from different machines line up.
pub fn format_time(time_ms: u64) -> String {
    let seconds = time_ms / 1000;
    let (days, day_seconds) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        day_seconds / 3600,
        day_seconds / 60 % 60,
        day_seconds % 60,
        time_ms % 1000
    )
}

/// The calendar date `days` after 1970-01-01, by Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_keeps_the_latest_records_and_reads_after_a_sequence() {
        let buffer = LogBuffer::new(3);
        for n in 1..=5 {
            buffer.push(LogLevel::Info, "t", format!("m{n}"));
        }
        buffer.push(LogLevel::Debug, "t", "detail".into());
        let messages = |entries: Vec<LogEntry>| -> Vec<String> {
            entries.into_iter().map(|entry| entry.message).collect()
        };
        assert_eq!(
            messages(buffer.since(0, LogLevel::Trace, 100)),
            ["m4", "m5", "detail"]
        );
        assert_eq!(messages(buffer.since(0, LogLevel::Info, 100)), ["m4", "m5"]);
        assert_eq!(
            messages(buffer.since(4, LogLevel::Trace, 100)),
            ["m5", "detail"]
        );
        assert_eq!(messages(buffer.since(0, LogLevel::Trace, 1)), ["detail"]);
        assert_eq!(buffer.next_seq(), 7);
        assert_eq!(buffer.counts(), [0, 0, 2, 1, 0]);
    }

    #[test]
    fn the_file_starts_over_past_its_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_file(dir.path());
        let mut file = LogFile::open(path.clone(), 10).unwrap();
        file.write_line("123456\n");
        file.write_line("abcdef\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abcdef\n");
        assert_eq!(
            std::fs::read_to_string(old_log_path(&path)).unwrap(),
            "123456\n"
        );
        assert_eq!(old_log_path(&path), dir.path().join("dowse.old.log"));
    }

    #[test]
    fn levels_parse_by_name() {
        assert_eq!(LogLevel::parse(" Warn "), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("loud"), None);
        assert!(LogLevel::Error < LogLevel::Debug);
    }

    #[test]
    fn times_are_utc_timestamps() {
        assert_eq!(format_time(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_time(1_790_410_542_123), "2026-09-26T08:15:42.123Z");
    }
}
