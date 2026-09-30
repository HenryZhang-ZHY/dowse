//! Source text and query match ranges for the read-only file preview.

use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use regex::Regex;
use tgrep_core::encoding::{self, EncodingMode};

use super::search::SearchLimits;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePreview {
    pub text: Arc<str>,
    /// UTF-8 byte offsets of logical lines, including a trailing empty line.
    pub line_starts: Vec<usize>,
    /// 0-based indexes of the lines the query matches, in order.
    pub matches: Vec<usize>,
    /// Query matches in the original source's UTF-8 byte coordinates.
    pub highlights: Vec<Range<usize>>,
}

impl Default for FilePreview {
    fn default() -> Self {
        Self {
            text: Arc::from(""),
            line_starts: vec![0],
            matches: Vec::new(),
            highlights: Vec::new(),
        }
    }
}

impl FilePreview {
    pub fn with_matches(&self, matcher: Option<&Regex>) -> Self {
        let mut preview = Self {
            text: self.text.clone(),
            line_starts: self.line_starts.clone(),
            ..Default::default()
        };
        preview.mark_matches(matcher);
        preview
    }

    fn mark_matches(&mut self, matcher: Option<&Regex>) {
        if let Some(matcher) = matcher {
            for (index, &start) in self.line_starts.iter().enumerate() {
                if start == self.text.len() {
                    break;
                }
                let end = self
                    .line_starts
                    .get(index + 1)
                    .copied()
                    .unwrap_or(self.text.len());
                let line = &self.text[start..end];
                let line = line.strip_suffix('\n').unwrap_or(line);
                let line = line.strip_suffix('\r').unwrap_or(line);
                let first = self.highlights.len();
                self.highlights.extend(
                    matcher
                        .find_iter(line)
                        .filter(|m| !m.is_empty())
                        .map(|m| start + m.start()..start + m.end()),
                );
                if self.highlights.len() != first || matcher.is_match(line) {
                    self.matches.push(index);
                }
            }
        }
    }

    pub fn line_offset(&self, line: usize) -> usize {
        self.line_starts[line.saturating_sub(1).min(self.line_starts.len() - 1)]
    }

    /// The first match after the 1-based `line`, as a 1-based line.
    pub fn next_match(&self, line: usize) -> Option<usize> {
        let index = self.matches.partition_point(|&m| m < line);
        self.matches.get(index).map(|m| m + 1)
    }

    /// The last match before the 1-based `line`, as a 1-based line.
    pub fn previous_match(&self, line: usize) -> Option<usize> {
        let index = self.matches.partition_point(|&m| m + 1 < line);
        index.checked_sub(1).map(|i| self.matches[i] + 1)
    }

    /// Which match `line` is, 1-based, when it is one.
    pub fn match_ordinal(&self, line: usize) -> Option<usize> {
        let index = line.checked_sub(1)?;
        self.matches.binary_search(&index).ok().map(|i| i + 1)
    }
}

/// Read `path` and mark where `matcher` matches, line by line.
pub fn load(path: &Path, matcher: Option<&Regex>) -> Result<FilePreview, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    let limit = SearchLimits::default().max_file_size;
    if metadata.len() > limit {
        return Err(format!(
            "The file is {} MB, too large to preview.",
            metadata.len() / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let (text, _) = encoding::decode_owned_with_fixups(bytes, EncodingMode::Auto);
    if memchr::memchr(0, text.as_bytes()).is_some() {
        return Err("This looks like a binary file.".into());
    }
    Ok(prepare(&text, matcher))
}

fn prepare(text: &str, matcher: Option<&Regex>) -> FilePreview {
    let line_starts = std::iter::once(0)
        .chain(memchr::memchr_iter(b'\n', text.as_bytes()).map(|offset| offset + 1))
        .collect();
    let mut preview = FilePreview {
        text: Arc::from(text),
        line_starts,
        ..Default::default()
    };
    preview.mark_matches(matcher);
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_matches_without_changing_source_text() {
        let matcher = Regex::new("foo").unwrap();
        let text = "a\r\n\tfoo foo\r\nbar\nwide line here\n";
        let preview = prepare(text, Some(&matcher));
        assert_eq!(preview.text.as_ref(), text);
        assert_eq!(preview.line_starts, vec![0, 3, 13, 17, 32]);
        assert_eq!(preview.matches, vec![1]);
        assert_eq!(preview.highlights, vec![4..7, 8..11]);
    }

    #[test]
    fn match_ranges_use_original_utf8_offsets() {
        let text = "\u{4e2d}\tfoo  \r\n\u{1f980}foo\t \n";
        let preview = prepare(text, Some(&Regex::new("foo").unwrap()));
        assert_eq!(preview.text.as_ref(), text);
        assert_eq!(preview.highlights, vec![4..7, 15..18]);
        assert_eq!(preview.matches, vec![0, 1]);
        for range in &preview.highlights {
            assert_eq!(&preview.text[range.clone()], "foo");
        }
    }

    #[test]
    fn preserves_long_lines_and_more_than_ten_thousand_lines() {
        let text = format!("{}\n{}foo\n", "x\n".repeat(10_001), "a".repeat(20_000));
        let preview = prepare(&text, Some(&Regex::new("foo").unwrap()));
        assert_eq!(preview.text.as_ref(), text);
        assert_eq!(preview.line_starts.len(), 10_004);
        assert_eq!(preview.matches, vec![10_002]);
        assert_eq!(&preview.text[preview.highlights[0].clone()], "foo");
    }

    #[test]
    fn refreshing_matches_reuses_source_and_clears_old_ranges() {
        let preview = prepare("\tfoo  \r\nbar\n", Some(&Regex::new("foo").unwrap()));
        let refreshed = preview.with_matches(Some(&Regex::new("bar").unwrap()));
        assert!(std::sync::Arc::ptr_eq(&preview.text, &refreshed.text));
        assert_eq!(preview.matches, vec![0]);
        assert_eq!(refreshed.matches, vec![1]);
        assert_eq!(refreshed.highlights, vec![8..11]);
        let cleared = refreshed.with_matches(None);
        assert!(cleared.matches.is_empty());
        assert!(cleared.highlights.is_empty());
    }

    #[test]
    fn line_offsets_include_empty_and_trailing_lines() {
        assert_eq!(prepare("", None).line_starts, vec![0]);
        assert_eq!(prepare("a\n\n", None).line_starts, vec![0, 2, 3]);
        assert_eq!(prepare("a", None).line_starts, vec![0]);
        assert_eq!(FilePreview::default().line_offset(1), 0);
        let preview = prepare("a\nb", None);
        assert_eq!(preview.line_offset(0), 0);
        assert_eq!(preview.line_offset(1), 0);
        assert_eq!(preview.line_offset(2), 2);
        assert_eq!(preview.line_offset(usize::MAX), 2);
    }

    #[test]
    fn zero_width_matches_navigate_without_empty_decorations() {
        let preview = prepare("foo\r\n\nbar\n", Some(&Regex::new("^foo$").unwrap()));
        assert_eq!(preview.matches, vec![0]);
        let preview = preview.with_matches(Some(&Regex::new("^$").unwrap()));
        assert_eq!(preview.matches, vec![1]);
        assert!(preview.highlights.is_empty());
        assert!(
            prepare("", Some(&Regex::new("^$").unwrap()))
                .matches
                .is_empty()
        );
    }

    #[test]
    fn steps_between_matches() {
        let matcher = Regex::new("x").unwrap();
        let preview = prepare("x\n-\nx\n-\nx", Some(&matcher));
        assert_eq!(preview.next_match(1), Some(3));
        assert_eq!(preview.next_match(2), Some(3));
        assert_eq!(preview.next_match(5), None);
        assert_eq!(preview.previous_match(5), Some(3));
        assert_eq!(preview.previous_match(1), None);
        assert_eq!(preview.match_ordinal(3), Some(2));
        assert_eq!(preview.match_ordinal(2), None);
    }

    #[test]
    fn loads_files_and_refuses_binaries() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("a.rs");
        std::fs::write(&text, "fn main() {}\n").unwrap();
        let preview = load(&text, None).unwrap();
        assert_eq!(preview.text.as_ref(), "fn main() {}\n");
        assert_eq!(preview.line_starts.len(), 2);
        assert!(preview.matches.is_empty());

        let binary = dir.path().join("b.bin");
        std::fs::write(&binary, b"a\0b").unwrap();
        assert!(load(&binary, None).is_err());
        assert!(load(&dir.path().join("missing"), None).is_err());
    }
}
