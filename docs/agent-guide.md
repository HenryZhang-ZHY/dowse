# dowse for coding agents

[English](agent-guide.md) | [简体中文](zh-CN/agent-guide.md)

dowse searches every repository its user has added (their own working
copies, mirrors of their team's repositories, the libraries they depend on)
in one query, from trigram indexes kept warm by the running app. Use it to
find how code outside the current repository defines, calls or configures
something, and inside it when `grep` would be slow.

## The mental model

```bash
dowse search 'parse_config lang:rust'   # search everything dowse knows
dowse search --here 'parse_config'      # only the repository you are in
dowse repos                             # what dowse knows, with tags
```

Every subcommand talks to the running dowse app. If it is not running, the
first command starts it in the background (no window) and says so on stderr;
that first query waits for indexes to load. The app watches files, so results
include edits made a moment ago, yours included.

Results go to stdout. Counts, timings, suggestions for narrowing and notes go
to stderr. Exit status: 0 found, 1 nothing found, 2 error.

## Queries

GitHub code search syntax. Terms combine per file, not per line.

| Query                                        | Finds files                                                   |
| -------------------------------------------- | ------------------------------------------------------------- |
| `parse config`                               | containing both; lines with either are shown                  |
| `parse OR config`                            | containing either                                             |
| `parse NOT test`                             | containing `parse` but not `test`                             |
| `"fn main()"`                                | containing the exact text (quote it for the shell too)        |
| `/fn \w+_test/`                              | with a line matching the regular expression                   |
| `path:src/*.rs`, `path:engine`               | path glob (anchored when it holds a `/`), or text in the path |
| `language:rust`, `lang:ts`                   | in the language                                               |
| `repo:api`, `branch:main`, `tag:owner:alice` | from matching repositories                                    |
| `-path:tests`, `-lang:md`                    | not matching                                                  |

Case is ignored unless you pass `-s`. `-w` matches whole words. `-r` takes the
whole query as one regular expression, for patterns pasted from elsewhere.

## Keeping output small

- Output is capped at 100 matching lines (`-n` changes it, `-n 0` removes
  it) and 20 per file (`-m`). The footer says how many there were in all.
- When results are cut, the footer suggests qualifiers that narrow them,
  with how many files each keeps: `narrow with: repo:api (120) language:Rust (80) path:src/** (64)`. Append one to the query.
- `-l` prints only paths; `-c` paths with counts. Start broad with `-l`, then
  search the files that matter.
- `-C 2` adds context lines when you need to read around a match.
- `-q` prints nothing; read the exit status.

## Scope

Without scope options, a search covers every repository dowse knows.

- `--here`: only the repository holding the current directory.
- `-t TAG` (repeatable): repositories with the tag. Tags in one group
  (`owner:alice`, `owner:bob`) are alternatives; groups narrow each other
  (`-t dev -t owner:alice`). Every repository also has `branch:<name>`.
- `-W NAME`: the repositories of a saved workspace.
- `repo:`, `branch:` and `tag:` in the query do the same per query.

`dowse repos` lists repositories with branch, index state, file count, when
they were indexed and tags; `--json` for structured output.

## Machine-readable output

`--json` prints one object per line: a `file` per matching file, then one
`summary`.

```json
{"type":"file","repo":"api","branch":"main","path":"src/config.rs","abs_path":"/src/api/src/config.rs","language":"Rust","matched_lines":2,"lines":[{"line":12,"text":"pub fn parse_config(","match":true,"ranges":[[7,19]]}]}
{"type":"summary","matched_lines":2,"files":1,"shown_lines":2,"shown_files":1,"searched_files":3,"corpus_files":4120,"repos":5,"unindexed_repos":0,"truncated":false,"elapsed_ms":8.1,"candidates_ms":0.9,"bytes_read":20480,"facets":[]}
```

`--table csv|tsv|md|json` prints one row per matching line with repository,
branch, path, line, column, language, the match and the line. A regex with
capture groups adds a column per group, which turns a search into data:

```bash
dowse search --table csv '/version = "(?<version>[^"]+)"/ path:Cargo.toml'
```

## Managing repositories

```bash
dowse repos add ~/src/api ~/src/web -t dev   # add, tagging them
dowse repos add ~/mirrors                    # a folder of repositories adds each
dowse repos tag api owner:alice -r mirror    # add owner:alice, remove mirror
dowse index --here --wait                    # update an index and wait
dowse index --here --full                    # rebuild it from every file
dowse repos index-location api              # where its index is kept
dowse repos index-location api --external   # keep it outside the repository
dowse settings                               # the app's settings
dowse status                                 # the app, its windows and indexing
```

Getting code from GitHub (through `gh`, which holds the user's sign-in):

```bash
dowse repos github my-org -q                 # the org's repositories, names only
dowse repos clone my-org/api --into ~/mirrors --wait   # clone, add, wait
dowse repos clone --from my-org --pull-every 1h        # all of them, kept current
dowse repos pull api --wait                  # fetch; fast-forward when safe
dowse tasks                                  # clones and pulls in the background
```

Clones land in `<folder>/<owner>/<name>`, tagged `owner:<owner>`, blobless
by default (full history, file contents fetched on demand). A pull never
merges or touches local changes: on another branch, with uncommitted changes
or local commits it only fetches, and says why.

A repository without an index is scanned until its index is built in the
background; the footer notes it.

## When something looks wrong

`dowse dev logs` prints the app's latest log records (`--level debug` for
more, `-f` to follow). `dowse status` shows the log file's path.
