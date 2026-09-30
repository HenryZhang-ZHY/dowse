//! App-wide settings, kept in `settings.json` beside the library. A
//! repository can override some of them in the library.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::index::display_path;
use super::store;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default, skip_serializing_if = "IndexSettings::is_default")]
    pub index: IndexSettings,
}

impl Settings {
    /// Read the settings, or the defaults when the file does not exist yet.
    pub fn load(file: &Path) -> Result<Self> {
        Ok(store::load_json(file, "settings file")?.unwrap_or_default())
    }

    pub fn save(&self, file: &Path) -> Result<()> {
        store::save_json(file, self)
    }

    /// Set a setting by its key, as the command line names it.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            LOCATION_KEY => self.index.location = Some(value.parse()?),
            EXTERNAL_DIR_KEY => {
                let dir = PathBuf::from(value);
                if !dir.is_absolute() {
                    return Err(format!("{key} must be an absolute path, not {value}"));
                }
                self.index.external_dir = Some(dir);
            }
            _ => return Err(unknown_key(key)),
        }
        Ok(())
    }

    /// Put a setting back to its default.
    pub fn unset(&mut self, key: &str) -> Result<(), String> {
        match key {
            LOCATION_KEY => self.index.location = None,
            EXTERNAL_DIR_KEY => self.index.external_dir = None,
            _ => return Err(unknown_key(key)),
        }
        Ok(())
    }

    /// Every setting: its key, its value, and whether that is the default.
    pub fn entries(&self) -> Vec<(&'static str, String, bool)> {
        vec![
            (
                LOCATION_KEY,
                self.index.location().to_string(),
                self.index.location.is_none(),
            ),
            (
                EXTERNAL_DIR_KEY,
                display_path(&self.index.external_dir()),
                self.index.external_dir.is_none(),
            ),
        ]
    }
}

/// Where the indexes of repositories without a setting of their own go.
pub const LOCATION_KEY: &str = "index.location";
/// The folder external indexes go under.
pub const EXTERNAL_DIR_KEY: &str = "index.external-dir";

fn unknown_key(key: &str) -> String {
    format!("there is no setting {key}; there are {LOCATION_KEY} and {EXTERNAL_DIR_KEY}")
}

/// Where indexes are kept. Unset fields take their defaults, so a file only
/// holds what the user changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexSettings {
    /// Where the indexes of repositories without a setting of their own go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<IndexLocation>,
    /// The folder external indexes go under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_dir: Option<PathBuf>,
}

impl IndexSettings {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn location(&self) -> IndexLocation {
        self.location.unwrap_or_default()
    }

    pub fn external_dir(&self) -> PathBuf {
        self.external_dir
            .clone()
            .unwrap_or_else(default_external_dir)
    }

    /// Where the index of a repository whose own setting is `own` goes.
    pub fn place(&self, own: Option<IndexLocation>) -> IndexPlace {
        match own.unwrap_or_else(|| self.location()) {
            IndexLocation::Repo => IndexPlace::Repo,
            IndexLocation::External => IndexPlace::External(self.external_dir()),
        }
    }
}

/// Where a repository's index is kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndexLocation {
    /// `<repo>/.tgrep`, where the tgrep command line looks for it.
    #[default]
    Repo,
    /// Under the external index folder, outside the working tree, where
    /// `git clean -x` cannot delete it.
    External,
}

impl IndexLocation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Repo => "repo",
            Self::External => "external",
        }
    }
}

impl std::fmt::Display for IndexLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for IndexLocation {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "repo" => Ok(Self::Repo),
            "external" => Ok(Self::External),
            _ => Err(format!(
                "{text:?} is not an index location; use repo or external"
            )),
        }
    }
}

/// A repository's index location resolved against the settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexPlace {
    Repo,
    /// Under this folder.
    External(PathBuf),
}

impl IndexPlace {
    pub fn location(&self) -> IndexLocation {
        match self {
            Self::Repo => IndexLocation::Repo,
            Self::External(_) => IndexLocation::External,
        }
    }
}

/// The external index folder when the settings name none: the user's local,
/// non-roaming data folder, or the configuration folder when
/// `DOWSE_CONFIG_DIR` points elsewhere, so a portable install or a test
/// keeps its indexes to itself.
pub fn default_external_dir() -> PathBuf {
    if std::env::var_os(super::config::CONFIG_DIR_ENV).is_some() {
        return super::config::default_root().join("indexes");
    }
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("dowse")
        .join("indexes")
}

/// The index folder of the repository at the canonical `root`, under
/// `external_dir`: its name, for people looking in the folder, and a hash of
/// its path, so two repositories with the same name do not share one.
pub fn external_index_dir(external_dir: &Path, root: &Path) -> PathBuf {
    let path = display_path(root);
    let name: String = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    external_dir.join(format!("{name}-{:016x}", fnv1a(path.as_bytes())))
}

/// FNV-1a: small, and stable across Rust versions and runs, unlike std's
/// hasher, so an index keeps its folder.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_leaves_defaults_out() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        assert_eq!(Settings::load(&file).unwrap(), Settings::default());

        Settings::default().save(&file).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "{}");

        let settings = Settings {
            index: IndexSettings {
                location: Some(IndexLocation::External),
                external_dir: Some(PathBuf::from("/indexes")),
            },
        };
        settings.save(&file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains(r#""location": "external""#), "{text}");
        assert_eq!(Settings::load(&file).unwrap(), settings);
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        std::fs::write(&file, "{ not json").unwrap();
        assert!(Settings::load(&file).is_err());
    }

    #[test]
    fn a_repository_setting_overrides_the_app_setting() {
        let mut settings = IndexSettings::default();
        assert_eq!(settings.place(None), IndexPlace::Repo);
        assert_eq!(
            settings.place(Some(IndexLocation::External)),
            IndexPlace::External(default_external_dir())
        );

        settings.location = Some(IndexLocation::External);
        settings.external_dir = Some(PathBuf::from("/indexes"));
        assert_eq!(
            settings.place(None),
            IndexPlace::External(PathBuf::from("/indexes"))
        );
        assert_eq!(settings.place(Some(IndexLocation::Repo)), IndexPlace::Repo);
    }

    #[test]
    fn external_index_folders_are_stable_and_distinct() {
        let external = Path::new("/indexes");
        let api = external_index_dir(external, Path::new("/mirrors/api"));
        assert_eq!(api, external_index_dir(external, Path::new("/mirrors/api")));
        assert!(
            api.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("api-"),
            "{api:?}"
        );
        assert_ne!(api, external_index_dir(external, Path::new("/dev/api")));
        let odd = external_index_dir(external, Path::new("/dev/my repo"));
        assert!(
            odd.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("my_repo-")
        );
    }

    #[test]
    fn sets_and_unsets_by_key() {
        let mut settings = Settings::default();
        assert!(settings.entries().iter().all(|(_, _, default)| *default));

        settings.set(LOCATION_KEY, "external").unwrap();
        let dir = std::env::temp_dir().join("indexes");
        settings
            .set(EXTERNAL_DIR_KEY, dir.to_str().unwrap())
            .unwrap();
        assert_eq!(settings.index.location, Some(IndexLocation::External));
        assert_eq!(settings.index.external_dir(), dir);
        assert!(settings.entries().iter().all(|(_, _, default)| !*default));
        assert_eq!(settings.entries()[0].1, "external");

        assert!(settings.set(LOCATION_KEY, "elsewhere").is_err());
        assert!(settings.set(EXTERNAL_DIR_KEY, "relative/dir").is_err());
        assert!(settings.set("index.colour", "blue").is_err());
        assert!(settings.unset("index.colour").is_err());

        settings.unset(LOCATION_KEY).unwrap();
        settings.unset(EXTERNAL_DIR_KEY).unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn parses_locations() {
        assert_eq!("repo".parse(), Ok(IndexLocation::Repo));
        assert_eq!("external".parse(), Ok(IndexLocation::External));
        assert!("elsewhere".parse::<IndexLocation>().is_err());
    }
}
