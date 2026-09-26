//! Running a compiled query over the corpora of several repositories and
//! shaping the hits into snippets.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use regex::Regex;
use tgrep_core::encoding::{self, EncodingMode};

use super::index::Corpus;
use super::language;
use super::query::CompiledQuery;
use super::repo::RepoInfo;

/// Files searched in parallel between checks of the result limit.
const SEARCH_CHUNK: usize = 512;

/// Bounds that keep a broad query from flooding the UI.
#[derive(Clone, Debug)]
pub struct SearchLimits {
    /// Lines shown around each matching line.
    pub context_lines: usize,
    /// Matching lines kept per file; the rest are only counted.
    pub max_lines_per_file: usize,
    /// Once about this many matching lines are found, remaining files are skipped.
    pub max_total_lines: usize,
    /// Longer lines are clipped around their first match.
    pub max_line_len: usize,
    /// Files above this size are skipped, like tgrep's default `--max-filesize`.
    pub max_file_size: u64,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            context_lines: 1,
            max_lines_per_file: 200,
            max_total_lines: 20_000,
            max_line_len: 400,
            max_file_size: tgrep_core::walker::DEFAULT_MAX_FILE_SIZE.unwrap_or(u64::MAX),
        }
    }
}

/// One line of a snippet, ready to display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetLine {
    /// 1-based line number in the file.
    pub number: usize,
    /// The line with tabs expanded, clipped to [`SearchLimits::max_line_len`].
    pub text: String,
    /// Byte ranges of `text` to highlight. Empty for context lines.
    pub highlights: Vec<Range<usize>>,
    pub is_match: bool,
}

/// A run of consecutive lines: matches plus their context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    pub lines: Vec<SnippetLine>,
}

#[derive(Clone, Debug)]
pub struct FileMatch {
    pub repo: Arc<RepoInfo>,
    /// Relative to the repository root, `/`-separated.
    pub path: String,
    pub language: Option<&'static str>,
    /// Every matching line in the file, including ones not kept in `snippets`.
    pub matched_lines: usize,
    pub snippets: Vec<Snippet>,
}

impl FileMatch {
    pub fn first_match_line(&self) -> Option<usize> {
        self.snippets
            .iter()
            .flat_map(|snippet| &snippet.lines)
            .find(|line| line.is_match)
            .map(|line| line.number)
    }

    /// Matching lines kept in `snippets`.
    pub fn kept_lines(&self) -> usize {
        self.snippets
            .iter()
            .flat_map(|snippet| &snippet.lines)
            .filter(|line| line.is_match)
            .count()
    }
}

#[derive(Clone, Debug, Default)]
pub struct SearchOutcome {
    /// In source order, then by path.
    pub files: Vec<FileMatch>,
    pub matched_lines: usize,
    /// Files read after index narrowing and path filtering.
    pub searched_files: usize,
    /// Files in every searched corpus.
    pub corpus_files: usize,
    /// Repositories searched.
    pub repos: usize,
    /// Repositories without a usable index, whose folders were scanned.
    pub unindexed_repos: usize,
    /// The result limit was hit, so some files were not searched.
    pub truncated: bool,
    pub cancelled: bool,
    pub elapsed: Duration,
}

/// One repository to search: its files, and those changed since its index
/// was built (see [`super::watch::ChangeTracker`]). Changed files are read
/// regardless of what the index says, so edits are found before a rebuild.
#[derive(Clone)]
pub struct SearchSource {
    pub repo: Arc<RepoInfo>,
    pub corpus: Arc<Corpus>,
    pub changed: Vec<String>,
}

/// Search every source for `query`, checking `cancel` between files.
pub fn search(
    sources: &[SearchSource],
    query: &CompiledQuery,
    limits: &SearchLimits,
    cancel: &AtomicBool,
) -> SearchOutcome {
    let started = Instant::now();
    let mut candidates: Vec<(usize, String)> = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let mut paths = source.corpus.candidates(&query.plan);
        if !source.changed.is_empty() {
            paths.extend(source.changed.iter().cloned());
            paths.sort();
            paths.dedup();
        }
        paths.retain(|path| query.path_filter.matches(path));
        candidates.extend(paths.into_iter().map(|path| (index, path)));
    }

    // Files are searched in ordered chunks, each in parallel, so a truncated
    // result is always the first files in order rather than whichever threads
    // happened to finish first.
    let mut files: Vec<FileMatch> = Vec::new();
    let mut total_lines = 0;
    let mut truncated = false;
    let mut chunks = candidates.chunks(SEARCH_CHUNK).peekable();
    while let Some(chunk) = chunks.next() {
        let found: Vec<FileMatch> = chunk
            .par_iter()
            .filter_map(|(index, path)| {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let source = &sources[*index];
                let text = read_text(&source.corpus.full_path(path), limits.max_file_size)?;
                match_text(&source.repo, path, &text, &query.matcher, limits)
            })
            .collect();
        total_lines += found.iter().map(|file| file.matched_lines).sum::<usize>();
        files.extend(found);
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if total_lines >= limits.max_total_lines && chunks.peek().is_some() {
            truncated = true;
            break;
        }
    }

    SearchOutcome {
        matched_lines: files.iter().map(|file| file.matched_lines).sum(),
        files,
        searched_files: candidates.len(),
        corpus_files: sources
            .iter()
            .map(|source| source.corpus.file_count())
            .sum(),
        repos: sources.len(),
        unindexed_repos: sources
            .iter()
            .filter(|source| !source.corpus.is_indexed())
            .count(),
        truncated,
        cancelled: cancel.load(Ordering::Relaxed),
        elapsed: started.elapsed(),
    }
}

/// Read a file as text the way tgrep does: BOM sniffing, lossy UTF-8. Files
/// containing NUL are treated as binary and skipped.
fn read_text(path: &std::path::Path, max_file_size: u64) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > max_file_size {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let (text, _) = encoding::decode_owned_with_fixups(bytes, EncodingMode::Auto);
    memchr::memchr(0, text.as_bytes()).is_none().then_some(text)
}

/// A matching line found while scanning, before it is shaped for display.
struct Hit {
    /// 0-based line index.
    index: usize,
    start: usize,
    end: usize,
    ranges: Vec<Range<usize>>,
}

/// Find matching lines in `text` and build its snippets, or `None` when
/// nothing matches.
pub fn match_text(
    repo: &Arc<RepoInfo>,
    path: &str,
    text: &str,
    matcher: &Regex,
    limits: &SearchLimits,
) -> Option<FileMatch> {
    let bytes = text.as_bytes();
    let mut hits = Vec::new();
    let mut matched_lines = 0;
    let mut position = 0;
    let mut line_index = 0;
    let mut counted_to = 0;

    while position <= bytes.len() {
        let Some(found) = matcher.find_at(text, position) else {
            break;
        };
        let start = found.start();
        // An empty match after a trailing newline is not on a real line.
        if start == bytes.len() && (bytes.is_empty() || bytes[start - 1] == b'\n') {
            break;
        }
        let line_start =
            memchr::memrchr(b'\n', &bytes[position..start]).map_or(position, |i| position + i + 1);
        let line_end = memchr::memchr(b'\n', &bytes[start..]).map_or(bytes.len(), |i| start + i);
        line_index += memchr::memchr_iter(b'\n', &bytes[counted_to..line_start]).count();
        counted_to = line_start;

        let content_end = trim_carriage_return(bytes, line_start, line_end);
        let line = &text[line_start..content_end];
        // The whole-text match may have crossed a newline (`\s`, `[^x]`), so
        // confirm the line matches on its own.
        let ranges: Vec<Range<usize>> = matcher
            .find_iter(line)
            .filter(|m| !m.is_empty())
            .map(|m| m.range())
            .collect();
        if !ranges.is_empty() || matcher.is_match(line) {
            matched_lines += 1;
            if hits.len() < limits.max_lines_per_file {
                hits.push(Hit {
                    index: line_index,
                    start: line_start,
                    end: content_end,
                    ranges,
                });
            }
        }
        if line_end >= bytes.len() {
            break;
        }
        position = line_end + 1;
    }

    if matched_lines == 0 {
        return None;
    }
    Some(FileMatch {
        repo: repo.clone(),
        path: path.to_string(),
        language: language::detect(path),
        matched_lines,
        snippets: build_snippets(text, &hits, limits),
    })
}

fn trim_carriage_return(bytes: &[u8], start: usize, end: usize) -> usize {
    if end > start && bytes[end - 1] == b'\r' {
        end - 1
    } else {
        end
    }
}

/// A line's byte range in the file, with its highlights when it matched.
type LineSpan<'a> = (usize, usize, Option<&'a [Range<usize>]>);

/// Merge matching lines and their context into runs of consecutive lines.
fn build_snippets(text: &str, hits: &[Hit], limits: &SearchLimits) -> Vec<Snippet> {
    let bytes = text.as_bytes();
    // line index -> (start, end, highlight ranges if matching)
    let mut lines: BTreeMap<usize, LineSpan> = BTreeMap::new();
    for hit in hits {
        let (mut start, mut index) = (hit.start, hit.index);
        for _ in 0..limits.context_lines {
            if start == 0 {
                break;
            }
            let end = start - 1;
            start = memchr::memrchr(b'\n', &bytes[..end]).map_or(0, |i| i + 1);
            index -= 1;
            lines
                .entry(index)
                .or_insert((start, trim_carriage_return(bytes, start, end), None));
        }
        lines.insert(hit.index, (hit.start, hit.end, Some(&hit.ranges)));

        let (mut end, mut index) = (hit.end, hit.index);
        for _ in 0..limits.context_lines {
            let Some(newline) = memchr::memchr(b'\n', &bytes[end..]).map(|i| end + i) else {
                break;
            };
            let start = newline + 1;
            if start >= bytes.len() {
                break;
            }
            end = memchr::memchr(b'\n', &bytes[start..]).map_or(bytes.len(), |i| start + i);
            index += 1;
            let content_end = trim_carriage_return(bytes, start, end);
            lines.entry(index).or_insert((start, content_end, None));
            end = content_end;
        }
    }

    let mut snippets: Vec<Snippet> = Vec::new();
    let mut previous: Option<usize> = None;
    for (index, (start, end, ranges)) in lines {
        let (display, highlights) = display_line(
            &text[start..end],
            ranges.unwrap_or(&[]),
            limits.max_line_len,
        );
        let line = SnippetLine {
            number: index + 1,
            text: display,
            highlights,
            is_match: ranges.is_some(),
        };
        match (previous, snippets.last_mut()) {
            (Some(prev), Some(snippet)) if prev + 1 == index => snippet.lines.push(line),
            _ => snippets.push(Snippet { lines: vec![line] }),
        }
        previous = Some(index);
    }
    snippets
}

const TAB_WIDTH: usize = 4;
const ELLIPSIS: &str = "…";

/// Expand tabs and clip long lines around the first highlight, carrying the
/// highlight ranges along.
pub(super) fn display_line(
    line: &str,
    ranges: &[Range<usize>],
    max_len: usize,
) -> (String, Vec<Range<usize>>) {
    let (mut window_start, mut window_end) = (0, line.len());
    if line.len() > max_len {
        let anchor = ranges.first().map_or(0, |range| range.start);
        window_start = floor_char_boundary(line, anchor.saturating_sub(max_len / 4));
        window_end = floor_char_boundary(line, (window_start + max_len).min(line.len()));
    }

    let mut output = String::with_capacity(window_end - window_start + 8);
    if window_start > 0 {
        output.push_str(ELLIPSIS);
    }
    // Source byte offset -> output byte offset, for every char boundary.
    let mut offsets = Vec::with_capacity(window_end - window_start + 1);
    let mut column = 0;
    for (offset, ch) in line[window_start..window_end].char_indices() {
        let source = window_start + offset;
        offsets.push((source, output.len()));
        if ch == '\t' {
            let spaces = TAB_WIDTH - column % TAB_WIDTH;
            output.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            output.push(ch);
            column += 1;
        }
    }
    offsets.push((window_end, output.len()));
    if window_end < line.len() {
        output.push_str(ELLIPSIS);
    }

    let map = |source: usize| -> usize {
        match offsets.binary_search_by_key(&source, |&(s, _)| s) {
            Ok(i) => offsets[i].1,
            Err(i) => offsets[i.min(offsets.len() - 1)].1,
        }
    };
    let highlights = ranges
        .iter()
        .filter(|range| range.end > window_start && range.start < window_end)
        .map(|range| map(range.start.max(window_start))..map(range.end.min(window_end)))
        .filter(|range| !range.is_empty())
        .collect();
    (output, highlights)
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
// Highlight ranges are ranges, not a request to collect them.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use crate::engine::index::RepoIndex;
    use crate::engine::query::SearchQuery;

    fn limits() -> SearchLimits {
        SearchLimits::default()
    }

    fn repo_at(name: &str, root: &std::path::Path) -> Arc<RepoInfo> {
        Arc::new(RepoInfo {
            id: name.into(),
            name: name.into(),
            root: root.to_path_buf(),
            branch: None,
            tags: vec![],
        })
    }

    fn repo() -> Arc<RepoInfo> {
        repo_at("repo", std::path::Path::new("."))
    }

    /// Index `root` and wrap it as a search source.
    fn indexed_source(name: &str, root: &std::path::Path) -> SearchSource {
        let index = RepoIndex::open(root).unwrap();
        index.build_index().unwrap().publish().unwrap();
        let corpus = index.load_corpus();
        assert!(corpus.is_indexed());
        SearchSource {
            repo: repo_at(name, root),
            corpus: Arc::new(corpus),
            changed: vec![],
        }
    }

    fn compile(pattern: &str) -> CompiledQuery {
        CompiledQuery::new(&SearchQuery {
            pattern: pattern.into(),
            ..Default::default()
        })
        .unwrap()
    }

    fn matcher(pattern: &str) -> Regex {
        regex::RegexBuilder::new(pattern)
            .multi_line(true)
            .build()
            .unwrap()
    }

    fn lines(file: &FileMatch) -> Vec<(usize, &str, bool)> {
        file.snippets
            .iter()
            .flat_map(|s| &s.lines)
            .map(|l| (l.number, l.text.as_str(), l.is_match))
            .collect()
    }

    #[test]
    fn finds_lines_with_context_and_merges_adjacent_runs() {
        let text = "a\nb\nneedle 1\nc\nneedle 2\nd\ne\nf\nneedle 3\n";
        let file = match_text(&repo(), "x.txt", text, &matcher("needle"), &limits()).unwrap();
        assert_eq!(file.matched_lines, 3);
        assert_eq!(file.snippets.len(), 2);
        assert_eq!(
            lines(&file),
            vec![
                (2, "b", false),
                (3, "needle 1", true),
                (4, "c", false),
                (5, "needle 2", true),
                (6, "d", false),
                (8, "f", false),
                (9, "needle 3", true),
            ]
        );
        let first = &file.snippets[0].lines[1];
        assert_eq!(first.highlights, vec![0..6]);
    }

    #[test]
    fn handles_crlf_and_anchors() {
        let text = "fn a() {}\r\n  fn b() {}\r\nfn c() {}";
        let file = match_text(&repo(), "x.rs", text, &matcher("^fn"), &limits()).unwrap();
        let matched: Vec<usize> = lines(&file).iter().filter(|l| l.2).map(|l| l.0).collect();
        assert_eq!(matched, vec![1, 3]);
        assert!(lines(&file).iter().all(|l| !l.1.ends_with('\r')));
    }

    #[test]
    fn a_match_spanning_lines_is_not_reported() {
        let text = "foo\nbar\n";
        assert!(match_text(&repo(), "x", text, &matcher(r"foo\sbar"), &limits()).is_none());
    }

    #[test]
    fn multiple_matches_on_a_line_are_all_highlighted() {
        let file = match_text(&repo(), "x", "ab ab ab\n", &matcher("ab"), &limits()).unwrap();
        assert_eq!(file.snippets[0].lines[0].highlights, vec![0..2, 3..5, 6..8]);
    }

    #[test]
    fn caps_kept_lines_but_counts_all() {
        let text = "hit\n".repeat(10);
        let limits = SearchLimits {
            max_lines_per_file: 3,
            context_lines: 0,
            ..limits()
        };
        let file = match_text(&repo(), "x", &text, &matcher("hit"), &limits).unwrap();
        assert_eq!(file.matched_lines, 10);
        assert_eq!(file.kept_lines(), 3);
    }

    #[test]
    fn expands_tabs_and_shifts_highlights() {
        let (text, highlights) = display_line("\tx = 1", &[1..2], 400);
        assert_eq!(text, "    x = 1");
        assert_eq!(highlights, vec![4..5]);
    }

    #[test]
    fn clips_long_lines_around_the_match() {
        let line = format!("{}needle{}", "a".repeat(1000), "b".repeat(1000));
        let (text, highlights) = display_line(&line, &[1000..1006], 100);
        assert!(text.starts_with(ELLIPSIS) && text.ends_with(ELLIPSIS));
        assert_eq!(&text[highlights[0].clone()], "needle");
    }

    #[test]
    fn clipping_respects_multibyte_characters() {
        let line = "é".repeat(500) + "needle";
        let (text, highlights) = display_line(&line, &[1000..1006], 50);
        assert_eq!(&text[highlights[0].clone()], "needle");
    }

    #[test]
    fn end_to_end_search_over_an_indexed_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "fn Parse() {}\nfn parse_all() {}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("notes.md"), "parse later\n").unwrap();
        let sources = [indexed_source("repo", dir.path())];

        let run = |query: SearchQuery| {
            let compiled = CompiledQuery::new(&query).unwrap();
            search(&sources, &compiled, &limits(), &AtomicBool::new(false))
        };
        let base = SearchQuery {
            pattern: "parse".into(),
            ..Default::default()
        };

        let outcome = run(base.clone());
        assert_eq!(outcome.files.len(), 2);
        assert_eq!(outcome.matched_lines, 3);

        let outcome = run(SearchQuery {
            case_sensitive: true,
            whole_word: true,
            ..base.clone()
        });
        assert_eq!(outcome.files.len(), 1);
        assert_eq!(outcome.files[0].path, "notes.md");

        let outcome = run(SearchQuery {
            path_filter: "*.rs".into(),
            ..base.clone()
        });
        assert_eq!(outcome.files.len(), 1);
        assert_eq!(outcome.files[0].matched_lines, 2);

        let outcome = run(SearchQuery {
            pattern: r"fn \w+\(\)".into(),
            regex: true,
            ..base
        });
        assert_eq!(outcome.matched_lines, 2);
    }

    #[test]
    fn changed_files_are_searched_even_when_the_index_predates_them() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.rs"), "fn old() {}\n").unwrap();
        let mut source = indexed_source("repo", dir.path());
        std::fs::write(dir.path().join("new.rs"), "fn brand_new() {}\n").unwrap();

        let compiled = compile("brand_new");
        let run = |source: &SearchSource| {
            search(
                std::slice::from_ref(source),
                &compiled,
                &limits(),
                &AtomicBool::new(false),
            )
        };
        assert!(run(&source).files.is_empty());
        source.changed = vec!["new.rs".into()];
        assert_eq!(run(&source).files.len(), 1);
        // A changed file that was deleted is skipped quietly.
        source.changed = vec!["gone.rs".into()];
        assert!(run(&source).files.is_empty());
    }

    #[test]
    fn searches_across_repositories_in_source_order() {
        let (main, dev) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(main.path().join("auth.rs"), "fn login() {}\n").unwrap();
        std::fs::write(
            dev.path().join("auth.rs"),
            "fn login() {}\nfn login_v2() {}\n",
        )
        .unwrap();
        std::fs::write(dev.path().join("other.rs"), "nothing here\n").unwrap();
        let sources = [
            indexed_source("api", main.path()),
            indexed_source("api-dev", dev.path()),
        ];

        let outcome = search(
            &sources,
            &compile("login"),
            &limits(),
            &AtomicBool::new(false),
        );
        let found: Vec<(&str, &str, usize)> = outcome
            .files
            .iter()
            .map(|file| {
                (
                    file.repo.name.as_str(),
                    file.path.as_str(),
                    file.matched_lines,
                )
            })
            .collect();
        assert_eq!(
            found,
            vec![("api", "auth.rs", 1), ("api-dev", "auth.rs", 2)]
        );
        assert_eq!(outcome.repos, 2);
        assert_eq!(outcome.unindexed_repos, 0);
        assert_eq!(outcome.corpus_files, 3);
        // The index narrowed each repository to the one file mentioning it.
        assert_eq!(outcome.searched_files, 2);
    }
}
