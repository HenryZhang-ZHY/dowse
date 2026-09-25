//! Repositories, their tags, and the scope that picks which ones a search reads.
//!
//! A tag is either plain (`mirror`, `dev`) or `key:value` (`owner:henry`,
//! `project:billing`). Tags sharing a key form a group; plain tags form their
//! own group. Every repository also carries an implicit `branch:<name>` tag
//! for the branch it has checked out, so the scope can select by branch.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The group of implicit tags naming the checked-out branch.
pub const BRANCH_GROUP: &str = "branch";

/// A repository as a search sees it: a snapshot of its name, location,
/// branch and tags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoInfo {
    /// Stable identity: the folder as the user would type it.
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub branch: Option<String>,
    /// Tags the user assigned, without the implicit branch tag.
    pub tags: Vec<String>,
}

impl RepoInfo {
    /// `name (branch)`, or just `name` outside a git repository.
    pub fn label(&self) -> String {
        match &self.branch {
            Some(branch) => format!("{} ({branch})", self.name),
            None => self.name.clone(),
        }
    }

    /// The user's tags plus `branch:<name>`.
    pub fn effective_tags(&self) -> Vec<String> {
        let mut tags = self.tags.clone();
        if let Some(branch) = &self.branch {
            tags.push(format!("{BRANCH_GROUP}:{branch}"));
        }
        tags
    }
}

/// The group a tag belongs to: its key, or `""` for a plain tag.
pub fn tag_group(tag: &str) -> &str {
    match tag.split_once(':') {
        Some((key, value)) if !key.is_empty() && !value.is_empty() => key,
        _ => "",
    }
}

/// Parse tags typed as free text, separated by spaces or commas. Duplicates
/// are dropped and the order kept.
pub fn parse_tags(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    text.split(|c: char| c.is_whitespace() || c == ',')
        .map(|tag| tag.trim_matches(':'))
        .filter(|tag| !tag.is_empty())
        .filter(|tag| seen.insert(tag.to_string()))
        .map(str::to_string)
        .collect()
}

/// Which repositories a search reads, as a set of selected tags.
///
/// A repository is in scope when, for every group with a selected tag, it
/// carries at least one of that group's selected tags: tags in one group are
/// alternatives, groups narrow each other. An empty scope includes every
/// repository.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    tags: BTreeSet<String>,
}

impl Scope {
    pub fn new(tags: impl IntoIterator<Item = String>) -> Self {
        Self {
            tags: tags.into_iter().collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    pub fn tags(&self) -> impl Iterator<Item = &str> {
        self.tags.iter().map(String::as_str)
    }

    pub fn contains(&self, tag: &str) -> bool {
        self.tags.contains(tag)
    }

    /// Select `tag`, or unselect it when it is already selected.
    pub fn toggle(&mut self, tag: &str) {
        if !self.tags.remove(tag) {
            self.tags.insert(tag.to_string());
        }
    }

    pub fn clear(&mut self) {
        self.tags.clear();
    }

    pub fn includes(&self, repo: &RepoInfo) -> bool {
        let tags = repo.effective_tags();
        let mut groups: BTreeMap<&str, bool> = BTreeMap::new();
        for selected in &self.tags {
            let hit = tags.iter().any(|tag| tag == selected);
            *groups.entry(tag_group(selected)).or_default() |= hit;
        }
        groups.values().all(|hit| *hit)
    }
}

/// Every tag across `repos`, grouped by key, with how many repositories carry
/// it. Plain tags come first, then keyed groups alphabetically.
pub fn tag_catalog<'a>(
    repos: impl IntoIterator<Item = &'a RepoInfo>,
) -> Vec<(String, Vec<(String, usize)>)> {
    let mut groups: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for repo in repos {
        for tag in repo.effective_tags() {
            *groups
                .entry(tag_group(&tag).to_string())
                .or_default()
                .entry(tag)
                .or_default() += 1;
        }
    }
    groups
        .into_iter()
        .map(|(group, tags)| (group, tags.into_iter().collect()))
        .collect()
}

/// The branch checked out in `root`, read from `.git/HEAD`. A detached HEAD
/// is shown as `detached@<short sha>`; `None` outside a git repository.
pub fn current_branch(root: &Path) -> Option<String> {
    let git = root.join(".git");
    let git_dir = if git.is_dir() {
        git
    } else {
        // A worktree or submodule: `.git` is a file pointing at the real one.
        let pointer = std::fs::read_to_string(&git).ok()?;
        let target = pointer.trim().strip_prefix("gitdir:")?.trim();
        root.join(target)
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref:") {
        Some(reference) => {
            let reference = reference.trim();
            Some(
                reference
                    .strip_prefix("refs/heads/")
                    .unwrap_or(reference)
                    .to_string(),
            )
        }
        None => Some(format!("detached@{}", &head[..head.len().min(7)])),
    }
}

/// The repositories to add for `folder`: the folder itself when it is a git
/// repository, otherwise the git repositories directly inside it. A folder
/// with neither is added as-is, since plain folders are searchable too.
pub fn discover(folder: &Path) -> Vec<PathBuf> {
    if folder.join(".git").exists() {
        return vec![folder.to_path_buf()];
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir() && path.join(".git").exists())
        .collect();
    found.sort();
    if found.is_empty() {
        found.push(folder.to_path_buf());
    }
    found
}

/// A display name for a new repository at `root`: its folder name, or
/// `parent/folder` when another repository already uses that name.
pub fn unique_name<'a>(root: &Path, taken: impl IntoIterator<Item = &'a str>) -> String {
    let taken: BTreeSet<&str> = taken.into_iter().collect();
    let folder = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned());
    if !taken.contains(folder.as_str()) {
        return folder;
    }
    let with_parent = match root.parent().and_then(Path::file_name) {
        Some(parent) => format!("{}/{folder}", parent.to_string_lossy()),
        None => folder.clone(),
    };
    if !taken.contains(with_parent.as_str()) {
        return with_parent;
    }
    (2..)
        .map(|n| format!("{with_parent} ({n})"))
        .find(|name| !taken.contains(name.as_str()))
        .expect("an unused name")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str, branch: Option<&str>, tags: &[&str]) -> RepoInfo {
        RepoInfo {
            id: name.into(),
            name: name.into(),
            root: PathBuf::from(name),
            branch: branch.map(String::from),
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn parses_tags_from_free_text() {
        assert_eq!(
            parse_tags(" mirror, owner:henry  owner:henry,,project:x "),
            vec!["mirror", "owner:henry", "project:x"]
        );
        assert_eq!(parse_tags(": :: "), Vec::<String>::new());
        assert_eq!(tag_group("owner:henry"), "owner");
        assert_eq!(tag_group("mirror"), "");
        assert_eq!(tag_group("odd:"), "");
    }

    #[test]
    fn scope_ors_within_a_group_and_ands_across_groups() {
        let api_main = repo("api", Some("main"), &["mirror", "owner:henry"]);
        let api_dev = repo("api-dev", Some("feature/login"), &["dev", "owner:henry"]);
        let web_dev = repo("web-dev", Some("fix/header"), &["dev", "owner:alice"]);

        assert!(Scope::default().includes(&api_main));

        let dev = Scope::new(["dev".to_string()]);
        assert!(!dev.includes(&api_main) && dev.includes(&api_dev) && dev.includes(&web_dev));

        let mine = Scope::new(["dev".to_string(), "owner:henry".to_string()]);
        assert!(mine.includes(&api_dev) && !mine.includes(&web_dev));

        let either_owner = Scope::new(["owner:henry".to_string(), "owner:alice".to_string()]);
        assert!(either_owner.includes(&api_main) && either_owner.includes(&web_dev));

        let on_main = Scope::new(["branch:main".to_string()]);
        assert!(on_main.includes(&api_main) && !on_main.includes(&api_dev));
    }

    #[test]
    fn catalog_groups_tags_and_counts_repositories() {
        let repos = [
            repo("api", Some("main"), &["mirror", "owner:henry"]),
            repo("web", Some("main"), &["mirror"]),
        ];
        let catalog = tag_catalog(&repos);
        assert_eq!(
            catalog,
            vec![
                ("".into(), vec![("mirror".into(), 2)]),
                ("branch".into(), vec![("branch:main".into(), 2)]),
                ("owner".into(), vec![("owner:henry".into(), 1)]),
            ]
        );
    }

    #[test]
    fn reads_branches_including_worktrees_and_detached_heads() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(current_branch(root), None);

        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/feature/login\n").unwrap();
        assert_eq!(current_branch(root).as_deref(), Some("feature/login"));

        std::fs::write(root.join(".git/HEAD"), "0123456789abcdef\n").unwrap();
        assert_eq!(current_branch(root).as_deref(), Some("detached@0123456"));

        let worktree = root.join("wt");
        let git_dir = root.join(".git/worktrees/wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/dev\n").unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .unwrap();
        assert_eq!(current_branch(&worktree).as_deref(), Some("dev"));
    }

    #[test]
    fn discovers_repositories_in_a_parent_folder() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["b", "a"] {
            std::fs::create_dir_all(dir.path().join(name).join(".git")).unwrap();
        }
        std::fs::create_dir_all(dir.path().join("not-a-repo")).unwrap();
        assert_eq!(
            discover(dir.path()),
            vec![dir.path().join("a"), dir.path().join("b")]
        );
        assert_eq!(discover(&dir.path().join("a")), vec![dir.path().join("a")]);
        let plain = dir.path().join("not-a-repo");
        assert_eq!(discover(&plain), vec![plain.clone()]);
    }

    #[test]
    fn names_collide_into_parent_qualified_names() {
        let root = Path::new("/src/dev/api");
        assert_eq!(unique_name(root, ["web"]), "api");
        assert_eq!(unique_name(root, ["api"]), "dev/api");
        assert_eq!(unique_name(root, ["api", "dev/api"]), "dev/api (2)");
    }
}
