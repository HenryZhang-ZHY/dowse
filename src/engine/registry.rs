//! The saved list of repositories, their tags and the last search scope.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use super::repo;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub repos: Vec<RepoEntry>,
    /// Tags selected in the scope bar.
    #[serde(default)]
    pub scope: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEntry {
    /// The folder as the user would type it; also the repository's identity.
    pub path: PathBuf,
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Registry {
    /// Read the registry, or start empty when the file does not exist yet.
    pub fn load(file: &Path) -> Result<Self> {
        match std::fs::read_to_string(file) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("{} is not a valid repository list", file.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("cannot read {}", file.display())),
        }
    }

    /// Write the registry atomically, so a crash never leaves half a file.
    pub fn save(&self, file: &Path) -> Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let staging = file.with_extension("json.tmp");
        std::fs::write(&staging, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&staging, file).with_context(|| format!("cannot write {}", file.display()))
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.repos.iter().any(|entry| entry.path == path)
    }

    /// Add a repository with a unique display name. Returns `false` when it
    /// was already registered.
    pub fn add(&mut self, path: PathBuf, tags: Vec<String>) -> bool {
        if self.contains(&path) {
            return false;
        }
        let name = repo::unique_name(&path, self.repos.iter().map(|entry| entry.name.as_str()));
        self.repos.push(RepoEntry { path, name, tags });
        true
    }

    pub fn remove(&mut self, path: &Path) {
        self.repos.retain(|entry| entry.path != path);
    }

    pub fn set_tags(&mut self, path: &Path, tags: Vec<String>) {
        if let Some(entry) = self.repos.iter_mut().find(|entry| entry.path == path) {
            entry.tags = tags;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_deduplicates() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nested/repos.json");
        assert_eq!(Registry::load(&file).unwrap(), Registry::default());

        let mut registry = Registry::default();
        assert!(registry.add(PathBuf::from("/m/api"), vec!["mirror".into()]));
        assert!(!registry.add(PathBuf::from("/m/api"), vec![]));
        assert!(registry.add(PathBuf::from("/dev/api"), vec!["dev".into()]));
        assert_eq!(registry.repos[1].name, "dev/api");
        registry.set_tags(Path::new("/dev/api"), vec!["dev".into(), "owner:me".into()]);
        registry.scope = vec!["dev".into()];
        registry.save(&file).unwrap();

        let loaded = Registry::load(&file).unwrap();
        assert_eq!(loaded, registry);

        let mut loaded = loaded;
        loaded.remove(Path::new("/m/api"));
        assert_eq!(loaded.repos.len(), 1);
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("repos.json");
        std::fs::write(&file, "{ not json").unwrap();
        assert!(Registry::load(&file).is_err());
    }
}
