//! Keeping a clone in step with its remote's default branch: fetch, then
//! fast-forward when that is safe, and say why not otherwise. Nothing here
//! merges, rebases or stashes, so a working copy is never changed behind its
//! owner's back.

use std::fmt;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::process;

/// The implicit tag group naming how often a repository is pulled.
pub const SYNC_GROUP: &str = "sync";

/// How often a repository is pulled: a whole number of minutes, hours or
/// days, written `15m`, `1h`, `3d`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Interval(Duration);

impl Interval {
    /// Pulling more often than this only loads the remote.
    pub const MIN: Duration = Duration::from_secs(5 * 60);
    /// The choices the repositories page offers.
    pub const PRESETS: [&'static str; 7] = ["15m", "30m", "1h", "6h", "1d", "3d", "7d"];

    pub fn duration(self) -> Duration {
        self.0
    }

    /// Whether a repository last pulled at `last` should be pulled at `now`.
    pub fn is_due(self, last: Option<SystemTime>, now: SystemTime) -> bool {
        match last {
            None => true,
            Some(last) => now.duration_since(last).unwrap_or_default() >= self.0,
        }
    }
}

impl FromStr for Interval {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let text = text.trim().to_ascii_lowercase();
        let split = text
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len());
        let (number, unit) = text.split_at(split);
        let number: u64 = number
            .parse()
            .map_err(|_| format!("`{text}` is not an interval such as 15m, 1h or 3d"))?;
        let seconds = match unit.trim() {
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            "d" | "day" | "days" => 86400,
            "w" | "week" | "weeks" => 7 * 86400,
            _ => {
                return Err(format!(
                    "`{text}` needs a unit: m, h or d, as in 15m, 1h or 3d"
                ));
            }
        };
        let duration = Duration::from_secs(number.saturating_mul(seconds));
        if duration < Self::MIN {
            return Err(format!("`{text}` is too often; pull at most every 5m"));
        }
        Ok(Self(duration))
    }
}

impl fmt::Display for Interval {
    /// The largest unit that divides it: `90m`, `2h`, `3d`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let minutes = self.0.as_secs() / 60;
        if minutes.is_multiple_of(24 * 60) {
            write!(f, "{}d", minutes / (24 * 60))
        } else if minutes.is_multiple_of(60) {
            write!(f, "{}h", minutes / 60)
        } else {
            write!(f, "{minutes}m")
        }
    }
}

impl Serialize for Interval {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Interval {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// What a pull did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PullOutcome {
    /// Fast-forwarded `branch` by `commits`.
    Updated {
        branch: String,
        commits: usize,
    },
    UpToDate {
        branch: String,
    },
    /// Fetched, but left the working copy as it was.
    Skipped {
        reason: String,
    },
}

impl PullOutcome {
    pub fn summary(&self) -> String {
        match self {
            PullOutcome::Updated { branch, commits } => format!(
                "pulled {commits} new commit{} on {branch}",
                if *commits == 1 { "" } else { "s" }
            ),
            PullOutcome::UpToDate { branch } => format!("{branch} is up to date"),
            PullOutcome::Skipped { reason } => format!("fetched only: {reason}"),
        }
    }
}

/// Fetch `root`'s `origin`, then fast-forward its checked-out branch to
/// origin's default branch when it is that branch, it has no uncommitted
/// changes to tracked files and no commits of its own.
pub fn pull(root: &Path, cancel: &AtomicBool) -> Result<PullOutcome> {
    if !root.join(".git").exists() {
        bail!("{} is not a git repository", root.display());
    }
    if git_text(root, &["remote"])?
        .lines()
        .all(|remote| remote.trim() != "origin")
    {
        bail!("it has no remote called origin");
    }
    let mut fetch = process::git(root);
    fetch.args(["fetch", "--prune", "--progress", "origin"]);
    process::run_with_progress(fetch, cancel, |_| {})?;

    let default = default_branch(root)?;
    let Ok(branch) = git_text(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]) else {
        return Ok(skipped("HEAD is detached"));
    };
    if branch != default {
        return Ok(skipped(format!("on {branch}, not {default}")));
    }
    if !git_text(root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Ok(skipped("uncommitted changes"));
    }
    let upstream = format!("origin/{default}");
    let counts = git_text(
        root,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...{upstream}"),
        ],
    )?;
    let mut numbers = counts
        .split_whitespace()
        .map(|number| number.parse::<usize>().unwrap_or(0));
    let (ahead, behind) = (numbers.next().unwrap_or(0), numbers.next().unwrap_or(0));
    if ahead > 0 {
        return Ok(skipped(format!(
            "{ahead} local commit{} not on {upstream}",
            if ahead == 1 { "" } else { "s" }
        )));
    }
    if behind == 0 {
        return Ok(PullOutcome::UpToDate { branch });
    }
    let mut merge = process::git(root);
    merge.args(["merge", "--ff-only", "--quiet", &upstream]);
    process::output(merge)?;
    Ok(PullOutcome::Updated {
        branch,
        commits: behind,
    })
}

fn skipped(reason: impl Into<String>) -> PullOutcome {
    PullOutcome::Skipped {
        reason: reason.into(),
    }
}

/// origin's default branch: what `origin/HEAD` points at, else `main` or
/// `master`, whichever origin has.
fn default_branch(root: &Path) -> Result<String> {
    if let Ok(head) = git_text(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) && let Some(branch) = head.strip_prefix("origin/")
    {
        return Ok(branch.to_string());
    }
    for branch in ["main", "master"] {
        if git_text(
            root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/remotes/origin/{branch}"),
            ],
        )
        .is_ok()
        {
            return Ok(branch.to_string());
        }
    }
    bail!("cannot tell origin's default branch; run `git remote set-head origin --auto`")
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let mut git = process::git(root);
    git.args(args);
    process::stdout(git)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_parse_and_print_in_their_largest_unit() {
        let parse = |text: &str| text.parse::<Interval>();
        assert_eq!(parse("15m").unwrap().duration(), Duration::from_secs(900));
        assert_eq!(parse(" 2H ").unwrap().to_string(), "2h");
        assert_eq!(parse("90m").unwrap().to_string(), "90m");
        assert_eq!(parse("120m").unwrap().to_string(), "2h");
        assert_eq!(parse("3d").unwrap().to_string(), "3d");
        assert_eq!(parse("1w").unwrap().to_string(), "7d");
        assert_eq!(parse("48 hours").unwrap().to_string(), "2d");
        assert!(parse("1m").unwrap_err().contains("too often"));
        assert!(parse("60").unwrap_err().contains("unit"));
        assert!(parse("soon").is_err());
        let json = serde_json::to_string(&parse("6h").unwrap()).unwrap();
        assert_eq!(json, "\"6h\"");
        assert_eq!(
            serde_json::from_str::<Interval>(&json).unwrap(),
            parse("6h").unwrap()
        );
        assert!(serde_json::from_str::<Interval>("\"1s\"").is_err());
    }

    #[test]
    fn a_pull_is_due_once_the_interval_has_passed() {
        let hour: Interval = "1h".parse().unwrap();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100_000);
        assert!(hour.is_due(None, now));
        assert!(!hour.is_due(Some(now - Duration::from_secs(3599)), now));
        assert!(hour.is_due(Some(now - Duration::from_secs(3600)), now));
        assert!(!hour.is_due(Some(now + Duration::from_secs(60)), now));
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let mut git = process::git(root);
        git.args(["-c", "user.name=t", "-c", "user.email=t@t"]);
        git.args(args);
        process::stdout(git).unwrap()
    }

    fn commit(root: &Path, file: &str, text: &str) {
        std::fs::write(root.join(file), text).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "--quiet", "-m", text]);
    }

    /// An origin, a clone of it, and a second clone to push new commits from.
    fn remote_and_clones() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        git(
            &origin,
            &["init", "--quiet", "--bare", "--initial-branch=main"],
        );
        let upstream = dir.path().join("upstream");
        git(
            dir.path(),
            &["clone", "--quiet", origin.to_str().unwrap(), "upstream"],
        );
        git(&upstream, &["checkout", "--quiet", "-b", "main"]);
        commit(&upstream, "a.txt", "one");
        git(&upstream, &["push", "--quiet", "origin", "main"]);
        git(
            dir.path(),
            &["clone", "--quiet", origin.to_str().unwrap(), "mirror"],
        );
        let mirror = dir.path().join("mirror");
        (dir, mirror, upstream)
    }

    #[test]
    fn fast_forwards_the_default_branch() {
        let (_dir, mirror, upstream) = remote_and_clones();
        let cancel = AtomicBool::new(false);
        assert_eq!(
            pull(&mirror, &cancel).unwrap(),
            PullOutcome::UpToDate {
                branch: "main".into()
            }
        );
        commit(&upstream, "a.txt", "two");
        commit(&upstream, "b.txt", "three");
        git(&upstream, &["push", "--quiet", "origin", "main"]);
        // Untracked files do not get in the way.
        std::fs::write(mirror.join("scratch.txt"), "mine").unwrap();
        let outcome = pull(&mirror, &cancel).unwrap();
        assert_eq!(
            outcome,
            PullOutcome::Updated {
                branch: "main".into(),
                commits: 2
            }
        );
        assert_eq!(outcome.summary(), "pulled 2 new commits on main");
        assert_eq!(
            std::fs::read_to_string(mirror.join("a.txt")).unwrap(),
            "two"
        );
    }

    #[test]
    fn leaves_working_copies_alone_and_says_why() {
        let (_dir, mirror, upstream) = remote_and_clones();
        let cancel = AtomicBool::new(false);
        commit(&upstream, "a.txt", "two");
        git(&upstream, &["push", "--quiet", "origin", "main"]);

        std::fs::write(mirror.join("a.txt"), "edited").unwrap();
        assert_eq!(
            pull(&mirror, &cancel).unwrap(),
            skipped("uncommitted changes")
        );
        git(&mirror, &["checkout", "--quiet", "--", "a.txt"]);

        git(&mirror, &["checkout", "--quiet", "-b", "feature"]);
        assert_eq!(
            pull(&mirror, &cancel).unwrap(),
            skipped("on feature, not main")
        );
        git(&mirror, &["checkout", "--quiet", "main"]);

        commit(&mirror, "c.txt", "local");
        assert_eq!(
            pull(&mirror, &cancel).unwrap(),
            skipped("1 local commit not on origin/main")
        );
        // It still fetched.
        let behind = git(&mirror, &["rev-list", "--count", "HEAD..origin/main"]);
        assert_eq!(behind, "1");

        git(&mirror, &["checkout", "--quiet", "--detach"]);
        assert_eq!(pull(&mirror, &cancel).unwrap(), skipped("HEAD is detached"));
    }

    #[test]
    fn refuses_folders_that_cannot_be_pulled() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        assert!(pull(dir.path(), &cancel).is_err());
        git(dir.path(), &["init", "--quiet"]);
        let error = pull(dir.path(), &cancel).unwrap_err().to_string();
        assert!(error.contains("origin"), "{error}");
    }
}
