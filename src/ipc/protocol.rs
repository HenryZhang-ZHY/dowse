//! What the command line and the running app say to each other. Each
//! connection carries one [`ClientMessage`] line from the command line,
//! answered by [`Frame`] lines that end with [`Frame::Done`] or
//! [`Frame::Error`]. Everything is JSON, one value per line.
//!
//! The app answers with data, not text: the command line decides how to
//! show it, so the same answer serves people, scripts and agents.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::diagnostics::log::{LogEntry, LogLevel};
use crate::diagnostics::metrics::MetricsSnapshot;
use crate::engine::config::ConfigDir;
use crate::engine::facets::{FacetFilter, FacetKind, Facets, ROOT_DIRECTORY};
use crate::engine::library::Library;
use crate::engine::query::SearchQuery;
use crate::engine::repo::{self, RepoInfo, Scope};
use crate::engine::search::{FileMatch, SearchOutcome};
use crate::engine::table::ExportFormat;
use crate::engine::workspace;
use crate::launch::Command;

/// Bumped whenever a message changes shape, so a command line and an app
/// from different builds notice instead of misreading each other.
pub const PROTOCOL_VERSION: u32 = 1;

/// Facet values sent per facet: enough to suggest how to narrow a query.
const FACET_VALUES: usize = 8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// A launch of the desktop app, handed to the running one.
    Launch(Command),
    Request(RequestEnvelope),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub version: u32,
    /// The command line's working directory, for `--here`.
    pub cwd: PathBuf,
    pub request: Request,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    Search(SearchRequest),
    /// The repositories in scope, with their indexes.
    Repos(ScopeSpec),
    /// Add folders to the library, discovering repositories inside them.
    AddRepos {
        folders: Vec<PathBuf>,
        tags: Vec<String>,
    },
    /// Change a repository's tags. `repo` is a name or a path.
    Tag {
        repo: String,
        add: Vec<String>,
        remove: Vec<String>,
    },
    /// Bring the indexes of the repositories in scope up to date, or with
    /// `full` build them again from every file.
    Index {
        scope: ScopeSpec,
        wait: bool,
        #[serde(default)]
        full: bool,
    },
    Status,
    Logs(LogsRequest),
    /// The app's metrics.
    Metrics,
    /// Open the developer tools window.
    OpenDevTools,
    /// Close every window and quit.
    Quit,
}

/// Which repositories of the library a request covers. Empty means all.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeSpec {
    /// As in the scope bar: tags in one group are alternatives, groups
    /// narrow each other.
    pub tags: Vec<String>,
    /// A saved workspace, by name or path: only its repositories.
    pub workspace: Option<String>,
    /// Only the repository holding the working directory.
    pub here: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: SearchQuery,
    pub scope: ScopeSpec,
    /// Lines shown around each matching line.
    pub context: usize,
    /// Matching lines kept per file.
    pub max_per_file: usize,
    /// Matching lines sent back; 0 sends every line kept.
    pub limit: usize,
    /// Only which files match and how often, without their lines.
    pub files_only: bool,
    /// Lay the lines out as a table in this format instead.
    pub table: Option<ExportFormat>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogsRequest {
    /// Only records after this sequence number.
    pub after: u64,
    pub level: LogLevel,
    /// At most this many of the latest records.
    pub limit: usize,
    /// Keep sending records as they come.
    pub follow: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Frame {
    Search(Box<SearchResponse>),
    Repos(Vec<RepoStatus>),
    Status(AppStatus),
    Log(LogEntry),
    Metrics(Box<MetricsSnapshot>),
    /// Something done, for a person to read.
    Message(String),
    Done,
    Error(String),
}

// ----- search ---------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchResponse {
    pub files: Vec<FileHit>,
    pub summary: SearchSummary,
    /// How the matching files spread over repositories, languages and
    /// top-level directories, the most common first.
    pub facets: Vec<FacetCounts>,
    /// The table export, when asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// What a reader should know about the results, such as repositories
    /// that were scanned for want of an index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileHit {
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Relative to the repository, `/`-separated.
    pub path: String,
    pub abs_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Every matching line in the file, including those not sent.
    pub matched_lines: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<LineHit>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineHit {
    pub line: usize,
    pub text: String,
    /// A matching line, rather than context.
    #[serde(rename = "match")]
    pub is_match: bool,
    /// Byte ranges of `text` that matched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranges: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchSummary {
    /// Matching lines found, sent or not.
    pub matched_lines: usize,
    /// Files that matched.
    pub files: usize,
    /// Matching lines and files sent back.
    pub shown_lines: usize,
    pub shown_files: usize,
    /// Files read after the indexes and filters narrowed the candidates.
    pub searched_files: usize,
    /// Files in the repositories searched.
    pub corpus_files: usize,
    pub repos: usize,
    /// Repositories without a usable index, whose folders were scanned.
    pub unindexed_repos: usize,
    /// The search stopped at its limit, so files after these went unread.
    pub truncated: bool,
    pub elapsed_ms: f64,
    /// Of `elapsed_ms`, choosing candidates before reading files.
    pub candidates_ms: f64,
    pub bytes_read: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacetCounts {
    /// `repo`, `branch`, `language` or `path`: the qualifier that narrows
    /// to a value.
    pub qualifier: String,
    /// Values and how many matching files have each, most first.
    pub values: Vec<(String, usize)>,
}

impl SearchResponse {
    /// Shape a search's outcome for the command line: send at most `limit`
    /// matching lines (all when 0), or no lines with `files_only`, and count
    /// the facets over every file found.
    pub fn new(outcome: &SearchOutcome, limit: usize, files_only: bool) -> Self {
        let budget = if limit == 0 { usize::MAX } else { limit };
        let mut shown_lines = 0;
        let mut files = Vec::new();
        for file in &outcome.files {
            if shown_lines >= budget {
                break;
            }
            let mut hit = FileHit::new(file);
            if !files_only {
                hit.lines = take_lines(file, budget - shown_lines);
                shown_lines += hit.lines.iter().filter(|line| line.is_match).count();
            }
            files.push(hit);
        }
        let summary = SearchSummary {
            matched_lines: outcome.matched_lines,
            files: outcome.files.len(),
            shown_lines,
            shown_files: files.len(),
            searched_files: outcome.searched_files,
            corpus_files: outcome.corpus_files,
            repos: outcome.repos,
            unindexed_repos: outcome.unindexed_repos,
            truncated: outcome.truncated,
            elapsed_ms: outcome.elapsed.as_secs_f64() * 1000.0,
            candidates_ms: outcome.candidates_elapsed.as_secs_f64() * 1000.0,
            bytes_read: outcome.bytes_read,
        };
        Self {
            files,
            summary,
            facets: facet_counts(&outcome.files),
            table: None,
            notes: Vec::new(),
        }
    }
}

impl FileHit {
    fn new(file: &FileMatch) -> Self {
        Self {
            repo: file.repo.name.clone(),
            branch: file.repo.branch.clone(),
            path: file.path.clone(),
            // The platform's separators, so the path can be pasted as is.
            abs_path: file
                .repo
                .root
                .join(file.path.split('/').collect::<PathBuf>()),
            language: file.language.map(str::to_string),
            matched_lines: file.matched_lines,
            lines: Vec::new(),
        }
    }
}

/// The file's lines up to its `budget`-th matching line, with the context
/// that follows it.
fn take_lines(file: &FileMatch, budget: usize) -> Vec<LineHit> {
    let mut lines = Vec::new();
    let mut matches = 0;
    for snippet in &file.snippets {
        if matches == budget {
            break;
        }
        for line in &snippet.lines {
            if line.is_match {
                if matches == budget {
                    return lines;
                }
                matches += 1;
            }
            lines.push(LineHit {
                line: line.number,
                text: line.text.clone(),
                is_match: line.is_match,
                ranges: line
                    .highlights
                    .iter()
                    .map(|range| (range.start, range.end))
                    .collect(),
            });
        }
    }
    lines
}

fn facet_counts(files: &[FileMatch]) -> Vec<FacetCounts> {
    let facets = Facets::new(files, &FacetFilter::default());
    [
        (FacetKind::Repository, "repo"),
        (FacetKind::Branch, "branch"),
        (FacetKind::Language, "language"),
        (FacetKind::Directory, "path"),
    ]
    .into_iter()
    .filter_map(|(kind, qualifier)| {
        let values: Vec<(String, usize)> = facets
            .section(&kind)?
            .iter()
            .filter(|(value, _)| value != ROOT_DIRECTORY)
            .take(FACET_VALUES)
            .cloned()
            .collect();
        (values.len() > 1).then(|| FacetCounts {
            qualifier: qualifier.to_string(),
            values,
        })
    })
    .collect()
}

// ----- repositories and the app -----------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoStatus {
    pub name: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub tags: Vec<String>,
    /// `ready`, `missing` (no index yet), `unusable`, `loading`, `queued`,
    /// `building`, `failed` or `not-found` (the folder is gone); `closed`
    /// when nothing in the app has it open, so only the index on disk is
    /// known.
    pub index: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Files in the index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<u64>,
    /// When the index was last built, in milliseconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_at_ms: Option<u64>,
    /// Files changed since the index was built, which searches read
    /// directly.
    pub changed_files: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppStatus {
    pub version: String,
    pub pid: u32,
    pub uptime_ms: u64,
    pub config_dir: PathBuf,
    pub log_file: PathBuf,
    pub windows: usize,
    /// With no windows, how long the app waits for another request before
    /// quitting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quits_in_ms: Option<u64>,
    /// Repositories in the library, and those the app has open.
    pub library_repos: usize,
    pub open_repos: usize,
    /// Repositories being indexed, and waiting to be.
    pub building: Vec<String>,
    pub queued: Vec<String>,
}

// ----- scopes -----------------------------------------------------------------

impl ScopeSpec {
    pub fn is_everything(&self) -> bool {
        self.tags.is_empty() && self.workspace.is_none() && !self.here
    }

    /// The library's repositories this covers, sorted by name. Branch tags
    /// are read from each checkout. Fails when a workspace or the working
    /// directory names nothing in the library.
    pub fn select(
        &self,
        library: &Library,
        cwd: &Path,
        config: &ConfigDir,
        recent: &[PathBuf],
    ) -> Result<Vec<Arc<RepoInfo>>, String> {
        let mut entries: Vec<_> = library.repos.iter().collect();
        if let Some(name) = &self.workspace {
            let file = find_workspace(name, cwd, config, recent)
                .ok_or_else(|| format!("no saved workspace named {name}"))?;
            let repos = workspace::load(&file).map_err(|error| format!("{error:#}"))?;
            entries.retain(|entry| repos.contains(&entry.path));
        }
        if self.here {
            let here = repo::identity(cwd);
            let holder = entries
                .iter()
                .filter(|entry| here.starts_with(&entry.path))
                .max_by_key(|entry| entry.path.as_os_str().len())
                .copied()
                .ok_or_else(|| {
                    format!(
                        "{} is not in a repository dowse knows; add it with `dowse repos add .`",
                        here.display()
                    )
                })?;
            entries = vec![holder];
        }
        let scope = Scope::new(self.tags.iter().cloned());
        let mut repos: Vec<Arc<RepoInfo>> = entries
            .into_iter()
            .map(|entry| {
                Arc::new(RepoInfo {
                    id: entry.path.to_string_lossy().into_owned(),
                    name: entry.name.clone(),
                    root: entry.path.clone(),
                    branch: repo::current_branch(&entry.path),
                    tags: entry.tags.clone(),
                    pull_every: entry.pull_every,
                })
            })
            .filter(|info| scope.includes(info))
            .collect();
        repos.sort_by_key(|info| info.name.to_lowercase());
        Ok(repos)
    }
}

/// A saved workspace by path, by file name in the workspaces folder, or by
/// name among the recent ones.
fn find_workspace(
    name: &str,
    cwd: &Path,
    config: &ConfigDir,
    recent: &[PathBuf],
) -> Option<PathBuf> {
    let as_path = cwd.join(name);
    if workspace::is_workspace_file(&as_path) && as_path.is_file() {
        return Some(repo::identity(&as_path));
    }
    [workspace::EXTENSION, workspace::LEGACY_EXTENSION]
        .into_iter()
        .map(|extension| config.workspaces_dir().join(format!("{name}.{extension}")))
        .find(|file| file.is_file())
        .or_else(|| {
            recent
                .iter()
                .find(|file| workspace::name(file).eq_ignore_ascii_case(name))
                .cloned()
        })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::engine::library::LibraryEntry;
    use crate::engine::search::{Snippet, SnippetLine};

    fn line(number: usize, is_match: bool) -> SnippetLine {
        SnippetLine::for_test(number, &format!("line {number}"), is_match)
    }

    fn file(repo: &Arc<RepoInfo>, path: &str, lines: Vec<Vec<SnippetLine>>) -> FileMatch {
        let snippets: Vec<Snippet> = lines.into_iter().map(|lines| Snippet { lines }).collect();
        FileMatch {
            repo: repo.clone(),
            path: path.into(),
            language: crate::engine::language::detect(path),
            matched_lines: snippets
                .iter()
                .flat_map(|s| &s.lines)
                .filter(|l| l.is_match)
                .count(),
            snippets,
        }
    }

    fn info(name: &str) -> Arc<RepoInfo> {
        Arc::new(RepoInfo {
            id: name.into(),
            name: name.into(),
            root: PathBuf::from("/src").join(name),
            branch: Some("main".into()),
            tags: vec![],
            pull_every: None,
        })
    }

    fn numbers(hit: &FileHit) -> Vec<(usize, bool)> {
        hit.lines.iter().map(|l| (l.line, l.is_match)).collect()
    }

    #[test]
    fn sends_matching_lines_up_to_the_limit_with_their_context() {
        let api = info("api");
        let outcome = SearchOutcome {
            files: vec![
                file(
                    &api,
                    "src/a.rs",
                    vec![vec![
                        line(1, false),
                        line(2, true),
                        line(3, false),
                        line(4, true),
                        line(5, false),
                    ]],
                ),
                file(&api, "src/b.rs", vec![vec![line(9, true)]]),
            ],
            matched_lines: 3,
            searched_files: 2,
            corpus_files: 10,
            repos: 1,
            elapsed: Duration::from_millis(12),
            candidates_elapsed: Duration::from_millis(2),
            bytes_read: 300,
            ..Default::default()
        };

        let all = SearchResponse::new(&outcome, 0, false);
        assert_eq!(all.files.len(), 2);
        assert_eq!(all.summary.shown_lines, 3);
        assert_eq!(
            all.files[0].abs_path.as_os_str(),
            api.root.join("src").join("a.rs").as_os_str()
        );
        assert_eq!(all.files[0].language.as_deref(), Some("Rust"));
        assert_eq!(all.summary.elapsed_ms, 12.0);

        let one = SearchResponse::new(&outcome, 1, false);
        assert_eq!(one.files.len(), 1);
        assert_eq!(numbers(&one.files[0]), [(1, false), (2, true), (3, false)]);
        assert_eq!(one.summary.shown_lines, 1);
        assert_eq!(one.summary.shown_files, 1);
        assert_eq!(one.summary.matched_lines, 3);
        assert_eq!(one.summary.files, 2);

        let two = SearchResponse::new(&outcome, 2, false);
        assert_eq!(two.files.len(), 1);
        assert_eq!(numbers(&two.files[0]).len(), 5);

        let names = SearchResponse::new(&outcome, 1, true);
        assert_eq!(names.files.len(), 2);
        assert!(names.files.iter().all(|file| file.lines.is_empty()));
        assert_eq!(names.files[1].matched_lines, 1);
    }

    #[test]
    fn facets_suggest_qualifiers_only_when_they_would_narrow() {
        let (api, web) = (info("api"), info("web"));
        let outcome = SearchOutcome {
            files: vec![
                file(&api, "src/a.rs", vec![vec![line(1, true)]]),
                file(&api, "src/b.rs", vec![vec![line(1, true)]]),
                file(&web, "app/c.ts", vec![vec![line(1, true)]]),
                file(&web, "top.ts", vec![vec![line(1, true)]]),
            ],
            ..Default::default()
        };
        let response = SearchResponse::new(&outcome, 0, false);
        let facet = |qualifier: &str| {
            response
                .facets
                .iter()
                .find(|facet| facet.qualifier == qualifier)
                .map(|facet| facet.values.clone())
        };
        assert_eq!(
            facet("repo"),
            Some(vec![("api".into(), 2), ("web".into(), 2)])
        );
        // One branch narrows nothing.
        assert_eq!(facet("branch"), None);
        assert_eq!(
            facet("language"),
            Some(vec![("Rust".into(), 2), ("TypeScript".into(), 2)])
        );
        // Files at the root have no directory to narrow to.
        assert_eq!(
            facet("path"),
            Some(vec![("src".into(), 2), ("app".into(), 1)])
        );
    }

    #[test]
    fn scopes_select_by_tag_workspace_and_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = repo::identity(dir.path());
        let entry = |name: &str, tags: &[&str]| {
            let path = root.join(name);
            std::fs::create_dir_all(&path).unwrap();
            LibraryEntry {
                path,
                name: name.into(),
                tags: tags.iter().map(|tag| tag.to_string()).collect(),
                pull_every: None,
                pulled_at: None,
            }
        };
        let library = Library {
            repos: vec![
                entry("web", &["dev", "owner:bob"]),
                entry("api", &["dev", "owner:alice"]),
                entry("docs", &["mirror"]),
            ],
        };
        let config = ConfigDir::new(root.join("config"));
        let names = |spec: ScopeSpec, cwd: &Path| -> Result<Vec<String>, String> {
            spec.select(&library, cwd, &config, &[])
                .map(|repos| repos.iter().map(|repo| repo.name.clone()).collect())
        };

        assert_eq!(
            names(ScopeSpec::default(), &root).unwrap(),
            ["api", "docs", "web"]
        );
        let tags = |tags: &[&str]| ScopeSpec {
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            ..Default::default()
        };
        assert_eq!(names(tags(&["dev"]), &root).unwrap(), ["api", "web"]);
        assert_eq!(names(tags(&["dev", "owner:bob"]), &root).unwrap(), ["web"]);
        assert_eq!(
            names(tags(&["dev", "mirror"]), &root).unwrap(),
            ["api", "docs", "web"]
        );

        let here = ScopeSpec {
            here: true,
            ..Default::default()
        };
        let nested = root.join("api").join("src");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(names(here.clone(), &nested).unwrap(), ["api"]);
        let error = names(here, &root).unwrap_err();
        assert!(error.contains("dowse repos add ."), "{error}");

        let saved = config
            .workspaces_dir()
            .join(format!("team.{}", workspace::EXTENSION));
        workspace::save(&saved, &[root.join("docs"), root.join("web")]).unwrap();
        let team = |name: &str| ScopeSpec {
            workspace: Some(name.into()),
            ..Default::default()
        };
        assert_eq!(names(team("team"), &root).unwrap(), ["docs", "web"]);
        assert_eq!(
            names(team("config/workspaces/team.dowse-workspace"), &root).unwrap(),
            ["docs", "web"]
        );
        assert!(names(team("nope"), &root).is_err());
    }

    #[test]
    fn messages_survive_a_round_trip() {
        let message = ClientMessage::Request(RequestEnvelope {
            version: PROTOCOL_VERSION,
            cwd: PathBuf::from("/src/api"),
            request: Request::Search(SearchRequest {
                query: SearchQuery {
                    pattern: "parse lang:rust".into(),
                    ..Default::default()
                },
                scope: ScopeSpec::default(),
                context: 1,
                max_per_file: 200,
                limit: 100,
                files_only: false,
                table: Some(ExportFormat::Csv),
            }),
        });
        let line = serde_json::to_string(&message).unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&line).unwrap(),
            message
        );
    }
}
