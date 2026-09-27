//! Human-readable numbers and times for the status lines.

use std::time::{Duration, SystemTime};

/// `12345` -> `12,345`.
pub fn count(value: usize) -> String {
    let digits = value.to_string();
    let mut output = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            output.push(',');
        }
        output.push(ch);
    }
    output
}

/// `1` -> `1 file`, `2` -> `2 files`.
pub fn plural(value: usize, singular: &str, plural: &str) -> String {
    format!(
        "{} {}",
        count(value),
        if value == 1 { singular } else { plural }
    )
}

/// A size GitHub gives in kilobytes: `512 KB`, `1.5 MB`, `2.3 GB`.
pub fn kilobytes(kb: u64) -> String {
    let kb = kb as f64;
    if kb < 1024.0 {
        format!("{kb:.0} KB")
    } else if kb < 1024.0 * 1024.0 {
        format!("{:.1} MB", kb / 1024.0)
    } else {
        format!("{:.1} GB", kb / 1024.0 / 1024.0)
    }
}

pub fn duration(elapsed: Duration) -> String {
    let millis = elapsed.as_secs_f64() * 1000.0;
    if millis < 10.0 {
        format!("{millis:.1} ms")
    } else if millis < 1000.0 {
        format!("{millis:.0} ms")
    } else {
        format!("{:.1} s", millis / 1000.0)
    }
}

/// How long ago `time` was, coarsely: `just now`, `5 min ago`, `3 h ago`, `2 d ago`.
pub fn ago(time: SystemTime, now: SystemTime) -> String {
    let seconds = now.duration_since(time).unwrap_or_default().as_secs();
    match seconds {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_counts_with_separators() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1000), "1,000");
        assert_eq!(count(1234567), "1,234,567");
        assert_eq!(plural(1, "file", "files"), "1 file");
        assert_eq!(plural(2000, "file", "files"), "2,000 files");
    }

    #[test]
    fn formats_relative_times() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let before = |secs| now - Duration::from_secs(secs);
        assert_eq!(ago(before(5), now), "just now");
        assert_eq!(ago(before(300), now), "5 min ago");
        assert_eq!(ago(before(7200), now), "2 h ago");
        assert_eq!(ago(before(200_000), now), "2 d ago");
        assert_eq!(ago(now + Duration::from_secs(10), now), "just now");
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(kilobytes(511), "511 KB");
        assert_eq!(kilobytes(1536), "1.5 MB");
        assert_eq!(kilobytes(3 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(duration(Duration::from_micros(1500)), "1.5 ms");
        assert_eq!(duration(Duration::from_millis(42)), "42 ms");
        assert_eq!(duration(Duration::from_millis(2500)), "2.5 s");
    }
}
