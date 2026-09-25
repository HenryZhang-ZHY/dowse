# tgrep-gpui

A cross-platform desktop code search tool: [tgrep](https://github.com/microsoft/tgrep)'s
trigram index as the engine, [GPUI](https://github.com/zed-industries/zed) (through
[GPUI Kit](https://gpui-kit.com)) as the UI. The scope of the first version is
[grep.app](https://grep.app): one search box, a few toggles, facet filters, and
results as code snippets.

## Features

- **Search as you type** with toggles for match case (`Alt+C`), whole word (`Alt+W`)
  and regular expression (`Alt+R`). On macOS the shortcuts are `Cmd+Alt+C/W/R`.
- **Path filter**: space-separated terms. `src` keeps paths containing `src`, `*.rs`
  keeps matching globs, and `!tests` or `-*.md` drops paths.
- **Facets** for language and top-level directory, each counted under the other
  facet's filter, as on grep.app.
- **Results as snippets**: every match is highlighted, with one line of context. Long
  files collapse to their first matches ("Show N more matches").
- **Open in your editor**: click a line. VS Code, Cursor, Zed or Sublime Text is used
  when found on `PATH`, otherwise the system default application. Set
  `TGREP_GPUI_EDITOR` to choose, for example `code -g {file}:{line}` or
  `nvim-qt +{line} {file}`.
- **Shares the index with the tgrep CLI**. The index lives in `<folder>/.tgrep`, the
  same place `tgrep index` and `tgrep serve` use. Opening a folder without an index
  builds one in the background; searches scan the folder until it is ready.
- **Stays fresh between builds**. A file watcher tracks files changed since the last
  build, and searches read them directly. After 2,000 changes the index is rebuilt
  automatically; `Ctrl+Shift+R` rebuilds on demand. Rebuilds happen in a staging
  directory, so searching keeps working while one runs.
- Light and dark themes, recent folders, `Ctrl+O` to open a folder, `Ctrl+F`/`Ctrl+K`
  to focus the search box, `Ctrl+P` for the path filter.

## Build and run

Requires a recent stable Rust (the repository's `mise.toml` pins `latest`).

```bash
cargo run --release -- path/to/repo
```

Without an argument, the app opens on a welcome screen with recent folders.

On Linux, GPUI needs the usual X11/Wayland and Vulkan development packages; see the
[Zed Linux build notes](https://github.com/zed-industries/zed/blob/main/docs/src/development/linux.md).

## Layout

| Path | What it holds |
| --- | --- |
| `src/engine/` | The search engine, a library with no UI dependency. `workspace.rs` opens, builds and publishes the tgrep index; `query.rs` compiles the query and path filter; `search.rs` narrows candidates through the index and matches lines in parallel; `watch.rs` tracks changed files. |
| `src/ui/` | The GPUI view: `app.rs` holds state and behaviour, `render.rs` the layout. |
| `src/editor.rs` | Launching an editor at a line. |
| `examples/bench.rs` | Times indexing and a few searches: `cargo run --release --example bench -- <folder> [pattern...]`. |

## How a search runs

1. The query becomes a regex (literal text is escaped; whole word adds `\b`) and a
   tgrep trigram plan.
2. The plan selects candidate files from the index. Files the watcher saw change are
   added, and the path filter is applied.
3. Candidates are read and matched in path-ordered parallel chunks. Once about 20,000
   matching lines are found, the rest are skipped and the summary says so.

On a 42,000-file tree (1.3 GB of crate sources), the index builds in about 6 s and typical
queries finish in 20–60 ms.

## Testing

```bash
cargo test
```

The engine tests build real indexes in temporary directories.

## License

[MIT](LICENSE)
