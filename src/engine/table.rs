//! Results as a table of matching lines: one row per line, for sorting and
//! for exporting to a spreadsheet or a script.
//!
//! Besides where each line is, a row holds the text the query matched on it
//! and, when the query's regex has capture groups, one column per group, so
//! `/version = "(?<version>[^"]+)"/` tabulates versions.

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::path::Path;

use regex::Regex;

use super::search::FileMatch;

/// What a column holds, which decides how it sorts and exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnKind {
    Repository,
    Branch,
    Path,
    Line,
    Column,
    Language,
    Match,
    /// A capture group of the query's regex, by its index.
    Group(usize),
    Text,
}

impl ColumnKind {
    pub fn is_number(self) -> bool {
        matches!(self, Self::Line | Self::Column)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableColumn {
    pub kind: ColumnKind,
    pub name: String,
}

/// One matching line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRow {
    /// Index into the search outcome's files.
    pub file: usize,
    /// 1-based line number.
    pub line: usize,
    /// One per column, as shown and exported.
    pub cells: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResultTable {
    pub columns: Vec<TableColumn>,
    pub rows: Vec<TableRow>,
}

impl ResultTable {
    /// A row for every kept matching line of `files[index]` for each index in
    /// `visible`, in that order. `matcher` is the query's line matcher.
    pub fn new(files: &[FileMatch], visible: &[usize], matcher: &Regex) -> Self {
        let groups: Vec<(usize, String)> = matcher
            .capture_names()
            .enumerate()
            .skip(1)
            .map(|(index, name)| {
                let name = name.map_or_else(|| format!("Group {index}"), str::to_string);
                (index, name)
            })
            .collect();
        let mut columns = vec![
            column(ColumnKind::Repository, "Repository"),
            column(ColumnKind::Branch, "Branch"),
            column(ColumnKind::Path, "Path"),
            column(ColumnKind::Line, "Line"),
            column(ColumnKind::Column, "Column"),
            column(ColumnKind::Language, "Language"),
            column(ColumnKind::Match, "Match"),
        ];
        columns.extend(
            groups
                .iter()
                .map(|(index, name)| column(ColumnKind::Group(*index), name)),
        );
        columns.push(column(ColumnKind::Text, "Text"));

        let mut rows = Vec::new();
        for &file_index in visible {
            let Some(file) = files.get(file_index) else {
                continue;
            };
            let matched_lines = file
                .snippets
                .iter()
                .flat_map(|snippet| &snippet.lines)
                .filter(|line| line.is_match);
            for line in matched_lines {
                let source = line.source();
                let captures = matcher.captures(source);
                let whole = captures.as_ref().and_then(|captures| captures.get(0));
                let cells = columns
                    .iter()
                    .map(|column| match column.kind {
                        ColumnKind::Repository => file.repo.name.clone(),
                        ColumnKind::Branch => file.repo.branch.clone().unwrap_or_default(),
                        ColumnKind::Path => file.path.clone(),
                        ColumnKind::Line => line.number.to_string(),
                        ColumnKind::Column => whole
                            .map(|found| (source[..found.start()].chars().count() + 1).to_string())
                            .unwrap_or_default(),
                        ColumnKind::Language => file.language.unwrap_or_default().to_string(),
                        ColumnKind::Match => whole
                            .map(|found| found.as_str().to_string())
                            .unwrap_or_default(),
                        ColumnKind::Group(index) => captures
                            .as_ref()
                            .and_then(|captures| captures.get(index))
                            .map(|group| group.as_str().to_string())
                            .unwrap_or_default(),
                        ColumnKind::Text => source.trim().to_string(),
                    })
                    .collect();
                rows.push(TableRow {
                    file: file_index,
                    line: line.number,
                    cells,
                });
            }
        }
        Self { columns, rows }
    }

    /// Row indexes ordered by `column`, numbers numerically, text ignoring
    /// case; ties keep the result order.
    pub fn sorted(&self, column: usize, descending: bool) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        let Some(kind) = self.columns.get(column).map(|column| column.kind) else {
            return order;
        };
        let cell = |row: usize| self.rows[row].cells[column].as_str();
        order.sort_by(|&a, &b| {
            let ordering = if kind.is_number() {
                let number = |row| cell(row).parse::<u64>().ok();
                number(a).cmp(&number(b))
            } else {
                compare_text(cell(a), cell(b))
            };
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
        order
    }

    /// The rows in `order` as `format`.
    pub fn export(&self, order: &[usize], format: ExportFormat) -> String {
        let rows = order.iter().filter_map(|&row| self.rows.get(row));
        let headers = self.columns.iter().map(|column| column.name.as_str());
        match format {
            ExportFormat::Csv => {
                // A byte order mark tells Excel the file is UTF-8.
                let mut out = String::from("\u{feff}");
                push_delimited(&mut out, headers, ',', csv_field);
                for row in rows {
                    push_delimited(
                        &mut out,
                        row.cells.iter().map(String::as_str),
                        ',',
                        csv_field,
                    );
                }
                out
            }
            ExportFormat::Tsv => {
                let mut out = String::new();
                push_delimited(&mut out, headers, '\t', tsv_field);
                for row in rows {
                    push_delimited(
                        &mut out,
                        row.cells.iter().map(String::as_str),
                        '\t',
                        tsv_field,
                    );
                }
                out
            }
            ExportFormat::Markdown => {
                let mut out = String::new();
                let names: Vec<&str> = headers.collect();
                push_markdown_row(&mut out, names.iter().copied());
                let rule = self.columns.iter().map(|column| {
                    if column.kind.is_number() {
                        "--:"
                    } else {
                        "---"
                    }
                });
                push_markdown_row(&mut out, rule);
                for row in rows {
                    push_markdown_row(&mut out, row.cells.iter().map(String::as_str));
                }
                out
            }
            ExportFormat::Json => {
                let objects: Vec<serde_json::Value> = rows
                    .map(|row| {
                        let object = self
                            .columns
                            .iter()
                            .zip(&row.cells)
                            .map(|(column, cell)| {
                                let value = match cell.parse::<u64>() {
                                    Ok(number) if column.kind.is_number() => number.into(),
                                    _ => cell.clone().into(),
                                };
                                (column.name.clone(), value)
                            })
                            .collect();
                        serde_json::Value::Object(object)
                    })
                    .collect();
                let mut out = serde_json::to_string_pretty(&objects).unwrap_or_default();
                out.push('\n');
                out
            }
        }
    }
}

fn column(kind: ColumnKind, name: &str) -> TableColumn {
    TableColumn {
        kind,
        name: name.to_string(),
    }
}

fn compare_text(a: &str, b: &str) -> Ordering {
    let lowered = |text: &str| {
        text.chars()
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    lowered(a).cmp(&lowered(b)).then_with(|| a.cmp(b))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Csv,
    Tsv,
    Markdown,
    Json,
}

impl ExportFormat {
    pub const ALL: [Self; 4] = [Self::Csv, Self::Tsv, Self::Markdown, Self::Json];

    /// The format a file name asks for, CSV unless its extension says otherwise.
    pub fn for_path(path: &Path) -> Self {
        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
        match extension.as_deref() {
            Some("tsv" | "tab") => Self::Tsv,
            Some("md" | "markdown") => Self::Markdown,
            Some("json") => Self::Json,
            _ => Self::Csv,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Markdown => "md",
            Self::Json => "json",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::Tsv => "TSV",
            Self::Markdown => "Markdown",
            Self::Json => "JSON",
        }
    }
}

fn push_delimited<'a>(
    out: &mut String,
    fields: impl Iterator<Item = &'a str>,
    delimiter: char,
    escape: fn(&str) -> std::borrow::Cow<'_, str>,
) {
    for (index, field) in fields.enumerate() {
        if index > 0 {
            out.push(delimiter);
        }
        out.push_str(&escape(field));
    }
    out.push_str("\r\n");
}

/// RFC 4180: quote fields holding a comma, quote or line break.
fn csv_field(field: &str) -> std::borrow::Cow<'_, str> {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\"")).into()
    } else {
        field.into()
    }
}

/// TSV has no quoting, so tabs and line breaks become spaces.
fn tsv_field(field: &str) -> std::borrow::Cow<'_, str> {
    if field.contains(['\t', '\n', '\r']) {
        field.replace(['\t', '\n', '\r'], " ").into()
    } else {
        field.into()
    }
}

fn push_markdown_row<'a>(out: &mut String, cells: impl Iterator<Item = &'a str>) {
    out.push('|');
    for cell in cells {
        let cell = cell.replace('\\', "\\\\").replace('|', "\\|");
        let _ = write!(out, " {} |", cell.replace(['\n', '\r'], " "));
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::engine::repo::RepoInfo;
    use crate::engine::search::{SearchLimits, match_text};

    fn repo(name: &str, branch: Option<&str>) -> Arc<RepoInfo> {
        Arc::new(RepoInfo {
            id: name.into(),
            name: name.into(),
            root: ".".into(),
            branch: branch.map(str::to_string),
            tags: vec![],
        })
    }

    fn matcher(pattern: &str) -> Regex {
        regex::RegexBuilder::new(pattern)
            .multi_line(true)
            .build()
            .unwrap()
    }

    fn files(matcher: &Regex) -> Vec<FileMatch> {
        let limits = SearchLimits::default();
        vec![
            match_text(
                &repo("api", Some("main")),
                "Cargo.toml",
                "[package]\nversion = \"1.2.0\"\n",
                matcher,
                &limits,
            )
            .unwrap(),
            match_text(
                &repo("web", None),
                "pkg/Cargo.toml",
                "a\n\tversion = \"0.9, beta\"\nversion = \"10.0.0\"\n",
                matcher,
                &limits,
            )
            .unwrap(),
        ]
    }

    fn column_of(table: &ResultTable, name: &str) -> Vec<String> {
        let index = table
            .columns
            .iter()
            .position(|column| column.name == name)
            .unwrap();
        table
            .rows
            .iter()
            .map(|row| row.cells[index].clone())
            .collect()
    }

    #[test]
    fn one_row_per_matching_line_with_its_location_and_match() {
        let matcher = matcher("version");
        let table = ResultTable::new(&files(&matcher), &[0, 1], &matcher);
        let names: Vec<&str> = table.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Repository",
                "Branch",
                "Path",
                "Line",
                "Column",
                "Language",
                "Match",
                "Text"
            ]
        );
        assert_eq!(column_of(&table, "Repository"), ["api", "web", "web"]);
        assert_eq!(column_of(&table, "Branch"), ["main", "", ""]);
        assert_eq!(column_of(&table, "Line"), ["2", "2", "3"]);
        // Columns count characters in the file's own line, tabs included.
        assert_eq!(column_of(&table, "Column"), ["1", "2", "1"]);
        assert_eq!(column_of(&table, "Language"), ["TOML", "TOML", "TOML"]);
        assert_eq!(
            column_of(&table, "Match"),
            ["version", "version", "version"]
        );
        assert_eq!(column_of(&table, "Text")[1], "version = \"0.9, beta\"");
        assert_eq!(table.rows[2].file, 1);
    }

    #[test]
    fn only_visible_files_become_rows() {
        let matcher = matcher("version");
        let table = ResultTable::new(&files(&matcher), &[1], &matcher);
        assert_eq!(column_of(&table, "Repository"), ["web", "web"]);
    }

    #[test]
    fn capture_groups_get_columns() {
        let matcher = matcher(r#"version = "(?<major>\d+)\.(\d+)"#);
        let table = ResultTable::new(&files(&matcher), &[0, 1], &matcher);
        assert_eq!(column_of(&table, "major"), ["1", "0", "10"]);
        assert_eq!(column_of(&table, "Group 2"), ["2", "9", "0"]);
        assert_eq!(table.columns.last().unwrap().name, "Text");
    }

    #[test]
    fn sorts_numbers_numerically_and_text_ignoring_case() {
        let matcher = matcher(r#"version = "(\d+)"#);
        let table = ResultTable::new(&files(&matcher), &[0, 1], &matcher);
        let group = table.columns.len() - 2;
        let sorted = |descending| -> Vec<String> {
            table
                .sorted(group, descending)
                .into_iter()
                .map(|row| table.rows[row].cells[group].clone())
                .collect()
        };
        assert_eq!(sorted(false), ["0", "1", "10"]);
        assert_eq!(sorted(true), ["10", "1", "0"]);
        let line = 3;
        assert_eq!(table.sorted(line, false), [0, 1, 2]);
        assert_eq!(table.sorted(line, true), [2, 0, 1]);
        assert_eq!(compare_text("b", "A"), Ordering::Greater);
    }

    fn small_table() -> ResultTable {
        ResultTable {
            columns: vec![
                column(ColumnKind::Path, "Path"),
                column(ColumnKind::Line, "Line"),
                column(ColumnKind::Text, "Text"),
            ],
            rows: vec![
                TableRow {
                    file: 0,
                    line: 7,
                    cells: vec!["a.rs".into(), "7".into(), "say \"hi\", |x|\tok".into()],
                },
                TableRow {
                    file: 0,
                    line: 9,
                    cells: vec!["b.rs".into(), "9".into(), "plain".into()],
                },
            ],
        }
    }

    #[test]
    fn exports_csv_tsv_markdown_and_json() {
        let table = small_table();
        let order = [1, 0];
        assert_eq!(
            table.export(&order, ExportFormat::Csv),
            "\u{feff}Path,Line,Text\r\nb.rs,9,plain\r\na.rs,7,\"say \"\"hi\"\", |x|\tok\"\r\n"
        );
        assert_eq!(
            table.export(&order, ExportFormat::Tsv),
            "Path\tLine\tText\r\nb.rs\t9\tplain\r\na.rs\t7\tsay \"hi\", |x| ok\r\n"
        );
        assert_eq!(
            table.export(&order, ExportFormat::Markdown),
            "| Path | Line | Text |\n| --- | --: | --- |\n| b.rs | 9 | plain |\n| a.rs | 7 | say \"hi\", \\|x\\|\tok |\n"
        );
        let json: serde_json::Value =
            serde_json::from_str(&table.export(&order, ExportFormat::Json)).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {"Path": "b.rs", "Line": 9, "Text": "plain"},
                {"Path": "a.rs", "Line": 7, "Text": "say \"hi\", |x|\tok"},
            ])
        );
    }

    #[test]
    fn the_file_name_picks_the_format() {
        assert_eq!(
            ExportFormat::for_path(Path::new("x.TSV")),
            ExportFormat::Tsv
        );
        assert_eq!(
            ExportFormat::for_path(Path::new("x.json")),
            ExportFormat::Json
        );
        assert_eq!(
            ExportFormat::for_path(Path::new("x.md")),
            ExportFormat::Markdown
        );
        assert_eq!(
            ExportFormat::for_path(Path::new("x.csv")),
            ExportFormat::Csv
        );
        assert_eq!(ExportFormat::for_path(Path::new("x")), ExportFormat::Csv);
    }
}
