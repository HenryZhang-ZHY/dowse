//! Where dowse keeps its settings, and moving settings from earlier
//! versions forward.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use super::library::{Library, LibraryEntry};
use super::registry::Registry;
use super::session::{Session, WindowSession};
use super::workspace;

/// Overrides where settings are kept, e.g. for a portable install or tests.
pub const CONFIG_DIR_ENV: &str = "DOWSE_CONFIG_DIR";

/// Where settings are kept: the user configuration directory's `dowse`, or
/// `DOWSE_CONFIG_DIR`.
pub fn default_root() -> PathBuf {
    match std::env::var_os(CONFIG_DIR_ENV) {
        Some(dir) => super::repo::identity(Path::new(&dir)),
        None => user_config_dir().join("dowse"),
    }
}

/// Where settings were kept before the app was renamed from tgrep-gpui, when
/// the settings in use are the default ones.
pub fn legacy_root() -> Option<PathBuf> {
    std::env::var_os(CONFIG_DIR_ENV)
        .is_none()
        .then(|| user_config_dir().join("tgrep-gpui"))
}

fn user_config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir)
}

/// The configuration directory and the files in it.
#[derive(Clone, Debug)]
pub struct ConfigDir {
    root: PathBuf,
}

impl ConfigDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every known repository with its name and tags.
    pub fn library_file(&self) -> PathBuf {
        self.root.join("library.json")
    }

    /// The windows to restore and recently opened workspaces.
    pub fn session_file(&self) -> PathBuf {
        self.root.join("session.json")
    }

    /// The app's log files.
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Where workspace files are saved unless the user picks elsewhere.
    pub fn workspaces_dir(&self) -> PathBuf {
        self.root.join("workspaces")
    }

    /// Earlier versions kept one list of repositories, their tags and the
    /// scope here.
    fn legacy_registry_file(&self) -> PathBuf {
        self.root.join("repos.json")
    }

    /// Take over the settings the app kept under its earlier name,
    /// tgrep-gpui, when there are none here yet: the folder moves here, and
    /// the session's references to workspaces saved inside it follow.
    /// Returns whether anything was adopted.
    pub fn adopt_legacy(&self, legacy: &Path) -> Result<bool> {
        if self.root.exists() || !legacy.is_dir() {
            return Ok(false);
        }
        if let Some(parent) = self.root.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(legacy, &self.root).with_context(|| {
            format!(
                "could not move {} to {}",
                legacy.display(),
                self.root.display()
            )
        })?;
        let file = self.session_file();
        if file.exists() {
            let mut session = Session::load(&file)?;
            let moved = |path: &mut PathBuf| {
                if let Ok(relative) = path.strip_prefix(legacy) {
                    *path = self.root.join(relative);
                }
            };
            for window in &mut session.windows {
                window.workspace.as_mut().map(moved);
            }
            session.recent.iter_mut().for_each(moved);
            session.scopes = std::mem::take(&mut session.scopes)
                .into_iter()
                .map(|(mut path, scope)| {
                    moved(&mut path);
                    (path, scope)
                })
                .collect();
            session.save(&file)?;
        }
        Ok(true)
    }

    /// Carry an earlier version's repository list forward, once: its
    /// repositories and tags become the library, and the list becomes a
    /// saved "Default" workspace that the next start opens. `repos.json` is
    /// left in place. Returns whether anything was migrated.
    pub fn migrate(&self) -> Result<bool> {
        let legacy = self.legacy_registry_file();
        if self.library_file().exists() || !legacy.exists() {
            return Ok(false);
        }
        let registry = Registry::load(&legacy)?;
        if !registry.repos.is_empty() {
            let default = self
                .workspaces_dir()
                .join(format!("Default.{}", workspace::EXTENSION));
            let repos: Vec<PathBuf> = registry.repos.iter().map(|e| e.path.clone()).collect();
            workspace::save(&default, &repos)?;
            let mut session = Session::load(&self.session_file())?;
            session.windows.push(WindowSession {
                workspace: Some(default.clone()),
                repos: Vec::new(),
                scope: registry.scope.clone(),
                ..Default::default()
            });
            session.remember(&default);
            session.scopes.insert(default, registry.scope.clone());
            session.save(&self.session_file())?;
        }
        // Written last: its presence marks the migration done.
        let library = Library {
            repos: registry
                .repos
                .into_iter()
                .map(|entry| LibraryEntry {
                    path: entry.path,
                    name: entry.name,
                    tags: entry.tags,
                    pull_every: None,
                    pulled_at: None,
                })
                .collect(),
        };
        library.save(&self.library_file())?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_the_repository_list_once() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(dir.path());
        assert!(!config.migrate().unwrap());

        let api = dir.path().join("api");
        std::fs::create_dir_all(&api).unwrap();
        let api = super::super::repo::identity(&api);
        std::fs::write(
            dir.path().join("repos.json"),
            serde_json::json!({
                "repos": [{ "path": api, "name": "api", "tags": ["mirror"] }],
                "scope": ["mirror"],
            })
            .to_string(),
        )
        .unwrap();

        assert!(config.migrate().unwrap());
        let library = Library::load(&config.library_file()).unwrap();
        assert_eq!(library.repos.len(), 1);
        assert_eq!(library.repos[0].tags, vec!["mirror".to_string()]);

        let session = Session::load(&config.session_file()).unwrap();
        let default = session.windows[0].workspace.clone().unwrap();
        assert_eq!(session.windows[0].scope, vec!["mirror".to_string()]);
        assert_eq!(session.recent, vec![default.clone()]);
        assert_eq!(workspace::load(&default).unwrap(), vec![api]);

        // A second start leaves everything alone.
        assert!(!config.migrate().unwrap());
        assert_eq!(Session::load(&config.session_file()).unwrap(), session);
        assert!(dir.path().join("repos.json").exists());
    }

    #[test]
    fn adopts_the_settings_of_tgrep_gpui() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("tgrep-gpui");
        let saved = legacy.join("workspaces").join("team.tgrep-workspace");
        workspace::save(&saved, &[]).unwrap();
        let elsewhere = dir.path().join("elsewhere.tgrep-workspace");
        let mut session = Session::default();
        session.windows.push(WindowSession {
            workspace: Some(saved.clone()),
            ..Default::default()
        });
        session.remember(&elsewhere);
        session.remember(&saved);
        session.scopes.insert(saved.clone(), vec!["dev".into()]);
        session.save(&legacy.join("session.json")).unwrap();
        Library::default()
            .save(&legacy.join("library.json"))
            .unwrap();

        let config = ConfigDir::new(dir.path().join("dowse"));
        assert!(config.adopt_legacy(&legacy).unwrap());
        assert!(!legacy.exists());
        assert!(config.library_file().exists());

        let moved = config.workspaces_dir().join("team.tgrep-workspace");
        assert!(moved.exists());
        let session = Session::load(&config.session_file()).unwrap();
        assert_eq!(session.windows[0].workspace, Some(moved.clone()));
        assert_eq!(session.recent, vec![moved.clone(), elsewhere]);
        assert_eq!(session.scopes.get(&moved), Some(&vec!["dev".to_string()]));

        // Once there are settings of its own, nothing is adopted.
        std::fs::create_dir_all(&legacy).unwrap();
        assert!(!config.adopt_legacy(&legacy).unwrap());
    }

    #[test]
    fn a_corrupt_legacy_file_is_reported_and_not_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(dir.path());
        std::fs::write(dir.path().join("repos.json"), "{ nope").unwrap();
        assert!(config.migrate().is_err());
        assert!(!config.library_file().exists());
    }
}
