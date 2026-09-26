//! One repository's tgrep index: where it lives, how it is built and
//! published, and the set of files a search reads.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};
use tgrep_core::builder::{self, BuildOptions};
use tgrep_core::meta::{INDEX_FORMAT_VERSION, IndexMeta};
use tgrep_core::path_index;
use tgrep_core::query::{self, QueryPlan};
use tgrep_core::reader::IndexReader;
use tgrep_core::visibility::PathVisibility;
use tgrep_core::walker::{self, WalkOptions};

/// A searchable folder and its index. The index lives in `<root>/.tgrep`, the same
/// place `tgrep index` and `tgrep serve` use, so the GUI and the CLI share it.
#[derive(Clone, Debug)]
pub struct RepoIndex {
    root: PathBuf,
    index_dir: PathBuf,
}

/// Whether the on-disk index can answer searches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexStatus {
    /// No index has been built for this folder.
    Missing,
    /// An index exists but is partial, from an incompatible tgrep version, or
    /// built for a different folder. Searches scan the filesystem.
    Unusable,
    /// A complete index, last published at `updated_at`.
    Ready { files: u64, updated_at: SystemTime },
}

impl RepoIndex {
    pub fn open(root: &Path) -> Result<Self> {
        let root = std::fs::canonicalize(root)
            .with_context(|| format!("cannot open folder {}", root.display()))?;
        anyhow::ensure!(root.is_dir(), "{} is not a folder", display_path(&root));
        let index_dir = builder::default_index_dir(&root);
        Ok(Self { root, index_dir })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The root without Windows' `\\?\` verbatim prefix, for display.
    pub fn display_root(&self) -> String {
        display_path(&self.root)
    }

    /// The folder's own name, for titles.
    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.display_root())
    }

    pub fn index_status(&self) -> IndexStatus {
        match IndexMeta::load(&self.index_dir) {
            Err(_) => IndexStatus::Missing,
            Ok(meta) if !self.meta_is_usable(&meta) => IndexStatus::Unusable,
            Ok(meta) => IndexStatus::Ready {
                files: meta.num_files,
                updated_at: SystemTime::UNIX_EPOCH + Duration::from_secs(meta.updated_at),
            },
        }
    }

    fn meta_is_usable(&self, meta: &IndexMeta) -> bool {
        meta.complete
            && meta.hidden_complete
            && meta.version == INDEX_FORMAT_VERSION
            && std::fs::canonicalize(&meta.root_path).is_ok_and(|root| root == self.root)
    }

    /// Build a new index next to the live one, with the same options as
    /// `tgrep index <root>`. The live index keeps answering searches until
    /// [`StagedIndex::publish`] swaps the new one in.
    pub fn build_index(&self) -> Result<StagedIndex> {
        std::fs::create_dir_all(&self.index_dir)?;
        let index_dir = std::fs::canonicalize(&self.index_dir)?;
        let staging = index_dir.join(STAGING_DIR);
        if staging.exists() {
            std::fs::remove_dir_all(&staging)
                .with_context(|| format!("cannot clear {}", display_path(&staging)))?;
        }
        builder::build_index_with_options(
            &self.root,
            Some(&staging),
            &BuildOptions {
                exclude_paths: vec![index_dir.clone()],
                ..Default::default()
            },
        )
        .with_context(|| format!("failed to index {}", self.display_root()))?;
        Ok(StagedIndex { staging, index_dir })
    }

    /// Load the files a search reads: the index when it is usable, otherwise
    /// a walk of the folder that honours `.gitignore` like ripgrep.
    pub fn load_corpus(&self) -> Corpus {
        match self.open_index() {
            Some((reader, visibility)) => Corpus {
                root: self.root.clone(),
                source: Source::Index { reader, visibility },
            },
            None => Corpus {
                root: self.root.clone(),
                source: Source::Walk {
                    paths: self.walk_files(),
                },
            },
        }
    }

    fn open_index(&self) -> Option<(IndexReader, PathVisibility)> {
        let meta = IndexMeta::load(&self.index_dir).ok()?;
        if !self.meta_is_usable(&meta) {
            return None;
        }
        let reader = IndexReader::open(&self.index_dir).ok()?;
        if reader.is_degenerate() || reader.validate_lookup().is_err() {
            return None;
        }
        let visibility = path_index::read_filename_index(&self.index_dir)
            .ok()??
            .visibility?;
        visibility
            .covers_index(&meta, reader.file_table_id())
            .then_some((reader, visibility.paths))
    }

    /// Files a search would read that were modified after `time`: those an
    /// index built then may not have seen. Takes a walk of the folder, and
    /// reads no file.
    pub fn modified_since(&self, time: SystemTime) -> Vec<String> {
        self.walk_files()
            .into_iter()
            .filter(|relative| {
                std::fs::metadata(self.root.join(relative))
                    .and_then(|metadata| metadata.modified())
                    .is_ok_and(|modified| modified > time)
            })
            .collect()
    }

    fn walk_files(&self) -> Vec<String> {
        let mut exclude_paths = Vec::new();
        if let Ok(index_dir) = std::fs::canonicalize(&self.index_dir) {
            exclude_paths.push(index_dir);
        }
        let result = walker::walk_dir(
            &self.root,
            &WalkOptions {
                exclude_paths,
                ..Default::default()
            },
        );
        let mut paths: Vec<String> = result
            .files
            .iter()
            .filter_map(|path| relative_path(&self.root, path))
            .collect();
        paths.sort();
        paths
    }
}

/// Where [`RepoIndex::build_index`] writes, inside the index directory so that
/// tgrep's walks skip it too.
const STAGING_DIR: &str = "gui-staging";
/// tgrep-core reads this file to decide whether an index exists.
const META_FILE: &str = "meta.json";

/// A freshly built index waiting to replace the live one.
#[must_use = "the new index is not used until it is published"]
pub struct StagedIndex {
    staging: PathBuf,
    index_dir: PathBuf,
}

impl StagedIndex {
    /// Replace the live index with this one.
    ///
    /// Every [`Corpus`] reading the live index must be dropped first: Windows
    /// refuses to replace files that are memory-mapped.
    pub fn publish(self) -> Result<()> {
        let hint = "is another process, such as `tgrep serve`, using the index?";
        // Sidecars tied to the old generation, as an in-place build removes them.
        tgrep_core::meta::remove_file_evidence(&self.index_dir).context(hint)?;
        path_index::remove_extra_paths(&self.index_dir).context(hint)?;
        // Without metadata a reader treats the index as missing, so nobody
        // opens a half-swapped index. It comes back last.
        match std::fs::remove_file(self.index_dir.join(META_FILE)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(anyhow::Error::new(error).context(hint));
            }
            _ => {}
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.staging)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_file())
            .collect();
        files.sort_by_key(|path| path.file_name().is_some_and(|name| name == META_FILE));
        for file in files {
            let target = self.index_dir.join(file.file_name().unwrap_or_default());
            std::fs::rename(&file, &target)
                .with_context(|| format!("cannot replace {}; {hint}", display_path(&target)))?;
        }
        let _ = std::fs::remove_dir_all(&self.staging);
        // Keep the index out of `git status` without editing the user's
        // `.gitignore`. Failing to write it is harmless.
        let _ = std::fs::write(self.index_dir.join(".gitignore"), "*\n");
        Ok(())
    }
}

/// The files one search reads, loaded once per index generation and shared by
/// every search until the index changes.
pub struct Corpus {
    root: PathBuf,
    source: Source,
}

enum Source {
    Index {
        reader: IndexReader,
        visibility: PathVisibility,
    },
    Walk {
        paths: Vec<String>,
    },
}

impl Corpus {
    pub fn is_indexed(&self) -> bool {
        matches!(self.source, Source::Index { .. })
    }

    pub fn file_count(&self) -> usize {
        match &self.source {
            Source::Index { reader, .. } => reader.num_files(),
            Source::Walk { paths } => paths.len(),
        }
    }

    /// Repository-relative, `/`-separated paths of every file that can match
    /// `plan`. Hidden files are left out, as in a default `tgrep` search.
    pub fn candidates(&self, plan: &QueryPlan) -> Vec<String> {
        match &self.source {
            Source::Index { reader, visibility } => {
                let ids = if plan.is_match_all() {
                    reader.all_file_ids()
                } else {
                    query::execute_plan_with_masks(plan, &|trigram| {
                        reader.lookup_trigram_with_masks(trigram)
                    })
                };
                let mut paths: Vec<String> = ids
                    .into_iter()
                    .filter_map(|id| reader.file_path(id))
                    .filter(|path| visibility.is_visible(path, "", false))
                    .map(str::to_string)
                    .collect();
                paths.sort();
                paths
            }
            Source::Walk { paths } => paths.clone(),
        }
    }

    pub fn full_path(&self, relative: &str) -> PathBuf {
        self.root
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
    }
}

fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let text = relative.to_string_lossy().replace('\\', "/");
    (!text.is_empty()).then_some(text)
}

/// A path as the user would type it: no `\\?\` prefix on Windows.
pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(&text).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn needle_here() {}\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "no match in here\n").unwrap();
        dir
    }

    #[test]
    fn finds_files_modified_after_a_point_in_time() {
        let dir = tree();
        let now = SystemTime::now();
        let touch = |path: &str, time: SystemTime| {
            std::fs::File::options()
                .write(true)
                .open(dir.path().join(path))
                .unwrap()
                .set_modified(time)
                .unwrap();
        };
        touch("README.md", now - Duration::from_secs(600));
        touch("src/lib.rs", now + Duration::from_secs(60));
        let index = RepoIndex::open(dir.path()).unwrap();
        assert_eq!(index.modified_since(now), vec!["src/lib.rs".to_string()]);
    }

    #[test]
    fn missing_index_falls_back_to_walking() {
        let dir = tree();
        let index = RepoIndex::open(dir.path()).unwrap();
        assert_eq!(index.index_status(), IndexStatus::Missing);
        let corpus = index.load_corpus();
        assert!(!corpus.is_indexed());
        assert_eq!(
            corpus.candidates(&QueryPlan::MatchAll),
            vec!["README.md".to_string(), "src/lib.rs".to_string()]
        );
    }

    #[test]
    fn built_index_narrows_candidates() {
        let dir = tree();
        let index = RepoIndex::open(dir.path()).unwrap();
        index.build_index().unwrap().publish().unwrap();
        assert!(matches!(
            index.index_status(),
            IndexStatus::Ready { files: 2, .. }
        ));
        let corpus = index.load_corpus();
        assert!(corpus.is_indexed());
        let plan = query::build_literal_plan("needle_here", false);
        assert_eq!(corpus.candidates(&plan), vec!["src/lib.rs".to_string()]);
    }

    #[test]
    fn rebuilding_while_the_index_is_open_publishes_after_readers_close() {
        let dir = tree();
        let index = RepoIndex::open(dir.path()).unwrap();
        index.build_index().unwrap().publish().unwrap();
        let corpus = index.load_corpus();
        assert!(corpus.is_indexed());

        std::fs::write(dir.path().join("src/new.rs"), "fn added_later() {}\n").unwrap();
        // The build must not touch the mapped files of the open index.
        let staged = index.build_index().unwrap();
        let old_plan = query::build_literal_plan("needle_here", false);
        assert_eq!(corpus.candidates(&old_plan), vec!["src/lib.rs".to_string()]);
        drop(corpus);
        staged.publish().unwrap();

        assert!(matches!(
            index.index_status(),
            IndexStatus::Ready { files: 3, .. }
        ));
        let corpus = index.load_corpus();
        let plan = query::build_literal_plan("added_later", false);
        assert_eq!(corpus.candidates(&plan), vec!["src/new.rs".to_string()]);
        // The staging directory is gone and never indexed itself.
        assert!(!index.index_dir.join(STAGING_DIR).exists());
        assert!(
            corpus
                .candidates(&QueryPlan::MatchAll)
                .iter()
                .all(|p| !p.starts_with('.'))
        );
    }

    #[test]
    fn display_path_strips_verbatim_prefix() {
        assert_eq!(display_path(Path::new(r"\\?\C:\repo")), r"C:\repo");
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\srv\share")),
            r"\\srv\share"
        );
        assert_eq!(display_path(Path::new("/home/me")), "/home/me");
    }
}
