<p align="right">
  <strong>English</strong> | <a href="zh-CN/app.md">简体中文</a>
</p>

# The Desktop Application

dowse provides a hardware-accelerated, responsive desktop application built with
[GPUI](https://github.com/zed-industries/zed) (via [GPUI Kit](https://gpui-kit.com)).
It combines multi-repository search, real-time query evaluation, live file preview,
and automated git synchronization in a clean, keyboard-friendly interface.

- [Workspaces and Windows](#workspaces-and-windows)
- [Adding Repositories](#adding-repositories)
- [The Repositories Page](#the-repositories-page)
- [Cloning from GitHub](#cloning-from-github)
- [Background Tasks and Scheduled Pulls](#background-tasks-and-scheduled-pulls)
- [Tags and Search Scopes](#tags-and-search-scopes)
- [Results: Snippets and Table View](#results-snippets-and-table-view)
- [Tabs and Navigation History](#tabs-and-navigation-history)
- [File Preview and External Editor](#file-preview-and-external-editor)
- [Menu, Command Palette, and Shortcuts](#menu-command-palette-and-shortcuts)
- [Developer Tools](#developer-tools)

For query syntax details, see [Searching](search.md). For how trigram indexes are built
and maintained, see [Indexes](indexes.md).

---

## Workspaces and Windows

Each window represents a **workspace**: the specific set of repositories it searches,
analogous to multi-root workspaces in VS Code.

- **Untitled and Saved Workspaces**: New windows start as untitled workspaces. Save them
  as `.dowse-workspace` files to preserve or share them. Any subsequent repository changes
  are saved automatically back to the file.
- **Session Restoration**: When dowse starts, all windows from the previous session—including
  untitled workspaces—are restored automatically.
- **Shared Resources**: Multiple windows sharing a repository share its in-memory trigram
  index and file watcher; there is no duplicate memory overhead or indexing work.
- **Workspace Menu**: The menu bar's **Workspace** section lets you create, open, save,
  and switch workspaces. Recent workspaces are listed with a quick-remove (`×`) button
  (which removes the item from the menu without deleting the file). The active workspace
  name appears at the bottom-left of the status bar.

### Launching from the Terminal

Launching dowse follows conventions familiar from VS Code's `code` CLI:

| Command                        | Action                                                                                                                                   |
| ------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------- |
| `dowse`                        | Restores windows from the previous session.                                                                                              |
| `dowse <folder>...`            | Opens specified folders as a new untitled workspace. A folder containing git repositories automatically registers each child repository. |
| `dowse <file>.dowse-workspace` | Opens that workspace, or focuses the window already displaying it.                                                                       |
| `dowse --add <folder>...`      | Adds specified folders to the workspace of the most recently focused window.                                                             |
| `dowse --remove <folder>...`   | Removes specified folders from the active workspace.                                                                                     |

> [!NOTE]
> Only one dowse process runs at a time. Launching `dowse` while an instance is active
> forwards commands over a local socket to the running application and exits immediately.
> Each distinct [settings directory](install.md#where-settings-live) (`DOWSE_CONFIG_DIR`)
> maintains its own independent process.

---

## Adding Repositories

You can add repositories individually or point dowse to a parent folder containing
multiple git repositories:

- **Drag and Drop**: Drag one or more folders directly onto the window, or drop a
  `.dowse-workspace` file to open it.
- **Keyboard Shortcut**: Press `Ctrl+O` (`Cmd+O` on macOS) to open the folder picker.
- **Command Line**: Run `dowse --add <folder>` from your terminal.
- **Windows Explorer Integration**: In the Repositories page's Settings section, you can
  enable an optional **"Add to dowse"** entry in the Windows Explorer context menu. This
  allows adding folders or opening workspace files via right-click without manual path entry
  (writes only to `HKEY_CURRENT_USER`).

Every search result displays both the repository name and current branch alongside the
matching file path.

---

## The Repositories Page

Press `Ctrl+,` (`Cmd+,` on macOS) or select **Repositories** from the menu to open the
repository management dashboard.

![The repositories page: cloned repositories with their branch, owner and mirror tags, and how many files each index holds](images/repositories-page.webp)

Key capabilities on this page:

- **Filter and Search**: Filter repositories by name, path, branch, or tag.
- **Batch Actions**: Select multiple repositories using row checkboxes (or the table
  header checkbox to select all visible repositories) to perform batch operations:
  - Pull latest changes (`git pull`)
  - Update or rebuild trigram indexes
  - Add or remove tags (`-tag` removes an existing tag)
  - Configure scheduled pull intervals
  - Change index storage location (in-repo vs external)
  - Remove repositories from the current workspace
- **Repository Details**: Clicking a row reveals its assigned tags, pull schedule,
  index health, file count, and last pull status.
- **Global Settings**: Configure global defaults for index storage locations and
  system shell integrations.
- **Navigation**: Press `Alt+Left` (or the Back button) to return to your previous search.

---

## Cloning from GitHub

The Repositories page includes a built-in GitHub cloning pane powered by the
[GitHub CLI](https://cli.github.com) (`gh`).

- **Zero-Credential Security**: dowse delegates authentication entirely to `gh` (simply run
  `gh auth login` once). dowse never touches or stores personal credentials or tokens.
- **Fast Pagination**: Repository lists for any user or organization are fetched in parallel
  pages. Even organizations with 2,000+ repositories load in seconds.
- **Filtering**: Filter repositories by name, fork status, and archive status.
- **Clone Modes**:
  - **Blobless** _(default and recommended)_: Clones full commit history but fetches file
    contents on demand upon checkout. Large repositories clone in seconds while git logs
    and blame remain fully functional.
  - **Shallow**: Fetches only the latest commit (`--depth 1`) for minimal disk footprint.
  - **Full**: Standard complete `git clone`.
- **Destination & Auto-Tagging**: Clones are placed into `<folder>/<owner>/<name>` and
  automatically tagged with `owner:<owner>`. Completed clones are immediately added to the
  active workspace and queued for background indexing.
- **Status Awareness**: Already cloned repositories are marked with a badge and can be added
  to the workspace with a single click.

---

## Background Tasks and Scheduled Pulls

Background operations—such as cloning and pulling—run asynchronously on dedicated worker
threads to keep the user interface fluid and responsive:

- **Concurrency Limits**: By default, up to 4 clones and 4 pulls run concurrently. Adjust this
  limit in the Tasks UI or via CLI:
  ```bash
  dowse settings set tasks.clones 8
  dowse settings set tasks.pulls 8
  ```
- **Task Management**: The Tasks section on the Repositories page shows live progress,
  error logs, and controls to cancel (`dowse tasks cancel <id>`), retry, or clear tasks.
  The status bar reflects active background workers.
- **Background Daemon**: Closing the last window leaves dowse running quietly in the
  background until active clones or pulls complete.
- **Safe Scheduled Pulls**: Assign any repository a pull interval (e.g. `15m`, `1h`, `3d`).
  Whenever dowse is running and the interval elapses, it pulls automatically:
  - It fetches remote references (`git fetch origin`).
  - It **fast-forwards only** if the tracking default branch is checked out, working files
    are clean, and there are no unpushed local commits.
  - If conditions are unsafe (e.g. `on feature/x, not main`, or `uncommitted changes`), it
    only fetches and logs the exact reason. It never performs merges, rebases, or stashes.
  - New commits automatically trigger an incremental index update.
  - Synced repositories receive the tag `sync:<interval>`, allowing instant scoping via
    the scope bar.

---

## Tags and Search Scopes

Tags categorize repositories and define search boundaries:

- **Tag Formats**: Use plain tags (e.g. `mirror`, `dev`, `core`) or key-value pairs
  (e.g. `owner:alice`, `project:billing`).
- **Automatic Branch Tag**: Every repository is automatically tagged with `branch:<name>`
  reflecting its currently checked-out branch, updating dynamically when you switch branches.
- **Scope Bar**: Located above search results, the scope bar lets you pick which repositories
  to search:
  - Tags within the same key group (`owner:alice` and `owner:bob`) act as **OR** alternatives.
  - Tags across different groups (`dev` and `owner:alice`) act as **AND** intersections.
- **Facets Sidebar**: The left sidebar (`Ctrl+B`) displays dynamic Facet counts for
  repository, branch, tag groups, language, and top-level directory:
  - Counts reflect items matching the query under current filters, exactly like [grep.app](https://grep.app).
  - Clicking any facet narrows results immediately without altering the broader workspace scope.

![Results for trigram index -path:test across multiple repositories with facets](images/search-across-repositories.webp)

---

## Results: Snippets and Table View

dowse offers two presentation modes for search results:

### 1. Snippet View _(Default)_

- Displays matching lines syntax-highlighted via Tree-sitter grammars with contextual lines.
- Highlights query terms and regex matches in distinct accents.
- Files with numerous matches collapse gracefully ("Show N more matches").

### 2. Table View (`Alt+T` / `Cmd+Alt+T`)

- Presents results in a structured grid: one row per matching line with columns for
  `Repository`, `Branch`, `Path`, `Line`, `Column`, `Language`, `Match`, and `Line Text`.
- **Dynamic Regex Capture Columns**: When your query contains named regex capture groups,
  each group automatically becomes a dedicated, sortable column:
  ```
  /version = "(?<version>[^"]+)"/ path:Cargo.toml
  ```
  This turns code search into an instant structural data extraction pipeline.
- **Sorting & Resizing**: Click any column header to sort (numerically for numbers);
  drag column borders to adjust widths.
- **Exporting Data**: Press `Ctrl+Shift+E` (`Cmd+Shift+E`) or click **Export** to output
  visible rows as **CSV** (with UTF-8 BOM for Microsoft Excel), **TSV**, **Markdown**,
  or **JSON**. You can also copy the table directly as TSV or Markdown for spreadsheets
  and issue trackers.

![The table view: one row per Cargo.toml version line, with a version column from the named capture group](images/table-view-capture-groups.webp)

---

## Tabs and Navigation History

- **Multi-Tab Searching**: Keep multiple queries open simultaneously:
  - `Ctrl+T` (`Cmd+T`): Opens a new search tab.
  - `Ctrl+W` (`Cmd+W`): Closes the active tab (or middle-click the tab header).
  - `Ctrl+Tab` / `Ctrl+Shift+Tab`: Cycles through open tabs.
  - Each tab preserves its own query, options, path filters, and preview scroll state.
- **Browser-Style History**: Use title bar arrows, `Alt+Left` / `Alt+Right`
  (`Ctrl+-` / `Ctrl+Shift+-` on macOS), or mouse back/forward buttons to traverse
  search history:
  - Navigating back restores the previous query, active filters, selected file preview,
    and exact scroll position.
  - History entries are created upon pause, pressing Enter, toggling filters, or clicking
    a result—preventing single-keystroke clutter.

---

## File Preview and External Editor

Click any search result to open the full-file preview pane beside your results:

![The preview pane showing a whole file beside the results, with the query's matches marked](images/file-preview.webp)

- **Instant Zero-Wait Rendering**: Files render immediately inside the GPUI engine
  with full syntax highlighting; no waiting for an external editor to launch.
- **Occurrence Stepping**: Use `F4` and `Shift+F4` (or the up/down toolbar buttons) to
  step through matches across the file, landing precisely on the matched token.
- **Split Resizing**: Drag the pane divider to resize; press `Esc` or `Ctrl+Alt+B`
  (`Cmd+Alt+B`) to toggle the preview pane.
- **Read-Only Selection**: Drag to select, double-click words, triple-click lines, and press
  `Ctrl+C` (`Cmd+C`) to copy text. Long lines wrap cleanly without modifying content.
- **Open in External Editor**: Click **Open in Editor** or press `Ctrl+Click` on any search
  result to jump directly to that exact line and column in your preferred editor.
  - Automatically detects VS Code, Cursor, Zed, or Sublime Text on `PATH`.
  - Override via the `DOWSE_EDITOR` environment variable:
    ```bash
    export DOWSE_EDITOR="code -g {file}:{line}"
    export DOWSE_EDITOR="nvim-qt +{line} {file}"
    ```

---

## Menu, Command Palette, and Shortcuts

Press `Ctrl+K` (`Cmd+K` on macOS, or `Ctrl+Shift+P`) to open the **Command Palette**:

- Fuzzy-searches all commands, open tabs, active scopes, recent workspaces, and query qualifiers.
- Type initials (e.g. `nt` for "New Tab") and press Enter to execute.
- Switch between Light and Dark themes via the title bar menu (`⋮`).

### Primary Keyboard Shortcuts

| Shortcut (Win / Linux)          | Shortcut (macOS)      | Description                               |
| ------------------------------- | --------------------- | ----------------------------------------- |
| `Ctrl+O`                        | `Cmd+O`               | Add repositories or folders               |
| `Ctrl+,`                        | `Cmd+,`               | Open Repositories dashboard               |
| `Ctrl+F`                        | `Cmd+F`               | Focus search query input                  |
| `Ctrl+P`                        | `Cmd+P`               | Toggle path filter funnel                 |
| `Alt+T`                         | `Cmd+Alt+T`           | Toggle Snippet / Table view               |
| `Alt+C`                         | `Cmd+Alt+C`           | Toggle Case-sensitive matching            |
| `Alt+W`                         | `Cmd+Alt+W`           | Toggle Whole-word matching                |
| `Alt+R`                         | `Cmd+Alt+R`           | Toggle Full-query Regular Expression mode |
| `Ctrl+B`                        | `Cmd+B`               | Toggle Facets sidebar                     |
| `Ctrl+Alt+B`                    | `Cmd+Alt+B`           | Toggle File preview pane                  |
| `Ctrl+T` / `Ctrl+W`             | `Cmd+T` / `Cmd+W`     | Open / Close search tab                   |
| `Ctrl+Shift+N`                  | `Cmd+Shift+N`         | Open new window                           |
| `Ctrl+Shift+O` / `Ctrl+Shift+S` | `Cmd+Shift+O / S`     | Open / Save workspace                     |
| `Ctrl+Shift+E`                  | `Cmd+Shift+E`         | Export results table as CSV               |
| `Ctrl+Shift+R`                  | `Cmd+Shift+R`         | Update indexes in scope                   |
| `F4` / `Shift+F4`               | `F4` / `Shift+F4`     | Step to next / previous match             |
| `Ctrl+Shift+I` / `F12`          | `Cmd+Shift+I` / `F12` | Open Developer Tools                      |

---

## Developer Tools

Press `Ctrl+Shift+I` (`Cmd+Shift+I` on macOS) or `F12` to open the integrated diagnostics suite:

- **Live Log Stream**: View real-time application logs, filterable by level (`debug`, `info`, `warn`, `error`) and regex.
- **Engine Metrics**: Inspect memory usage, query timings, cache efficiency, index build/update durations, and disk read avoidance stats.
- **Query Profiling**: Review recent searches with candidate pruning counts and verification elapsed times.
- **Copy Diagnostics**: One-click export of system diagnostics to clipboard for issue reports.
- **CLI Equivalents**: The same information can be inspected from the terminal via:
  ```bash
  dowse dev logs -f
  dowse dev metrics
  dowse status
  ```
