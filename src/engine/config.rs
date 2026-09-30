//! Where dowse keeps its settings.

use std::path::{Path, PathBuf};

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

    /// App-wide settings, such as where indexes are kept.
    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
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
}
