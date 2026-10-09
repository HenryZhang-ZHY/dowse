<p align="right">
  <strong>English</strong> | <a href="README.zh-CN.md">简体中文</a>
</p>

<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/brand/dowse-wordmark-dark.svg">
    <img alt="dowse" src="docs/brand/dowse-wordmark-light.svg" height="48">
  </picture>
</h1>

Search code across many repositories at once, from a desktop app or the command
line, in the spirit of [grep.app](https://grep.app) and GitHub code search, but over
the clones on your own disk: the mirrors you keep on `main` and the working copies
you develop in. [tgrep](https://github.com/microsoft/tgrep)'s trigram index is the
engine and [GPUI](https://github.com/zed-industries/zed) (through
[GPUI Kit](https://gpui-kit.com)) the UI.

![dowse searching 12 public repositories as the query is typed, narrowing to Rust with a facet, then showing the results as a table](docs/images/dowse-search.gif)

Twelve public repositories, 45,629 files: results arrive as the query is typed, a
facet narrows them to Rust, and the table lays them out one row per line.

## Highlights

- **Many repositories, one search box.** Add individual repositories or folders of
  them. Workspaces group them per window (like in VS Code), and tags such as
  `mirror`, `owner:alice`, or `branch:main` choose exactly what a search covers.
  [More in Desktop App](docs/app.md)
- **GitHub code search syntax, as you type.** `parse config lang:rust -path:tests`,
  exact phrases, regular expressions, boolean operators (`AND`, `OR`, `NOT`), and
  faceted narrowing by repository, branch, tag, language, and directory.
  [More in Searching](docs/search.md)
- **Results as snippets or structured tables.** Regex capture groups automatically
  become table columns, exportable to CSV, TSV, Markdown, or JSON. An integrated
  preview pane beside the results renders the entire file instantly.
  [More in Results](docs/app.md#results)
- **Mirrors that keep themselves current.** Batch-clone an owner's GitHub
  repositories via `gh`, schedule background pulls, and fast-forward cleanly only
  when it is safe without touching working changes.
  [More in Cloning & Syncing](docs/app.md#clone-from-github)
- **Indexes that stay fresh.** Trigram indexes are shared with the `tgrep` CLI,
  updated incrementally on file change, and kept up to the second with a background
  file watcher. [More in Indexes](docs/indexes.md)
- **A command line for people and coding agents.** `dowse search` drives the running
  app and its warm in-memory indexes. Its output is budgeted and formatted for AI
  coding agents (`claude`, `copilot`, `cursor`, etc.).
  [More in CLI](docs/cli.md) & [Agent Guide](docs/agent-guide.md)
- **Self-updating.** Checks and installs new GitHub releases automatically, verified
  against published SHA-256 checksums. [More in Install](docs/install.md#updates)

## Quick start

### Desktop App

1. **Launch & Add Repositories**: Run `dowse`, press `Ctrl+O` (`Cmd+O` on macOS),
   or drag folders directly onto the window.
2. **Search in Real Time**: Type queries such as `parse config lang:rust -path:tests`.
   Click any match to preview its file; press `Alt+T` (`Cmd+Alt+T`) to switch to
   table view.
3. **Organize & Scope**: Open the Repositories page (`Ctrl+,`), tag repositories,
   and use the scope bar to focus your search on active projects or mirrors.

### Command Line

```bash
# Search across all configured repositories
dowse search 'parse_config lang:rust'

# Search only the repository in the current directory
dowse search --here 'parse_config'

# List repositories and their index status
dowse repos

# View the agent reference guide
dowse guide
```

Press `Ctrl+K` (`Cmd+K` on macOS) anywhere in the app to open the command palette,
which lists every action alongside its shortcut.

## Downloads

Download the pre-built archive for your platform from the
[latest release](https://github.com/HenryZhang-ZHY/dowse/releases/latest):

| Platform                              | Archive                               | Details                                                                                                                 |
| ------------------------------------- | ------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| **Windows (x64)**                     | `dowse-<version>-windows-x86_64.zip`  | Contains `dowse.exe` (GUI app) and `dowse.com` (CLI client). Unzip to a directory on your `PATH`.                       |
| **macOS 11+** (Apple Silicon / Intel) | `dowse-<version>-macos-universal.zip` | Contains universal `dowse.app`. Move to `/Applications`. Link `/Applications/dowse.app/Contents/MacOS/dowse` to `PATH`. |
| **Linux** (x64 / arm64)               | `dowse-<version>-linux-<arch>.tar.gz` | Single binary for both GUI and CLI. Requires glibc 2.35+ (Ubuntu 22.04+, Debian 12+, Fedora 36+) and Vulkan drivers.    |

The builds are not signed with a commercial certificate, so operating systems show
a security prompt on first launch. See [Install and update](docs/install.md) for
first-start instructions, environment configuration, and building from source.

## Documentation

Browse the complete documentation library in the [Documentation Hub](docs/README.md):

| Guide                                          | Description                                                           |
| ---------------------------------------------- | --------------------------------------------------------------------- |
| [Documentation Hub](docs/README.md)            | Central table of contents, topic roadmap, and keyboard shortcuts      |
| [Install & Update](docs/install.md)            | System requirements, installation, updates, and building from source  |
| [Desktop App](docs/app.md)                     | Workspaces, repository management, tabs, table exports, and shortcuts |
| [Searching](docs/search.md)                    | Query syntax, boolean operators, path filters, and matching pipeline  |
| [Indexes](docs/indexes.md)                     | Trigram index internals, storage locations, and incremental updates   |
| [Command Line](docs/cli.md)                    | Subcommand reference, JSON/table outputs, and daemon architecture     |
| [Guide for Coding Agents](docs/agent-guide.md) | Best practices and mental model for AI coding agents (`dowse guide`)  |
| [Development](docs/development.md)             | Codebase architecture, module layout, testing, and release workflow   |

## License

[MIT](LICENSE)
