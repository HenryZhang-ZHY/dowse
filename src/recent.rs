//! Recently opened folders, kept in the user's config directory.

use std::path::{Path, PathBuf};

const LIMIT: usize = 10;

fn store_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("tgrep-gpui")
            .join("recent-folders.txt"),
    )
}

/// Recent folders, most recent first, skipping ones that no longer exist.
pub fn load() -> Vec<PathBuf> {
    let Some(text) = store_path().and_then(|path| std::fs::read_to_string(path).ok()) else {
        return Vec::new();
    };
    parse(&text)
        .into_iter()
        .filter(|path| path.is_dir())
        .collect()
}

/// Move `folder` to the front of the list and save it. Failures are ignored:
/// the list is a convenience.
pub fn remember(folder: &Path) -> Vec<PathBuf> {
    let list = push_front(load(), folder);
    if let Some(path) = store_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text: Vec<String> = list
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let _ = std::fs::write(path, text.join("\n"));
    }
    list
}

fn parse(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn push_front(mut list: Vec<PathBuf>, folder: &Path) -> Vec<PathBuf> {
    list.retain(|existing| existing != folder);
    list.insert(0, folder.to_path_buf());
    list.truncate(LIMIT);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_most_recent_first_without_duplicates() {
        let list = parse("/a\n\n/b\n");
        assert_eq!(list, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        let list = push_front(list, Path::new("/b"));
        assert_eq!(list, vec![PathBuf::from("/b"), PathBuf::from("/a")]);
        let many = (0..20).fold(Vec::new(), |list, i| {
            push_front(list, Path::new(&format!("/{i}")))
        });
        assert_eq!(many.len(), LIMIT);
        assert_eq!(many[0], PathBuf::from("/19"));
    }
}
