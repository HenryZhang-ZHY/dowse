//! A whole file prepared for the preview pane: every line ready to display,
//! with the query's matches marked the way result snippets mark them.

use std::ops::Range;
use std::path::Path;

use regex::Regex;
use tgrep_core::encoding::{self, EncodingMode};

use super::search::{SearchLimits, display_line};

/// Longer lines are clipped around their first match, as a minified file
/// would otherwise be one enormous line.
pub const MAX_LINE_LEN: usize = 4000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewLine {
    /// The line with tabs expanded, clipped to [`MAX_LINE_LEN`].
    pub text: String,
    /// Byte ranges of `text` the query matches.
    pub highlights: Vec<Range<usize>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilePreview {
    pub lines: Vec<PreviewLine>,
    /// 0-based indexes of the lines the query matches, in order.
    pub matches: Vec<usize>,
    /// The line with the most characters, which sets the view's width.
    pub widest: usize,
}

impl FilePreview {
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
    let text = text.strip_suffix('\n').unwrap_or(text);
    let mut preview = FilePreview::default();
    let mut widest = 0;
    for (index, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let ranges: Vec<Range<usize>> = match matcher {
            Some(matcher) => {
                let ranges: Vec<_> = matcher
                    .find_iter(line)
                    .filter(|m| !m.is_empty())
                    .map(|m| m.range())
                    .collect();
                if !ranges.is_empty() || matcher.is_match(line) {
                    preview.matches.push(index);
                }
                ranges
            }
            None => Vec::new(),
        };
        let (display, highlights) = display_line(line, &ranges, MAX_LINE_LEN);
        let width = display.chars().count();
        if width > widest {
            widest = width;
            preview.widest = index;
        }
        preview.lines.push(PreviewLine {
            text: display,
            highlights,
        });
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_matches_and_expands_tabs() {
        let matcher = Regex::new("foo").unwrap();
        let preview = prepare("a\r\n\tfoo foo\r\nbar\nwide line here\n", Some(&matcher));
        let texts: Vec<&str> = preview.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["a", "    foo foo", "bar", "wide line here"]);
        assert_eq!(preview.matches, vec![1]);
        assert_eq!(preview.lines[1].highlights, vec![4..7, 8..11]);
        assert_eq!(preview.widest, 3);
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
        assert_eq!(preview.lines.len(), 1);
        assert!(preview.matches.is_empty());

        let binary = dir.path().join("b.bin");
        std::fs::write(&binary, b"a\0b").unwrap();
        assert!(load(&binary, None).is_err());
        assert!(load(&dir.path().join("missing"), None).is_err());
    }
}
