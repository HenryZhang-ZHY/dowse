//! What to bring back on the next start: the open windows and their
//! workspaces, recently opened workspace files, and the scope last used
//! with each one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::github::CloneMode;
use super::query::SearchQuery;
use super::store;

/// Recent workspace files kept.
const MAX_RECENT: usize = 10;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Open windows, the most recently focused last.
    #[serde(default)]
    pub windows: Vec<WindowSession>,
    /// Saved workspaces opened lately, the most recent first.
    #[serde(default)]
    pub recent: Vec<PathBuf>,
    /// The scope last used with each saved workspace.
    #[serde(default)]
    pub scopes: BTreeMap<PathBuf, Vec<String>>,
    /// What the last clone from GitHub used, to start the next one from.
    #[serde(default)]
    pub clone: CloneDefaults,
}

/// The choices of the last clone from GitHub.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneDefaults {
    /// The folder clones go under, as `<root>/<owner>/<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    #[serde(default)]
    pub mode: CloneMode,
    /// The owner last listed; `None` for the signed-in user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// One window: a saved workspace, or the repositories of an untitled one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    /// An untitled workspace's repositories.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<String>,
    /// The window's search tabs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<SearchQuery>,
    /// The tab shown, an index into `tabs`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub active_tab: usize,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

impl Session {
    /// Read the session, or start empty when the file does not exist yet.
    pub fn load(file: &Path) -> Result<Self> {
        Ok(store::load_json(file, "session file")?.unwrap_or_default())
    }

    pub fn save(&self, file: &Path) -> Result<()> {
        store::save_json(file, self)
    }

    /// Put a workspace file at the top of the recent list.
    pub fn remember(&mut self, file: &Path) {
        self.recent.retain(|recent| recent != file);
        self.recent.insert(0, file.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }

    /// Drop a workspace file that can no longer be opened.
    pub fn forget(&mut self, file: &Path) {
        self.recent.retain(|recent| recent != file);
        self.scopes.remove(file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_windows_recents_and_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("session.json");
        assert_eq!(Session::load(&file).unwrap(), Session::default());

        let mut session = Session {
            windows: vec![
                WindowSession {
                    workspace: Some("/w/team.dowse-workspace".into()),
                    repos: vec![],
                    scope: vec!["dev".into()],
                    tabs: vec![
                        SearchQuery {
                            pattern: "fn main".into(),
                            ..Default::default()
                        },
                        SearchQuery {
                            pattern: r"todo\(".into(),
                            regex: true,
                            path_filter: "*.rs".into(),
                            ..Default::default()
                        },
                    ],
                    active_tab: 1,
                },
                WindowSession {
                    workspace: None,
                    repos: vec!["/src/api".into()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        session
            .scopes
            .insert("/w/team.dowse-workspace".into(), vec!["dev".into()]);
        session.clone = CloneDefaults {
            root: Some("/src/mirrors".into()),
            mode: CloneMode::Shallow,
            owner: Some("microsoft".into()),
        };
        session.save(&file).unwrap();
        assert_eq!(Session::load(&file).unwrap(), session);
    }

    #[test]
    fn recent_list_is_most_recent_first_without_duplicates() {
        let mut session = Session::default();
        for n in 0..12 {
            session.remember(Path::new(&format!("/w/{n}")));
        }
        session.remember(Path::new("/w/5"));
        assert_eq!(session.recent.len(), MAX_RECENT);
        assert_eq!(session.recent[0], Path::new("/w/5"));
        assert_eq!(session.recent[1], Path::new("/w/11"));
        assert_eq!(
            session
                .recent
                .iter()
                .filter(|p| *p == Path::new("/w/5"))
                .count(),
            1
        );
        session.forget(Path::new("/w/5"));
        assert_eq!(session.recent[0], Path::new("/w/11"));
    }
}
