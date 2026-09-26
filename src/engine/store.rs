//! Reading and writing the small JSON files dowse keeps.

use std::path::Path;

use anyhow::{Context as _, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Read `file`, or `None` when it does not exist. A file that exists but
/// cannot be parsed is an error, so callers never overwrite it by mistake.
pub fn load_json<T: DeserializeOwned>(file: &Path, what: &str) -> Result<Option<T>> {
    match std::fs::read_to_string(file) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .with_context(|| format!("{} is not a valid {what}", file.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", file.display())),
    }
}

/// Write `value` to `file` atomically, so a crash never leaves half a file.
pub fn save_json<T: Serialize>(file: &Path, value: &T) -> Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut staging = file.as_os_str().to_owned();
    staging.push(".tmp");
    let staging = std::path::PathBuf::from(staging);
    std::fs::write(&staging, serde_json::to_string_pretty(value)? + "\n")?;
    std::fs::rename(&staging, file).with_context(|| format!("cannot write {}", file.display()))
}
