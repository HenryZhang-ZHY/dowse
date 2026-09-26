//! Every repository tgrep-gpui knows, with its display name and tags. A
//! repository's tags describe the clone itself (a mirror, a dev copy, its
//! owner), so they are kept here once rather than in each workspace.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{repo, store};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    pub repos: Vec<LibraryEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryEntry {
    /// The folder as the user would type it; also the repository's identity.
    pub path: PathBuf,
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Library {
    /// Read the library, or start empty when the file does not exist yet.
    pub fn load(file: &Path) -> Result<Self> {
        Ok(store::load_json(file, "repository library")?.unwrap_or_default())
    }

    pub fn save(&self, file: &Path) -> Result<()> {
        store::save_json(file, self)
    }

    pub fn get(&self, path: &Path) -> Option<&LibraryEntry> {
        self.repos.iter().find(|entry| entry.path == path)
    }

    /// Make sure the repository is known, giving a new one a unique display
    /// name. Returns `true` when it was added.
    pub fn ensure(&mut self, path: &Path) -> bool {
        if self.get(path).is_some() {
            return false;
        }
        let name = repo::unique_name(path, self.repos.iter().map(|entry| entry.name.as_str()));
        self.repos.push(LibraryEntry {
            path: path.to_path_buf(),
            name,
            tags: Vec::new(),
        });
        true
    }

    /// Replace the repository's tags. Returns `true` when they changed.
    pub fn set_tags(&mut self, path: &Path, tags: Vec<String>) -> bool {
        match self.repos.iter_mut().find(|entry| entry.path == path) {
            Some(entry) if entry.tags != tags => {
                entry.tags = tags;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_names_and_tags() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nested/library.json");
        assert_eq!(Library::load(&file).unwrap(), Library::default());

        let mut library = Library::default();
        assert!(library.ensure(Path::new("/m/api")));
        assert!(!library.ensure(Path::new("/m/api")));
        assert!(library.ensure(Path::new("/dev/api")));
        assert_eq!(library.repos[1].name, "dev/api");
        assert!(library.set_tags(Path::new("/dev/api"), vec!["dev".into()]));
        assert!(!library.set_tags(Path::new("/dev/api"), vec!["dev".into()]));
        assert!(!library.set_tags(Path::new("/nowhere"), vec!["dev".into()]));
        library.save(&file).unwrap();

        assert_eq!(Library::load(&file).unwrap(), library);
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_an_empty_library() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("library.json");
        std::fs::write(&file, "{ not json").unwrap();
        assert!(Library::load(&file).is_err());
    }
}
