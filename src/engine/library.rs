//! Every repository dowse knows, with its display name and tags. A
//! repository's tags describe the clone itself (a mirror, a dev copy, its
//! owner), so they are kept here once rather than in each workspace.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::settings::IndexLocation;
use super::sync::Interval;
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
    /// How often dowse pulls it, when it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_every: Option<Interval>,
    /// When dowse last tried to pull it, in seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulled_at: Option<u64>,
    /// Where its index is kept; the app's setting when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_location: Option<IndexLocation>,
}

impl LibraryEntry {
    pub fn pulled_at(&self) -> Option<SystemTime> {
        self.pulled_at
            .map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
    }
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
            pull_every: None,
            pulled_at: None,
            index_location: None,
        });
        true
    }

    /// Forget the repository. Returns `true` when it was known.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.repos.len();
        self.repos.retain(|entry| entry.path != path);
        self.repos.len() != before
    }

    /// Pull the repository every `every`, or never. Returns `true` when that
    /// changed.
    pub fn set_pull_every(&mut self, path: &Path, every: Option<Interval>) -> bool {
        match self.repos.iter_mut().find(|entry| entry.path == path) {
            Some(entry) if entry.pull_every != every => {
                entry.pull_every = every;
                true
            }
            _ => false,
        }
    }

    /// Keep the repository's index at `location`, or where the app's setting
    /// says with `None`. Returns `true` when that changed.
    pub fn set_index_location(&mut self, path: &Path, location: Option<IndexLocation>) -> bool {
        match self.repos.iter_mut().find(|entry| entry.path == path) {
            Some(entry) if entry.index_location != location => {
                entry.index_location = location;
                true
            }
            _ => false,
        }
    }

    /// Note that the repository was pulled at `time`.
    pub fn set_pulled_at(&mut self, path: &Path, time: SystemTime) {
        if let Some(entry) = self.repos.iter_mut().find(|entry| entry.path == path) {
            entry.pulled_at = time
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .map(|since| since.as_secs());
        }
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
        let hourly = "1h".parse().ok();
        assert!(library.set_pull_every(Path::new("/m/api"), hourly));
        assert!(!library.set_pull_every(Path::new("/m/api"), hourly));
        let external = Some(IndexLocation::External);
        assert!(library.set_index_location(Path::new("/dev/api"), external));
        assert!(!library.set_index_location(Path::new("/dev/api"), external));
        assert!(!library.set_index_location(Path::new("/nowhere"), external));
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        library.set_pulled_at(Path::new("/m/api"), now);
        assert_eq!(library.repos[0].pulled_at(), Some(now));
        library.save(&file).unwrap();

        assert_eq!(Library::load(&file).unwrap(), library);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains(r#""pull_every": "1h""#), "{text}");
        assert_eq!(
            text.matches("pull_every").count(),
            1,
            "unset fields stay out"
        );
        assert!(text.contains(r#""index_location": "external""#), "{text}");
        assert_eq!(text.matches("index_location").count(), 1);

        assert!(library.remove(Path::new("/m/api")));
        assert!(!library.remove(Path::new("/m/api")));
        assert_eq!(library.repos.len(), 1);
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_an_empty_library() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("library.json");
        std::fs::write(&file, "{ not json").unwrap();
        assert!(Library::load(&file).is_err());
    }
}
