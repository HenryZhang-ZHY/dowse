//! Listing and cloning GitHub repositories through the `gh` command line, so
//! dowse never handles credentials: `gh` already holds the user's sign-in.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use super::process;

/// The fields `gh repo list --json` is asked for.
const LIST_FIELDS: &str = "name,nameWithOwner,owner,description,isPrivate,isFork,isArchived,\
primaryLanguage,diskUsage,pushedAt,defaultBranchRef,url";

/// How many repositories a listing asks for by default. `gh` fetches them
/// 100 at a time.
pub const DEFAULT_LIST_LIMIT: usize = 1000;

/// A repository on GitHub, as `gh repo list` describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteRepo {
    /// `owner/name`.
    pub full_name: String,
    pub name: String,
    pub owner: String,
    pub description: String,
    pub private: bool,
    pub fork: bool,
    pub archived: bool,
    pub language: Option<String>,
    /// As GitHub reports it, in kilobytes.
    pub size_kb: u64,
    /// RFC 3339, when the repository was last pushed to.
    pub pushed_at: Option<String>,
    pub default_branch: Option<String>,
    pub url: String,
}

/// `gh repo list`'s JSON, before it is flattened into [`RemoteRepo`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhRepo {
    name: String,
    name_with_owner: String,
    owner: GhLogin,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    is_private: bool,
    #[serde(default)]
    is_fork: bool,
    #[serde(default)]
    is_archived: bool,
    #[serde(default)]
    primary_language: Option<GhName>,
    #[serde(default)]
    disk_usage: Option<u64>,
    #[serde(default)]
    pushed_at: Option<String>,
    #[serde(default)]
    default_branch_ref: Option<GhName>,
    #[serde(default)]
    url: String,
}

#[derive(Deserialize)]
struct GhLogin {
    login: String,
}

#[derive(Deserialize)]
struct GhName {
    name: String,
}

/// Parse the output of `gh repo list --json <LIST_FIELDS>`.
pub fn parse_list(json: &str) -> Result<Vec<RemoteRepo>> {
    let repos: Vec<GhRepo> =
        serde_json::from_str(json).context("gh repo list printed something unexpected")?;
    Ok(repos
        .into_iter()
        .map(|repo| RemoteRepo {
            full_name: repo.name_with_owner,
            name: repo.name,
            owner: repo.owner.login,
            description: repo.description.unwrap_or_default(),
            private: repo.is_private,
            fork: repo.is_fork,
            archived: repo.is_archived,
            language: repo.primary_language.map(|language| language.name),
            size_kb: repo.disk_usage.unwrap_or(0),
            pushed_at: repo.pushed_at,
            default_branch: repo.default_branch_ref.map(|branch| branch.name),
            url: repo.url,
        })
        .collect())
}

/// The arguments for `gh` that list `owner`'s repositories, or the signed-in
/// user's when `owner` is `None`.
pub fn list_args(owner: Option<&str>, limit: usize) -> Vec<String> {
    let mut args = vec!["repo".to_string(), "list".to_string()];
    if let Some(owner) = owner.map(str::trim).filter(|owner| !owner.is_empty()) {
        args.push(owner.to_string());
    }
    args.extend([
        "--limit".to_string(),
        limit.max(1).to_string(),
        "--json".to_string(),
        LIST_FIELDS.to_string(),
    ]);
    args
}

/// List `owner`'s repositories (the signed-in user's when `None`), most
/// recently pushed first. Blocks while `gh` pages through them.
pub fn list(owner: Option<&str>, limit: usize) -> Result<Vec<RemoteRepo>> {
    let mut gh = process::command("gh");
    gh.args(list_args(owner, limit));
    let output = process::output(gh).map_err(explain_gh)?;
    let mut repos = parse_list(&String::from_utf8_lossy(&output.stdout))?;
    repos.sort_by(|a, b| b.pushed_at.cmp(&a.pushed_at));
    Ok(repos)
}

/// The signed-in user's login.
pub fn current_user() -> Result<String> {
    let mut gh = process::command("gh");
    gh.args(["api", "user", "--jq", ".login"]);
    process::stdout(gh).map_err(explain_gh)
}

fn explain_gh(error: anyhow::Error) -> anyhow::Error {
    let text = format!("{error:#}");
    if text.contains("cannot run gh") {
        error.context("the GitHub CLI (gh) is needed; install it from https://cli.github.com")
    } else if text.contains("gh auth login") || text.contains("not logged") {
        error.context("run `gh auth login` in a terminal first")
    } else {
        error
    }
}

/// A GitHub timestamp, `2026-09-26T11:57:50Z`, as a time.
pub fn parse_timestamp(text: &str) -> Option<std::time::SystemTime> {
    let (date, time) = text.trim().trim_end_matches('Z').split_once('T')?;
    let mut date = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    let mut time = time.split(':').map(|part| part.parse::<f64>().ok());
    let (hour, minute, second) = (time.next()??, time.next()??, time.next()??);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from the civil calendar date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + (hour * 3600.0 + minute * 60.0 + second) as i64;
    u64::try_from(seconds)
        .ok()
        .map(|seconds| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
}

// ----- choosing ------------------------------------------------------------------

/// What the clone list shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteFilter {
    /// Space-separated words, each of which the name, description or language
    /// must contain.
    pub text: String,
    pub forks: bool,
    pub archived: bool,
}

impl RemoteFilter {
    pub fn matches(&self, repo: &RemoteRepo) -> bool {
        if (repo.fork && !self.forks) || (repo.archived && !self.archived) {
            return false;
        }
        let haystack = format!(
            "{} {} {}",
            repo.full_name,
            repo.description,
            repo.language.as_deref().unwrap_or("")
        )
        .to_lowercase();
        self.text
            .split_whitespace()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }
}

// ----- cloning -------------------------------------------------------------------

/// How much of a repository's history a clone downloads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloneMode {
    /// Every commit and tree, but only the file contents checked out; older
    /// contents are fetched when needed. Nearly as small as shallow for big
    /// repositories, and history still works.
    #[default]
    Blobless,
    /// The latest commit only.
    Shallow,
    /// Everything.
    Full,
}

impl CloneMode {
    pub const ALL: [CloneMode; 3] = [CloneMode::Blobless, CloneMode::Shallow, CloneMode::Full];

    pub fn label(self) -> &'static str {
        match self {
            CloneMode::Blobless => "Blobless",
            CloneMode::Shallow => "Shallow",
            CloneMode::Full => "Full",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            CloneMode::Blobless => {
                "Full history, file contents on demand. Fast, and history works."
            }
            CloneMode::Shallow => "The latest commit only. Smallest; for mirrors you only search.",
            CloneMode::Full => "Everything. Slowest and largest.",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "blobless" | "partial" => Some(CloneMode::Blobless),
            "shallow" | "depth1" => Some(CloneMode::Shallow),
            "full" => Some(CloneMode::Full),
            _ => None,
        }
    }

    fn git_args(self) -> &'static [&'static str] {
        match self {
            CloneMode::Blobless => &["--filter=blob:none"],
            CloneMode::Shallow => &["--depth", "1"],
            CloneMode::Full => &[],
        }
    }
}

/// Where `full_name` (`owner/name`) is cloned under `root`: `root/owner/name`.
pub fn clone_destination(root: &Path, full_name: &str) -> PathBuf {
    full_name
        .split('/')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// The arguments for `gh` that clone `full_name` into `destination`.
pub fn clone_args(full_name: &str, destination: &Path, mode: CloneMode) -> Vec<String> {
    let mut args = vec![
        "repo".to_string(),
        "clone".to_string(),
        full_name.to_string(),
        destination.to_string_lossy().into_owned(),
        // A fork's parent would be fetched too; pull keeps to origin.
        "--no-upstream".to_string(),
        "--".to_string(),
        "--progress".to_string(),
    ];
    args.extend(mode.git_args().iter().map(|arg| arg.to_string()));
    args
}

/// Clone `full_name` into `destination`, which must not exist or be empty,
/// reporting progress. A clone that fails or is cancelled leaves nothing
/// behind.
pub fn clone(
    full_name: &str,
    destination: &Path,
    mode: CloneMode,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(CloneProgress),
) -> Result<()> {
    let existed = destination.exists();
    if existed
        && std::fs::read_dir(destination)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(true)
    {
        bail!("{} already exists", destination.display());
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let mut gh = process::command("gh");
    gh.args(clone_args(full_name, destination, mode));
    gh.env("GIT_TERMINAL_PROMPT", "0");
    let mut tracker = ProgressTracker::default();
    let result = process::run_with_progress(gh, cancel, |line| {
        if let Some(progress) = tracker.feed(line) {
            on_progress(progress);
        }
    })
    .map_err(explain_gh);
    if result.is_err() {
        // Only what this clone made: an empty folder that was there stays.
        if existed {
            if let Ok(entries) = std::fs::read_dir(destination) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        std::fs::remove_dir_all(&path).ok();
                    } else {
                        std::fs::remove_file(&path).ok();
                    }
                }
            }
        } else {
            std::fs::remove_dir_all(destination).ok();
        }
    }
    result
}

/// How far a clone has got.
#[derive(Clone, Debug, PartialEq)]
pub struct CloneProgress {
    /// What git is doing, such as `Receiving objects`.
    pub phase: String,
    /// The whole clone, from 0 to 1. Never goes backwards.
    pub fraction: f32,
    /// git's own detail, such as `1.20 GiB | 12.00 MiB/s`.
    pub detail: String,
}

/// Turns git's progress lines into an overall fraction. Each phase covers a
/// share of the whole; a blobless clone receives objects twice (history,
/// then the files checked out), which the second share absorbs.
#[derive(Default)]
struct ProgressTracker {
    fraction: f32,
}

impl ProgressTracker {
    fn feed(&mut self, line: &str) -> Option<CloneProgress> {
        let (phase, percent, detail) = parse_progress_line(line)?;
        let (start, span) = match phase.as_str() {
            "Enumerating objects" | "Counting objects" | "Compressing objects" => (0.0, 0.05),
            "Receiving objects" => (0.05, 0.75),
            "Resolving deltas" => (0.80, 0.10),
            "Updating files" | "Checking out files" => (0.90, 0.10),
            _ => (self.fraction, 0.0),
        };
        let fraction = (start + span * percent / 100.0).clamp(0.0, 1.0);
        self.fraction = self.fraction.max(fraction);
        Some(CloneProgress {
            phase,
            fraction: self.fraction,
            detail,
        })
    }
}

/// `remote: Counting objects:  45% (9/20)` or `Receiving objects:  45%
/// (123/456), 1.20 MiB | 3.00 MiB/s` → phase, percent, detail.
fn parse_progress_line(line: &str) -> Option<(String, f32, String)> {
    let line = line.trim().strip_prefix("remote:").unwrap_or(line).trim();
    let (phase, rest) = line.split_once(':')?;
    let rest = rest.trim();
    let (percent, rest) = rest.split_once('%')?;
    let percent: f32 = percent.trim().parse().ok()?;
    let detail = rest
        .split_once(',')
        .map(|(_, detail)| detail)
        .unwrap_or("")
        .trim()
        .trim_end_matches(", done.")
        .trim_end_matches(", done")
        .to_string();
    Some((phase.trim().to_string(), percent, detail))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[{"defaultBranchRef":{"name":"master"},"description":"","diskUsage":511,"isArchived":false,"isFork":false,"isPrivate":false,"name":"dowse","nameWithOwner":"alice/dowse","owner":{"id":"X","login":"alice"},"primaryLanguage":{"name":"Rust"},"pushedAt":"2026-09-26T11:57:50Z","url":"https://github.com/alice/dowse"},{"defaultBranchRef":null,"description":null,"diskUsage":null,"isArchived":true,"isFork":true,"isPrivate":true,"name":"old","nameWithOwner":"alice/old","owner":{"login":"alice"},"primaryLanguage":null,"pushedAt":null,"url":"https://github.com/alice/old"}]"#;

    #[test]
    fn parses_gh_repo_list_output() {
        let repos = parse_list(SAMPLE).unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].full_name, "alice/dowse");
        assert_eq!(repos[0].owner, "alice");
        assert_eq!(repos[0].language.as_deref(), Some("Rust"));
        assert_eq!(repos[0].size_kb, 511);
        assert_eq!(repos[0].default_branch.as_deref(), Some("master"));
        assert_eq!(repos[1].description, "");
        assert!(repos[1].private && repos[1].fork && repos[1].archived);
        assert_eq!(repos[1].language, None);
        assert_eq!(repos[1].default_branch, None);
        assert!(parse_list("not json").is_err());
    }

    #[test]
    fn parses_github_timestamps() {
        let at = |secs| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(at(0)));
        assert_eq!(
            parse_timestamp("2026-09-26T11:57:50Z"),
            Some(at(1_790_423_870))
        );
        assert_eq!(
            parse_timestamp("2000-02-29T00:00:01Z"),
            Some(at(951_782_401))
        );
        assert_eq!(parse_timestamp("yesterday"), None);
        assert_eq!(parse_timestamp("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn list_args_name_the_owner_only_when_given() {
        let mine = list_args(None, 1000);
        assert_eq!(&mine[..4], ["repo", "list", "--limit", "1000"]);
        let theirs = list_args(Some(" microsoft "), 0);
        assert_eq!(&theirs[..5], ["repo", "list", "microsoft", "--limit", "1"]);
        assert_eq!(list_args(Some("  "), 5)[2], "--limit");
    }

    #[test]
    fn filters_by_words_forks_and_archived() {
        let repos = parse_list(SAMPLE).unwrap();
        let mut filter = RemoteFilter::default();
        assert!(filter.matches(&repos[0]));
        assert!(!filter.matches(&repos[1]));
        filter.forks = true;
        assert!(!filter.matches(&repos[1]), "still archived");
        filter.archived = true;
        assert!(filter.matches(&repos[1]));
        filter.text = "RUST dow".into();
        assert!(filter.matches(&repos[0]) && !filter.matches(&repos[1]));
    }

    #[test]
    fn clones_go_under_owner_folders() {
        let root = Path::new("/src/mirrors");
        assert_eq!(
            clone_destination(root, "alice/api"),
            Path::new("/src/mirrors/alice/api")
        );
        assert_eq!(
            clone_destination(root, "../evil/./x"),
            Path::new("/src/mirrors/evil/x")
        );
        let args = clone_args("alice/api", Path::new("/m/alice/api"), CloneMode::Blobless);
        assert_eq!(args[..3], ["repo", "clone", "alice/api"]);
        assert!(args.ends_with(&[
            "--".into(),
            "--progress".into(),
            "--filter=blob:none".into()
        ]));
        let shallow = clone_args("a/b", Path::new("/x"), CloneMode::Shallow);
        assert!(shallow.ends_with(&["--depth".into(), "1".into()]));
        assert!(
            clone_args("a/b", Path::new("/x"), CloneMode::Full).ends_with(&["--progress".into()])
        );
        assert_eq!(CloneMode::parse("Shallow"), Some(CloneMode::Shallow));
        assert_eq!(CloneMode::parse("nope"), None);
    }

    #[test]
    fn progress_rises_through_the_phases_and_never_falls() {
        let mut tracker = ProgressTracker::default();
        assert!(tracker.feed("Cloning into 'x'...").is_none());
        let counting = tracker
            .feed("remote: Counting objects:  50% (5/10)")
            .unwrap();
        assert_eq!(counting.phase, "Counting objects");
        let receiving = tracker
            .feed("Receiving objects:  50% (50/100), 1.20 MiB | 3.00 MiB/s")
            .unwrap();
        assert_eq!(receiving.detail, "1.20 MiB | 3.00 MiB/s");
        assert!((receiving.fraction - 0.425).abs() < 1e-3);
        let resolving = tracker
            .feed("Resolving deltas: 100% (10/10), done.")
            .unwrap();
        assert!((resolving.fraction - 0.9).abs() < 1e-3);
        // A blobless checkout receives the files' contents afterwards.
        let again = tracker.feed("Receiving objects:  10% (1/10)").unwrap();
        assert!((again.fraction - 0.9).abs() < 1e-3);
        let done = tracker.feed("Updating files: 100% (3/3), done.").unwrap();
        assert!((done.fraction - 1.0).abs() < 1e-3);
    }

    #[test]
    fn cloning_over_a_non_empty_folder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), "x").unwrap();
        let cancel = AtomicBool::new(false);
        let error = clone("a/b", dir.path(), CloneMode::Blobless, &cancel, |_| {}).unwrap_err();
        assert!(error.to_string().contains("already exists"));
        assert!(dir.path().join("file").exists());
    }
}
