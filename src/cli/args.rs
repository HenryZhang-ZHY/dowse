//! The `dowse` subcommands and their options.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::diagnostics::log::LogLevel;
use crate::engine::github::CloneMode;
use crate::engine::sync::Interval;
use crate::engine::table::ExportFormat;
use crate::ipc::protocol::ScopeSpec;

#[derive(Debug, Parser)]
#[command(
    name = "dowse",
    bin_name = "dowse",
    version,
    about = "Search code across many repositories, from the command line.",
    after_help = "Run `dowse guide` for a guide written for coding agents.",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: CliCommand,
}

#[derive(Debug, Subcommand)]
pub enum CliCommand {
    /// Search the repositories dowse knows, in GitHub code search syntax.
    #[command(
        visible_alias = "s",
        after_help = "\
Query syntax (terms combine per file, like GitHub code search):
  parse config            files containing both, showing lines with either
  parse OR config         files containing either
  parse NOT test          files with parse but not test
  \"fn main()\"             exact text
  /fn \\w+_test/           a line matching the regular expression
  path:src/*.rs           path glob (anchored when it holds a /) or text
  language:rust lang:ts   language by name, alias or extension
  repo:api branch:main    repository, branch
  tag:owner:alice         repositories with the tag
  -path:tests -lang:md    exclude

Examples:
  dowse search 'parse_config lang:rust'
  dowse search -l 'TODO path:src/**' --tag dev
  dowse search -C 2 '\"impl Display for\"' --here
  dowse search --table csv '/version = \"(?<version>[^\"]+)\"/ path:Cargo.toml'

Exit status: 0 when something matched, 1 when nothing did, 2 on error."
    )]
    Search(SearchArgs),
    /// List the repositories dowse knows, add, clone, pull and tag them.
    Repos(ReposArgs),
    /// Bring the indexes of repositories up to date.
    Index(IndexArgs),
    /// Show the app's background tasks (clones and pulls), or cancel them.
    Tasks(TasksArgs),
    /// Show the app's settings, or change them.
    #[command(after_help = "\
Settings:
  index.location      where the indexes of repositories without a location of
                      their own are kept: repo (the repository's .tgrep, where
                      the tgrep command line finds it; the default) or external
                      (outside the repository, where cleaning it cannot delete it)
  index.external-dir  the folder external indexes go under
  updates.check       whether the app looks for a new release once a day: true
                      (the default) or false; Help > Check for Updates… asks at
                      any time

Changing where indexes are kept moves them; one that cannot be moved stays
where it was and is built again in its new place.

Examples:
  dowse settings set index.location external
  dowse settings set index.external-dir D:/dowse-indexes
  dowse settings unset index.location
  dowse settings set updates.check false")]
    Settings(SettingsArgs),
    /// Show what the running app is doing.
    Status(JsonArg),
    /// Quit the running app, closing its windows.
    Quit,
    /// The running app's logs and metrics, for finding out what went wrong
    /// and where time goes.
    Dev(DevArgs),
    /// Print the guide to using dowse, written for coding agents.
    Guide,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// The query. Several words are joined with spaces.
    #[arg(required = true, value_name = "QUERY")]
    pub query: Vec<String>,
    /// Match case (GitHub's default is to ignore it).
    #[arg(short = 's', long)]
    pub case_sensitive: bool,
    /// Match whole words.
    #[arg(short, long)]
    pub word: bool,
    /// Take the whole query as one regular expression, matched line by line.
    #[arg(short, long)]
    pub regex: bool,
    #[command(flatten)]
    pub scope: ScopeArgs,
    /// Print only the paths of matching files.
    #[arg(short = 'l', long, conflicts_with_all = ["count", "table"])]
    pub files_with_matches: bool,
    /// Print each matching file's path with its number of matching lines.
    #[arg(short, long, conflicts_with = "table")]
    pub count: bool,
    /// Print nothing; the exit status says whether anything matched.
    #[arg(short, long)]
    pub quiet: bool,
    /// Lines of context around each matching line.
    #[arg(short = 'C', long, default_value_t = 0, value_name = "N")]
    pub context: usize,
    /// Matching lines kept per file; the rest are counted.
    #[arg(short = 'm', long, default_value_t = 20, value_name = "N")]
    pub max_per_file: usize,
    /// Matching lines printed in all; 0 prints every line found.
    #[arg(short = 'n', long, default_value_t = 100, value_name = "N")]
    pub limit: usize,
    /// One JSON object per line: a `file` per matching file, then a `summary`.
    #[arg(long)]
    pub json: bool,
    /// One row per matching line, with a column per capture group of the
    /// query's regex.
    #[arg(long, value_name = "FORMAT", num_args = 0..=1, default_missing_value = "tsv")]
    pub table: Option<TableFormat>,
    /// Also print how the indexes narrowed the search.
    #[arg(long)]
    pub stats: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum TableFormat {
    Csv,
    Tsv,
    Md,
    Json,
}

impl From<TableFormat> for ExportFormat {
    fn from(format: TableFormat) -> Self {
        match format {
            TableFormat::Csv => Self::Csv,
            TableFormat::Tsv => Self::Tsv,
            TableFormat::Md => Self::Markdown,
            TableFormat::Json => Self::Json,
        }
    }
}

/// Which repositories to cover. Without any, all of them.
#[derive(Clone, Debug, Default, Args)]
pub struct ScopeArgs {
    /// Only repositories with this tag. Tags in one group (`owner:alice`,
    /// `owner:bob`) are alternatives; groups narrow each other.
    #[arg(short, long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,
    /// Only the repositories of this saved workspace, by name or path.
    #[arg(short = 'W', long, value_name = "NAME")]
    pub workspace: Option<String>,
    /// Only the repository holding the current directory.
    #[arg(long)]
    pub here: bool,
}

impl From<ScopeArgs> for ScopeSpec {
    fn from(scope: ScopeArgs) -> Self {
        Self {
            tags: scope.tags,
            workspace: scope.workspace,
            here: scope.here,
        }
    }
}

#[derive(Debug, Args)]
pub struct ReposArgs {
    #[command(subcommand)]
    pub action: Option<ReposAction>,
    #[command(flatten)]
    pub scope: ScopeArgs,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum ReposAction {
    /// Add folders; a folder holding git repositories adds each of them.
    Add {
        #[arg(required = true, value_name = "FOLDER")]
        folders: Vec<PathBuf>,
        /// Tag what is added.
        #[arg(short, long = "tag", value_name = "TAG")]
        tags: Vec<String>,
    },
    /// Add tags to a repository, or remove them.
    Tag {
        /// The repository, by name or path.
        repo: String,
        /// Tags to add, such as `dev` or `owner:alice`.
        tags: Vec<String>,
        /// Tags to remove.
        #[arg(short, long = "remove", value_name = "TAG")]
        remove: Vec<String>,
    },
    /// List an owner's repositories on GitHub, through the GitHub CLI (gh).
    Github(GithubArgs),
    /// Clone GitHub repositories in the background, into <DIR>/<owner>/<name>,
    /// and add them to the library.
    #[command(after_help = "\
Examples:
  dowse repos clone alice/api alice/web --into ~/mirrors -t mirror
  dowse repos clone --from my-org --into ~/mirrors --pull-every 1h --wait
  dowse repos clone --from my-org cli     # those whose names contain cli")]
    Clone(CloneArgs),
    /// Pull repositories in the background: fetch, then fast-forward the
    /// default branch when it is checked out and has no local changes or
    /// commits.
    Pull {
        /// Repositories by name or path; without any, those in scope.
        #[arg(value_name = "REPO")]
        repos: Vec<String>,
        #[command(flatten)]
        scope: ScopeArgs,
        /// Wait until the pulls finish.
        #[arg(long)]
        wait: bool,
    },
    /// Show where repositories keep their indexes, or keep them elsewhere:
    /// in the repository's .tgrep, where the tgrep command line finds them,
    /// or outside it, where cleaning the repository cannot delete them.
    /// Indexes that change place are moved.
    #[command(after_help = "\
Examples:
  dowse repos index-location api web --external
  dowse repos index-location api --default     # as `dowse settings` says
  tgrep search foo --index-path \"$(dowse repos index-location api -q)\"")]
    IndexLocation {
        #[arg(required = true, value_name = "REPO")]
        repos: Vec<String>,
        /// Keep them in the repository's .tgrep.
        #[arg(long, group = "change")]
        repo: bool,
        /// Keep them outside the repository, under index.external-dir.
        #[arg(long, group = "change")]
        external: bool,
        /// Keep them where the index.location setting says.
        #[arg(long, group = "change")]
        default: bool,
        /// Print only the index folders, one per line.
        #[arg(short = 'q', long)]
        quiet: bool,
        #[arg(long)]
        json: bool,
    },
    /// Pull repositories on an interval, such as 15m, 1h or 3d, while the
    /// app runs; or stop.
    Sync {
        #[arg(required = true, value_name = "REPO")]
        repos: Vec<String>,
        /// How often, such as 15m, 1h or 3d.
        #[arg(
            long,
            value_name = "INTERVAL",
            required_unless_present = "off",
            conflicts_with = "off"
        )]
        every: Option<Interval>,
        /// Stop pulling them.
        #[arg(long)]
        off: bool,
    },
}

#[derive(Debug, Args)]
pub struct GithubArgs {
    /// A user or organization; without one, the signed-in user.
    pub owner: Option<String>,
    /// Include forks.
    #[arg(long)]
    pub forks: bool,
    /// Include archived repositories.
    #[arg(long)]
    pub archived: bool,
    /// Only repositories whose name, description or language contain these
    /// words.
    #[arg(long = "filter", value_name = "WORDS")]
    pub filter: Option<String>,
    /// List at most this many; all of them without it.
    #[arg(short = 'L', long)]
    pub limit: Option<usize>,
    /// Print only `owner/name`, one per line.
    #[arg(short = 'q', long)]
    pub quiet: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct CloneArgs {
    /// Repositories as `owner/name`. With `--from`, words that the names of
    /// the owner's repositories must contain.
    #[arg(value_name = "REPO", required_unless_present = "from")]
    pub repos: Vec<String>,
    /// Clone the repositories of this user or organization, without forks
    /// or archived ones unless asked.
    #[arg(long, value_name = "OWNER")]
    pub from: Option<String>,
    /// With `--from`, include forks.
    #[arg(long, requires = "from")]
    pub forks: bool,
    /// With `--from`, include archived repositories.
    #[arg(long, requires = "from")]
    pub archived: bool,
    /// The folder clones go under; the last one used when not given.
    #[arg(long, value_name = "DIR")]
    pub into: Option<PathBuf>,
    /// How much history to fetch: blobless (all history, file contents on
    /// demand), shallow (the latest commit) or full.
    #[arg(long, default_value = "blobless", value_parser = parse_mode)]
    pub mode: CloneMode,
    /// Tag the clones, besides `owner:<owner>`.
    #[arg(short, long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,
    /// Pull the clones on this interval, such as 1h.
    #[arg(long, value_name = "INTERVAL")]
    pub pull_every: Option<Interval>,
    /// Wait until the clones finish.
    #[arg(long)]
    pub wait: bool,
}

#[derive(Debug, Args)]
pub struct SettingsArgs {
    #[command(subcommand)]
    pub action: Option<SettingsAction>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum SettingsAction {
    /// Change a setting.
    Set { key: String, value: String },
    /// Put a setting back to its default.
    Unset { key: String },
}

#[derive(Debug, Args)]
pub struct TasksArgs {
    #[command(subcommand)]
    pub action: Option<TasksAction>,
    /// Wait until every task has finished.
    #[arg(long)]
    pub wait: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum TasksAction {
    /// Cancel tasks by id, or all of them.
    Cancel {
        #[arg(value_name = "ID", required_unless_present = "all")]
        ids: Vec<u64>,
        #[arg(long)]
        all: bool,
    },
}

fn parse_mode(text: &str) -> Result<CloneMode, String> {
    CloneMode::parse(text).ok_or_else(|| "expected blobless, shallow or full".into())
}

#[derive(Debug, Args)]
pub struct IndexArgs {
    #[command(flatten)]
    pub scope: ScopeArgs,
    /// Wait until the indexes are up to date.
    #[arg(long)]
    pub wait: bool,
    /// Build the indexes again from every file, rather than reading only the
    /// files that changed.
    #[arg(long)]
    pub full: bool,
}

#[derive(Debug, Args)]
pub struct JsonArg {
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DevArgs {
    #[command(subcommand)]
    pub action: DevAction,
}

#[derive(Debug, Subcommand)]
pub enum DevAction {
    /// Print the latest log records.
    Logs {
        /// The least severe level to print.
        #[arg(long, default_value = "info", value_parser = parse_level)]
        level: LogLevel,
        /// How many of the latest records to print.
        #[arg(short = 'n', long, default_value_t = 100)]
        limit: usize,
        /// Keep printing records as they come.
        #[arg(short, long)]
        follow: bool,
        #[arg(long)]
        json: bool,
    },
    /// Print the app's key numbers: search and index timings, how much the
    /// indexes saved, memory, requests and the latest searches.
    Metrics {
        #[arg(long)]
        json: bool,
    },
    /// Open the developer tools window.
    Open,
}

fn parse_level(text: &str) -> Result<LogLevel, String> {
    LogLevel::parse(text).ok_or_else(|| "expected error, warn, info, debug or trace".into())
}

/// The subcommand names, so the launcher can tell `dowse search` from
/// `dowse <folder>`.
pub fn is_subcommand(arg: &str) -> bool {
    use clap::CommandFactory as _;
    Cli::command().get_subcommands().any(|command| {
        command.get_name() == arg || command.get_all_aliases().any(|alias| alias == arg)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("dowse").chain(args.iter().copied()))
    }

    #[test]
    fn search_joins_its_words_and_reads_its_options() {
        let Cli {
            command: CliCommand::Search(search),
        } = parse(&[
            "search",
            "parse",
            "lang:rust",
            "-t",
            "dev",
            "--tag",
            "owner:bob",
            "-l",
        ])
        .unwrap()
        else {
            panic!("expected search");
        };
        assert_eq!(search.query.join(" "), "parse lang:rust");
        assert_eq!(search.scope.tags, ["dev", "owner:bob"]);
        assert!(search.files_with_matches);
        assert_eq!(search.limit, 100);
        assert_eq!(search.context, 0);
    }

    #[test]
    fn table_defaults_to_tsv() {
        let Cli {
            command: CliCommand::Search(search),
        } = parse(&["s", "x", "--table"]).unwrap()
        else {
            panic!("expected search");
        };
        assert_eq!(search.table, Some(TableFormat::Tsv));
    }

    #[test]
    fn unknown_options_are_errors_not_ignored() {
        assert!(parse(&["search", "x", "--frobnicate"]).is_err());
        assert!(parse(&["search"]).is_err());
        assert!(parse(&["search", "x", "-l", "-c"]).is_err());
    }

    #[test]
    fn subcommands_are_told_from_folders() {
        assert!(is_subcommand("search"));
        assert!(is_subcommand("s"));
        assert!(is_subcommand("repos"));
        assert!(!is_subcommand("src"));
        assert!(!is_subcommand("--add"));
    }

    #[test]
    fn clone_pull_and_sync_read_their_options() {
        let Cli {
            command:
                CliCommand::Repos(ReposArgs {
                    action: Some(ReposAction::Clone(clone)),
                    ..
                }),
        } = parse(&[
            "repos",
            "clone",
            "a/b",
            "--into",
            "/m",
            "--mode",
            "shallow",
            "--pull-every",
            "1h",
        ])
        .unwrap()
        else {
            panic!("expected repos clone");
        };
        assert_eq!(clone.repos, ["a/b"]);
        assert_eq!(clone.mode, CloneMode::Shallow);
        assert_eq!(
            clone.pull_every.map(|every| every.to_string()).as_deref(),
            Some("1h")
        );
        assert!(
            parse(&["repos", "clone"]).is_err(),
            "needs repositories or --from"
        );
        assert!(parse(&["repos", "clone", "--from", "org"]).is_ok());
        assert!(parse(&["repos", "clone", "a/b", "--mode", "deep"]).is_err());
        assert!(
            parse(&["repos", "clone", "a/b", "--forks"]).is_err(),
            "--forks needs --from"
        );
        assert!(
            parse(&["repos", "sync", "api"]).is_err(),
            "needs --every or --off"
        );
        assert!(parse(&["repos", "sync", "api", "--every", "1m"]).is_err());
        assert!(parse(&["repos", "sync", "api", "--every", "1h", "--off"]).is_err());
        assert!(parse(&["repos", "sync", "api", "--off"]).is_ok());
        assert!(parse(&["repos", "pull", "--here", "--wait"]).is_ok());
        assert!(parse(&["tasks", "cancel"]).is_err());
        assert!(parse(&["tasks", "cancel", "--all"]).is_ok());
    }

    #[test]
    fn logs_take_a_level() {
        let Cli {
            command:
                CliCommand::Dev(DevArgs {
                    action: DevAction::Logs { level, .. },
                }),
        } = parse(&["dev", "logs", "--level", "warn"]).unwrap()
        else {
            panic!("expected dev logs");
        };
        assert_eq!(level, LogLevel::Warn);
        assert!(parse(&["dev", "logs", "--level", "loud"]).is_err());
    }
}
