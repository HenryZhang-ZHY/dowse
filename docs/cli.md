<p align="right">
  <strong>English</strong> | <a href="zh-CN/cli.md">简体中文</a>
</p>

# The Command Line

The `dowse` command-line interface drives the running dowse application over a local
IPC socket. Subcommands share the application's warm in-memory trigram indexes and
background file watchers. If dowse is not running when a command is issued, it
automatically starts a headless background instance without opening windows.

- [Single-Instance IPC Architecture](#single-instance-ipc-architecture)
- [Command Reference](#command-reference)
  - [Search and Inspection](#search-and-inspection)
  - [Repository Management](#repository-management)
  - [Index Administration](#index-administration)
  - [Tasks and Settings](#tasks-and-settings)
  - [Developer Diagnostics](#developer-diagnostics)
- [Output for People and Coding Agents](#output-for-people-and-coding-agents)
  - [Standard Output vs Standard Error](#standard-output-vs-standard-error)
  - [Output Budgeting and Formatting](#output-budgeting-and-formatting)
  - [Machine-Readable JSON and Table Modes](#machine-readable-json-and-table-modes)
- [Coding Agent Integration](#coding-agent-integration)

For agent-specific tips, see [Guide for Coding Agents](agent-guide.md) or run `dowse guide`.
For launching the desktop GUI with folders or workspaces, see
[The Desktop App](app.md#workspaces-and-windows).

---

## Single-Instance IPC Architecture

All CLI subcommands communicate with the primary application process through a
local domain socket (Unix) or named pipe (Windows):

- **Shared State**: The CLI incurs no cold index-loading penalty if the desktop app
  or background daemon is active.
- **Headless Daemon**: When invoked while the desktop app is closed, the CLI launches
  a headless instance. This background instance remains alive for **10 minutes** of
  inactivity before cleanly shutting down. Opened repositories remain warm in memory
  during this window, making sequential queries from agents or scripts instant.
- **Error Handling**: Unknown options produce an exit code of `2`, and typos in
  subcommands trigger helpful suggestions for the closest matching command.

---

## Command Reference

### Search and Inspection

| Command | Description |
| --- | --- |
| `dowse search <query>` | Searches all known repositories using the [query syntax](search.md). Scope can be narrowed using `--here`, `-t <tag>`, or `-W <workspace>`. |
| `dowse status` | Displays the status of the running app: open windows, loaded repositories, index status, configuration paths, and active log locations. |
| `dowse guide` | Prints the complete [Guide for Coding Agents](agent-guide.md) to stdout. |
| `dowse quit` | Gracefully shuts down the running desktop app or headless daemon. |

### Repository Management

| Command | Description |
| --- | --- |
| `dowse repos` | Lists all registered repositories with current branch, index status, file count, last indexed timestamp, and assigned tags. Add `--json` for machine output. |
| `dowse repos add <folder>... [-t <tag>]` | Adds one or more repositories (or all repositories found within a folder) to the library, optionally assigning tags. |
| `dowse repos tag <repo> <tag>... [-r <tag>]` | Adds tags to a repository, or removes them via `-r <tag>`. |
| `dowse repos github [<owner>]` | Lists an owner's GitHub repositories (defaults to authenticated user) via `gh`. Pass `-q` to print only `owner/name`. |
| `dowse repos clone <owner/name>... [--into <folder>]` | Clones repositories in the background into `<folder>/<owner>/<name>`. Flags: `--from <owner>` to clone all, `--mode shallow\|full` for clone depth, `-t` for tags, `--pull-every 1h` for scheduled pulls, and `--wait` to block until complete. |
| `dowse repos pull [<repo>...] [--wait]` | Fetches and safely fast-forwards repositories. Omit arguments to pull everything in scope (`--here`, `-t`, `-W`). |
| `dowse repos sync <repo>... --every <interval>` | Configures automated scheduled pulls (e.g. `15m`, `1h`, `3d`). Pass `--off` to disable. |

### Index Administration

| Command | Description |
| --- | --- |
| `dowse index [--wait] [--full]` | Brings indexes in scope up to date incrementally, reading only modified files. Pass `--full` to rebuild from scratch. |
| `dowse repos index-location <repo>... [--repo\|--external\|--default]` | Shows or changes index storage locations ([in-repo `.tgrep` vs centralized external](indexes.md#or-kept-out-of-the-way)), moving index files on the fly. Pass `-q` to print the raw folder path for direct use with `tgrep search --index-path`. |

### Tasks and Settings

| Command | Description |
| --- | --- |
| `dowse tasks [--wait]` | Lists active and queued background clones and pulls. Use `dowse tasks cancel <id>...` or `--all` to abort. |
| `dowse settings` | Displays all persistent configuration keys and values. |
| `dowse settings set <key> <value>` | Updates a setting: `index.location` (`repo` or `external`), `index.external-dir`, `tasks.clones` (1–16), `tasks.pulls` (1–16), `updates.check` (`true`/`false`). |
| `dowse settings unset <key>` | Resets a configuration key to its default value. |

### Developer Diagnostics

| Command | Description |
| --- | --- |
| `dowse dev logs [-f] [--level <level>]` | Displays recent application log entries. Pass `-f` to follow live output, or `--level debug` for verbose traces. |
| `dowse dev metrics` | Prints core performance counters: memory footprint, search timings, index build durations, and cache efficiency. |
| `dowse dev open` | Opens the graphical developer diagnostics window. |

---

## Output for People and Coding Agents

Output formatting is designed to be as ergonomic for automated agents as it is for
human terminal users:

### Standard Output vs Standard Error
- **stdout**: Strictly reserved for query matches and data payloads.
- **stderr**: Reserved for counts, timings, status notifications, and narrowing advice.
- **Exit Status**:
  - `0`: Matches were found.
  - `1`: Query executed successfully, but no matches were found.
  - `2`: Syntax error, unknown argument, or runtime failure.

### Output Budgeting and Formatting
To prevent flooding terminal buffers or exceeding LLM context windows:
- **Default Budget**: Output is capped at **100 matching lines** total (`-n`) and
  **20 lines per file** (`-m`). Pass `-n 0` to uncap.
- **Narrowing Suggestions**: When results are truncated, the stderr footer suggests
  specific qualifiers derived from Facet distributions that would narrow the results:
  ```
  narrow with: repo:api (120)  language:Rust (80)  path:src/** (64)
  ```
- **File List Only (`-l`)**: Prints only the relative paths of files containing matches.
  Ideal for broad preliminary reconnaissance before detailed inspection.
- **Count Mode (`-c`)**: Prints paths alongside the count of matches within each.
- **Context Lines (`-C <N>`)**: Includes `N` lines of context before and after each match
  (formatted as `line-context` to distinguish from `line:match`).
- **Silent Mode (`-q`)**: Suppresses all stdout output; check exit code for presence of matches.

### Machine-Readable JSON and Table Modes

#### 1. Streaming JSON (`--json`)
Outputs one newline-delimited JSON (NDJSON) object per matching file, followed by a
single final `summary` object:

```json
{"type":"file","repo":"api","branch":"main","path":"src/config.rs","abs_path":"/src/api/src/config.rs","language":"Rust","matched_lines":2,"lines":[{"line":12,"text":"pub fn parse_config(","match":true,"ranges":[[7,19]]}]}
{"type":"summary","matched_lines":2,"files":1,"shown_lines":2,"shown_files":1,"searched_files":3,"corpus_files":4120,"repos":5,"unindexed_repos":0,"truncated":false,"elapsed_ms":8.1,"candidates_ms":0.9,"bytes_read":20480,"facets":[]}
```

#### 2. Tabular Output (`--table <format>`)
Formats rows identically to the desktop table view, accepting `csv`, `tsv`, `md`, or `json`:
- Each row represents a matching line with columns for repository, branch, path, line, column, language, and line text.
- If the search query contains named regex capture groups, each capture group becomes an independent named column:
  ```bash
  dowse search --table csv '/version = "(?<version>[^"]+)"/ path:Cargo.toml'
  ```
- Combine with `--stats` to print index candidate reduction metrics to stderr.

---

## Coding Agent Integration

For automated coding agents (e.g. Claude Code, Cursor, Copilot Workspace), recommended
command invocation patterns include:

```bash
# 1. Check workspace inventory
dowse repos --json

# 2. Broad search: list matching files first
dowse search -l 'parse_config lang:rust'

# 3. Targeted inspection with budget and context
dowse search -n 20 -C 2 'fn parse_config repo:api'

# 4. Structured extraction via table export
dowse search --table json '/pub fn (?<func>\w+)\(/ lang:rust'
```
