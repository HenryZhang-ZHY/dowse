# tgrep-gpui

A cross-platform desktop code search tool: [tgrep](https://github.com/microsoft/tgrep)'s
trigram index as the engine, [GPUI](https://github.com/zed-industries/zed) (through
[GPUI Kit](https://gpui-kit.com)) as the UI. It searches across many repositories at
once, in the spirit of [grep.app](https://grep.app) and GitHub code search, but over
the clones on your own disk: the mirrors you keep on `main` and the working copies
you develop in.

## Features

- **Many repositories, one search box.** Add repositories one by one, or pick a folder
  that holds several git repositories to add them all. Results show which repository
  and branch each file comes from.
- **Workspaces, like VS Code's.** Each window shows a workspace: the repositories it
  searches. A new one is untitled; save it as a `.tgrep-workspace` file to reopen or
  share it, and later changes are written back to the file. Open as many windows as
  you like; windows sharing a repository share its index and file watcher. The open
  windows, untitled workspaces included, come back on the next start. Closing a
  window with an unsaved workspace, while others stay open, asks whether to save it.
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
- **Search as you type** with toggles for match case (`Alt+C`), whole word (`Alt+W`)
  and regular expression (`Alt+R`). On macOS the shortcuts are `Cmd+Alt+C/W/R`.
- **Path filter**: space-separated terms. `src` keeps paths containing `src`, `*.rs`
  keeps matching globs, and `!tests` or `-*.md` drops paths.
- **Results as snippets**: every match is highlighted, with one line of context. Long
  files collapse to their first matches ("Show N more matches").
- **Open in your editor**: click a line. VS Code, Cursor, Zed or Sublime Text is used
  when found on `PATH`, otherwise the system default application. Set
  `TGREP_GPUI_EDITOR` to choose, for example `code -g {file}:{line}` or
  `nvim-qt +{line} {file}`.
- **Shares indexes with the tgrep CLI**. Each repository's index lives in its
  `.tgrep` directory, the same place `tgrep index` and `tgrep serve` use. A repository
  without an index gets one in the background, one build at a time; until then its
  files are scanned.
- **Stays fresh between builds**. A file watcher per repository tracks files changed
  since its last build, and searches read them directly, so a `git pull` in a mirror
  or an edit in a working copy shows up right away. After 2,000 changes a repository is
  re-indexed automatically. Rebuilds happen in a staging directory, so searching keeps
  working while one runs.
- Light and dark themes. `Ctrl+O` adds repositories, `Ctrl+,` opens the repositories
  page, `Ctrl+F`/`Ctrl+K` focus the search box, `Ctrl+P` the path filter, and
  `Ctrl+Shift+R` rebuilds the indexes in scope. `Ctrl+Shift+N` opens a new window,
  `Ctrl+Shift+O` opens a workspace and `Ctrl+Shift+S` saves one.

## Build and run

Requires a recent stable Rust (the repository's `mise.toml` pins `latest`).

```bash
cargo run --release -- path/to/repo path/to/folder-of-repos
```

The command line follows VS Code's `code`:

| Command | What it does |
| --- | --- |
| `tgrep-gpui` | Restores the last session's windows. |
| `tgrep-gpui <folder>...` | Opens the folders as a new untitled workspace. A folder holding git repositories stands for each of them. |
| `tgrep-gpui <file>.tgrep-workspace` | Opens that workspace, or focuses the window already showing it. |
| `tgrep-gpui --add <folder>...` | Adds the folders to the last focused window's workspace. |
| `tgrep-gpui --remove <folder>...` | Takes them out again. |

As with `code`, only one tgrep-gpui runs at a time: a launch while it is running hands
its command line to the running app and exits at once, and a launch without arguments
opens a new window there. Each settings directory (see below) gets its own app.

Settings live under the user configuration directory (`%APPDATA%\tgrep-gpui` on
Windows, `~/.config/tgrep-gpui` on Linux, `~/Library/Application Support/tgrep-gpui`
on macOS): `library.json` holds every repository's name and tags, `session.json` the
windows to restore and recent workspaces, and `workspaces/` is where workspaces are
saved unless you pick elsewhere. The first start after upgrading from a version with
a single repository list (`repos.json`) turns that list into a saved "Default"
workspace. Set `TGREP_GPUI_CONFIG_DIR` to keep settings elsewhere.

On Linux, GPUI needs the usual X11/Wayland and Vulkan development packages; see the
[Zed Linux build notes](https://github.com/zed-industries/zed/blob/main/docs/src/development/linux.md).

## Layout

| Path | What it holds |
| --- | --- |
| `src/engine/` | The search engine and saved settings, a library with no UI dependency. `index.rs` opens, builds and publishes one repository's tgrep index; `repo.rs` holds repository metadata, tags, the scope and branch detection; `query.rs` compiles the query and path filter; `search.rs` narrows candidates through each index and matches lines in parallel; `facets.rs` counts and filters results; `watch.rs` tracks changed files. `library.rs` keeps repository names and tags, `workspace.rs` reads and writes workspace files, `session.rs` the windows to restore, and `config.rs` locates the settings and migrates `repos.json` (read by `registry.rs`). |
| `src/ui/` | The GPUI views: `windows.rs` opens, restores and remembers windows; `hub.rs` holds the repositories every window shares (their indexes, file watchers and the build queue); `app.rs` a window's search state, `workspace.rs` its workspace, `repos.rs` its repositories and scope; `render.rs` the search page and `repos_page.rs` the repositories page. |
| `src/cli.rs` | The command line. |
| `src/instance.rs` | Keeping to one running app: later launches forward their command line over a local socket. |
| `src/editor.rs` | Launching an editor at a line. |
| `examples/bench.rs` | Times indexing and a few searches: `cargo run --release --example bench -- <folder> [pattern...]`. |

## How a search runs

1. The query becomes a regex (literal text is escaped; whole word adds `\b`) and a
   tgrep trigram plan.
2. For every repository in scope, the plan selects candidate files from its index.
   Files the watcher saw change are added, and the path filter is applied.
3. Candidates are read and matched in ordered parallel chunks, repository by
   repository. Once about 20,000 matching lines are found, the rest are skipped and
   the summary says so.

On a 42,000-file tree (1.3 GB of crate sources), the index builds in about 6 s and typical
queries finish in 20–60 ms.

## Testing

```bash
cargo test
```

The engine tests build real indexes in temporary directories.

## License

[MIT](LICENSE)
