<p align="right">
  <strong>English</strong> | <a href="zh-CN/README.md">简体中文</a>
</p>

# dowse Documentation Hub

Welcome to the **dowse** documentation. dowse is a high-performance multi-repository
code search engine combining GitHub code search syntax with instant local Trigram
indexes and a GPU-accelerated desktop application.

Whether you are navigating millions of lines of code as an engineer, automating
workspace indexing for a team, or integrating search into an AI coding agent,
these guides will help you get the most out of dowse.

---

## Documentation Map

### Getting Started

- **[Install and Update](install.md)**  
  Download pre-built packages for Windows, macOS, and Linux. Covers first-start
  permissions, environment paths (`PATH`), automatic daily updates, configuration
  directory layouts, and building from source with Cargo.

- **[The Desktop App](app.md)**  
  The complete guide to the GPUI interface. Learn how workspaces organize windows,
  how to manage repositories, clone from GitHub, schedule background pulls, inspect
  results in snippet or table view, use the live file preview, and configure external
  editors.

---

### Core Search Engine

- **[Searching](search.md)**  
  Master the GitHub-compatible query language: literal terms, quoted phrases,
  regex patterns, boolean combinations (`AND`, `OR`, `NOT`), language qualifiers,
  and repository scopes. Explains the path filter funnel and how the parallel search
  pipeline operates.

- **[Indexes](indexes.md)**  
  Deep dive into the [tgrep](https://github.com/microsoft/tgrep) trigram index
  architecture. Learn how candidate pruning eliminates 99%+ of disk I/O, where indexes
  are stored (`.tgrep` vs centralized external directory), how incremental updates
  apply in seconds, and how the background file watcher keeps indexes fresh on save.

---

### CLI and Coding Agents

- **[The Command Line](cli.md)**  
  Full reference for every `dowse` subcommand (`search`, `repos`, `tasks`, `index`,
  `status`, `dev`, `settings`). Details the single-instance IPC architecture, stdout/stderr
  separation, JSON streaming, table formats, and daemon lifetimes.

- **[Guide for Coding Agents](agent-guide.md)**  
  The exact guide printed by `dowse guide`. Crafted specifically for AI coding agents
  (such as Claude Code, GitHub Copilot, and Cursor) to provide maximum signal with
  strict token budgets (`-n 100`, `-m 20`), narrowing recommendations, and structured
  data extraction.

---

### Architecture and Contributing

- **[Development](development.md)**  
  Overview of the Rust codebase layout (`src/engine`, `src/ui`, `src/cli`, `src/ipc`,
  `src/diagnostics`). Details unit and integration test strategies, benchmarking with
  `examples/bench.rs`, and the GitHub Actions release workflow.

---

## Frequent Workflows

| What you want to do                                | Where to look                                    | Quick solution                                                       |
| -------------------------------------------------- | ------------------------------------------------ | -------------------------------------------------------------------- |
| Search code across 50 team repositories            | [The Desktop App](app.md#workspaces-and-windows) | Drop the parent folder on the window or run `dowse ~/src/team/*`     |
| Extract structured data (e.g. versions, endpoints) | [The Desktop App](app.md#results)                | Use regex with capture groups, press `Alt+T`, and export as CSV/JSON |
| Keep local GitHub mirrors synced on schedule       | [The Desktop App](app.md#clone-from-github)      | Set a pull interval (`1h`) on cloned repositories via GUI or CLI     |
| Keep git working copies clean of `.tgrep` folders  | [Indexes](indexes.md#or-kept-out-of-the-way)     | `dowse settings set index.location external`                         |
| Pipe search results into an AI agent or script     | [CLI](cli.md#output-for-people-and-agents)       | `dowse search --json 'query'` or `dowse search -l 'query'`           |
| Run an ad-hoc query against a specific repo        | [Searching](search.md)                           | `dowse search --here 'pattern'` or `repo:name pattern`               |

---

## Global Keyboard Shortcuts

| Shortcut (Win / Linux)       | Shortcut (macOS)           | Action                                        |
| ---------------------------- | -------------------------- | --------------------------------------------- |
| `Ctrl+O`                     | `Cmd+O`                    | Add repositories or folders                   |
| `Ctrl+,`                     | `Cmd+,`                    | Open Repositories management page             |
| `Ctrl+F`                     | `Cmd+F`                    | Focus search query box                        |
| `Ctrl+P`                     | `Cmd+P`                    | Toggle path filter funnel                     |
| `Alt+T`                      | `Cmd+Alt+T`                | Toggle results between snippet and table view |
| `Alt+C` / `Alt+W` / `Alt+R`  | `Cmd+Alt+C / W / R`        | Toggle Case, Whole Word, or Regex mode        |
| `Ctrl+B` / `Ctrl+Alt+B`      | `Cmd+B` / `Cmd+Alt+B`      | Toggle Facets sidebar / File preview pane     |
| `Ctrl+T` / `Ctrl+W`          | `Cmd+T` / `Cmd+W`          | Open new search tab / Close active tab        |
| `Ctrl+Shift+E`               | `Cmd+Shift+E`              | Export results table as CSV                   |
| `Ctrl+Shift+R`               | `Cmd+Shift+R`              | Update indexes for all repositories in scope  |
| `Ctrl+K` (or `Ctrl+Shift+P`) | `Cmd+K` (or `Cmd+Shift+P`) | Open Command Palette                          |
| `Ctrl+Shift+I` or `F12`      | `Cmd+Shift+I` or `F12`     | Open Developer Tools (logs, metrics)          |
