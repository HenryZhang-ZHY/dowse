//! Facets: counting results by repository, branch, tag group, language and
//! directory, and filtering results by a selected value in each.
//!
//! Facets only narrow results that were already found. Which repositories are
//! searched in the first place is the [`super::repo::Scope`]'s job.

use std::collections::BTreeMap;

use super::repo::tag_group;
use super::search::FileMatch;

/// One dimension results can be counted and filtered by.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FacetKind {
    Repository,
    Branch,
    /// The repository's tags in one group: `""` for plain tags, otherwise the
    /// key of `key:value` tags.
    Tags(String),
    Language,
    Directory,
}

/// The facet value for files directly in a repository's root.
pub const ROOT_DIRECTORY: &str = "(root)";
pub const OTHER_LANGUAGE: &str = "Other";
pub const NO_BRANCH: &str = "(no branch)";

impl FacetKind {
    pub fn title(&self) -> String {
        match self {
            Self::Repository => "Repository".into(),
            Self::Branch => "Branch".into(),
            Self::Tags(group) if group.is_empty() => "Tags".into(),
            Self::Tags(group) => {
                let mut chars = group.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            }
            Self::Language => "Language".into(),
            Self::Directory => "Directory".into(),
        }
    }

    /// How a value reads in its section: `owner:henry` shows as `henry` under "Owner".
    pub fn display<'a>(&self, value: &'a str) -> &'a str {
        match self {
            Self::Tags(group) if !group.is_empty() => value
                .strip_prefix(group.as_str())
                .and_then(|rest| rest.strip_prefix(':'))
                .unwrap_or(value),
            _ => value,
        }
    }

    /// The values `file` has for this facet. Tags can give several.
    pub fn values<'a>(&self, file: &'a FileMatch) -> Vec<&'a str> {
        match self {
            Self::Repository => vec![file.repo.name.as_str()],
            Self::Branch => vec![file.repo.branch.as_deref().unwrap_or(NO_BRANCH)],
            Self::Tags(group) => file
                .repo
                .tags
                .iter()
                .filter(|tag| tag_group(tag) == group)
                .map(String::as_str)
                .collect(),
            Self::Language => vec![file.language.unwrap_or(OTHER_LANGUAGE)],
            Self::Directory => vec![top_directory(&file.path)],
        }
    }

    /// Repository-level facets only help once results span several values.
    fn hides_single_value(&self) -> bool {
        matches!(self, Self::Repository | Self::Branch | Self::Tags(_))
    }
}

/// The top-level directory a path belongs to.
pub fn top_directory(path: &str) -> &str {
    match path.split_once('/') {
        Some((first, _)) => first,
        None => ROOT_DIRECTORY,
    }
}

/// The selected value in each facet. A file passes when it has the selected
/// value of every facet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FacetFilter {
    selected: BTreeMap<FacetKind, String>,
}

impl FacetFilter {
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    pub fn get(&self, kind: &FacetKind) -> Option<&str> {
        self.selected.get(kind).map(String::as_str)
    }

    /// Select `value` in `kind`, or clear it when it is already selected.
    pub fn toggle(&mut self, kind: FacetKind, value: String) {
        if self.selected.get(&kind) == Some(&value) {
            self.selected.remove(&kind);
        } else {
            self.selected.insert(kind, value);
        }
    }

    pub fn clear(&mut self, kind: &FacetKind) {
        self.selected.remove(kind);
    }

    pub fn matches(&self, file: &FileMatch) -> bool {
        self.matches_except(file, None)
    }

    fn matches_except(&self, file: &FileMatch, skip: Option<&FacetKind>) -> bool {
        self.selected
            .iter()
            .filter(|(kind, _)| Some(*kind) != skip)
            .all(|(kind, value)| kind.values(file).contains(&value.as_str()))
    }
}

/// Facet counts over a result set: how many matching files have each value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facets {
    pub sections: Vec<(FacetKind, Vec<(String, usize)>)>,
}

impl Facets {
    /// Count each facet over the files that pass the *other* facets' filters,
    /// so picking a value in one facet shows how its files spread across the
    /// rest, while every value of the picked facet stays visible to switch to.
    pub fn new(files: &[FileMatch], filter: &FacetFilter) -> Self {
        let mut groups: Vec<String> = files
            .iter()
            .flat_map(|file| file.repo.tags.iter().map(|tag| tag_group(tag).to_string()))
            .collect();
        groups.sort();
        groups.dedup();

        let kinds = [FacetKind::Repository, FacetKind::Branch]
            .into_iter()
            .chain(groups.into_iter().map(FacetKind::Tags))
            .chain([FacetKind::Language, FacetKind::Directory]);

        let sections = kinds
            .filter_map(|kind| {
                let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
                for file in files
                    .iter()
                    .filter(|file| filter.matches_except(file, Some(&kind)))
                {
                    for value in kind.values(file) {
                        *counts.entry(value).or_default() += 1;
                    }
                }
                if kind.hides_single_value() && counts.len() < 2 && filter.get(&kind).is_none() {
                    return None;
                }
                let mut entries: Vec<(String, usize)> = counts
                    .into_iter()
                    .map(|(value, count)| (value.to_string(), count))
                    .collect();
                entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                Some((kind, entries))
            })
            .collect();
        Self { sections }
    }

    pub fn section(&self, kind: &FacetKind) -> Option<&[(String, usize)]> {
        self.sections
            .iter()
            .find(|(k, _)| k == kind)
            .map(|(_, entries)| entries.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::engine::language;
    use crate::engine::repo::RepoInfo;

    fn repo(name: &str, branch: &str, tags: &[&str]) -> Arc<RepoInfo> {
        Arc::new(RepoInfo {
            id: name.into(),
            name: name.into(),
            root: PathBuf::from(name),
            branch: Some(branch.into()),
            tags: tags.iter().map(|t| t.to_string()).collect(),
        })
    }

    fn file(repo: &Arc<RepoInfo>, path: &str) -> FileMatch {
        FileMatch {
            repo: repo.clone(),
            path: path.into(),
            language: language::detect(path),
            matched_lines: 1,
            snippets: vec![],
        }
    }

    #[test]
    fn counts_every_facet_and_hides_single_valued_repository_facets() {
        let api = repo("api", "main", &["mirror", "owner:henry"]);
        let web = repo("web", "main", &["mirror", "owner:alice"]);
        let files = [
            file(&api, "src/a.rs"),
            file(&api, "README.md"),
            file(&web, "src/b.ts"),
        ];
        let facets = Facets::new(&files, &FacetFilter::default());

        let kinds: Vec<FacetKind> = facets
            .sections
            .iter()
            .map(|(kind, _)| kind.clone())
            .collect();
        // Branch and plain tags have one value each, so they are hidden.
        assert_eq!(
            kinds,
            vec![
                FacetKind::Repository,
                FacetKind::Tags("owner".into()),
                FacetKind::Language,
                FacetKind::Directory
            ]
        );
        assert_eq!(
            facets.section(&FacetKind::Repository).unwrap(),
            &[("api".to_string(), 2), ("web".to_string(), 1)]
        );
        assert_eq!(
            facets.section(&FacetKind::Directory).unwrap(),
            &[("src".to_string(), 2), ("(root)".to_string(), 1)]
        );
        let owner = FacetKind::Tags("owner".into());
        assert_eq!(owner.title(), "Owner");
        assert_eq!(owner.display("owner:henry"), "henry");
    }

    #[test]
    fn each_facet_is_counted_under_the_other_facets_filters() {
        let api = repo("api", "main", &[]);
        let dev = repo("api-dev", "feature/x", &[]);
        let files = [
            file(&api, "src/a.rs"),
            file(&dev, "src/a.rs"),
            file(&dev, "docs/x.md"),
        ];
        let mut filter = FacetFilter::default();
        filter.toggle(FacetKind::Repository, "api-dev".into());

        let facets = Facets::new(&files, &filter);
        // The selected facet keeps every value to switch between.
        assert_eq!(facets.section(&FacetKind::Repository).unwrap().len(), 2);
        // Others only count the selected repository's files.
        assert_eq!(
            facets.section(&FacetKind::Language).unwrap(),
            &[("Markdown".to_string(), 1), ("Rust".to_string(), 1)]
        );
        assert_eq!(files.iter().filter(|f| filter.matches(f)).count(), 2);

        filter.toggle(FacetKind::Repository, "api-dev".into());
        assert!(filter.is_empty());
    }
}
