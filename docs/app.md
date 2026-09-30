# The desktop app

- [Workspaces and windows](#workspaces-and-windows)
- [Adding repositories](#adding-repositories)
- [The repositories page](#the-repositories-page)
- [Clone from GitHub](#clone-from-github)
- [Background tasks and scheduled pulls](#background-tasks-and-scheduled-pulls)
- [Tags choose what to search](#tags-choose-what-to-search)
- [Results](#results)
- [Tabs, back and forward](#tabs-back-and-forward)
- [Preview and editor](#preview-and-editor)
- [Menu, palette and shortcuts](#menu-palette-and-shortcuts)
- [Developer tools](#developer-tools)

What a query can say is in [Searching](search.md); how the indexes behind it are
kept is in [Indexes](indexes.md).

## Workspaces and windows

Each window shows a workspace: the repositories it searches, like VS Code's. A new
one is untitled; save it as a `.dowse-workspace` file to reopen or share it, and
later changes are written back to the file. Open as many windows as you like;
windows sharing a repository share its index and file watcher. The open windows,
untitled workspaces included, come back on the next start. Closing a window with
an unsaved workspace, while others stay open, asks whether to save it.

The menu's Workspace section opens, saves and switches workspaces, and lists the
recent ones; the `×` beside one takes it off the list (the file stays). The
window's workspace shows at the left of the status bar.

Launching the app follows VS Code's `code`:

| Command | What it does |
| --- | --- |
| `dowse` | Restores the last session's windows. |
| `dowse <folder>...` | Opens the folders as a new untitled workspace. A folder holding git repositories stands for each of them. |
| `dowse <file>.dowse-workspace` | Opens that workspace, or focuses the window already showing it. |
| `dowse --add <folder>...` | Adds the folders to the last focused window's workspace. |
| `dowse --remove <folder>...` | Takes them out again. |

As with `code`, only one dowse runs at a time: a launch while it is running hands
its command line to the running app and exits at once, and a launch without
arguments opens a new window there. Each [settings directory](install.md#where-settings-live)
gets its own app.

## Adding repositories

Add repositories one by one, or pick a folder that holds several git repositories
to add them all. Results show which repository and branch each file comes from.

Drop folders on a window to add them, or a workspace file to open it. From a
terminal, `dowse --add <folder>` adds to the running app's last focused window. On
Windows, the repositories page's Settings section can add "Add to dowse" to
Explorer's folder menu, which does the same, and open workspace files with a double
click; this writes to the current user's registry only when you ask.

## The repositories page

`Ctrl+,`, or Repositories in the menu, manages a workspace's repositories. Filter
them by name, path, branch or tag, select several (the box in the table's header
takes every one shown, and the header then holds what to do) and pull them, update
or rebuild their indexes, add or remove tags (`-tag` removes one), set how often
they are pulled, choose where their indexes are kept, or take them out of the
workspace. A row opens to its tags and pull settings, and shows its index and how
its last pull went. The page's Settings section holds what applies to every
repository: where indexes are kept unless a repository says otherwise, and on
Windows, Explorer's menu. Back (`Alt+Left`) returns to the search, and Forward
comes back to the page.

![The repositories page: cloned repositories with their branch, owner and mirror tags, and how many files each index holds](images/repositories-page.webp)

## Clone from GitHub

The page's GitHub section lists an owner's repositories (your own unless you name a
user or organization) through the [GitHub CLI](https://cli.github.com), so dowse
never handles your sign-in: run `gh auth login` once. Pages of the list are fetched
several at once and shown as they arrive (an organization of 2,000 repositories
takes seconds), and Stop keeps what has arrived.

Filter the list, show or hide forks and archived repositories, pick some (or all
shown) and clone them into `<folder>/<owner>/<name>`, tagged `owner:<owner>`.
Clones are blobless by default: every commit, but only the file contents checked
out, the rest fetched when needed, which makes large repositories quick to clone
while history still works. Shallow (the latest commit only) and full clones are
there too. Finished clones join the workspace and get indexed. Repositories already
cloned are marked, and can be added with a click.

## Background tasks and scheduled pulls

Clones and pulls run a few at a time (4 of each unless you change it in the Tasks
section or with `dowse settings set tasks.clones 8`; the app remembers), each on a
thread of its own, so the app stays usable while a large repository clones. The
page's Tasks section shows their progress and lets you cancel, retry or clear them;
the status bar shows what is running. Closing the last window leaves the app
running in the background until they finish.

Give a repository a pull interval (`15m`, `1h`, `3d` and so on) and dowse pulls it
whenever the interval has passed while it runs, including right after starting
when it is overdue. A pull fetches `origin`, then fast-forwards only when origin's
default branch is checked out, tracked files are unchanged and there are no local
commits; otherwise it only fetches, and says why (`on feature/x, not main`,
`uncommitted changes`). It never merges, rebases or stashes. A pull that brings
commits brings the index up to date. The interval shows as the tag
`sync:<interval>`, so the scope bar can pick the synced repositories.

## Tags choose what to search

Tag repositories freely: plain tags such as `mirror` or `dev`, or `key:value` tags
such as `owner:alice` or `project:billing`. Every repository is also tagged
`branch:<name>` with the branch it has checked out, which updates when you switch.
The scope bar above the results picks the tags to search: tags in one group are
alternatives (`owner:alice` or `owner:bob`), and groups narrow each other (`dev`
and `owner:alice`). Tags belong to the repository, so every workspace sees them;
the scope is remembered per workspace.

Facets narrow the results without changing the scope: repository, branch, each tag
group, language and top-level directory, each counted under the others' filters,
as on grep.app. The sidebar button at the left of the title bar (`Ctrl+B`) hides
them for more room.

![Results for trigram index -path:test across tgrep, Zoekt, Google codesearch, Hound, Django and Go, with repository, branch, owner and language facets](images/search-across-repositories.webp)

## Results

**As snippets**, coloured by language with tree-sitter grammars: every match is
highlighted, with one line of context. Long files collapse to their first matches
("Show N more matches").

**As a table**, for analysis: the Table switch above the results (`Alt+T`,
`Cmd+Alt+T` on macOS) lists one row per matching line, with its repository,
branch, path, line, column, language, the matched text and the line itself. When
the query's regex has capture groups, each gets a column, named after the group:
`/version = "(?<version>[^"]+)"/` tabulates versions. Click a header to sort
(numbers numerically), drag its edge to resize, click a row to preview it and
double-click to open it.

Export writes the rows, in the table's order and under the facet filters, as CSV
(with a byte order mark, for Excel), TSV, Markdown or JSON (`Ctrl+Shift+E` for
CSV), or copies them as TSV to paste into a spreadsheet or as Markdown. Rows are
the kept lines: up to 200 per file and about 20,000 in all. When every row comes
from one repository, the table hides its name and branch; exports always include
them.

![The table view: one row per Cargo.toml version line, with a version column from the named capture group](images/table-view-capture-groups.webp)

## Tabs, back and forward

Keep several searches open and switch between them without running them again:
`Ctrl+T` opens a tab, `Ctrl+W` closes it, `Ctrl+Tab` and `Ctrl+Shift+Tab` (or
`Ctrl+PageDown/PageUp`) step through them, and a middle click closes one. Each tab
has its own query, options, path filter and facet filters; the scope is the
window's. Tabs come back with their window on the next start.

Go back and forward through a tab's searches and visits to the repositories page,
as in a browser: the arrows at the left of the title bar, `Alt+Left` and
`Alt+Right` (`Ctrl+-` and `Ctrl+Shift+-` on macOS), or a mouse's back and forward
buttons. Going back brings the search back with its filters, the file it was
previewing and its results scrolled where you left them. A search counts as a step
once you pause, press `Enter`, change an option or filter, or preview a result, so
typing a query is one step, not one per letter. After that, changing its filters or
clicking a result in another file is a step of its own; moving between matches
with `F4` is not.

## Preview and editor

Click a line to see the whole file beside the results, coloured by language, with
the query's matches marked and the line in view; no waiting for an editor to start.
Drag the divider to resize it and `Esc` closes it. The pane button at the right of
the title bar (`Ctrl+Alt+B`) hides the pane and brings it back on the same file.

The up/down buttons, `F4` and `Shift+F4` step through every matching occurrence,
including multiple matches on one line, then into the next or previous file. The
cursor lands at the matching word rather than the line's start, and the preview's
counter counts occurrences. Clicking a result reveals the first match on that line.
Each tab keeps its own preview.

The preview is for reading; nothing is edited. Its read-only editor provides text
selection and copying: drag across lines, double-click a word or triple-click a
line, then press `Ctrl+C` (`Cmd+C` on macOS). Tabs, trailing whitespace and long
lines stay in the source text; long lines wrap to the pane's width without changing
the source. Syntax highlighting covers the languages the editor supports.

![The preview pane showing a whole file beside the results, with the query's matches marked](images/file-preview.webp)

Open a file in your editor from the preview's "Open in Editor" button (at the
active cursor's line, including after a text selection), or straight from the
results with `Ctrl+Click`. VS Code, Cursor, Zed or Sublime Text is used when found
on `PATH`, otherwise the system default application. Set `DOWSE_EDITOR` to choose,
for example `code -g {file}:{line}` or `nvim-qt +{line} {file}`.

## Menu, palette and shortcuts

The menu at the left of the title bar holds every command by kind: File (windows
and tabs), Workspace (new, open, save, recent), Repositories (the repositories
page, adding and cloning, pulling and indexing, background tasks, settings), View (back and
forward, the sidebar and preview pane, the table, the theme, the command palette)
and Help (the project on GitHub, release notes, reporting an issue,
[updates](install.md#updates), About).

The command palette, `Ctrl+K` (`Cmd+K` on macOS; `Ctrl+Shift+P` also works) or the
`⋮` menu at the right of the title bar, lists every command with its shortcut,
plus the open tabs, the scope's tags, recent workspaces and the query qualifiers,
which it adds to the query. Type a few letters of the name in order, `nt` for New
Tab, and press `Enter`; `Esc` clears the filter, then closes.

| Shortcut | Does |
| --- | --- |
| `Ctrl+O` | Add repositories |
| `Ctrl+,` | The repositories page |
| `Ctrl+F` | Focus the search box |
| `Ctrl+P` | The path filter |
| `Alt+C`, `Alt+W`, `Alt+R` | Match case, whole word, regular expression |
| `Alt+T` | Results as a table |
| `Ctrl+Shift+R` | Update the indexes in scope (the palette also rebuilds them) |
| `Ctrl+T`, `Ctrl+W` | Open, close a tab |
| `Ctrl+B`, `Ctrl+Alt+B` | Hide the facets, the preview |
| `Ctrl+Shift+N` | New window |
| `Ctrl+Shift+O`, `Ctrl+Shift+S` | Open, save a workspace |
| `Ctrl+Shift+E` | Export the table as CSV |
| `Ctrl+Shift+I`, `F12` | Developer tools |

On macOS, `Cmd` stands for `Ctrl`, and the `Alt` toggles are `Cmd+Alt`. Light and
dark themes switch from the `⋮` menu.

## Developer tools

`Ctrl+Shift+I` or `F12` opens a window with the app's log as it is written
(filtered by level and text), its key metrics (memory, search timings from windows
and the command line, how many file reads the indexes spared, index load, build and
update timings, log problems) and the latest searches with what each read and
found. Copy Diagnostics puts it all on the clipboard. The command line reads the
same with `dowse dev logs` and `dowse dev metrics`.
