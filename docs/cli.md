# The command line

`dowse search`, `dowse repos` and the other subcommands drive the running app, as
Obsidian's command line does, so they share its warm indexes and file watchers;
when the app is not running, the first command starts it in the background,
without windows. `dowse <subcommand> --help` tells more about each, and
[the guide for coding agents](agent-guide.md) (`dowse guide`) shows how to use them
well. Launching the app itself with folders or `--add` is described in
[the desktop app](app.md#workspaces-and-windows).

| Command | What it does |
| --- | --- |
| `dowse search <query>` | Searches every repository dowse knows, in the [query syntax](search.md). `--here`, `-t <tag>` and `-W <workspace>` narrow the scope. |
| `dowse repos` | Lists the repositories with branch, index state, file count, when they were indexed and tags. |
| `dowse repos add <folder>... [-t <tag>]` | Adds repositories, or every repository in a folder, to the library. |
| `dowse repos tag <repo> <tag>... [-r <tag>]` | Adds tags to a repository, or removes them. |
| `dowse repos github [<owner>]` | Lists an owner's GitHub repositories (yours by default) through `gh`; `-q` prints `owner/name` only. |
| `dowse repos clone <owner/name>... [--into <folder>]` | Clones in the app's background into `<folder>/<owner>/<name>` (the last folder used when not given). `--from <owner>` clones an owner's repositories, `--mode shallow\|full` fetches less or more, `-t` tags, `--pull-every 1h` schedules pulls and `--wait` waits. |
| `dowse repos pull [<repo>...] [--wait]` | Pulls repositories, or those in scope (`--here`, `-t`, `-W`). |
| `dowse repos sync <repo>... --every <interval>` | Pulls them every `15m`, `1h`, `3d`...; `--off` stops. |
| `dowse repos index-location <repo>... [--repo\|--external\|--default]` | Shows where repositories keep their [indexes](indexes.md#or-kept-out-of-the-way), or keeps them in their `.tgrep`, outside them, or where `index.location` says, moving them. `-q` prints only the folders: `tgrep search foo --index-path "$(dowse repos index-location api -q)"`. |
| `dowse settings [set <key> <value>\|unset <key>]` | Shows the app's settings, or changes them: `index.location` (`repo` or `external`), `index.external-dir`, and `updates.check` (`true` or `false`, whether the app looks for a new release once a day). |
| `dowse tasks [--wait]` | Lists the background clones and pulls; `dowse tasks cancel <id>...` or `--all` cancels them. |
| `dowse index [--wait] [--full]` | Brings the indexes in scope up to date, reading only the files that changed; `--full` builds them again from every file. |
| `dowse status` | Shows the running app: windows, repositories, indexing, where its settings and log are. |
| `dowse dev logs [-f] [--level debug]` | Prints the app's latest log records, or follows them. |
| `dowse dev metrics` | Prints the app's key metrics and latest searches. |
| `dowse dev open` | Opens the developer tools window. |
| `dowse quit` | Quits the app. |
| `dowse guide` | Prints [the guide for coding agents](agent-guide.md). |

Every subcommand is a request to the running app over its single-instance socket;
the app answers with data and the command line formats it. With no window open,
an app the command line started quits after ten minutes without requests, and
repositories opened for requests stay open that long, so an agent's next query is
fast too.

## Output for people and agents

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
