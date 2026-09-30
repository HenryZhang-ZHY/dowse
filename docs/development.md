# Development

Build and run from source as in [Install](install.md#build-from-source):

```bash
cargo run --release -- path/to/repo path/to/folder-of-repos
```

## Layout

| Path | What it holds |
| --- | --- |
| `src/engine/` | The search engine and saved settings, a library with no UI dependency. `github.rs` lists and clones GitHub repositories through `gh`; `sync.rs` pulls a repository when it is safe to and parses pull intervals; `tasks.rs` queues background tasks; `process.rs` runs git and gh. `index.rs` opens, builds, updates and publishes one repository's tgrep index; `repo.rs` holds repository metadata, tags, the scope and branch detection; `syntax.rs` parses the query language; `query.rs` compiles it and the path filter; `search.rs` narrows candidates through each index and matches lines in parallel; `facets.rs` counts and filters results; `table.rs` lays results out as rows, sorts and exports them; `preview.rs` prepares a whole file for the preview; `watch.rs` tracks changed files. `library.rs` keeps repository names, tags and settings, `settings.rs` the app's settings, such as where indexes are kept, `workspace.rs` reads and writes workspace files, `session.rs` the windows to restore, and `config.rs` locates the settings. `update/` keeps dowse up to date: `release.rs` reads GitHub's releases, `client.rs` asks for the latest, `state.rs` remembers the answer and the version skipped in `update.json`, and `install.rs` downloads, checks and installs a release over the running one. |
| `src/ui/` | The GPUI views: `windows.rs` opens, restores and remembers windows; `hub.rs` holds the repositories every window shares (their indexes, file watchers and the build queue); `app.rs` a window's searching, `tabs.rs` its search tabs, `history.rs` their back and forward, `main_menu.rs` the title bar's menu, `workspace.rs` its workspace, `repos.rs` its repositories and scope; `render.rs` the search page, `repos_page.rs` the repositories page and `manager.rs` its state; `tasks.rs` runs clones and pulls and pulls on schedule; `preview.rs` the preview pane; `table.rs` the table view and exports; `palette.rs` the command palette; `highlight.rs` colours code by language; `remote.rs` answers the command line; `updates.rs` looks for, offers and installs updates; `devtools.rs` is the developer tools window. |
| `src/cli/` | The `dowse` subcommands: `args.rs` their options, `client.rs` reaching the running app (starting it in the background when need be), `output.rs` formatting its answers as text or JSON. |
| `src/ipc/` | Keeping to one running app, and how the command line talks to it: later launches and subcommands send a JSON line over a local socket and read JSON lines back (`protocol.rs`). |
| `src/diagnostics/` | The log (a ring buffer and a rotating file) and the metrics, with no UI dependency. |
| `src/launch.rs` | Launching the desktop app from the command line, and the new version taking over after an update. |
| `src/bin/dowse-cli.rs` | The console program installed as `dowse.com` on Windows. |
| `src/shell.rs` | Explorer integration on Windows. |
| `src/editor.rs` | Launching an editor at a line. |
| `src/fuzzy.rs` | Fuzzy matching for the command palette. |
| `docs/` | This documentation; `agent-guide.md` is the guide `dowse guide` prints, and `images/` holds the screenshots and recording. |
| `.github/workflows/release.yml` | Builds the release archives for Windows, Linux and macOS when a `v<version>` tag is pushed, and publishes them. |
| `packaging/macos/Info.plist` | The `Info.plist` of the macOS `dowse.app`. |
| `examples/bench.rs` | Times indexing and a few searches: `cargo run --release --example bench -- <folder> [pattern...]`. |

## Testing

```bash
cargo test
```

The engine tests build real indexes in temporary directories. Two tests ask the
real GitHub, so they only run when asked: `cargo test -- --ignored` checks the
latest release and installs it into a temporary folder.

## Releasing

Set the version in `Cargo.toml`, commit, and push a tag of it:

```bash
git tag v1.0.0 && git push origin v1.0.0
```

The release workflow builds every platform's archive and publishes them as the
tag's release. Running the workflow by hand from the Actions tab builds the same
archives as workflow artifacts, without releasing, to try them first.

A published release is what running copies [update](install.md#updates) to, found
by its `v<version>` tag and its archive for each platform
(`dowse-<tag>-<platform>.<ext>`), so keep those names. The workflow sets
`DOWSE_RELEASE`, which lets its builds look for updates on their own.
