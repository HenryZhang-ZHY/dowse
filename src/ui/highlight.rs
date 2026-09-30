//! Syntax colours for code, from the tree-sitter grammars GPUI Kit bundles.
//! Lines are parsed together, so a run of lines (a snippet, or a whole file)
//! is highlighted with the context the grammar needs, then split per line.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Once;
use std::time::Duration;

use gpui_kit::component::Rope;
use gpui_kit::component::highlighter::{HighlightTheme, LanguageRegistry, SyntaxHighlighter};
use gpui_kit::{HighlightStyle, SharedString};

/// Styled byte ranges of one line, relative to the line.
pub(super) type LineStyles = Vec<(Range<usize>, HighlightStyle)>;

/// Parsing longer than this gives up and shows the text uncoloured.
const PARSE_TIMEOUT: Duration = Duration::from_millis(1500);

/// The grammar for a language the engine detected (see
/// [`dowse::engine::language::detect`]), when one is bundled.
pub(super) fn grammar(language: &str) -> Option<&'static str> {
    Some(match language {
        "Rust" => "rust",
        "C" => "c",
        "C++" => "cpp",
        "C#" => "csharp",
        "Go" => "go",
        "Java" => "java",
        "Kotlin" => "kotlin",
        "Scala" => "scala",
        "Swift" => "swift",
        "Python" => "python",
        "Ruby" => "ruby",
        "PHP" => "php",
        "Lua" => "lua",
        "Elixir" => "elixir",
        "Zig" => "zig",
        "JavaScript" | "JSX" => "javascript",
        "TypeScript" => "typescript",
        "TSX" => "tsx",
        "Svelte" => "svelte",
        "Astro" => "astro",
        "HTML" | "XML" | "Vue" => "html",
        "CSS" | "SCSS" | "Less" => "css",
        "JSON" => "json",
        "YAML" => "yaml",
        "TOML" => "toml",
        "Markdown" => "markdown",
        "Shell" => "bash",
        "SQL" => "sql",
        "GraphQL" => "graphql",
        "Protocol Buffers" => "proto",
        "CMake" => "cmake",
        "Makefile" => "make",
        _ => return None,
    })
}

/// Highlights queries for grammars GPUI Kit registers with an empty one, so
/// they parse but colour nothing. GraphQL's and Proto's crates export none.
const MISSING_QUERIES: [(&str, &str); 5] = [
    ("csharp", tree_sitter_c_sharp::HIGHLIGHTS_QUERY),
    ("swift", tree_sitter_swift::HIGHLIGHTS_QUERY),
    ("cmake", tree_sitter_cmake::HIGHLIGHTS_QUERY),
    ("graphql", include_str!("queries/graphql.scm")),
    ("proto", include_str!("queries/proto.scm")),
];

/// Fill in [`MISSING_QUERIES`] where the registry's query is still empty.
/// Runs once, before the first highlighter is built.
fn register_missing_queries() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let registry = LanguageRegistry::singleton();
        for (grammar, highlights) in MISSING_QUERIES {
            if let Some(mut config) = registry.language(grammar)
                && config.highlights.is_empty()
            {
                config.highlights = SharedString::from(highlights);
                registry.register(grammar, &config);
            }
        }
    });
}

fn new_highlighter(grammar: &str) -> SyntaxHighlighter {
    register_missing_queries();
    SyntaxHighlighter::new(grammar)
}

/// Highlighters by grammar, kept because building one compiles the
/// grammar's queries.
#[derive(Default)]
pub(super) struct Highlighters(HashMap<&'static str, SyntaxHighlighter>);

impl Highlighters {
    /// Styles for each of `lines`, parsed as consecutive lines of `grammar`.
    pub(super) fn highlight(
        &mut self,
        grammar: &'static str,
        lines: &[&str],
        theme: &HighlightTheme,
    ) -> Vec<LineStyles> {
        let highlighter = self
            .0
            .entry(grammar)
            .or_insert_with(|| new_highlighter(grammar));
        highlight_lines(highlighter, lines, theme)
    }
}

/// Styles for a run of lines using a fresh highlighter.
#[cfg(test)]
pub(super) fn highlight_once(
    grammar: &str,
    lines: &[&str],
    theme: &HighlightTheme,
) -> Vec<LineStyles> {
    highlight_lines(&mut new_highlighter(grammar), lines, theme)
}

fn highlight_lines(
    highlighter: &mut SyntaxHighlighter,
    lines: &[&str],
    theme: &HighlightTheme,
) -> Vec<LineStyles> {
    let text = lines.join("\n");
    let rope = Rope::from(text.as_str());
    if !highlighter.update(None, &rope, Some(PARSE_TIMEOUT)) {
        return vec![Vec::new(); lines.len()];
    }
    let styles = highlighter.styles(&(0..text.len()), theme);
    split_by_line(lines, styles)
}

/// Cut styles over the joined text into per-line styles.
fn split_by_line(lines: &[&str], styles: Vec<(Range<usize>, HighlightStyle)>) -> Vec<LineStyles> {
    let mut result = Vec::with_capacity(lines.len());
    let mut styles = styles
        .into_iter()
        .filter(|(range, style)| !range.is_empty() && *style != HighlightStyle::default())
        .peekable();
    let mut line_start = 0;
    for line in lines {
        let line_end = line_start + line.len();
        let mut line_styles = Vec::new();
        while let Some((range, style)) = styles.peek() {
            if range.start >= line_end {
                break;
            }
            let start = range.start.max(line_start);
            let end = range.end.min(line_end);
            if start < end {
                line_styles.push((start - line_start..end - line_start, *style));
            }
            if range.end <= line_end {
                styles.next();
            } else {
                break;
            }
        }
        result.push(line_styles);
        // Skip the newline joining this line to the next.
        line_start = line_end + 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold() -> HighlightStyle {
        HighlightStyle {
            font_weight: Some(gpui_kit::FontWeight::BOLD),
            ..Default::default()
        }
    }

    #[test]
    fn splits_styles_spanning_lines() {
        // "ab\ncd\nef" with a style over "b\ncd\ne".
        let lines = ["ab", "cd", "ef"];
        let split = split_by_line(&lines, vec![(1..7, bold())]);
        assert_eq!(
            split,
            vec![
                vec![(1..2, bold())],
                vec![(0..2, bold())],
                vec![(0..1, bold())]
            ]
        );
    }

    #[test]
    fn highlights_rust_keywords() {
        let theme = HighlightTheme::default_dark();
        let styles = highlight_once("rust", &["fn main() {", "    let x = 1;", "}"], &theme);
        assert_eq!(styles.len(), 3);
        assert!(styles[0].iter().any(|(range, _)| *range == (0..2)));
        assert!(styles[1].iter().any(|(range, _)| *range == (4..7)));
    }

    #[test]
    fn highlights_grammars_with_a_filled_in_query() {
        // A query that fails to compile only logs a warning and colours
        // nothing, so check a keyword is styled for each.
        let cases: [(&str, &[&str], usize, Range<usize>); 5] = [
            ("csharp", &["class A {", "    public int X;", "}"], 1, 4..10),
            ("swift", &["func f() {}"], 0, 0..4),
            ("cmake", &["if(X)", "endif()"], 0, 0..2),
            ("graphql", &["query Q {", "  a", "}"], 0, 0..5),
            ("proto", &["message M {", "}"], 0, 0..7),
        ];
        let theme = HighlightTheme::default_dark();
        for (grammar, lines, line, keyword) in cases {
            let styles = highlight_once(grammar, lines, &theme);
            assert!(
                styles[line].iter().any(|(range, _)| *range == keyword),
                "{grammar}: {:?}",
                styles
            );
        }
    }

    #[test]
    fn detected_languages_with_a_grammar_parse() {
        for language in ["Rust", "TypeScript", "Python", "C#", "Go", "JSON", "SQL"] {
            let grammar = grammar(language).unwrap();
            let theme = HighlightTheme::default_light();
            let styles = highlight_once(grammar, &["x"], &theme);
            assert_eq!(styles.len(), 1, "{language}");
        }
    }
}
