# Searching

Queries use GitHub code search syntax and run as you type. Terms combine per file,
not per line: `parse config` finds files containing both, wherever they are, and
shows the lines with either.

| Query | Finds files |
| --- | --- |
| `parse config` | containing both (`AND` between them is optional) |
| `parse OR config` | containing either; `AND` binds tighter than `OR` |
| `parse NOT config`, `NOT (a OR b)` | containing `parse` but not `config`; without `a` or `b` |
| `"fn main()"` | containing the exact text; `\"` and `\\` escape inside quotes |
| `/fn \w+_test/` | with a line matching the regular expression |
| `path:src/*.rs`, `path:engine` | whose path matches the glob (anchored when it holds a `/`) or contains the text |
| `language:rust`, `lang:ts` | in the language, by name, alias or extension |
| `repo:api`, `branch:main`, `tag:owner:alice` | from matching repositories |
| `-path:tests`, `-lang:md` | not matching the qualifier |

Qualifier values may be quoted (`language:"Visual Basic"`) or a `/regex/`, and
`content:` makes a plain term of text that looks like a qualifier. A query of only
qualifiers, such as `path:*.proto`, lists the files without reading them.

Toggles set match case (`Alt+C`) and whole word (`Alt+W`) for every term; regular
expression (`Alt+R`) takes the whole box as one regex, matched line by line. On
macOS the shortcuts are `Cmd+Alt+C/W/R`.

## The path filter

`path:` in the query is usually enough; for more, the funnel in the search box
(`Ctrl+P`) opens a box of space-separated terms beside it. `src` keeps paths
containing `src`, `*.rs` keeps matching globs, and `!tests` or `-*.md` drops paths.
The box stays open while a tab has a filter in it.

## Choosing what to search

The scope bar picks repositories by their tags, and facets narrow the results
without changing it; see [Tags choose what to search](app.md#tags-choose-what-to-search).
From the command line, `--here`, `-t <tag>` and `-W <workspace>` do the same; see
[the command line](cli.md).

## How a search runs

1. The query is parsed, and each term becomes a regex (literal text is escaped;
   whole word adds `\b`) and a tgrep trigram plan. The terms outside any `NOT` also
   make up one combined regex, which finds the lines to show.
2. Repository qualifiers rule out whole repositories. For every other repository in
   scope, the plans select candidate files from its [index](indexes.md): terms that
   must all match intersect their candidates, alternatives unite them, and a negated
   term narrows nothing. Files the watcher saw change are added, then the path
   filter and the path and language qualifiers drop files before they are read.
3. Candidates are read and matched in ordered parallel chunks, repository by
   repository; each term is checked only when the answer still depends on it. Once
   about 20,000 matching lines (or 10,000 files) are found, the rest are skipped and
   the summary says so.

On a 42,000-file tree (1.3 GB of crate sources), the index builds in about 6 s and
typical queries finish in 20–60 ms.
