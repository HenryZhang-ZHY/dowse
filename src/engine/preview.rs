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
    /// UTF-8 byte offsets of every query match, including zero-width matches.
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
                for found in matcher.find_iter(line) {
                    let range = start + found.start()..start + found.end();
                    self.matches.push(range.start);
                    if !range.is_empty() {
                        self.highlights.push(range);
                    }
                }
            }
        }
    }

    pub fn line_offset(&self, line: usize) -> usize {
        self.line_starts[line.saturating_sub(1).min(self.line_starts.len() - 1)]
    }

    /// The 1-based logical line containing a UTF-8 byte offset.
    pub fn line_at_offset(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|&start| start <= offset)
    }

    /// The first match on a line, or its start when the line has no match.
    pub fn match_offset(&self, line: usize) -> usize {
        let start = self.line_offset(line);
        let index = self.matches.partition_point(|&offset| offset < start);
        self.matches
            .get(index)
            .copied()
            .filter(|&offset| self.line_at_offset(offset) == self.line_at_offset(start))
            .unwrap_or(start)
    }

    /// The next occurrence's UTF-8 offset, excluding the current offset.
    pub fn next_match(&self, offset: usize) -> Option<usize> {
        let index = self.matches.partition_point(|&start| start <= offset);
        self.matches.get(index).copied()
    }

    /// The preceding occurrence's UTF-8 offset, excluding the current offset.
    pub fn previous_match(&self, offset: usize) -> Option<usize> {
        let index = self.matches.partition_point(|&start| start < offset);
        index.checked_sub(1).map(|i| self.matches[i])
    }

    /// Which occurrence `offset` starts, 1-based.
    pub fn match_ordinal(&self, offset: usize) -> Option<usize> {
        self.matches.binary_search(&offset).ok().map(|i| i + 1)
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
        assert_eq!(preview.matches, vec![4, 8]);
        assert_eq!(preview.highlights, vec![4..7, 8..11]);
    }

    #[test]
    fn match_ranges_use_original_utf8_offsets() {
        let text = "\u{4e2d}\tfoo  \r\n\u{1f980}foo\t \n";
        let preview = prepare(text, Some(&Regex::new("foo").unwrap()));
        assert_eq!(preview.text.as_ref(), text);
        assert_eq!(preview.highlights, vec![4..7, 15..18]);
        assert_eq!(preview.matches, vec![4, 15]);
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
        assert_eq!(preview.matches, vec![text.len() - 4]);
        assert_eq!(&preview.text[preview.highlights[0].clone()], "foo");
    }

    #[test]
    fn refreshing_matches_reuses_source_and_clears_old_ranges() {
        let preview = prepare("\tfoo  \r\nbar\n", Some(&Regex::new("foo").unwrap()));
        let refreshed = preview.with_matches(Some(&Regex::new("bar").unwrap()));
        assert!(std::sync::Arc::ptr_eq(&preview.text, &refreshed.text));
        assert_eq!(preview.matches, vec![1]);
        assert_eq!(refreshed.matches, vec![8]);
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
        assert_eq!(preview.matches, vec![5]);
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
        assert_eq!(preview.next_match(0), Some(4));
        assert_eq!(preview.next_match(1), Some(4));
        assert_eq!(preview.next_match(8), None);
        assert_eq!(preview.previous_match(8), Some(4));
        assert_eq!(preview.previous_match(0), None);
        assert_eq!(preview.match_ordinal(4), Some(2));
        assert_eq!(preview.match_ordinal(2), None);
    }

    #[test]
    fn navigates_each_occurrence_and_reveals_the_first_match_on_a_line() {
        let preview = prepare("prefix foo foo\n\tfoo", Some(&Regex::new("foo").unwrap()));
        assert_eq!(preview.matches, vec![7, 11, 16]);
        assert_eq!(preview.next_match(7), Some(11));
        assert_eq!(preview.next_match(11), Some(16));
        assert_eq!(preview.previous_match(16), Some(11));
        assert_eq!(preview.previous_match(12), Some(11));
        assert_eq!(preview.match_ordinal(11), Some(2));
        assert_eq!(preview.match_offset(1), 7);
        assert_eq!(preview.match_offset(2), 16);
        assert_eq!(preview.line_at_offset(16), 2);
        let unmatched = prepare("no match\n\tfoo", Some(&Regex::new("foo").unwrap()));
        assert_eq!(unmatched.match_offset(1), 0);
        assert_eq!(unmatched.match_offset(2), 10);
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
