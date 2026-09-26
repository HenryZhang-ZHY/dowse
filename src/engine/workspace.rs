//! Saved workspaces: `.tgrep-workspace` files listing the repositories a
//! window searches, in the spirit of VS Code's `.code-workspace`.
//!
//! ```json
//! { "folders": [ { "path": "api" }, { "path": "D:/mirror/web" } ] }
//! ```
//!
//! Repositories inside the file's folder are written relative to it, so a
//! folder of repositories can move, or be shared, with its workspace file.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{repo, store};

/// The extension of saved workspace files.
pub const EXTENSION: &str = "tgrep-workspace";

#[derive(Debug, Default, Serialize, Deserialize)]
struct WorkspaceFile {
    #[serde(default)]
    folders: Vec<Folder>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Folder {
    path: String,
}

/// The repositories `file` lists, as repository identities (see
/// [`repo::identity`]), without duplicates.
pub fn load(file: &Path) -> Result<Vec<PathBuf>> {
    let parsed: WorkspaceFile = store::load_json(file, "workspace file")?
        .ok_or_else(|| anyhow::anyhow!("{} does not exist", file.display()))?;
    let base = file.parent().unwrap_or(Path::new(""));
    let mut repos: Vec<PathBuf> = Vec::new();
    for folder in parsed.folders {
        let path = repo::identity(&base.join(&folder.path));
        if !repos.contains(&path) {
            repos.push(path);
        }
    }
    Ok(repos)
}

/// Write `repos` to `file`.
pub fn save(file: &Path, repos: &[PathBuf]) -> Result<()> {
    let base = file.parent().map(repo::identity);
    let folders = repos
        .iter()
        .map(|path| {
            let relative = base
                .as_deref()
                .and_then(|base| path.strip_prefix(base).ok())
                .filter(|relative| !relative.as_os_str().is_empty());
            let path = match relative {
                // `/` reads the same on every platform.
                Some(relative) => relative.to_string_lossy().replace('\\', "/"),
                None => path.to_string_lossy().into_owned(),
            };
            Folder { path }
        })
        .collect();
    store::save_json(file, &WorkspaceFile { folders })
}

/// A workspace's display name: its file name without the extension.
pub fn name(file: &Path) -> String {
    file.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.to_string_lossy().into_owned())
}

/// Whether `path` names a workspace file rather than a folder.
pub fn is_workspace_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == EXTENSION)
        && !path.is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_nested_repositories_relative_and_others_absolute() {
        let dir = tempfile::tempdir().unwrap();
        let root = repo::identity(dir.path());
        let inside = root.join("group").join("api");
        let outside = tempfile::tempdir().unwrap();
        let outside = repo::identity(outside.path());
        std::fs::create_dir_all(&inside).unwrap();

        let file = root.join("team.tgrep-workspace");
        save(&file, &[inside.clone(), outside.clone()]).unwrap();

        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains(r#""path": "group/api""#), "{text}");
        assert_eq!(load(&file).unwrap(), vec![inside, outside]);
        assert_eq!(name(&file), "team");
        assert!(is_workspace_file(&file));
        assert!(!is_workspace_file(&root));
    }

    #[test]
    fn resolves_relative_paths_against_the_file_and_drops_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let root = repo::identity(dir.path());
        std::fs::create_dir_all(root.join("api")).unwrap();
        let file = root.join("w.tgrep-workspace");
        std::fs::write(
            &file,
            r#"{ "folders": [ { "path": "api" }, { "path": "./api/" }, { "path": "missing" } ] }"#,
        )
        .unwrap();
        assert_eq!(
            load(&file).unwrap(),
            vec![root.join("api"), root.join("missing")]
        );
    }

    #[test]
    fn a_missing_or_corrupt_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("w.tgrep-workspace");
        assert!(load(&file).is_err());
        std::fs::write(&file, "nope").unwrap();
        assert!(load(&file).is_err());
    }
}
