<p align="right">
  <strong>English</strong> | <a href="zh-CN/indexes.md">简体中文</a>
</p>

# Trigram Index Architecture and Lifecycle

Every repository registered in dowse receives a [tgrep](https://github.com/microsoft/tgrep)
trigram index. This inverted index narrows candidate file sets down to only the files
capable of satisfying the query before any file content is read from disk (see
[How a Search Runs](search.md#how-a-search-runs)).

- [How Trigram Indexing Works](#how-trigram-indexing-works)
- [Compatibility with the tgrep CLI](#compatibility-with-the-tgrep-cli)
- [Index Storage Locations](#index-storage-locations)
- [Real-Time Freshness via File Watchers](#real-time-freshness-via-file-watchers)
- [Incremental Updates and Auto-Sync](#incremental-updates-and-auto-sync)
- [Staging Directory and Continuous Search Availability](#staging-directory-and-continuous-search-availability)
- [Performance Benchmarks](#performance-benchmarks)

---

## How Trigram Indexing Works

A **trigram** is a 3-character sliding window extracted from text (for example, the term
`config` decomposes into `con`, `onf`, `nfi`, `fig`). The index maintains an inverted
posting list mapping each trigram to the IDs of files containing that trigram:

1. When a query is compiled, literal terms and regular expression patterns are analyzed
   for required trigrams.
2. The index computes intersections across posting lists for terms joined by `AND`, and
   unions for terms joined by `OR`.
3. Non-matching files (often 99% or more of the repository) are completely discarded
   before disk reads take place.

---

## Compatibility with the tgrep CLI

Each repository's default index resides in its `.tgrep/` root directory—the exact same
format and location used by Microsoft's `tgrep index` and `tgrep serve` tools.

- **Background Construction**: If a repository lacks an index upon being added, dowse
  queues a background build thread (one build at a time to prevent CPU starvation).
  Until the index completes, queries perform a standard file scan on that repository.
- **Accidental Deletion Recovery**: If an index directory is wiped while dowse is running
  (for instance, via `git clean -fdx`), the file watcher detects the deletion within
  seconds and automatically schedules a background rebuild.

---

## Index Storage Locations

While storing `.tgrep/` inside the repository working tree is standard, developers who
frequently run aggressive build cleaning or git hygiene tools can choose to store indexes
externally in a centralized directory.

### Configuration via CLI

```bash
# Store indexes externally for all repositories by default
dowse settings set index.location external

# Specify a custom external directory (defaults to user data directory)
dowse settings set index.external-dir D:/dowse-indexes

# Keep an exception in-repo for a specific repository
dowse repos index-location api --repo

# Retrieve the raw index path for scripting with tgrep
tgrep search foo --index-path "$(dowse repos index-location api -q)"
```

### Storage Migration Mechanics
- When changing a repository's index location, existing index files are moved on disk
  rather than rebuilt from scratch. If moving across filesystems is disallowed, the
  original is left intact and a new index is constructed at the target path.
- When repositories are deleted or moved, their orphaned external indexes are cleaned
  up automatically.

---

## Real-Time Freshness via File Watchers

Between index rebuilds, dowse maintains continuous freshness through real-time file
system watchers:

- **Active File Watcher**: A background watcher per repository tracks every file created,
  modified, or deleted since the index was last serialized.
- **Dirty-Set Merging**: When a search runs, modified and newly created files are
  automatically merged into the candidate set and read directly from disk. A `git pull`
  in a mirror repository or an editor save in an active working copy is searchable
  immediately with zero latency.
- **Offline Change Detection**: When dowse launches, it compares filesystem modification
  timestamps with the recorded index generation stamps, instantly identifying any edits,
  renames, or deletions that occurred while dowse was shut down.

---

## Incremental Updates and Auto-Sync

Re-indexing a repository does not require scanning the entire codebase:

- **Streamed Merging**: Incremental updates scan only the dirty files identified by
  the watcher. These files are indexed independently and streamed into a copy of the
  existing index, while unmodified posting lists are copied directly without re-tokenization.
- **Automatic Triggers**: A repository's index triggers an automatic incremental update
  once **500 file modifications** have accumulated.
- **Manual Triggers**:
  - `Ctrl+Shift+R` (`Cmd+Shift+R` on macOS): Incrementally updates all indexes in the active scope.
  - `dowse index [--wait]`: Updates all indexes in scope from the terminal.
  - `dowse index --full`: Forces a complete full rebuild from disk.
- **Full Rebuild Conditions**: Complete rebuilds occur only if no index exists, if
  filesystem permissions prevent reading directory metadata, if the majority of files
  have changed, or upon explicit user request.

---

## Staging Directory and Continuous Search Availability

All index updates and full rebuilds are written into an isolated staging directory:
- Queries can be executed concurrently without locking or interruption while an index
  build or update is underway.
- Once the new index is fully written and verified, an atomic pointer swap replaces the
  in-memory reader and filesystem reference.

---

## Performance Benchmarks

Performance metrics on a 245,000-file repository with a 1.7 GB index:

| Operation | Elapsed Time | Note |
| --- | --- | --- |
| Full Initial Build | ~9 minutes | Evaluates every file, builds complete trigram posting tables |
| Incremental Update | ~10 seconds | Re-indexes dirty files and streams postings into new index |
| Up-to-Date Verification | ~3 seconds | Quick timestamp comparison against index file table |
| Search Latency | 20–60 ms | Typical round-trip time across warm repositories |
