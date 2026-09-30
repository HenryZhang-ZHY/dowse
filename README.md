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

- **Many repositories, one search box.** Add repositories, or a folder of them.
  Workspaces group them per window, as in VS Code, and tags such as `mirror` or
  `owner:alice` choose which ones a search covers. [More](docs/app.md)
- **GitHub code search syntax, as you type**: `parse config lang:rust -path:tests`,
  exact text, regular expressions, and facets that narrow by repository, branch,
  tag, language and folder. [More](docs/search.md)
- **Results as snippets or a table.** Capture groups in a regex become columns, and
  the table exports to CSV, TSV, Markdown or JSON. A preview beside the results
  shows the whole file. [More](docs/app.md#results)
- **Mirrors that keep themselves current.** Clone an owner's GitHub repositories
  through `gh`, many at once, and pull them on a schedule, fast-forwarding only
  when it is safe. [More](docs/app.md#clone-from-github)
- **Indexes that stay fresh.** Indexes are shared with the tgrep command line and
  updated incrementally, and a file watcher makes an edit searchable right away.
  [More](docs/indexes.md)
- **A command line for people and agents.** `dowse search` drives the running app
  and its warm indexes, and its output is budgeted for coding agents.
  [More](docs/cli.md)
- **Updates itself** from GitHub releases, checked against their SHA-256.
  [More](docs/install.md#updates)

## Install

Download the archive for your platform from the
[latest release](https://github.com/HenryZhang-ZHY/dowse/releases/latest):

| Platform | Archive |
| --- | --- |
| Windows (x64) | `dowse-<version>-windows-x86_64.zip`: unzip `dowse.exe` and `dowse.com` to a folder you can write to |
| macOS 11 or later | `dowse-<version>-macos-universal.zip`: move `dowse.app` to Applications |
| Linux (x64, arm64) | `dowse-<version>-linux-<arch>.tar.gz`: needs glibc 2.35 or later and a Vulkan driver |

The builds are not signed with a paid certificate, so the system asks once before
the first start. [Install and update](docs/install.md) covers that, putting
`dowse` on `PATH`, updates, and building from source.

## Quick start

1. Start dowse and add repositories: Add Repositories… (`Ctrl+O`), drop folders on
   the window, or run `dowse ~/src/api ~/src/mirrors`.
2. Type a query, such as `parse config lang:rust -path:tests`. Click a line to
   preview its file; `Alt+T` shows the results as a table.
3. Tag repositories on the repositories page (`Ctrl+,`) and pick tags in the scope
   bar to choose what to search.
4. From a terminal or a coding agent: `dowse search 'parse_config lang:rust'`.

`Ctrl+K` opens the command palette, which lists every command with its shortcut.

## Documentation

| Page | What it covers |
| --- | --- |
| [Install and update](docs/install.md) | Downloads, the first start, updates, building from source, where settings live |
| [The desktop app](docs/app.md) | Workspaces, the repositories page, cloning and pulling, tags and facets, results, preview, shortcuts |
| [Searching](docs/search.md) | The query syntax, the path filter, how a search runs |
| [Indexes](docs/indexes.md) | Where indexes are kept and how they stay fresh |
| [The command line](docs/cli.md) | Every subcommand, and output for people and agents |
| [Guide for coding agents](docs/agent-guide.md) | What `dowse guide` prints: using dowse from an agent |
| [Development](docs/development.md) | The source layout, testing, releasing |

## License

[MIT](LICENSE)
