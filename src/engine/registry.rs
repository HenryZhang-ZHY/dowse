//! The repository list earlier versions saved in `repos.json`: repositories,
//! their tags and the last scope. Only read, to migrate it (see
//! [`super::config::ConfigDir::migrate`]).

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Deserialize;

use super::store;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub repos: Vec<RepoEntry>,
    /// Tags selected in the scope bar.
    #[serde(default)]
    pub scope: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RepoEntry {
    pub path: PathBuf,
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Registry {
    /// Read the list, or an empty one when the file does not exist.
    pub fn load(file: &Path) -> Result<Self> {
        Ok(store::load_json(file, "repository list")?.unwrap_or_default())
    }
}
