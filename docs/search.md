<p align="right">
  <strong>English</strong> | <a href="zh-CN/search.md">简体中文</a>
</p>

# Search Syntax and Query Engine

dowse evaluates searches in real time as you type. Queries adopt the syntax of
GitHub code search: terms combine **per file**, not per line. A query such as
`parse config` finds files that contain both `parse` and `config` anywhere across
their contents, and displays the matching lines for each.

- [Query Syntax](#query-syntax)
- [Qualifiers Reference](#qualifiers-reference)
- [The Path Filter Funnel](#the-path-filter-funnel)
- [Matching Modes and Toggles](#matching-modes-and-toggles)
- [Choosing What to Search](#choosing-what-to-search)
- [How a Search Runs](#how-a-search-runs)
- [Performance Characteristics](#performance-characteristics)

---

## Query Syntax

| Syntax | Behavior | Example |
| --- | --- | --- |
| `term1 term2` | Finds files containing all terms (`AND` is implicit). | `parse config` |
| `term1 AND term2` | Explicit `AND`. Binds tighter than `OR`. | `parse AND config` |
| `term1 OR term2` | Finds files containing either term. | `sqlite OR postgres` |
| `NOT term`, `!term` | Excludes files containing the term. | `NOT test`, `!mock` |
| `NOT (a OR b)` | Groups boolean expressions with parentheses. | `config NOT (test OR mock)` |
| `"exact phrase"` | Matches exact text; escape with `\"` and `\\`. | `"fn main() {"` |
| `/pattern/` | Matches a regular expression line by line. | `/fn \w+_test/` |
| `content:term` | Treats a qualifier-like token as plain text. | `content:path:foo` |

> [!NOTE]
> Positive terms compile into a combined regular expression that highlights matching
> lines. A query consisting solely of qualifiers (such as `path:*.proto`) lists all
> candidate files immediately without reading file contents from disk.

---

## Qualifiers Reference

Qualifiers filter candidate files before content matching begins. Values containing
spaces can be quoted (e.g. `language:"Visual Basic"`), and regex patterns can be used
in values (e.g. `path:/src\/(api|web)\//`).

| Qualifier | Description | Examples |
| --- | --- | --- |
| `path:<glob\|text>` | Matches against the relative file path. Anchored to root if it contains `/`. | `path:src/*.rs`, `path:engine`, `path:tests/` |
| `-path:<glob\|text>` | Excludes files whose path matches the pattern. | `-path:tests`, `-path:vendor/` |
| `language:<lang>` / `lang:<lang>` | Filters by language name, canonical alias, or extension. | `language:rust`, `lang:ts`, `lang:python` |
| `-language:<lang>` / `-lang:<lang>` | Excludes files of the specified language. | `-lang:md`, `-lang:json` |
| `repo:<name>` | Restricts search to repositories matching the name substring. | `repo:api`, `repo:frontend` |
| `-repo:<name>` | Excludes matching repositories. | `-repo:legacy` |
| `branch:<name>` | Restricts search to repositories with that branch checked out. | `branch:main`, `branch:v2` |
| `tag:<tag>` | Restricts search to repositories bearing that tag. | `tag:mirror`, `tag:owner:alice` |

---

## The Path Filter Funnel

While `path:` qualifiers inside the query box are suitable for quick filters, complex
trees benefit from the dedicated **Path Filter** box:

- Press `Ctrl+P` (`Cmd+P` on macOS) or click the funnel icon beside the search bar.
- Input space-separated filter terms:
  - `src` keeps paths containing `src`.
  - `*.rs` or `src/**/*.ts` matches globs.
  - `!tests` or `-*.md` excludes paths.
- The filter box remains visible across tabs as long as an active filter is defined.
- Path filter expressions drop candidate files before they are read from disk.

---

## Matching Modes and Toggles

Three global matching switches sit beside the search box:

| Shortcut (Win / Linux) | Shortcut (macOS) | Toggle | Description |
| --- | --- | --- | --- |
| `Alt+C` | `Cmd+Alt+C` | **Match Case** | Makes all query terms case-sensitive. By default, queries are case-insensitive. |
| `Alt+W` | `Cmd+Alt+W` | **Whole Word** | Wraps terms in word boundary markers (`\b`), matching complete identifiers only. |
| `Alt+R` | `Cmd+Alt+R` | **Regex Mode** | Treats the entire search input as a single line-by-line regular expression. |

---

## Choosing What to Search

1. **Workspace Scope**: In the desktop app, the scope bar above the results filters
   active repositories using tags (`owner:alice`, `mirror`, etc.). See
   [Tags and Search Scopes](app.md#tags-and-search-scopes).
2. **Facet Narrowing**: The left sidebar (`Ctrl+B`) provides real-time counts across
   repositories, branches, languages, and directories, allowing instant narrowing
   without altering saved workspace configurations.
3. **CLI Scoping**: From the terminal, use `--here` (current repo only), `-t <tag>`
   (filter by tag), or `-W <workspace>` (filter by saved workspace file). See
   [The Command Line](cli.md).

---

## How a Search Runs

Every search executes through an optimized three-phase pipeline:

1. **Query Compilation**: The query string is parsed. Plain terms have regex meta-characters
   escaped; whole-word flags inject `\b` boundaries. Positive terms form a combined
   display regex, while the expression tree generates a [tgrep](indexes.md) trigram query plan.
2. **Candidate Pruning**: Repository qualifiers rule out ineligible repositories. For each
   remaining repository, the trigram plan queries its index: required terms intersect
   candidate lists, alternative terms merge them, and negated terms narrow nothing. Files
   flagged as modified by the background file watcher are added to the candidate set.
   Path and language qualifiers then discard non-matching candidates before any disk I/O.
3. **Parallel Verification**: Candidate files are read and matched in parallel chunks using
   Rayon. Terms are verified conditionally, short-circuiting as soon as the file's match
   status is determined. Once roughly 20,000 matching lines (or 10,000 files) are reached,
   further file reads are skipped and the summary indicates truncation.

---

## Performance Characteristics

Because candidate pruning eliminates the overwhelming majority of non-matching files
without disk reads, dowse delivers consistent sub-100ms response times even over massive
codebases:

- **Index Build**: ~6 seconds for 42,000 source files (1.3 GB of source code).
- **Interactive Queries**: 20–60 ms typical round-trip query time.
- **Incremental Freshness**: File watcher updates keep search results current within
  milliseconds of saving in an external editor.
