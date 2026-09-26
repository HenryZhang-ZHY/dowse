//! The `dowse` subcommands and their options.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::diagnostics::log::LogLevel;
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
    /// List the repositories dowse knows, add them and tag them.
    Repos(ReposArgs),
    /// Rebuild the indexes of repositories.
    Index(IndexArgs),
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
