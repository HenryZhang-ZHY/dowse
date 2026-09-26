//! Where tgrep-gpui keeps its settings, and moving settings from earlier
//! versions forward.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::library::{Library, LibraryEntry};
use super::registry::Registry;
use super::session::{Session, WindowSession};
use super::workspace;

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

    /// Where workspace files are saved unless the user picks elsewhere.
    pub fn workspaces_dir(&self) -> PathBuf {
        self.root.join("workspaces")
    }

    /// Earlier versions kept one list of repositories, their tags and the
    /// scope here.
    fn legacy_registry_file(&self) -> PathBuf {
        self.root.join("repos.json")
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
    fn a_corrupt_legacy_file_is_reported_and_not_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(dir.path());
        std::fs::write(dir.path().join("repos.json"), "{ nope").unwrap();
        assert!(config.migrate().is_err());
        assert!(!config.library_file().exists());
    }
}
