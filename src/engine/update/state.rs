//! What dowse remembers about updates, in `update.json`: whether to look
//! for them, when it last looked and what it found, and the version the
//! user chose to skip.

use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use semver::Version;
use serde::{Deserialize, Serialize};

use super::release::Release;
use crate::engine::store;

/// How long an answer from GitHub is good for.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateState {
    /// Look for updates on a schedule. Checking by hand works either way.
    #[serde(default = "yes")]
    pub automatic: bool,
    /// When GitHub last answered, in seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<u64>,
    /// The `ETag` of that answer, so asking again costs nothing when it is
    /// unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// The latest release it named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<Release>,
    /// A version not to offer again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<Version>,
}

fn yes() -> bool {
    true
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            automatic: true,
            checked_at: None,
            etag: None,
            latest: None,
            skipped: None,
        }
    }
}

impl UpdateState {
    /// Read the state, or start afresh when the file does not exist yet.
    pub fn load(file: &Path) -> Result<Self> {
        Ok(store::load_json(file, "update file")?.unwrap_or_default())
    }

    pub fn save(&self, file: &Path) -> Result<()> {
        store::save_json(file, self)
    }

    pub fn checked_at(&self) -> Option<SystemTime> {
        self.checked_at
            .map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
    }

    /// Whether a scheduled check is due at `now`: never when turned off,
    /// otherwise once [`CHECK_EVERY`] has passed since GitHub last answered.
    /// A clock set back counts as due, rather than waiting until it catches up.
    pub fn is_due(&self, now: SystemTime) -> bool {
        self.automatic
            && self
                .checked_at()
                .is_none_or(|at| match now.duration_since(at) {
                    Ok(since) => since >= CHECK_EVERY,
                    Err(_) => true,
                })
    }

    /// Note GitHub's answer at `now`. `None` for the release means it was
    /// unchanged since the last answer.
    pub fn record(&mut self, release: Option<Release>, etag: Option<String>, now: SystemTime) {
        self.checked_at = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .map(|since| since.as_secs());
        if let Some(release) = release {
            self.latest = Some(release);
            self.etag = etag;
        } else if etag.is_some() {
            self.etag = etag;
        }
    }

    /// The release to offer instead of `current`: the latest one when it is
    /// newer, unless the user skipped it.
    pub fn offer(&self, current: &Version) -> Option<&Release> {
        self.newer_than(current)
            .filter(|release| self.skipped.as_ref() != Some(&release.version))
    }

    /// The latest release when it is newer than `current`, skipped or not.
    pub fn newer_than(&self, current: &Version) -> Option<&Release> {
        self.latest
            .as_ref()
            .filter(|release| release.version > *current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str) -> Release {
        Release {
            version: Version::parse(version).unwrap(),
            tag: format!("v{version}"),
            page: format!("https://github.com/HenryZhang-ZHY/dowse/releases/tag/v{version}"),
            notes: String::new(),
            assets: Vec::new(),
        }
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn starts_automatic_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("update.json");
        let fresh = UpdateState::load(&file).unwrap();
        assert!(fresh.automatic);
        assert_eq!(fresh, UpdateState::default());

        let mut state = UpdateState::default();
        state.record(Some(release("1.2.0")), Some("W/\"abc\"".into()), at(1000));
        state.skipped = Some(Version::new(1, 2, 0));
        state.automatic = false;
        state.save(&file).unwrap();
        assert_eq!(UpdateState::load(&file).unwrap(), state);
    }

    #[test]
    fn a_file_without_the_switch_is_automatic() {
        let state: UpdateState = serde_json::from_str("{}").unwrap();
        assert!(state.automatic);
    }

    #[test]
    fn a_check_is_due_a_day_after_the_last_answer() {
        let mut state = UpdateState::default();
        assert!(state.is_due(at(0)));
        let day = CHECK_EVERY.as_secs();
        state.record(None, None, at(10 * day));
        assert!(!state.is_due(at(10 * day + day - 1)));
        assert!(state.is_due(at(10 * day + day)));
        // The clock went back.
        assert!(state.is_due(at(5 * day)));

        state.automatic = false;
        assert!(!state.is_due(at(20 * day)));
    }

    #[test]
    fn an_unchanged_answer_keeps_the_release_it_had() {
        let mut state = UpdateState::default();
        state.record(Some(release("1.2.0")), Some("one".into()), at(1));
        state.record(None, None, at(2));
        assert_eq!(state.latest, Some(release("1.2.0")));
        assert_eq!(state.etag.as_deref(), Some("one"));
        assert_eq!(state.checked_at, Some(2));
        // An answer without an ETag forgets the old one.
        state.record(Some(release("1.3.0")), None, at(3));
        assert_eq!(state.etag, None);
    }

    #[test]
    fn offers_a_newer_release_unless_skipped() {
        let current = Version::new(1, 1, 1);
        let mut state = UpdateState::default();
        assert!(state.offer(&current).is_none());

        state.latest = Some(release("1.1.1"));
        assert!(state.offer(&current).is_none());

        state.latest = Some(release("1.2.0"));
        assert_eq!(state.offer(&current), Some(&release("1.2.0")));

        state.skipped = Some(Version::new(1, 2, 0));
        assert!(state.offer(&current).is_none());
        assert_eq!(state.newer_than(&current), Some(&release("1.2.0")));

        // Skipping one version does not skip the next.
        state.latest = Some(release("1.3.0"));
        assert_eq!(state.offer(&current), Some(&release("1.3.0")));
    }
}
