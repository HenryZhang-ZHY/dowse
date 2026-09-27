//! Listing and cloning GitHub repositories through the `gh` command line, so
//! dowse never handles credentials: `gh` already holds the user's sign-in.
//! Listing reads the REST API through `gh api`, several pages at once.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use super::process;

/// Repositories per page of GitHub's REST API, its most.
const PAGE_SIZE: usize = 100;

/// How many pages are fetched at once. GitHub serves each page in a second
/// or two; one after another, an organization of 2,000 repositories would
/// take half a minute.
const PAGES_AT_ONCE: usize = 8;

/// A repository on GitHub, as its REST API describes it.
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

/// A repository in the REST API's JSON, before it is flattened into
/// [`RemoteRepo`]. Only the fields dowse shows; there are many more.
#[derive(Deserialize)]
struct RestRepo {
    name: String,
    full_name: String,
    owner: RestLogin,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    fork: bool,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    pushed_at: Option<String>,
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    html_url: String,
}

#[derive(Deserialize)]
struct RestLogin {
    login: String,
}

/// Parse one page of `GET /orgs/{org}/repos` (or `/users/…`, `/user/repos`).
pub fn parse_page(json: &str) -> Result<Vec<RemoteRepo>> {
    let repos: Vec<RestRepo> =
        serde_json::from_str(json).context("GitHub answered with something unexpected")?;
    Ok(repos
        .into_iter()
        .map(|repo| RemoteRepo {
            full_name: repo.full_name,
            name: repo.name,
            owner: repo.owner.login,
            description: repo.description.unwrap_or_default(),
            private: repo.private,
            fork: repo.fork,
            archived: repo.archived,
            language: repo.language,
            size_kb: repo.size.unwrap_or(0),
            pushed_at: repo.pushed_at,
            default_branch: repo.default_branch,
            url: repo.html_url,
        })
        .collect())
}

/// Whose repositories a listing reads, which decides the API's path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The signed-in user's own, private ones included.
    Me,
    Organization(String),
    /// Another user's; GitHub shows only the public ones.
    User(String),
}

impl Owner {
    /// The API path of `page` (from 1) of the owner's repositories, in name
    /// order: an order that stays put while pages are fetched at once.
    pub fn page_path(&self, page: usize) -> String {
        let (base, kind) = match self {
            Owner::Me => ("user/repos".to_string(), "affiliation=owner"),
            Owner::Organization(org) => (format!("orgs/{org}/repos"), "type=all"),
            Owner::User(user) => (format!("users/{user}/repos"), "type=owner"),
        };
        format!("{base}?{kind}&sort=full_name&per_page={PAGE_SIZE}&page={page}")
    }
}

/// A login as GitHub allows them, so it can go in a path unescaped.
pub fn check_login(login: &str) -> Result<&str> {
    let login = login.trim();
    if login.is_empty()
        || !login
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!("{login:?} is not a GitHub user or organization");
    }
    Ok(login)
}

/// Something that says when to give up.
type Cancelled<'a> = &'a (dyn Fn() -> bool + Sync);

/// Whether `owner` (the signed-in user when `None`) is an organization, a
/// user, or the signed-in user.
fn resolve_owner(owner: Option<&str>, cancelled: Cancelled) -> Result<Owner> {
    let Some(owner) = owner.map(str::trim).filter(|owner| !owner.is_empty()) else {
        return Ok(Owner::Me);
    };
    let owner = check_login(owner)?;
    let api = |path: &str, jq: &str| {
        let mut gh = process::command("gh");
        gh.args(["api", path, "--jq", jq]);
        process::output_until(gh, cancelled)
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    // Both at once: each is a round trip to GitHub.
    let (kind, me) = std::thread::scope(|scope| {
        let me = scope.spawn(|| api("user", ".login"));
        let kind = api(&format!("users/{owner}"), ".type");
        (kind, me.join().ok().and_then(Result::ok))
    });
    let kind = kind.map_err(|error| {
        if format!("{error:#}").contains("Not Found") {
            anyhow::anyhow!("GitHub has no user or organization called {owner}")
        } else {
            explain_gh(error)
        }
    })?;
    Ok(match kind.as_str() {
        "Organization" => Owner::Organization(owner.to_string()),
        _ if me.is_some_and(|me| me.eq_ignore_ascii_case(owner)) => Owner::Me,
        _ => Owner::User(owner.to_string()),
    })
}

/// How far a listing has got.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListProgress {
    pub pages_done: usize,
    /// How many pages there are, as the first one said.
    pub pages: usize,
    /// Repositories delivered so far.
    pub listed: usize,
}

/// The last page of a listing, from the `Link` header of its first page:
/// `<…&page=2>; rel="next", <…&page=19>; rel="last"`. One page when there is
/// no link.
pub fn last_page(headers: &str) -> usize {
    headers
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim().eq_ignore_ascii_case("link").then_some(value)
        })
        .flat_map(|value| value.split(','))
        .filter(|link| link.contains("rel=\"last\""))
        .filter_map(|link| {
            let url = link.split_once('<')?.1.split_once('>')?.0;
            let query = url.split_once('?')?.1;
            query.split('&').find_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                (key == "page").then(|| value.parse().ok()).flatten()
            })
        })
        .max()
        .unwrap_or(1)
}

/// `gh api --include` output split into its headers and its body.
fn split_response(text: &str) -> (&str, &str) {
    let blank = [
        text.find("\n\r\n").map(|at| (at, 3)),
        text.find("\n\n").map(|at| (at, 2)),
    ]
    .into_iter()
    .flatten()
    .min_by_key(|(at, _)| *at);
    match blank {
        Some((at, skip)) => (&text[..at], &text[at + skip..]),
        None => ("", text),
    }
}

/// A page of a listing. The first also says how many pages there are.
#[derive(Debug, Default)]
struct Page {
    repos: Vec<RemoteRepo>,
    last: Option<usize>,
}

/// Fetches one page of a listing, giving up when told to.
type FetchPage<'a> = &'a (dyn Fn(usize, Cancelled) -> Result<Page> + Sync);

/// Fetch every page of a listing, several at once, handing each to
/// `on_page` on this thread as it arrives, without repeating a repository
/// (one shifts between pages when another is created meanwhile). The first
/// page says how many there are; until it does, the pages after it are
/// fetched on the chance that they exist. Stops at the first failure, when
/// `cancel` is set, or once `limit` repositories have been delivered.
fn fetch_rest(
    limit: Option<usize>,
    fetch: FetchPage,
    cancel: &AtomicBool,
    on_page: &mut dyn FnMut(Vec<RemoteRepo>, ListProgress),
) -> Result<()> {
    let limit = limit.unwrap_or(usize::MAX).max(1);
    let most_pages = limit.div_ceil(PAGE_SIZE);
    // Unknown until the first page arrives.
    let last = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let cancelled = || cancel.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed);
    let next = AtomicUsize::new(1);
    let (sender, received) = mpsc::channel();
    let outcome = std::thread::scope(|scope| {
        for _ in 0..PAGES_AT_ONCE.min(most_pages) {
            let (sender, next, last, cancelled) = (sender.clone(), &next, &last, &cancelled);
            scope.spawn(move || {
                loop {
                    let page = next.fetch_add(1, Ordering::Relaxed);
                    // Guess no further than one round ahead of the first page.
                    while last.load(Ordering::Relaxed) == 0 && page > PAGES_AT_ONCE {
                        if cancelled() {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    let known = last.load(Ordering::Relaxed);
                    if (known != 0 && page > known) || page > most_pages || cancelled() {
                        break;
                    }
                    if sender.send((page, fetch(page, cancelled))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);

        let mut seen = HashSet::new();
        let mut progress = ListProgress::default();
        let mut failures: Vec<(usize, anyhow::Error)> = Vec::new();
        // Pages that arrived before the first said how many there are.
        let mut waiting = Vec::new();
        for (page, result) in received {
            if stop.load(Ordering::Relaxed) {
                continue;
            }
            match result {
                Ok(Page { repos, last: said }) => {
                    if let Some(said) = said {
                        progress.pages = said.clamp(1, most_pages);
                        last.store(progress.pages, Ordering::Relaxed);
                    }
                    waiting.push((page, repos));
                }
                // Without the first page there is nothing to go on.
                Err(error) if page == 1 => {
                    stop.store(true, Ordering::Relaxed);
                    failures.push((page, error));
                }
                Err(error) => failures.push((page, error)),
            }
            let known = last.load(Ordering::Relaxed);
            if known == 0 {
                continue;
            }
            // A guessed page past the end that failed does not count.
            if failures.iter().any(|(at, _)| *at <= known) {
                stop.store(true, Ordering::Relaxed);
                continue;
            }
            for (_, repos) in waiting.drain(..).filter(|(page, _)| *page <= known) {
                let fresh: Vec<RemoteRepo> = repos
                    .into_iter()
                    .filter(|repo| seen.insert(repo.full_name.clone()))
                    .take(limit - progress.listed)
                    .collect();
                progress.pages_done += 1;
                progress.listed += fresh.len();
                on_page(fresh, progress);
            }
            if progress.listed >= limit {
                stop.store(true, Ordering::Relaxed);
            }
        }
        let known = last.load(Ordering::Relaxed);
        failures
            .into_iter()
            .filter(|(at, _)| known == 0 || *at <= known)
            .min_by_key(|(at, _)| *at)
            .map_or(Ok(()), |(_, error)| Err(error))
    });
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    outcome
}

/// List `owner`'s repositories (the signed-in user's when `None`), up to
/// `limit`, handing them to `on_page` a page at a time as they arrive, in
/// no particular order. Blocks until every page is in, the first failure,
/// or `cancel` is set (which fails with "cancelled").
pub fn list_each(
    owner: Option<&str>,
    limit: Option<usize>,
    cancel: &AtomicBool,
    mut on_page: impl FnMut(Vec<RemoteRepo>, ListProgress),
) -> Result<()> {
    let cancelled = || cancel.load(Ordering::Relaxed);
    let owner = resolve_owner(owner, &cancelled)?;
    let fetch = |page: usize, cancelled: Cancelled| {
        let mut gh = process::command("gh");
        gh.arg("api");
        // Only the first page's headers are read, for how many there are.
        if page == 1 {
            gh.arg("--include");
        }
        gh.arg(owner.page_path(page));
        let output = process::output_until(gh, cancelled).map_err(explain_gh)?;
        let text = String::from_utf8_lossy(&output.stdout);
        if page == 1 {
            let (headers, body) = split_response(&text);
            Ok(Page {
                repos: parse_page(body)?,
                last: Some(last_page(headers)),
            })
        } else {
            Ok(Page {
                repos: parse_page(&text)?,
                last: None,
            })
        }
    };
    fetch_rest(limit, &fetch, cancel, &mut on_page)
}

/// List `owner`'s repositories (the signed-in user's when `None`), up to
/// `limit`, most recently pushed first. Blocks while GitHub answers.
pub fn list(owner: Option<&str>, limit: Option<usize>) -> Result<Vec<RemoteRepo>> {
    let mut repos = Vec::new();
    list_each(owner, limit, &AtomicBool::new(false), |page, _| {
        repos.extend(page)
    })?;
    sort_by_pushed(&mut repos);
    Ok(repos)
}

/// Most recently pushed first, then by name.
pub fn sort_by_pushed(repos: &mut [RemoteRepo]) {
    repos.sort_by(|a, b| {
        b.pushed_at
            .cmp(&a.pushed_at)
            .then_with(|| a.full_name.cmp(&b.full_name))
    });
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

    const SAMPLE: &str = r#"[{"id":1,"name":"dowse","full_name":"alice/dowse","private":false,"owner":{"login":"alice","id":2},"html_url":"https://github.com/alice/dowse","description":"","fork":false,"size":511,"language":"Rust","archived":false,"pushed_at":"2026-09-26T11:57:50Z","default_branch":"master","topics":[]},{"name":"old","full_name":"alice/old","private":true,"owner":{"login":"alice"},"html_url":"https://github.com/alice/old","description":null,"fork":true,"size":0,"language":null,"archived":true,"pushed_at":null,"default_branch":null}]"#;

    #[test]
    fn parses_a_page_of_the_rest_api() {
        let repos = parse_page(SAMPLE).unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].full_name, "alice/dowse");
        assert_eq!(repos[0].owner, "alice");
        assert_eq!(repos[0].language.as_deref(), Some("Rust"));
        assert_eq!(repos[0].size_kb, 511);
        assert_eq!(repos[0].default_branch.as_deref(), Some("master"));
        assert_eq!(repos[0].url, "https://github.com/alice/dowse");
        assert_eq!(repos[1].description, "");
        assert!(repos[1].private && repos[1].fork && repos[1].archived);
        assert_eq!(repos[1].language, None);
        assert_eq!(repos[1].default_branch, None);
        assert!(parse_page("[]").unwrap().is_empty());
        assert!(parse_page("not json").is_err());
    }

    #[test]
    fn reads_the_last_page_from_the_link_header() {
        let response = "HTTP/2.0 200 OK\nContent-Type: application/json\r\nLink: <https://api.github.com/organizations/1/repos?per_page=100&page=2>; rel=\"next\", <https://api.github.com/organizations/1/repos?per_page=100&page=19>; rel=\"last\"\r\n\r\n[]";
        let (headers, body) = split_response(response);
        assert_eq!(body, "[]");
        assert_eq!(last_page(headers), 19);
        // The last page links only back.
        assert_eq!(last_page("link: <https://x/?page=1>; rel=\"first\""), 1);
        assert_eq!(last_page(""), 1);
        assert_eq!(split_response("[]"), ("", "[]"));
    }

    #[test]
    fn pages_are_read_in_name_order_by_owner_kind() {
        assert_eq!(
            Owner::Me.page_path(3),
            "user/repos?affiliation=owner&sort=full_name&per_page=100&page=3"
        );
        assert_eq!(
            Owner::Organization("acme".into()).page_path(1),
            "orgs/acme/repos?type=all&sort=full_name&per_page=100&page=1"
        );
        assert!(
            Owner::User("bob".into())
                .page_path(1)
                .starts_with("users/bob/repos?type=owner&")
        );
        assert_eq!(check_login(" micro-soft ").unwrap(), "micro-soft");
        assert!(check_login("a/b").is_err());
        assert!(check_login("x?y").is_err());
    }

    fn repo(name: &str) -> RemoteRepo {
        RemoteRepo {
            full_name: format!("o/{name}"),
            name: name.into(),
            owner: "o".into(),
            description: String::new(),
            private: false,
            fork: false,
            archived: false,
            language: None,
            size_kb: 0,
            pushed_at: None,
            default_branch: None,
            url: String::new(),
        }
    }

    /// Page `n` of a listing of `total` repositories named `r0`, `r1`, ….
    fn page(n: usize, total: usize) -> Vec<RemoteRepo> {
        ((n - 1) * PAGE_SIZE..(n * PAGE_SIZE).min(total))
            .map(|i| repo(&format!("r{i}")))
            .collect()
    }

    /// Page `n` as GitHub answers it: the first says how many there are.
    fn answer(n: usize, total: usize) -> Result<Page> {
        Ok(Page {
            repos: if n <= total.div_ceil(PAGE_SIZE) {
                page(n, total)
            } else {
                Vec::new()
            },
            last: (n == 1).then(|| total.div_ceil(PAGE_SIZE).max(1)),
        })
    }

    #[test]
    fn fetches_every_page_once_and_reports_progress() {
        let total = 1859;
        let fetched = std::sync::Mutex::new(Vec::new());
        let fetch = |n: usize, _: Cancelled| {
            fetched.lock().unwrap().push(n);
            answer(n, total)
        };
        let mut names = HashSet::new();
        let mut last = ListProgress::default();
        let cancel = AtomicBool::new(false);
        fetch_rest(None, &fetch, &cancel, &mut |repos, progress| {
            names.extend(repos.into_iter().map(|repo| repo.full_name));
            last = progress;
        })
        .unwrap();
        assert_eq!(names.len(), total);
        assert_eq!(
            last,
            ListProgress {
                pages_done: 19,
                pages: 19,
                listed: total
            }
        );
        let mut pages = fetched.into_inner().unwrap();
        pages.sort();
        assert_eq!(pages, (1..=19).collect::<Vec<_>>());
    }

    #[test]
    fn pages_guessed_past_the_end_are_ignored() {
        // Three pages; the ones after them are fetched before the first
        // says so, and fail or come back empty.
        let fetch = |n: usize, _: Cancelled| {
            if n == 6 {
                bail!("HTTP 422")
            } else {
                answer(n, 250)
            }
        };
        let cancel = AtomicBool::new(false);
        let mut listed = 0;
        let mut last = ListProgress::default();
        fetch_rest(None, &fetch, &cancel, &mut |repos, progress| {
            listed += repos.len();
            last = progress;
        })
        .unwrap();
        assert_eq!(listed, 250);
        assert_eq!(last.pages_done, 3);

        let mut one = 0;
        fetch_rest(None, &|n, _| answer(n, 0), &cancel, &mut |repos, _| {
            one += 1 + repos.len()
        })
        .unwrap();
        assert_eq!(one, 1, "one empty page");
    }

    #[test]
    fn repeats_no_repository_and_stops_at_the_limit() {
        // Page 2 repeats page 1's last repository, as when one is created
        // while the listing runs.
        let fetch = |n: usize, _: Cancelled| {
            let mut answered = answer(n, 1000)?;
            if n == 2 {
                answered.repos.insert(0, repo("r99"));
            }
            Ok(answered)
        };
        let cancel = AtomicBool::new(false);
        let mut listed = Vec::new();
        fetch_rest(Some(250), &fetch, &cancel, &mut |repos, _| {
            listed.extend(repos)
        })
        .unwrap();
        assert_eq!(listed.len(), 250);
        let unique: HashSet<_> = listed.iter().map(|repo| &repo.full_name).collect();
        assert_eq!(unique.len(), 250);
    }

    #[test]
    fn a_failed_page_fails_the_listing_and_cancelling_stops_it() {
        let cancel = AtomicBool::new(false);
        let failing = |n: usize, _: Cancelled| {
            if n == 5 {
                bail!("HTTP 502")
            } else {
                answer(n, 1000)
            }
        };
        let error = fetch_rest(None, &failing, &cancel, &mut |_, _| {}).unwrap_err();
        assert_eq!(error.to_string(), "HTTP 502");
        let first = |n: usize, _: Cancelled| {
            if n == 1 {
                bail!("HTTP 401")
            } else {
                answer(n, 5000)
            }
        };
        let error = fetch_rest(None, &first, &cancel, &mut |_, _| {}).unwrap_err();
        assert_eq!(error.to_string(), "HTTP 401");

        // Cancelled while the first pages are fetched: no more are.
        let fetched = AtomicUsize::new(0);
        let cancelling = |n: usize, _: Cancelled| {
            fetched.fetch_add(1, Ordering::Relaxed);
            cancel.store(true, Ordering::Relaxed);
            answer(n, 5000)
        };
        let error = fetch_rest(None, &cancelling, &cancel, &mut |_, _| {}).unwrap_err();
        assert_eq!(error.to_string(), "cancelled");
        assert!(fetched.load(Ordering::Relaxed) <= PAGES_AT_ONCE);
    }

    #[test]
    fn sorts_the_latest_pushed_first() {
        let mut repos = vec![repo("a"), repo("b"), repo("c")];
        repos[0].pushed_at = Some("2026-01-01T00:00:00Z".into());
        repos[2].pushed_at = Some("2026-09-01T00:00:00Z".into());
        sort_by_pushed(&mut repos);
        let names: Vec<&str> = repos.iter().map(|repo| repo.name.as_str()).collect();
        assert_eq!(names, ["c", "a", "b"]);
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
    fn filters_by_words_forks_and_archived() {
        let repos = parse_page(SAMPLE).unwrap();
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
