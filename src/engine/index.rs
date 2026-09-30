//! One repository's tgrep index: where it lives, how it is built, brought up
//! to date and published, and the set of files a search reads.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};
use rayon::prelude::*;
use tgrep_core::builder::{self, BuildOptions};
use tgrep_core::hybrid::HybridIndex;
use tgrep_core::live::LiveIndex;
use tgrep_core::meta::{self, FileEvidence, FileTableId, INDEX_FORMAT_VERSION, IndexMeta};
use tgrep_core::path_index;
use tgrep_core::query::QueryPlan;
use tgrep_core::reader::IndexReader;
use tgrep_core::trigram::{self, TrigramMaskMap};
use tgrep_core::visibility::{self, PathVisibility};
use tgrep_core::walker::{self, FileMeta, MetaWalkOptions, MetaWalkResult, WalkOptions};

use super::watch::Change;

/// How [`RepoIndex::update_index`] brought an index up to date.
#[must_use = "a changed index is not used until it is published"]
pub enum IndexUpdate {
    /// Every file matched the index, so there is nothing to publish.
    UpToDate,
    /// Only the files that changed were read, and merged into a copy of the
    /// index.
    Merged {
        staged: StagedIndex,
        changes: FileChanges,
    },
    /// The index was built again from every file, for `reason`.
    Rebuilt { staged: StagedIndex, reason: String },
}

impl IndexUpdate {
    /// The new index to publish, if there is one.
    pub fn into_staged(self) -> Option<StagedIndex> {
        match self {
            Self::UpToDate => None,
            Self::Merged { staged, .. } | Self::Rebuilt { staged, .. } => Some(staged),
        }
    }
}

/// How many files differ from an index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileChanges {
    pub modified: usize,
    pub added: usize,
    pub deleted: usize,
}

impl FileChanges {
    pub fn total(&self) -> usize {
        self.modified + self.added + self.deleted
    }
}

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

    /// Bring the index up to date, reading only the files that changed.
    ///
    /// Compares every file's metadata with the stamps the index keeps of the
    /// reads that built it, indexes the modified and new files on their own,
    /// and streams them into a copy of the index without the stale and
    /// deleted entries. The merge copies the old postings as they are rather
    /// than reading the files again, so its cost follows the size of the
    /// index, not of the source.
    ///
    /// Builds the whole index instead, as [`Self::build_index`] does, when
    /// there is no usable index to start from, when part of the folder
    /// could not be read (its files would look deleted), or when most files
    /// changed anyway. Like a build, it leaves the live index alone until
    /// the result is published.
    pub fn update_index(&self) -> Result<IndexUpdate> {
        let rebuild = |reason: String| -> Result<IndexUpdate> {
            Ok(IndexUpdate::Rebuilt {
                staged: self.build_index()?,
                reason,
            })
        };
        let Some((reader, _)) = self.open_index() else {
            return rebuild("there is no usable index".into());
        };
        let started = SystemTime::now();
        let walk = self.walk_metadata();
        if walk.skipped_error > 0 {
            return rebuild(format!(
                "{} could not be read",
                plural(walk.skipped_error, "entry", "entries")
            ));
        }
        let evidence = meta::read_file_evidence(&self.index_dir).unwrap_or_default();
        if evidence.stamps.is_empty() && reader.num_files() > 0 {
            return rebuild("the index keeps no file stamps".into());
        }
        let stale = StaleFiles::classify(
            &walk.files,
            &evidence,
            reader.all_paths().iter().map(String::as_str),
        );
        let changes = stale.changes();
        if changes.total() == 0 {
            drop(reader);
            self.mark_current(started)?;
            return Ok(IndexUpdate::UpToDate);
        }
        if changes.total() * 2 > walk.files.len().max(reader.num_files()) {
            drop(reader);
            return rebuild(format!(
                "{} of {} changed",
                changes.total(),
                plural(walk.files.len(), "file", "files")
            ));
        }
        let staged = self
            .merge_changes(&reader, walk, evidence, stale)
            .with_context(|| format!("failed to update the index of {}", self.display_root()))?;
        Ok(IndexUpdate::Merged { staged, changes })
    }

    /// Stage the live index with `stale`'s files read again: a delta index of
    /// the modified and new files, merged with the live one minus their old
    /// entries and those of deleted files.
    fn merge_changes(
        &self,
        reader: &IndexReader,
        walk: MetaWalkResult,
        mut evidence: FileEvidence,
        stale: StaleFiles,
    ) -> Result<StagedIndex> {
        let index_dir = std::fs::canonicalize(&self.index_dir)?;
        let staging = index_dir.join(STAGING_DIR);
        let delta_dir = index_dir.join(DELTA_DIR);
        for dir in [&staging, &delta_dir] {
            if dir.exists() {
                std::fs::remove_dir_all(dir)
                    .with_context(|| format!("cannot clear {}", display_path(dir)))?;
            }
        }

        let files: Vec<PathBuf> = stale
            .read
            .iter()
            .map(|relative| self.full_path(relative))
            .collect();
        let outcome = builder::build_index_for_files(
            &self.root,
            &delta_dir,
            &files,
            builder::DEFAULT_INDEX_BUFFER_BYTES,
        )?;

        // Every stale path loses its old entry and its stamp. A file that
        // could not be read keeps its old entry, and goes without a stamp so
        // that the next update tries it again.
        let mut removed: HashSet<String> =
            stale.read.iter().chain(&stale.deleted).cloned().collect();
        for path in &removed {
            evidence.remove(path);
        }
        for path in &outcome.unreadable {
            if let Some(relative) = relative_path(&self.root, path) {
                log::warn!("could not read {relative} to index it; keeping its old entry");
                removed.remove(&relative);
            }
        }
        let mut content_ids = outcome.content_ids;
        for (path, version) in outcome.versions {
            let content_id = content_ids.remove(&path);
            evidence.insert_verified(path, version.stamp().clone(), content_id, Some(version));
        }

        let delta = IndexReader::open(&delta_dir)?;
        builder::merge_index_with_delta(&self.root, &staging, reader, &delta, &removed, true)?;

        // What a full build writes beside the postings: which entries are
        // hidden, the listed files that have no content entry, and the stamps.
        let mut staged_meta = IndexMeta::load(&staging)?;
        staged_meta.visibility = walk.visibility.clone();
        staged_meta.hidden_complete = true;
        staged_meta.save(&staging)?;
        let file_table_id = staged_meta
            .file_table_id
            .context("the merged index has no file-table identity")?;
        let content: HashSet<&str> = reader
            .all_paths()
            .iter()
            .filter(|path| !removed.contains(path.as_str()))
            .chain(delta.all_paths())
            .map(String::as_str)
            .collect();
        let mut extra_paths: Vec<String> = walk
            .listed_files
            .into_iter()
            .filter(|path| !content.contains(path.as_str()))
            .collect();
        extra_paths.sort_unstable();
        drop(content);
        path_index::write_extra_paths_with_visibility(
            &staging,
            &extra_paths,
            &walk.visibility,
            file_table_id,
            true,
        )?;
        meta::write_file_evidence(&evidence, &staging)?;

        // Windows cannot delete mapped files.
        drop(delta);
        let _ = std::fs::remove_dir_all(&delta_dir);
        Ok(StagedIndex { staging, index_dir })
    }

    /// Record that the live index was found current as of `time`, when the
    /// walk that checked it started.
    fn mark_current(&self, time: SystemTime) -> Result<()> {
        let index_dir = std::fs::canonicalize(&self.index_dir)?;
        let staging = index_dir.join(STAGING_DIR);
        std::fs::create_dir_all(&staging)?;
        let mut current = IndexMeta::load(&index_dir)?;
        if let Ok(since) = time.duration_since(SystemTime::UNIX_EPOCH) {
            current.updated_at = since.as_secs();
        }
        // Written beside it and swapped in, so no reader sees half a file.
        current.save(&staging)?;
        std::fs::rename(staging.join(META_FILE), index_dir.join(META_FILE))?;
        let _ = std::fs::remove_dir_all(&staging);
        Ok(())
    }

    /// Load the files a search reads: the index when it is usable, otherwise
    /// a walk of the folder that honours `.gitignore` like ripgrep.
    pub fn load_corpus(&self) -> Corpus {
        let source = match self.open_hybrid() {
            Some((index, visibility)) => Source::Index {
                index: Box::new(RwLock::new(index)),
                visibility,
                caught_up: Mutex::new(HashMap::new()),
            },
            None => Source::Walk {
                paths: self.walk_files(),
                changed: Mutex::new(BTreeSet::new()),
            },
        };
        Corpus {
            root: self.root.clone(),
            source,
        }
    }

    fn open_index(&self) -> Option<(IndexReader, PathVisibility)> {
        let meta = self.usable_meta()?;
        let reader = IndexReader::open(&self.index_dir).ok()?;
        if reader.is_degenerate() || reader.validate_lookup().is_err() {
            return None;
        }
        let visibility = self.visibility(&meta, reader.file_table_id())?;
        Some((reader, visibility))
    }

    /// The index with an empty overlay for the files that change after it,
    /// as `tgrep serve` keeps it.
    fn open_hybrid(&self) -> Option<(HybridIndex, PathVisibility)> {
        let meta = self.usable_meta()?;
        let index = HybridIndex::open(&self.index_dir, &self.root).ok()?;
        let visibility = self.visibility(&meta, index.reader_arc().file_table_id())?;
        Some((index, visibility))
    }

    fn usable_meta(&self) -> Option<IndexMeta> {
        let meta = IndexMeta::load(&self.index_dir).ok()?;
        self.meta_is_usable(&meta).then_some(meta)
    }

    /// Which of the index's entries are hidden, when that was recorded for
    /// this very generation of it.
    fn visibility(&self, meta: &IndexMeta, file_table_id: FileTableId) -> Option<PathVisibility> {
        let visibility = path_index::read_filename_index(&self.index_dir)
            .ok()??
            .visibility?;
        visibility
            .covers_index(meta, file_table_id)
            .then_some(visibility.paths)
    }

    /// Files that differ from the index: modified, added or deleted since it
    /// read them, found by comparing the folder with the file stamps the
    /// index keeps. Takes a walk of the folder, and reads no file. `None`
    /// when the index keeps no stamps.
    pub fn stale_files(&self) -> Option<Vec<String>> {
        let evidence = meta::read_file_evidence(&self.index_dir).ok()?;
        if evidence.stamps.is_empty() {
            return None;
        }
        let walk = self.walk_metadata();
        let stale = StaleFiles::classify(&walk.files, &evidence, std::iter::empty());
        Some(stale.read.into_iter().chain(stale.deleted).collect())
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

    /// Every file's metadata, walked with the rules a build walks with.
    fn walk_metadata(&self) -> MetaWalkResult {
        walker::walk_file_metadata(
            &self.root,
            &MetaWalkOptions {
                exclude_paths: std::fs::canonicalize(&self.index_dir).into_iter().collect(),
                ..Default::default()
            },
        )
    }

    fn full_path(&self, relative: &str) -> PathBuf {
        self.root
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
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
/// Where [`RepoIndex::update_index`] indexes changed files before merging them.
const DELTA_DIR: &str = "gui-delta";
/// tgrep-core reads this file to decide whether an index exists.
const META_FILE: &str = "meta.json";

/// The files that differ from an index, by their stamps.
struct StaleFiles {
    /// Modified and new files, to read.
    read: Vec<String>,
    /// How many of `read` are new.
    added: usize,
    /// Files the index stamped or holds that are gone.
    deleted: Vec<String>,
}

impl StaleFiles {
    /// Compare the walked `files` with the `evidence` of the reads that built
    /// the index, as `tgrep serve` does. A file counts as modified when its
    /// stamp or its precise version differs, or its version is unknown. A
    /// file without a stamp is new; that includes one whose last read
    /// failed, so it is tried again. `indexed` adds the index's own paths,
    /// so that entries without a stamp go too when their file is gone.
    fn classify<'a>(
        files: &'a [FileMeta],
        evidence: &'a FileEvidence,
        indexed: impl Iterator<Item = &'a str>,
    ) -> Self {
        let mut read = Vec::new();
        let mut added = 0;
        for file in files {
            let path = &file.relative_path;
            match evidence.stamps.get(path) {
                None => {
                    added += 1;
                    read.push(path.clone());
                }
                Some(stamp)
                    if stamp.mtime != file.mtime
                        || stamp.size != file.size
                        || file.version.is_none()
                        || evidence.versions.get(path) != file.version.as_ref() =>
                {
                    read.push(path.clone());
                }
                Some(_) => {}
            }
        }
        let present: HashSet<&str> = files
            .iter()
            .map(|file| file.relative_path.as_str())
            .collect();
        let mut deleted: Vec<String> = evidence
            .stamps
            .keys()
            .map(String::as_str)
            .chain(indexed)
            .filter(|path| !present.contains(path))
            .map(str::to_string)
            .collect();
        deleted.sort_unstable();
        deleted.dedup();
        Self {
            read,
            added,
            deleted,
        }
    }

    fn changes(&self) -> FileChanges {
        FileChanges {
            modified: self.read.len() - self.added,
            added: self.added,
            deleted: self.deleted.len(),
        }
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

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
/// every search until the index changes. Files changed since the index was
/// built join it through [`Corpus::catch_up`].
pub struct Corpus {
    root: PathBuf,
    source: Source,
}

enum Source {
    Index {
        /// The on-disk index, and in its overlay the changed files a default
        /// search can see.
        index: Box<RwLock<HybridIndex>>,
        visibility: PathVisibility,
        /// Changed path -> the version of its change the overlay holds.
        caught_up: Mutex<HashMap<String, u64>>,
    },
    Walk {
        paths: Vec<String>,
        /// Visible files changed since the walk.
        changed: Mutex<BTreeSet<String>>,
    },
}

/// What the overlay learns of a changed file.
enum Overlay {
    /// Text to search, as its trigrams.
    Text(TrigramMaskMap),
    /// Read, but not text.
    Binary,
    /// Not read: gone, unreadable, too large, binary by its extension, or
    /// hidden since the index was built.
    Gone,
}

impl Corpus {
    pub fn is_indexed(&self) -> bool {
        matches!(self.source, Source::Index { .. })
    }

    pub fn file_count(&self) -> usize {
        match &self.source {
            Source::Index { index, .. } => index.read().unwrap().num_files(),
            Source::Walk { paths, .. } => paths.len(),
        }
    }

    /// Bring the corpus up to date with the files changed since its index was
    /// built, reading only those whose change it has not seen yet, and only
    /// when a default search can see them. Changes no longer listed, which a
    /// newer index covers, are let go. Returns how many files were read.
    pub fn catch_up(&self, changes: &[Change]) -> usize {
        match &self.source {
            Source::Index {
                index,
                visibility,
                caught_up,
            } => {
                let mut caught_up = caught_up.lock().unwrap();
                let listed: HashSet<&str> = changes.iter().map(|c| c.path.as_str()).collect();
                let forgotten: Vec<String> = caught_up
                    .keys()
                    .filter(|path| !listed.contains(path.as_str()))
                    .cloned()
                    .collect();
                let fresh: Vec<&Change> = changes
                    .iter()
                    .filter(|c| caught_up.get(&c.path) != Some(&c.version))
                    .collect();
                if forgotten.is_empty() && fresh.is_empty() {
                    return 0;
                }
                // Read outside the index lock, so searches already caught up
                // carry on meanwhile. Files the index hides are left alone:
                // their entries stay hidden, and they are never read.
                let entries: Vec<(&Change, Option<Overlay>)> = fresh
                    .into_par_iter()
                    .map(|change| {
                        let entry = if !visibility.is_visible(&change.path, "", false) {
                            None
                        } else if self.newly_hidden(&change.path) {
                            Some(Overlay::Gone)
                        } else {
                            Some(self.read_for_overlay(&change.path))
                        };
                        (change, entry)
                    })
                    .collect();
                let read = entries
                    .iter()
                    .filter(|(_, entry)| matches!(entry, Some(Overlay::Text(_) | Overlay::Binary)))
                    .count();
                let mut index = index.write().unwrap();
                index.live.clear_reconciled_paths(&forgotten);
                for path in &forgotten {
                    caught_up.remove(path);
                }
                for (change, entry) in entries {
                    match entry {
                        Some(Overlay::Text(trigrams)) => {
                            index.live.commit_upsert(&change.path, trigrams)
                        }
                        // Nothing to search: the index's old entry goes.
                        Some(Overlay::Binary | Overlay::Gone) => {
                            index.live.delete_file(&change.path)
                        }
                        None => {}
                    }
                    caught_up.insert(change.path.clone(), change.version);
                }
                read
            }
            Source::Walk { changed, .. } => {
                // Every candidate is read anyway; only which ones is kept.
                let mut changed = changed.lock().unwrap();
                changed.clear();
                changed.extend(
                    changes
                        .iter()
                        .filter(|c| !self.newly_hidden(&c.path))
                        .map(|c| c.path.clone()),
                );
                0
            }
        }
    }

    /// Read a changed file the way a build reads it.
    fn read_for_overlay(&self, relative: &str) -> Overlay {
        let path = self.full_path(relative);
        let too_large = |len| walker::DEFAULT_MAX_FILE_SIZE.is_some_and(|limit| len > limit);
        let readable = std::fs::metadata(&path)
            .is_ok_and(|metadata| metadata.is_file() && !too_large(metadata.len()));
        if !readable || walker::is_binary_extension(&path) {
            return Overlay::Gone;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            return Overlay::Gone;
        };
        let text = tgrep_core::encoding::decode_for_index(&bytes);
        if trigram::is_binary(&text) {
            Overlay::Binary
        } else {
            Overlay::Text(LiveIndex::compute_trigram_masks(&text))
        }
    }

    /// Whether the file, or a folder on its way, is hidden by its name or, on
    /// Windows, its attributes, by tgrep's rule: what the index could not
    /// know of folders made after it was built.
    fn newly_hidden(&self, relative: &str) -> bool {
        relative
            .match_indices('/')
            .map(|(end, _)| &relative[..end])
            .chain(std::iter::once(relative))
            .any(|prefix| {
                let path = self.full_path(prefix);
                let metadata = std::fs::symlink_metadata(&path).ok();
                visibility::is_hidden(&path, metadata.as_ref())
            })
    }

    /// Repository-relative, `/`-separated paths of every file that can match
    /// `plan`, changed files included once caught up. Hidden files are left
    /// out, as in a default `tgrep` search.
    pub fn candidates(&self, plan: &QueryPlan) -> Vec<String> {
        match &self.source {
            Source::Index {
                index, visibility, ..
            } => {
                let index = index.read().unwrap();
                let (ids, reader) = index.execute_query_with_masks(plan);
                let mut paths: Vec<String> = ids
                    .into_iter()
                    .filter_map(|id| index.resolve_path(id, &reader))
                    .filter(|path| visibility.is_visible(path, "", false))
                    .collect();
                paths.sort();
                paths
            }
            Source::Walk { paths, changed } => {
                let mut paths = paths.clone();
                let changed = changed.lock().unwrap();
                if !changed.is_empty() {
                    paths.extend(changed.iter().cloned());
                    paths.sort();
                    paths.dedup();
                }
                paths
            }
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
    use tgrep_core::query;

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

    fn candidates(index: &RepoIndex, literal: &str) -> Vec<String> {
        let corpus = index.load_corpus();
        assert!(corpus.is_indexed());
        corpus.candidates(&query::build_literal_plan(literal, false))
    }

    /// Enough files that a few changes stay under the full-rebuild cut-off.
    fn larger_tree() -> tempfile::TempDir {
        let dir = tree();
        for n in 0..8 {
            std::fs::write(
                dir.path().join(format!("src/filler{n}.rs")),
                format!("fn filler_{n}() {{}}\n"),
            )
            .unwrap();
        }
        dir
    }

    #[test]
    fn updating_reads_only_changed_files_and_merges_them() {
        let dir = larger_tree();
        let index = RepoIndex::open(dir.path()).unwrap();
        index.build_index().unwrap().publish().unwrap();

        std::fs::write(dir.path().join("src/lib.rs"), "pub fn moved_needle() {}\n").unwrap();
        std::fs::write(dir.path().join("src/added.rs"), "fn brand_new() {}\n").unwrap();
        std::fs::remove_file(dir.path().join("src/filler0.rs")).unwrap();
        std::fs::write(dir.path().join(".hidden.rs"), "fn brand_new() {}\n").unwrap();

        let update = index.update_index().unwrap();
        let IndexUpdate::Merged { staged, changes } = update else {
            panic!("expected a merge");
        };
        assert_eq!(
            changes,
            FileChanges {
                modified: 1,
                added: 2,
                deleted: 1
            }
        );
        staged.publish().unwrap();

        assert!(matches!(
            index.index_status(),
            IndexStatus::Ready { files: 11, .. }
        ));
        assert!(candidates(&index, "needle_here").is_empty());
        assert_eq!(candidates(&index, "moved_needle"), ["src/lib.rs"]);
        // Hidden files are indexed but, as after a build, not searched.
        assert_eq!(candidates(&index, "brand_new"), ["src/added.rs"]);
        assert!(candidates(&index, "filler_0").is_empty());
        assert_eq!(candidates(&index, "filler_1"), ["src/filler1.rs"]);
        assert!(!index.index_dir.join(DELTA_DIR).exists());
        assert!(!index.index_dir.join(STAGING_DIR).exists());

        // The merged index stamps what it read, so nothing looks stale now.
        assert_eq!(index.stale_files(), Some(Vec::new()));
        assert!(matches!(
            index.update_index().unwrap(),
            IndexUpdate::UpToDate
        ));
    }

    #[test]
    fn updating_an_unchanged_index_publishes_nothing() {
        let dir = tree();
        let index = RepoIndex::open(dir.path()).unwrap();
        index.build_index().unwrap().publish().unwrap();
        let corpus = index.load_corpus();
        // An open corpus is no obstacle: nothing is replaced.
        assert!(matches!(
            index.update_index().unwrap(),
            IndexUpdate::UpToDate
        ));
        drop(corpus);
        assert_eq!(candidates(&index, "needle_here"), ["src/lib.rs"]);
    }

    #[test]
    fn updating_with_only_deletions_merges_an_empty_delta() {
        let dir = larger_tree();
        let index = RepoIndex::open(dir.path()).unwrap();
        index.build_index().unwrap().publish().unwrap();
        std::fs::remove_file(dir.path().join("src/lib.rs")).unwrap();

        let IndexUpdate::Merged { staged, changes } = index.update_index().unwrap() else {
            panic!("expected a merge");
        };
        assert_eq!(changes.deleted, 1);
        staged.publish().unwrap();
        assert!(candidates(&index, "needle_here").is_empty());
        assert_eq!(candidates(&index, "filler_7"), ["src/filler7.rs"]);
    }

    #[test]
    fn updating_without_an_index_or_after_many_changes_rebuilds() {
        let dir = tree();
        let index = RepoIndex::open(dir.path()).unwrap();
        let update = index.update_index().unwrap();
        assert!(matches!(update, IndexUpdate::Rebuilt { .. }));
        update.into_staged().unwrap().publish().unwrap();

        // Both files change: reading them all is no dearer than a merge.
        std::fs::write(dir.path().join("src/lib.rs"), "fn one() {}\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "two\n").unwrap();
        let update = index.update_index().unwrap();
        assert!(matches!(update, IndexUpdate::Rebuilt { .. }));
        update.into_staged().unwrap().publish().unwrap();
        assert_eq!(candidates(&index, "one()"), ["src/lib.rs"]);
    }

    #[test]
    fn stale_files_include_deleted_and_moved_files() {
        let dir = tree();
        let index = RepoIndex::open(dir.path()).unwrap();
        assert_eq!(index.stale_files(), None);
        index.build_index().unwrap().publish().unwrap();
        assert_eq!(index.stale_files(), Some(Vec::new()));

        // A move keeps the file's modification time, which a check by time
        // alone would miss.
        std::fs::rename(dir.path().join("src/lib.rs"), dir.path().join("lib.rs")).unwrap();
        let mut stale = index.stale_files().unwrap();
        stale.sort();
        assert_eq!(stale, ["lib.rs", "src/lib.rs"]);
    }

    fn change(path: &str, version: u64) -> Change {
        Change {
            path: path.into(),
            version,
        }
    }

    #[test]
    fn changed_files_are_searched_by_their_new_contents() {
        let dir = tree();
        let index = RepoIndex::open(dir.path()).unwrap();
        index.build_index().unwrap().publish().unwrap();
        let corpus = index.load_corpus();

        std::fs::write(dir.path().join("src/lib.rs"), "pub fn moved_needle() {}\n").unwrap();
        std::fs::write(dir.path().join("src/new.rs"), "fn brand_new() {}\n").unwrap();
        std::fs::remove_file(dir.path().join("README.md")).unwrap();
        let read = corpus.catch_up(&[
            change("README.md", 0),
            change("src/lib.rs", 1),
            change("src/new.rs", 2),
        ]);
        assert_eq!(read, 2);

        let find = |literal| corpus.candidates(&query::build_literal_plan(literal, false));
        assert!(find("needle_here").is_empty());
        assert_eq!(find("moved_needle"), ["src/lib.rs"]);
        assert_eq!(find("brand_new"), ["src/new.rs"]);
        assert_eq!(
            corpus.candidates(&QueryPlan::MatchAll),
            ["src/lib.rs", "src/new.rs"]
        );
    }

    #[test]
    fn catching_up_reads_only_files_that_changed_again() {
        let dir = tree();
        let index = RepoIndex::open(dir.path()).unwrap();
        index.build_index().unwrap().publish().unwrap();
        let corpus = index.load_corpus();

        std::fs::write(dir.path().join("src/lib.rs"), "fn first() {}\n").unwrap();
        assert_eq!(corpus.catch_up(&[change("src/lib.rs", 4)]), 1);
        assert_eq!(corpus.catch_up(&[change("src/lib.rs", 4)]), 0);
        std::fs::write(dir.path().join("src/lib.rs"), "fn second() {}\n").unwrap();
        assert_eq!(corpus.catch_up(&[change("src/lib.rs", 5)]), 1);
        let find = |literal| corpus.candidates(&query::build_literal_plan(literal, false));
        assert!(find("first()").is_empty());
        assert_eq!(find("second()"), ["src/lib.rs"]);

        // A change no longer listed is covered by the index again.
        assert_eq!(corpus.catch_up(&[]), 0);
        assert_eq!(find("needle_here"), ["src/lib.rs"]);
    }

    #[test]
    fn hidden_changed_files_are_neither_read_nor_searched() {
        let dir = tree();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/FETCH_HEAD"), "abc branch 'main'\n").unwrap();
        let index = RepoIndex::open(dir.path()).unwrap();
        index.build_index().unwrap().publish().unwrap();
        let corpus = index.load_corpus();
        let files = corpus.file_count();

        std::fs::write(dir.path().join(".git/FETCH_HEAD"), "fresh_fetch\n").unwrap();
        // A hidden folder that did not exist when the index was built.
        std::fs::create_dir_all(dir.path().join("src/.cache")).unwrap();
        std::fs::write(dir.path().join("src/.cache/tmp.rs"), "fresh_cache\n").unwrap();
        let read = corpus.catch_up(&[change(".git/FETCH_HEAD", 0), change("src/.cache/tmp.rs", 1)]);
        assert_eq!(read, 0);
        // The index's own hidden entry is left as it is.
        assert_eq!(corpus.file_count(), files);
        let find = |literal| corpus.candidates(&query::build_literal_plan(literal, false));
        assert!(find("fresh_fetch").is_empty());
        assert!(find("fresh_cache").is_empty());
        assert_eq!(
            corpus.candidates(&QueryPlan::MatchAll),
            ["README.md", "src/lib.rs"]
        );
    }

    #[test]
    fn a_walked_folder_searches_its_visible_changed_files_too() {
        let dir = tree();
        let index = RepoIndex::open(dir.path()).unwrap();
        let corpus = index.load_corpus();
        assert!(!corpus.is_indexed());
        std::fs::write(dir.path().join("added.rs"), "fn added() {}\n").unwrap();
        std::fs::write(dir.path().join(".env"), "SECRET=1\n").unwrap();
        corpus.catch_up(&[change(".env", 0), change("added.rs", 1)]);
        assert_eq!(
            corpus.candidates(&QueryPlan::MatchAll),
            ["README.md", "added.rs", "src/lib.rs"]
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
