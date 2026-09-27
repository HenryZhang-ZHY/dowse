//! How the command line shows the app's answers. Results go to stdout and
//! everything about them (counts, timings, how to narrow, notes) to stderr,
//! so a pipe only carries results. Text is grep-like and compact, since
//! agents pay for every token; `--json` gives one JSON object per line.

use std::fmt::Write as _;

use serde::Serialize;

use crate::diagnostics::log::{LogEntry, format_time};
use crate::diagnostics::metrics::{MetricsSnapshot, TimingSummary};
use crate::ipc::protocol::{AppStatus, FacetCounts, FileHit, RepoStatus, SearchResponse};

/// Whether to colour text for a terminal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub color: bool,
}

impl Style {
    fn paint(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

// ----- search -------------------------------------------------------------------

/// Results grouped by file: the file's absolute path with its repository
/// and branch, then `line:text` for matching lines and `line-text` for
/// context, as grep and ripgrep print them.
pub fn search_text(response: &SearchResponse, style: Style) -> String {
    let mut out = String::new();
    for (index, file) in response.files.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let _ = writeln!(
            out,
            "{}  {}",
            style.paint("35", &file.abs_path.display().to_string()),
            style.paint("2", &format!("[{}]", repo_label(file)))
        );
        // `--` separates runs of lines, as grep prints it with context.
        let with_context = file.lines.iter().any(|line| !line.is_match);
        let mut previous = None;
        for line in &file.lines {
            if with_context && previous.is_some_and(|previous| line.line > previous + 1) {
                out.push_str("--\n");
            }
            previous = Some(line.line);
            let separator = if line.is_match { ':' } else { '-' };
            let text = if line.is_match {
                highlight(&line.text, &line.ranges, style)
            } else {
                line.text.clone()
            };
            let _ = writeln!(
                out,
                "{}{separator}{text}",
                style.paint("32", &line.line.to_string())
            );
        }
        let shown = file.lines.iter().filter(|line| line.is_match).count();
        if !file.lines.is_empty() && shown < file.matched_lines {
            let _ = writeln!(
                out,
                "{}",
                style.paint(
                    "2",
                    &format!("(+{} more in this file)", file.matched_lines - shown)
                )
            );
        }
    }
    out
}

fn repo_label(file: &FileHit) -> String {
    match &file.branch {
        Some(branch) => format!("{}@{branch}", file.repo),
        None => file.repo.clone(),
    }
}

fn highlight(text: &str, ranges: &[(usize, usize)], style: Style) -> String {
    if !style.color || ranges.is_empty() {
        return text.to_string();
    }
    let mut out = String::new();
    let mut position = 0;
    for &(start, end) in ranges {
        if start < position
            || end > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            continue;
        }
        out.push_str(&text[position..start]);
        out.push_str(&style.paint("1;31", &text[start..end]));
        position = end;
    }
    out.push_str(&text[position..]);
    out
}

/// One absolute path per matching file, for `-l`.
pub fn file_list(response: &SearchResponse) -> String {
    response
        .files
        .iter()
        .map(|file| format!("{}\n", file.abs_path.display()))
        .collect()
}

/// `path:count` per matching file, for `-c`.
pub fn file_counts(response: &SearchResponse) -> String {
    response
        .files
        .iter()
        .map(|file| format!("{}:{}\n", file.abs_path.display(), file.matched_lines))
        .collect()
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum JsonRecord<'a> {
    File(&'a FileHit),
    Summary {
        #[serde(flatten)]
        summary: &'a crate::ipc::protocol::SearchSummary,
        facets: &'a [FacetCounts],
        #[serde(skip_serializing_if = "<[String]>::is_empty")]
        notes: &'a [String],
        #[serde(skip_serializing_if = "Vec::is_empty")]
        narrow: Vec<String>,
    },
}

/// One `file` object per line, then a `summary`.
pub fn search_json(response: &SearchResponse) -> String {
    let mut out = String::new();
    for file in &response.files {
        out.push_str(&to_json(&JsonRecord::File(file)));
        out.push('\n');
    }
    let summary = JsonRecord::Summary {
        summary: &response.summary,
        facets: &response.facets,
        notes: &response.notes,
        narrow: narrowing(response),
    };
    out.push_str(&to_json(&summary));
    out.push('\n');
    out
}

fn to_json(value: &impl Serialize) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// What goes to stderr after the results: how many were found and shown,
/// how long it took, how to narrow when not everything was shown, and
/// notes. With `stats`, how the index narrowed the search too.
pub fn search_footer(response: &SearchResponse, stats: bool, style: Style) -> String {
    let summary = &response.summary;
    let mut out = String::new();
    let lines = if summary.shown_lines < summary.matched_lines && !names_only(response) {
        format!(
            "{} of {} matching lines",
            number(summary.shown_lines),
            number(summary.matched_lines)
        )
    } else {
        plural(summary.matched_lines, "matching line", "matching lines")
    };
    let files = if summary.shown_files < summary.files {
        format!(
            "{} of {} files",
            number(summary.shown_files),
            number(summary.files)
        )
    } else {
        plural(summary.files, "file", "files")
    };
    let _ = writeln!(
        out,
        "{}",
        style.paint(
            "2",
            &format!(
                "{lines} in {files} · {} · {:.0} ms",
                plural(summary.repos, "repository", "repositories"),
                summary.elapsed_ms
            )
        )
    );
    if summary.truncated {
        let _ = writeln!(
            out,
            "the search stopped at its limit, so counts are a lower bound"
        );
    }
    let narrow = narrowing(response);
    if !narrow.is_empty() {
        let _ = writeln!(out, "narrow with: {}", narrow.join("  "));
    }
    if stats {
        let _ = writeln!(
            out,
            "stats: read {} of {} files ({}) after {:.1} ms choosing candidates; {} of {} repositories unindexed",
            number(summary.searched_files),
            number(summary.corpus_files),
            bytes(summary.bytes_read),
            summary.candidates_ms,
            summary.unindexed_repos,
            summary.repos
        );
    }
    for note in &response.notes {
        let _ = writeln!(out, "note: {note}");
    }
    out
}

/// Qualifiers that would narrow the results, with how many files each
/// keeps, when not everything was shown. Each is valid query syntax to
/// append to the query.
pub fn narrowing(response: &SearchResponse) -> Vec<String> {
    let summary = &response.summary;
    let lines_shown = summary.shown_lines >= summary.matched_lines || names_only(response);
    if summary.shown_files >= summary.files && lines_shown {
        return Vec::new();
    }
    response
        .facets
        .iter()
        .flat_map(|facet| {
            facet
                .values
                .iter()
                .take(3)
                .map(|(value, count)| format!("{} ({count})", qualifier(&facet.qualifier, value)))
        })
        .collect()
}

/// Whether only file names were asked for, as with `-l` and `-c`.
fn names_only(response: &SearchResponse) -> bool {
    response.files.iter().all(|file| file.lines.is_empty())
}

/// `name:value`, quoting values the query parser would split, and matching
/// a whole top-level directory for `path`.
pub fn qualifier(name: &str, value: &str) -> String {
    let value = if name == "path" {
        format!("{value}/**")
    } else {
        value.to_string()
    };
    if value.contains([' ', '"', '(', ')']) {
        format!(
            "{name}:\"{}\"",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        format!("{name}:{value}")
    }
}

// ----- repositories, status and logs --------------------------------------------

/// One repository per line: name, branch, index state, files, when indexed,
/// tags and path.
pub fn repos_text(repos: &[RepoStatus], now_ms: u64) -> String {
    let rows: Vec<[String; 6]> = repos
        .iter()
        .map(|repo| {
            let mut index = repo.index.clone();
            if repo.changed_files > 0 {
                let _ = write!(index, " +{} changed", repo.changed_files);
            }
            if let Some(error) = &repo.error {
                let _ = write!(index, " ({error})");
            }
            [
                repo.name.clone(),
                repo.branch.clone().unwrap_or_else(|| "-".into()),
                index,
                repo.files
                    .map_or_else(|| "-".into(), |files| number(files as usize)),
                repo.indexed_at_ms
                    .map_or_else(|| "-".into(), |at| ago(now_ms.saturating_sub(at))),
                if repo.tags.is_empty() {
                    "-".into()
                } else {
                    repo.tags.join(" ")
                },
            ]
        })
        .collect();
    let header = ["NAME", "BRANCH", "INDEX", "FILES", "INDEXED", "TAGS"].map(String::from);
    let mut widths = header.clone().map(|cell| cell.chars().count());
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for (row, repo) in std::iter::once((&header, None))
        .chain(rows.iter().zip(repos).map(|(row, repo)| (row, Some(repo))))
    {
        for (cell, width) in row.iter().zip(widths) {
            let _ = write!(out, "{cell:width$}  ");
        }
        match repo {
            Some(repo) => {
                let _ = writeln!(out, "{}", repo.path.display());
            }
            None => out.push_str("PATH\n"),
        }
    }
    out
}

pub fn status_text(status: &AppStatus) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "dowse {}  pid {}  up {}",
        status.version,
        status.pid,
        duration(status.uptime_ms)
    );
    let windows = match (status.windows, status.quits_in_ms) {
        (0, Some(quits)) => format!("none (quits after {} idle)", duration(quits)),
        (0, None) => "none".into(),
        (count, _) => count.to_string(),
    };
    let _ = writeln!(out, "windows:      {windows}");
    let _ = writeln!(
        out,
        "repositories: {} in the library, {} open",
        status.library_repos, status.open_repos
    );
    if !status.building.is_empty() || !status.queued.is_empty() {
        let _ = writeln!(
            out,
            "indexing:     {}{}",
            status.building.join(", "),
            if status.queued.is_empty() {
                String::new()
            } else {
                format!(" (then {})", status.queued.join(", "))
            }
        );
    }
    let _ = writeln!(out, "settings:     {}", status.config_dir.display());
    let _ = writeln!(out, "log:          {}", status.log_file.display());
    out
}

pub fn log_line(entry: &LogEntry, style: Style) -> String {
    let level = entry.level.name().to_uppercase();
    let level = match entry.level {
        crate::diagnostics::log::LogLevel::Error => style.paint("31", &format!("{level:5}")),
        crate::diagnostics::log::LogLevel::Warn => style.paint("33", &format!("{level:5}")),
        _ => format!("{level:5}"),
    };
    format!(
        "{} {level} {}: {}\n",
        format_time(entry.time_ms),
        style.paint("2", &entry.target),
        entry.message
    )
}

pub fn metrics_text(metrics: &MetricsSnapshot) -> String {
    let mut out = String::new();
    let memory = metrics
        .memory_bytes
        .map_or_else(String::new, |memory| format!(" · memory {}", bytes(memory)));
    let _ = writeln!(out, "up {}{memory}", duration(metrics.uptime_ms));
    let timing = |out: &mut String, name: &str, timing: &TimingSummary| {
        let _ = writeln!(
            out,
            "{name:<16} {:>6}  p50 {:>9}  p95 {:>9}  max {:>9}",
            number(timing.count as usize),
            ms(timing.p50_ms),
            ms(timing.p95_ms),
            ms(timing.max_ms)
        );
    };
    timing(&mut out, "window searches", &metrics.window_searches);
    timing(&mut out, "cli searches", &metrics.cli_searches);
    timing(&mut out, "index loads", &metrics.index_loads);
    timing(&mut out, "index builds", &metrics.index_builds);
    timing(&mut out, "index updates", &metrics.index_updates);
    if metrics.index_build_failures > 0 {
        let _ = writeln!(out, "index builds failed: {}", metrics.index_build_failures);
    }
    let _ = writeln!(
        out,
        "the indexes spared {:.1}% of file reads",
        metrics.index_savings * 100.0
    );
    let [errors, warnings, ..] = metrics.log_counts;
    let _ = writeln!(
        out,
        "log: {}, {}",
        plural(errors, "error", "errors"),
        plural(warnings, "warning", "warnings")
    );
    if !metrics.requests.is_empty() {
        let requests: Vec<String> = metrics
            .requests
            .iter()
            .map(|(kind, count)| format!("{kind} {count}"))
            .collect();
        let _ = writeln!(out, "requests: {}", requests.join(", "));
    }
    if !metrics.recent_searches.is_empty() {
        let _ = writeln!(out, "latest searches:");
        for search in metrics.recent_searches.iter().take(10) {
            let _ = writeln!(
                out,
                "  {}  {:<6} {:>9}  read {}/{}  {} in {}  {:?}",
                &format_time(search.time_ms)[11..19],
                search.origin.name(),
                ms(search.elapsed_ms),
                number(search.searched_files),
                number(search.corpus_files),
                plural(search.matched_lines, "line", "lines"),
                plural(search.files, "file", "files"),
                search.query
            );
        }
    }
    out
}

/// `12.3 ms`, or `4.1 s` past a second.
pub fn ms(value: f64) -> String {
    if value >= 1000.0 {
        format!("{:.1} s", value / 1000.0)
    } else {
        format!("{value:.1} ms")
    }
}

// ----- numbers ------------------------------------------------------------------

/// `12,345`.
pub fn number(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{} {}", number(count), if count == 1 { one } else { many })
}

fn bytes(value: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h {}m", seconds / 3600, seconds / 60 % 60),
        _ => format!("{}d {}h", seconds / 86_400, seconds / 3600 % 24),
    }
}

fn ago(ms: u64) -> String {
    format!("{} ago", duration(ms))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::engine::query::{CompiledQuery, SearchQuery};
    use crate::engine::repo::RepoInfo;
    use crate::ipc::protocol::{LineHit, SearchSummary};

    fn line(number: usize, text: &str, is_match: bool) -> LineHit {
        LineHit {
            line: number,
            text: text.into(),
            is_match,
            ranges: if is_match { vec![(0, 2)] } else { vec![] },
        }
    }

    fn response() -> SearchResponse {
        SearchResponse {
            files: vec![
                FileHit {
                    repo: "api".into(),
                    branch: Some("main".into()),
                    path: "src/a.rs".into(),
                    abs_path: PathBuf::from("/src/api/src/a.rs"),
                    language: Some("Rust".into()),
                    matched_lines: 5,
                    lines: vec![
                        line(1, "fn a", true),
                        line(2, "  x", false),
                        line(7, "fn b", true),
                    ],
                },
                FileHit {
                    repo: "notes".into(),
                    branch: None,
                    path: "b.md".into(),
                    abs_path: PathBuf::from("/notes/b.md"),
                    language: None,
                    matched_lines: 2,
                    lines: vec![line(3, "fn", true), line(9, "fn", true)],
                },
            ],
            summary: SearchSummary {
                matched_lines: 1234,
                files: 40,
                shown_lines: 3,
                shown_files: 2,
                searched_files: 50,
                corpus_files: 9000,
                repos: 2,
                elapsed_ms: 12.4,
                candidates_ms: 1.5,
                bytes_read: 2048,
                ..Default::default()
            },
            facets: vec![
                FacetCounts {
                    qualifier: "repo".into(),
                    values: vec![("api".into(), 30), ("notes".into(), 10)],
                },
                FacetCounts {
                    qualifier: "language".into(),
                    values: vec![("Visual Basic".into(), 20), ("Rust".into(), 12)],
                },
                FacetCounts {
                    qualifier: "path".into(),
                    values: vec![("src".into(), 25), ("docs".into(), 5)],
                },
            ],
            table: None,
            notes: vec!["notes: no index yet, so its files were scanned".into()],
        }
    }

    #[test]
    fn text_groups_lines_under_their_file() {
        let text = search_text(&response(), Style::default());
        let path = |path: &str| PathBuf::from(path).display().to_string();
        assert_eq!(
            text,
            format!(
                "{}  [api@main]\n1:fn a\n2-  x\n--\n7:fn b\n(+3 more in this file)\n\n{}  [notes]\n3:fn\n9:fn\n",
                path("/src/api/src/a.rs"),
                path("/notes/b.md")
            )
        );
    }

    #[test]
    fn colour_marks_the_matched_text() {
        let text = search_text(&response(), Style { color: true });
        assert!(text.contains("\x1b[1;31mfn\x1b[0m a"), "{text:?}");
    }

    #[test]
    fn the_footer_counts_and_suggests_how_to_narrow() {
        let footer = search_footer(&response(), true, Style::default());
        let lines: Vec<&str> = footer.lines().collect();
        assert_eq!(
            lines[0],
            "3 of 1,234 matching lines in 2 of 40 files · 2 repositories · 12 ms"
        );
        assert_eq!(
            lines[1],
            "narrow with: repo:api (30)  repo:notes (10)  language:\"Visual Basic\" (20)  language:Rust (12)  path:src/** (25)  path:docs/** (5)"
        );
        assert!(
            lines[2].starts_with("stats: read 50 of 9,000 files (2.0 KB) after 1.5 ms"),
            "{}",
            lines[2]
        );
        assert_eq!(
            lines[3],
            "note: notes: no index yet, so its files were scanned"
        );
    }

    #[test]
    fn nothing_to_narrow_when_everything_is_shown() {
        let mut all = response();
        all.summary.matched_lines = 3;
        all.summary.files = 2;
        assert!(narrowing(&all).is_empty());
        let footer = search_footer(&all, false, Style::default());
        assert!(
            footer.starts_with("3 matching lines in 2 files · 2 repositories"),
            "{footer}"
        );
    }

    #[test]
    fn file_names_alone_count_as_showing_their_lines() {
        let mut names = response();
        names.summary.files = 2;
        names.summary.shown_lines = 0;
        for file in &mut names.files {
            file.lines.clear();
        }
        let footer = search_footer(&names, false, Style::default());
        assert!(
            footer.starts_with("1,234 matching lines in 2 files"),
            "{footer}"
        );
        assert!(narrowing(&names).is_empty());
    }

    #[test]
    fn suggested_qualifiers_are_queries_that_narrow() {
        let repo = RepoInfo {
            id: "api".into(),
            name: "api".into(),
            root: PathBuf::from("/src/api"),
            branch: Some("main".into()),
            tags: vec![],
            pull_every: None,
        };
        let verdict = |qualifier: &str, path: &str| {
            let query = SearchQuery {
                pattern: format!("x {qualifier}"),
                ..Default::default()
            };
            CompiledQuery::new(&query)
                .unwrap_or_else(|error| panic!("{qualifier}: {error}"))
                .verdict(&repo, Some(path))
        };
        assert_ne!(
            verdict(&qualifier("path", "src"), "src/engine/a.rs"),
            Some(false)
        );
        assert_eq!(
            verdict(&qualifier("path", "src"), "lib/src/a.rs"),
            Some(false)
        );
        assert_ne!(
            verdict(&qualifier("language", "Visual Basic"), "a.vb"),
            Some(false)
        );
        assert_eq!(
            verdict(&qualifier("language", "Visual Basic"), "a.rs"),
            Some(false)
        );
        assert_ne!(verdict(&qualifier("repo", "api"), "a.rs"), Some(false));
        assert_eq!(verdict(&qualifier("repo", "web"), "a.rs"), Some(false));
    }

    #[test]
    fn json_is_one_object_per_line_ending_with_the_summary() {
        let json = search_json(&response());
        let records: Vec<serde_json::Value> = json
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["type"], "file");
        assert_eq!(records[0]["repo"], "api");
        assert_eq!(records[0]["lines"][0]["match"], true);
        assert_eq!(records[2]["type"], "summary");
        assert_eq!(records[2]["matched_lines"], 1234);
        assert_eq!(records[2]["narrow"][0], "repo:api (30)");
    }

    #[test]
    fn lists_and_counts_use_absolute_paths() {
        let path = PathBuf::from("/src/api/src/a.rs").display().to_string();
        assert!(file_list(&response()).starts_with(&format!("{path}\n")));
        assert!(file_counts(&response()).starts_with(&format!("{path}:5\n")));
    }

    #[test]
    fn metrics_read_as_a_few_lines() {
        use crate::diagnostics::metrics::{Origin, SearchRecord};
        let metrics = MetricsSnapshot {
            uptime_ms: 125_000,
            memory_bytes: Some(200 * 1024 * 1024),
            cli_searches: TimingSummary {
                count: 3,
                mean_ms: 10.0,
                p50_ms: 8.0,
                p95_ms: 20.0,
                max_ms: 1500.0,
            },
            index_savings: 0.973,
            log_counts: [1, 2, 3, 0, 0],
            requests: [("search".to_string(), 3)].into(),
            recent_searches: vec![SearchRecord {
                time_ms: 1_790_410_542_123,
                origin: Origin::Cli,
                query: "parse".into(),
                repos: 1,
                corpus_files: 9000,
                searched_files: 12,
                files: 2,
                matched_lines: 5,
                truncated: false,
                elapsed_ms: 8.25,
                candidates_ms: 1.0,
                bytes_read: 100,
            }],
            ..Default::default()
        };
        let text = metrics_text(&metrics);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "up 2m · memory 200.0 MB");
        assert_eq!(
            lines[2],
            "cli searches          3  p50    8.0 ms  p95   20.0 ms  max     1.5 s"
        );
        assert_eq!(lines[6], "the indexes spared 97.3% of file reads");
        assert_eq!(lines[7], "log: 1 error, 2 warnings");
        assert_eq!(lines[8], "requests: search 3");
        assert_eq!(
            lines[10],
            "  08:15:42  cli       8.2 ms  read 12/9,000  5 lines in 2 files  \"parse\""
        );
    }

    #[test]
    fn numbers_read_easily() {
        assert_eq!(number(0), "0");
        assert_eq!(number(999), "999");
        assert_eq!(number(1_234_567), "1,234,567");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(3 * 1024 * 1024), "3.0 MB");
        assert_eq!(duration(90_000), "1m");
        assert_eq!(duration(3_900_000), "1h 5m");
    }

    #[test]
    fn repositories_line_up_in_columns() {
        let repos = vec![RepoStatus {
            name: "api".into(),
            path: PathBuf::from("/src/api"),
            branch: Some("main".into()),
            tags: vec!["dev".into(), "owner:alice".into()],
            index: "ready".into(),
            error: None,
            files: Some(12_000),
            indexed_at_ms: Some(1_000),
            changed_files: 3,
        }];
        let text = repos_text(&repos, 121_000);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("NAME  BRANCH  INDEX"), "{}", lines[0]);
        assert!(
            lines[1]
                .starts_with("api   main    ready +3 changed  12,000  2m ago   dev owner:alice  "),
            "{}",
            lines[1]
        );
    }
}
