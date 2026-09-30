# Indexes

Every repository gets a [tgrep](https://github.com/microsoft/tgrep) trigram index,
which narrows a search to the files that can match before any is read (see
[how a search runs](search.md#how-a-search-runs)).

## Shared with the tgrep command line

Each repository's index lives in its `.tgrep` directory, the same place
`tgrep index` and `tgrep serve` use. A repository without an index gets one in the
background, one build at a time; until then its files are scanned. An index deleted
while dowse runs, as cleaning a repository's ignored files (`git clean -x`) does, is
noticed within seconds and built again.

## Or kept out of the way

A repository whose working copy you clean can keep its index outside it instead,
under a folder of dowse's (your local data folder unless you choose another), where
cleaning cannot touch it; tgrep reaches it with `--index-path`. The repositories
page sets this for all repositories and for each one, as do `dowse settings` and
`dowse repos index-location`:

```bash
dowse settings set index.location external          # every repository without its own
dowse settings set index.external-dir D:/dowse-indexes
dowse repos index-location api --repo               # this one keeps its .tgrep
tgrep search foo --index-path "$(dowse repos index-location api -q)"
```

Indexes that change place are moved rather than built again; one that cannot be
moved stays where it was and is built in its new place. The indexes of repositories
deleted or moved elsewhere are removed from that folder.

## Fresh between builds

A file watcher per repository tracks files changed since its last build, and
searches read them directly, so a `git pull` in a mirror or an edit in a working
copy shows up right away. Opening a repository also finds the files that changed
while dowse was not running, deleted and moved ones included, by comparing the
folder with the file stamps the index keeps.

## Incremental updates

Bringing an index up to date reads only the files that changed: they are indexed on
their own and streamed into a copy of the index, whose other postings are copied as
they are. On a 245,000-file repository with a 1.7 GB index, an update takes about
10 s where a full build takes about 9 minutes; finding the index current takes
about 3 s. After 500 changes a repository's index is updated automatically. The
whole index is built again only when there is none, when part of the folder cannot
be read, when most files changed, or when you ask (`Ctrl+Shift+R` updates the
indexes in scope; the palette and `dowse index --full` rebuild them). Updates and
builds happen in a staging directory, so searching keeps working while one runs.
