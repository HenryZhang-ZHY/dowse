# dowse

Search code across many repositories at once, from a desktop app or the command
line, in the spirit of [grep.app](https://grep.app) and GitHub code search, but over
the clones on your own disk: the mirrors you keep on `main` and the working copies
you develop in. [tgrep](https://github.com/microsoft/tgrep)'s trigram index is the
engine and [GPUI](https://github.com/zed-industries/zed) (through
[GPUI Kit](https://gpui-kit.com)) the UI.

dowse was called tgrep-gpui until it outgrew being a window around tgrep. The first
start after the rename moves tgrep-gpui's settings over; `.tgrep-workspace` files
still open, and each repository keeps its `.tgrep` index.

## Features

- **Many repositories, one search box.** Add repositories one by one, or pick a folder
  that holds several git repositories to add them all. Results show which repository
  and branch each file comes from.
- **Workspaces, like VS Code's.** Each window shows a workspace: the repositories it
  searches. A new one is untitled; save it as a `.dowse-workspace` file to reopen or
  share it, and later changes are written back to the file. Open as many windows as
  you like; windows sharing a repository share its index and file watcher. The open
  windows, untitled workspaces included, come back on the next start. Closing a
  window with an unsaved workspace, while others stay open, asks whether to save it.
- **The repositories page** (`Ctrl+,`) manages a workspace's repositories. Filter
  them by name, path, branch or tag, select several (the box in the table's header
  takes every one shown, and the header then holds what to do) and pull them, update or rebuild
  their indexes, add or remove tags (`-tag` removes one), set how often they are
  pulled, or take them out of the workspace. A row opens to its tags and pull
  settings, and shows its index and how its last pull went.
- **Clone from GitHub, many at once.** The page's GitHub section lists an owner's
  repositories (your own unless you name a user or organization) through the
  [GitHub CLI](https://cli.github.com), so dowse never handles your sign-in: run
  `gh auth login` once. Pages of the list are fetched several at once and shown as
  they arrive (an organization of 2,000 repositories takes seconds), and Stop keeps
  what has arrived. Filter the list, show or hide forks and archived
  repositories, pick some (or all shown) and clone them into
  `<folder>/<owner>/<name>`, tagged `owner:<owner>`. Clones are blobless by default:
  every commit, but only the file contents checked out, the rest fetched when
  needed, which makes large repositories quick to clone while history still works.
  Shallow (the latest commit only) and full clones are there too. Finished clones
  join the workspace and get indexed. Repositories already cloned are marked, and
  can be added with a click.
- **Background tasks.** Clones and pulls run a few at a time (4 of each unless you
  change it), each on a thread of its own, so the app stays usable while a large
  repository clones. The page's Tasks section shows their progress and lets you
  cancel, retry or clear them; the status bar shows what is running. Closing the
  last window leaves the app running in the background until they finish.
- **Pull on a schedule.** Give a repository a pull interval (`15m`, `1h`, `3d` and
  so on) and dowse pulls it whenever the interval has passed while it runs,
  including right after starting when it is overdue. A pull fetches `origin`, then
  fast-forwards only when origin's default branch is checked out, tracked files are
  unchanged and there are no local commits; otherwise it only fetches, and says why
  (`on feature/x, not main`, `uncommitted changes`). It never merges, rebases or
  stashes. A pull that brings commits brings the index up to date. The interval shows
  as the tag `sync:<interval>`, so the scope bar can pick the synced repositories.
- **Add from anywhere.** Drop folders on a window to add them, or a workspace file to
  open it. From a terminal, `dowse --add <folder>` adds to the running app's last
  focused window, like `code --add`. On Windows, the repositories page can add "Add to
  dowse" to Explorer's folder menu, which does the same, and open workspace files with
  a double click; this writes to the current user's registry only when you ask.
- **Tags choose what to search.** Tag repositories freely: plain tags such as `mirror`
  or `dev`, or `key:value` tags such as `owner:alice` or `project:billing`. Every
  repository is also tagged `branch:<name>` with the branch it has checked out, which
  updates when you switch. The scope bar above the results picks the tags to search:
  tags in one group are alternatives (`owner:alice` or `owner:bob`), and groups narrow
  each other (`dev` and `owner:alice`). Tags belong to the repository, so every
  workspace sees them; the scope is remembered per workspace.
- **Facets narrow the results** without changing the scope: repository, branch, each
  tag group, language and top-level directory, each counted under the others'
  filters, as on grep.app.
- **GitHub code search syntax**, as you type. Terms combine per file, not per line:
  `parse config` finds files containing both, wherever they are, and shows the lines
  with either.

  | Query | Finds files |
  | --- | --- |
  | `parse config` | containing both (`AND` between them is optional) |
  | `parse OR config` | containing either; `AND` binds tighter than `OR` |
  | `parse NOT config`, `NOT (a OR b)` | containing `parse` but not `config`; without `a` or `b` |
  | `"fn main()"` | containing the exact text; `\"` and `\\` escape inside quotes |
  | `/fn \w+_test/` | with a line matching the regular expression |
  | `path:src/*.rs`, `path:engine` | whose path matches the glob (anchored when it holds a `/`) or contains the text |
  | `language:rust`, `lang:ts` | in the language, by name, alias or extension |
  | `repo:api`, `branch:main`, `tag:owner:alice` | from matching repositories |
  | `-path:tests`, `-lang:md` | not matching the qualifier |

  Qualifier values may be quoted (`language:"Visual Basic"`) or a `/regex/`, and
  `content:` makes a plain term of text that looks like a qualifier. A query of only
  qualifiers, such as `path:*.proto`, lists the files without reading them. Toggles
  set match case (`Alt+C`) and whole word (`Alt+W`) for every term; regular
  expression (`Alt+R`) takes the whole box as one regex, matched line by line. On
  macOS the shortcuts are `Cmd+Alt+C/W/R`.
- **Path filter**: `path:` in the query is usually enough; for more, the funnel in
  the search box (`Ctrl+P`) opens a box of space-separated terms beside it. `src`
  keeps paths containing `src`, `*.rs` keeps matching globs, and `!tests` or `-*.md`
  drops paths. The box stays open while a tab has a filter in it.
- **Results as snippets**, coloured by language with tree-sitter grammars: every match
  is highlighted, with one line of context. Long files collapse to their first
  matches ("Show N more matches").
- **Results as a table**, for analysis: the Table switch above the results (`Alt+T`,
  `Cmd+Alt+T` on macOS) lists one row per matching line, with its repository,
  branch, path, line, column, language, the matched text and the line itself. When
  the query's regex has capture groups, each gets a column, named after the group:
  `/version = "(?<version>[^"]+)"/` tabulates versions. Click a header to sort
  (numbers numerically), drag its edge to resize, click a row to preview it and
  double-click to open it. Export writes the rows, in the table's order and under
  the facet filters, as CSV (with a byte order mark, for Excel), TSV, Markdown or
  JSON (`Ctrl+Shift+E` for CSV), or copies them as TSV to paste into a spreadsheet
  or as Markdown. Rows are the kept lines: up to 200 per file and about 20,000 in all.
  When every row comes from one repository, the table hides its name and branch;
  exports always include them.
- **Search tabs.** Keep several searches open and switch between them without
  running them again: `Ctrl+T` opens a tab, `Ctrl+W` closes it, `Ctrl+Tab` and
  `Ctrl+Shift+Tab` (or `Ctrl+PageDown/PageUp`) step through them, and a middle click
  closes one. Each tab has its own query, options, path filter and facet filters; the
  scope is the window's. Tabs come back with their window on the next start.
- **Preview in place**: click a line to see the whole file beside the results,
  coloured by language, with the query's matches marked and the line in view; no
  waiting for an editor to start. Drag the divider to resize it and `Esc` closes it.
  `F4` and `Shift+F4` step through the matches, on into the next or previous file.
  Each tab keeps its own preview. The preview is for reading; nothing is edited.
- **Open in your editor** from the preview's "Open in Editor" button (at the chosen
  line), by double-clicking a line there, or straight from the results with
  `Ctrl+Click`. VS Code, Cursor, Zed or Sublime Text is used when found on `PATH`,
  otherwise the system default application. Set `DOWSE_EDITOR` to choose, for
  example `code -g {file}:{line}` or `nvim-qt +{line} {file}`.
- **Shares indexes with the tgrep CLI**. Each repository's index lives in its
  `.tgrep` directory, the same place `tgrep index` and `tgrep serve` use. A repository
  without an index gets one in the background, one build at a time; until then its
  files are scanned.
- **Stays fresh between builds**. A file watcher per repository tracks files changed
  since its last build, and searches read them directly, so a `git pull` in a mirror
  or an edit in a working copy shows up right away. Opening a repository also finds
  the files that changed while dowse was not running, deleted and moved ones
  included, by comparing the folder with the file stamps the index keeps.
- **Incremental index updates**. Bringing an index up to date reads only the files
  that changed: they are indexed on their own and streamed into a copy of the index,
  whose other postings are copied as they are. On a 245,000-file repository with a
  1.7 GB index, an update takes about 10 s where a full build takes about 9 minutes;
  finding the index current takes about 3 s. After 500 changes a repository's index
  is updated automatically. The whole index is built again only when there is none,
  when part of the folder cannot be read, when most files changed, or when you ask.
  Updates and builds happen in a staging directory, so searching keeps working while
  one runs.
- **A command line for people and agents.** `dowse search`, `dowse repos` and the
  other subcommands drive the running app, as Obsidian's command line does, so they
  share its warm indexes and file watchers; when the app is not running, the first
  command starts it in the background, without windows. Output is made for coding
  agents as much as for people: see [the command line](#the-command-line) below.
- **Developer tools**: `Ctrl+Shift+I` or `F12` opens a window with the app's log as
  it is written (filtered by level and text), its key metrics (memory, search
  timings from windows and the command line, how many file reads the indexes spared,
  index load, build and update timings, log problems) and the latest searches with what each
  read and found. Copy Diagnostics puts it all on the clipboard. The command line
  reads the same with `dowse dev logs` and `dowse dev metrics`.
- **Command palette**: `Ctrl+K` (`Cmd+K` on macOS; `Ctrl+Shift+P` also works) lists
  every command with its shortcut, plus the open tabs, the scope's tags, recent
  workspaces and the query qualifiers, which it adds to the query. Type a few letters
  of the name in order, `nt` for New Tab, and press `Enter`; `Esc` clears the
  filter, then closes.
- Light and dark themes. `Ctrl+O` adds repositories, `Ctrl+,` opens the repositories
  page (the palette also has Clone from GitHub, Pull Repositories in Scope and Show
  Background Tasks), `Ctrl+F` focuses the search box, `Ctrl+P` opens the path filter, and
  `Ctrl+Shift+R` updates the indexes in scope (the palette also rebuilds them from
  scratch). `Ctrl+Shift+N` opens a new window,
  `Ctrl+Shift+O` opens a workspace and `Ctrl+Shift+S` saves one. `Ctrl+Shift+I` or
  `F12` opens the developer tools.

## Download

Each [release](https://github.com/HenryZhang-ZHY/dowse/releases/latest) has builds
ready to run, with their checksums in `SHA256SUMS`:

| Platform | Archive | What's in it |
| --- | --- | --- |
| Windows (x64) | `dowse-<version>-windows-x86_64.zip` | `dowse.exe`, the app, and `dowse.com`, the command line (see below). Unzip them to a folder, and add it to `PATH` to run `dowse` in a terminal. |
| macOS 11 or later (Apple silicon and Intel) | `dowse-<version>-macos-universal.zip` | `dowse.app`. Move it to Applications. For the command line, link `/Applications/dowse.app/Contents/MacOS/dowse` into a folder on `PATH`. |
| Linux (x64, arm64) | `dowse-<version>-linux-<arch>.tar.gz` | `dowse`, both the app and the command line. Needs glibc 2.35 or later (Ubuntu 22.04, Debian 12, Fedora 36), X11 or Wayland, and a Vulkan driver. |

The builds are not signed with a paid certificate, so the system asks once before
the first start. On Windows, SmartScreen's "Windows protected your PC" has a
**More info** link, then **Run anyway**. On macOS, open `dowse.app` once, then allow
it under System Settings > Privacy & Security > **Open Anyway**, or clear the
quarantine flag the browser set:

```bash
xattr -dr com.apple.quarantine /Applications/dowse.app
```

## Build and run

Requires a recent stable Rust (the repository's `mise.toml` pins `latest`).

```bash
cargo run --release -- path/to/repo path/to/folder-of-repos
```

Launching the app follows VS Code's `code`:

| Command | What it does |
| --- | --- |
| `dowse` | Restores the last session's windows. |
| `dowse <folder>...` | Opens the folders as a new untitled workspace. A folder holding git repositories stands for each of them. |
| `dowse <file>.dowse-workspace` | Opens that workspace, or focuses the window already showing it. |
| `dowse --add <folder>...` | Adds the folders to the last focused window's workspace. |
| `dowse --remove <folder>...` | Takes them out again. |

As with `code`, only one dowse runs at a time: a launch while it is running hands
its command line to the running app and exits at once, and a launch without arguments
opens a new window there. Each settings directory (see below) gets its own app.

On Windows the app is a GUI program, which a terminal neither waits for nor shows
output from. Install the console program Cargo builds as `dowse-cli.exe` next to
it as `dowse.com`; terminals prefer `.com` to `.exe`, so `dowse` in a terminal runs
the command line, which hands launches to `dowse.exe`, while Explorer runs the app:

```powershell
cargo build --release
Copy-Item target\release\dowse-cli.exe target\release\dowse.com
```

On macOS and Linux the `dowse` binary is both.

Settings live under the user configuration directory (`%APPDATA%\dowse` on
Windows, `~/.config/dowse` on Linux, `~/Library/Application Support/dowse`
on macOS): `library.json` holds every repository's name and tags, `session.json` the
windows to restore and recent workspaces, `workspaces/` is where workspaces are
saved unless you pick elsewhere, and `logs/dowse.log` is the log (it starts over
past 5 MB, keeping the previous one as `dowse.old.log`; `DOWSE_LOG=debug` records
more). The first start after upgrading from a version with a single repository
list (`repos.json`) turns that list into a saved "Default" workspace. Set
`DOWSE_CONFIG_DIR` to keep settings elsewhere.

## The command line

| Command | What it does |
| --- | --- |
| `dowse search <query>` | Searches every repository dowse knows, in the query syntax above. `--here`, `-t <tag>` and `-W <workspace>` narrow the scope. |
| `dowse repos` | Lists the repositories with branch, index state, file count, when they were indexed and tags. |
| `dowse repos add <folder>... [-t <tag>]` | Adds repositories, or every repository in a folder, to the library. |
| `dowse repos tag <repo> <tag>... [-r <tag>]` | Adds tags to a repository, or removes them. |
| `dowse repos github [<owner>]` | Lists an owner's GitHub repositories (yours by default) through `gh`; `-q` prints `owner/name` only. |
| `dowse repos clone <owner/name>... [--into <folder>]` | Clones in the app's background into `<folder>/<owner>/<name>` (the last folder used when not given). `--from <owner>` clones an owner's repositories, `--mode shallow\|full` fetches less or more, `-t` tags, `--pull-every 1h` schedules pulls and `--wait` waits. |
| `dowse repos pull [<repo>...] [--wait]` | Pulls repositories, or those in scope (`--here`, `-t`, `-W`). |
| `dowse repos sync <repo>... --every <interval>` | Pulls them every `15m`, `1h`, `3d`...; `--off` stops. |
| `dowse tasks [--wait]` | Lists the background clones and pulls; `dowse tasks cancel <id>...` or `--all` cancels them. |
| `dowse index [--wait] [--full]` | Brings the indexes in scope up to date, reading only the files that changed; `--full` builds them again from every file. |
| `dowse status` | Shows the running app: windows, repositories, indexing, where its settings and log are. |
| `dowse dev logs [-f] [--level debug]` | Prints the app's latest log records, or follows them. |
| `dowse dev metrics` | Prints the app's key metrics and latest searches. |
| `dowse dev open` | Opens the developer tools window. |
| `dowse quit` | Quits the app. |
| `dowse guide` | Prints [the guide for coding agents](docs/agent-guide.md). |

Every subcommand is a request to the running app over its single-instance socket;
the app answers with data and the command line formats it. With no window open,
an app the command line started quits after ten minutes without requests, and
repositories opened for requests stay open that long, so an agent's next query is
fast too.

Results are shaped for agents as much as for people, following what makes tgrep
work well for them:

- Results go to stdout; counts, timings, notes and suggestions go to stderr. Exit
  status is 0 when something matched, 1 when nothing did and 2 on error. Unknown
  options are errors, and a mistyped subcommand is reported with the one meant.
- Text output is compact: each file's absolute path with its repository and branch,
  then `line:text` (with `-C`, context as `line-text`). `-l` prints paths only, `-c`
  paths with counts, `-q` nothing.
- Output has a budget: 100 matching lines (`-n`) and 20 per file (`-m`), with the
  totals in the footer. When results are cut, the footer suggests the qualifiers
  that would narrow them, from the facets, with how many files each keeps:
  `narrow with: repo:api (120)  language:Rust (80)  path:src/** (64)`.
- `--json` prints one object per line: a `file` per matching file, then a
  `summary` with counts, timings, facets and the narrowing suggestions.
  `--table csv|tsv|md|json` prints the table view's rows, with a column per capture
  group, and `--stats` how the indexes narrowed the search.

On Linux, GPUI needs the usual X11/Wayland and Vulkan development packages; see the
[Zed Linux build notes](https://github.com/zed-industries/zed/blob/main/docs/src/development/linux.md).

## Layout

| Path | What it holds |
| --- | --- |
| `src/engine/` | The search engine and saved settings, a library with no UI dependency. `github.rs` lists and clones GitHub repositories through `gh`; `sync.rs` pulls a repository when it is safe to and parses pull intervals; `tasks.rs` queues background tasks; `process.rs` runs git and gh. `index.rs` opens, builds, updates and publishes one repository's tgrep index; `repo.rs` holds repository metadata, tags, the scope and branch detection; `syntax.rs` parses the query language; `query.rs` compiles it and the path filter; `search.rs` narrows candidates through each index and matches lines in parallel; `facets.rs` counts and filters results; `table.rs` lays results out as rows, sorts and exports them; `preview.rs` prepares a whole file for the preview; `watch.rs` tracks changed files. `library.rs` keeps repository names and tags, `workspace.rs` reads and writes workspace files, `session.rs` the windows to restore, and `config.rs` locates the settings and migrates `repos.json` (read by `registry.rs`) and tgrep-gpui's settings. |
| `src/ui/` | The GPUI views: `windows.rs` opens, restores and remembers windows; `hub.rs` holds the repositories every window shares (their indexes, file watchers and the build queue); `app.rs` a window's searching, `tabs.rs` its search tabs, `workspace.rs` its workspace, `repos.rs` its repositories and scope; `render.rs` the search page, `repos_page.rs` the repositories page and `manager.rs` its state; `tasks.rs` runs clones and pulls and pulls on schedule; `preview.rs` the preview pane; `table.rs` the table view and exports; `palette.rs` the command palette; `highlight.rs` colours code by language; `remote.rs` answers the command line; `devtools.rs` is the developer tools window. |
| `src/cli/` | The `dowse` subcommands: `args.rs` their options, `client.rs` reaching the running app (starting it in the background when need be), `output.rs` formatting its answers as text or JSON. |
| `src/ipc/` | Keeping to one running app, and how the command line talks to it: later launches and subcommands send a JSON line over a local socket and read JSON lines back (`protocol.rs`). |
| `src/diagnostics/` | The log (a ring buffer and a rotating file) and the metrics, with no UI dependency. |
| `src/launch.rs` | Launching the desktop app from the command line. |
| `src/bin/dowse-cli.rs` | The console program installed as `dowse.com` on Windows. |
| `docs/agent-guide.md` | The guide `dowse guide` prints for coding agents. |
| `src/shell.rs` | Explorer integration on Windows. |
| `src/editor.rs` | Launching an editor at a line. |
| `src/fuzzy.rs` | Fuzzy matching for the command palette. |
| `.github/workflows/release.yml` | Builds the release archives for Windows, Linux and macOS when a `v<version>` tag is pushed, and publishes them. |
| `packaging/macos/Info.plist` | The `Info.plist` of the macOS `dowse.app`. |
| `examples/bench.rs` | Times indexing and a few searches: `cargo run --release --example bench -- <folder> [pattern...]`. |

## How a search runs

1. The query is parsed, and each term becomes a regex (literal text is escaped; whole
   word adds `\b`) and a tgrep trigram plan. The terms outside any `NOT` also make up
   one combined regex, which finds the lines to show.
2. Repository qualifiers rule out whole repositories. For every other repository in
   scope, the plans select candidate files from its index: terms that must all match
   intersect their candidates, alternatives unite them, and a negated term narrows
   nothing. Files the watcher saw change are added, then the path filter and the
   path and language qualifiers drop files before they are read.
3. Candidates are read and matched in ordered parallel chunks, repository by
   repository; each term is checked only when the answer still depends on it. Once
   about 20,000 matching lines (or 10,000 files) are found, the rest are skipped and
   the summary says so.

On a 42,000-file tree (1.3 GB of crate sources), the index builds in about 6 s and typical
queries finish in 20–60 ms.

## Testing

```bash
cargo test
```

The engine tests build real indexes in temporary directories.

## Releasing

Set the version in `Cargo.toml`, commit, and push a tag of it:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

The release workflow builds every platform's archive and publishes them as the
tag's release. Running the workflow by hand from the Actions tab builds the same
archives as workflow artifacts, without releasing, to try them first.

## License

[MIT](LICENSE)
