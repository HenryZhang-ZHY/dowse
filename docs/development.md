<p align="right">
  <strong>English</strong> | <a href="zh-CN/development.md">简体中文</a>
</p>

# Architecture, Contributing, and Release Workflow

This document details the internal architecture, module layout, testing practices,
and release engineering workflows for the dowse codebase.

- [Getting Started](#getting-started)
- [Architecture Overview](#architecture-overview)
- [Codebase Layout](#codebase-layout)
  - [Search Engine (src/engine)](#search-engine-srcengine)
  - [Graphical Interface (src/ui)](#graphical-interface-srcui)
  - [Command-Line Interface (src/cli)](#command-line-interface-srccli)
  - [Inter-Process Communication (src/ipc)](#inter-process-communication-srcipc)
  - [Diagnostics and Metrics (src/diagnostics)](#diagnostics-and-metrics-srcdiagnostics)
  - [Platform Integration and Utilities](#platform-integration-and-utilities)
- [Testing Strategy](#testing-strategy)
- [Benchmarking](#benchmarking)
- [Release Workflow](#release-workflow)

---

## Getting Started

Building dowse requires a recent stable Rust toolchain (Rust 2024 edition support,
pinned to `latest` in `mise.toml`). See [Install and Update](install.md#building-from-source)
for platform-specific prerequisites.

```bash
# Build in release mode
cargo build --release

# Run locally against target repositories
cargo run --release -- path/to/repo path/to/folder-of-repos
```

---

## Architecture Overview

dowse is designed around a decoupled, headless core engine and a hardware-accelerated
presentation layer:

- **Zero UI Dependency in Engine**: `src/engine/` is completely decoupled from GPUI.
  It can be compiled, tested, and utilized as a standalone library.
- **Single-Instance IPC**: A single application process serves both the desktop GUI
  and subsequent terminal CLI commands over a high-throughput local socket.
- **Rayon Parallelism**: File candidate verification and line matching run across a
  Rayon work-stealing thread pool, ensuring maximum multi-core CPU saturation.
- **Hardware Acceleration**: The GUI runs on Zed's GPUI framework, rendering text
  and layout directly on the GPU with sub-millisecond frame times.

---

## Codebase Layout

### Search Engine (`src/engine/`)

The core search engine library manages querying, indexing, git synchronization, and persistence:

- **Syntax & Querying**:
  - `syntax.rs`: Parses the GitHub-compatible query string into an abstract expression tree.
  - `query.rs`: Compiles syntax trees into regular expressions, path filters, and tgrep query plans.
  - `search.rs`: Drives parallel candidate verification and line-matching across repositories via Rayon.
  - `facets.rs`: Calculates real-time facet distributions and counts under active query filters.
- **Index Management**:
  - `index.rs`: Opens, builds, incrementally updates, and atomic-swaps repository tgrep indexes.
  - `watch.rs`: Background filesystem watcher that tracks modified/dirty files in real time.
- **Repository & Workspace**:
  - `repo.rs`: Manages repository metadata, tags, checked-out branch detection, and query scoping.
  - `library.rs`: Persistent registry of all known repositories, assigned tags, and per-repo options.
  - `workspace.rs`: Reads and writes `.dowse-workspace` configuration files.
  - `session.rs`: Serializes open windows, tabs, and scroll offsets for session restoration.
  - `settings.rs`: Application configuration keys and values (`index.location`, `tasks.clones`, etc.).
  - `config.rs`: Resolves platform configuration directories (`DOWSE_CONFIG_DIR`).
- **Git & GitHub Automation**:
  - `github.rs`: Interacts with GitHub via `gh` CLI to list and clone repositories.
  - `sync.rs`: Implements scheduled automated pulls and safe fast-forward verification.
  - `tasks.rs`: Priority queue and worker pool for background clones and pulls.
  - `process.rs`: Process execution helpers for running git and gh commands.
- **Data Presentation**:
  - `table.rs`: Formats query results into structured rows, extracts regex capture groups, and exports CSV/TSV/JSON/Markdown.
  - `preview.rs`: Prepares full file contents and match offset ranges for live preview.
- **Self-Update Subsystem (`src/engine/update/`)**:
  - `release.rs`: Parses GitHub release manifests and assets.
  - `client.rs`: Queries the GitHub release API.
  - `state.rs`: Manages update notification state and version skip preferences (`update.json`).
  - `install.rs`: Downloads release archives, verifies SHA-256 checksums, and performs atomic binary swaps.

---

### Graphical Interface (`src/ui/`)

The GPUI desktop interface implemented using [GPUI Kit](https://gpui-kit.com):

- `windows.rs`: Manages application windows, positions, and multi-window restoration.
- `hub.rs`: Central coordinator shared across windows; holds repository handles, file watchers, and index queues.
- `app.rs`: Window-level search controller and keyboard dispatch.
- `tabs.rs`: Search tabs container; maintains independent query state per tab.
- `history.rs`: Browser-style navigation history (back/forward) with state snapshots.
- `render.rs`: Renders the primary search page, query input bar, and scope selectors.
- `table.rs`: Virtualized data table view, header sorting, column resizing, and export actions.
- `preview.rs`: Monaco-style read-only file preview pane with occurrence stepping (`F4`).
- `repos_page.rs` & `manager.rs`: Repository management dashboard and batch operation controllers.
- `tasks.rs`: Tasks monitoring pane displaying background clone/pull progress.
- `palette.rs`: Global command palette with fuzzy filtering.
- `highlight.rs`: Tree-sitter syntax highlighting pipeline.
- `main_menu.rs`: Native desktop menu bar integration.
- `updates.rs`: Update notification toasts and installation dialogs.
- `devtools.rs`: Developer tools window (live logs, metrics, query profiling).
- `remote.rs`: Bridge routing incoming CLI IPC requests to UI state when applicable.

---

### Command-Line Interface (`src/cli/`)

- `args.rs`: Clap command-line argument parsing and subcommand definitions.
- `client.rs`: Socket client that connects to the running dowse daemon or starts a headless background instance.
- `output.rs`: Terminal output formatting: text snippets, JSON lines, tables, and stderr summaries.
- `mod.rs`: CLI command execution dispatch and embedded `GUIDE` export (`dowse guide`).

---

### Inter-Process Communication (`src/ipc/`)

- `protocol.rs`: Strongly-typed JSON-RPC protocol frames sent between CLI clients and the main process over local domain sockets or named pipes.
- `mod.rs`: Socket server listener and client connector logic.

---

### Diagnostics and Metrics (`src/diagnostics/`)

- `log.rs`: High-performance in-memory ring buffer logger paired with a 5 MB rotating disk file.
- `metrics.rs`: Performance telemetry (search durations, candidate pruning counts, memory usage).

---

### Platform Integration and Utilities

- `src/launch.rs`: Manages binary execution handover and seamless restart after self-updates.
- `src/bin/dowse-cli.rs`: The Windows console stub compiled as `dowse.com`.
- `src/shell.rs`: Windows Explorer context menu registry integration.
- `src/editor.rs`: External editor detection and URI scheme execution (`code -g {file}:{line}`).
- `src/fuzzy.rs`: Fuzzy substring matcher powering the command palette.
- `packaging/`: Platform packaging metadata (`Info.plist` for macOS, icons, Windows resource scripts).

---

## Testing Strategy

Run the test suite with:

```bash
cargo test
```

- **In-Memory and Isolated Engine Tests**: Engine tests construct temporary repositories
  and real tgrep trigram indexes within isolated temporary directories (`tempfile`),
  verifying complete round-trip search behavior without side effects.
- **Integration Tests**: Tests in `src/ui/history/tests.rs` and `src/ui/preview/tests.rs`
  validate navigation and editor components using GPUI's test harness.
- **Ignored Live Network Tests**: Two network-dependent tests verify GitHub release
  fetching and package installation against the live GitHub API:
  ```bash
  cargo test -- --ignored
  ```

---

## Benchmarking

A standalone benchmarking binary measures index build speed and query throughput:

```bash
cargo run --release --example bench -- <path/to/repositories> [optional_query_patterns...]
```

This times full index generation, incremental index scans, and multiple search executions,
printing detailed candidate filtering rates and matching throughput.

---

## Release Workflow

dowse uses automated GitHub Actions workflows to compile, test, package, and publish
release archives for all target platforms:

1. Update the version number in `Cargo.toml`.
2. Commit the change and push a corresponding git tag:
   ```bash
   git tag v1.2.1
   git push origin v1.2.1
   ```
3. `.github/workflows/release.yml` triggers automatically:
   - Builds release binaries on Windows (`x86_64`), macOS (`universal` Apple Silicon + Intel),
     and Linux (`x86_64` and `aarch64`).
   - Pairs `dowse.exe` and `dowse.com` on Windows.
   - Ad-hoc codesigns `dowse.app` on macOS.
   - Generates release archives and a verified `SHA256SUMS` file.
   - Publishes a new GitHub Release under the pushed tag.
4. Active installations automatically detect and update to the new release during their daily checks.
