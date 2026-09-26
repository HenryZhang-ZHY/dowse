//! The `dowse` command line: `dowse search`, `dowse repos` and the rest.
//!
//! As with Obsidian's command line, each subcommand is a remote control for
//! the running app, which answers over its socket (see [`crate::ipc`]): the
//! app keeps indexes warm and watches for changes, so the command line gets
//! the same fresh results as a window. When the app is not running, the
//! first subcommand starts it in the background, without windows.

pub mod args;
pub mod client;
pub mod output;

use std::ffi::OsString;
use std::io::{self, IsTerminal as _, Write as _};

use clap::Parser as _;

use crate::engine::query::SearchQuery;
use crate::ipc::protocol::{Frame, LogsRequest, Request, SearchRequest};
use args::{Cli, CliCommand, DevAction, ReposAction, SearchArgs};
use output::Style;

/// The guide `dowse guide` prints.
pub const GUIDE: &str = include_str!("../../docs/agent-guide.md");

/// Exit statuses, as grep has them.
const FOUND: i32 = 0;
const NOT_FOUND: i32 = 1;
const FAILED: i32 = 2;

/// Whether the arguments after the program name ask for the command line
/// rather than the desktop app.
pub fn wants_cli(first: Option<&OsString>) -> bool {
    first
        .and_then(|arg| arg.to_str())
        .is_some_and(|arg| args::is_subcommand(arg) || matches!(arg, "-V" | "--version"))
}

/// Read a launch of the desktop app from the arguments after the program
/// name. Folders and workspace files to open or add must exist, so a
/// mistyped subcommand is reported, with the one meant, rather than opening
/// an empty window.
pub fn parse_launch(
    args: impl IntoIterator<Item = OsString>,
    cwd: &std::path::Path,
) -> Result<crate::launch::Command, String> {
    use crate::launch::Command;
    let args: Vec<OsString> = args.into_iter().collect();
    let command = crate::launch::parse(args.iter().cloned(), cwd)?;
    let paths: Vec<&std::path::PathBuf> = match &command {
        Command::Open {
            folders,
            workspaces,
        } => folders.iter().chain(workspaces).collect(),
        Command::Add(folders) => folders.iter().collect(),
        _ => Vec::new(),
    };
    let Some(missing) = paths.into_iter().find(|path| !path.exists()) else {
        return Ok(command);
    };
    let first = args
        .first()
        .and_then(|arg| arg.to_str())
        .unwrap_or_default();
    Err(match suggest_subcommand(first) {
        Some(meant) if cwd.join(first) == *missing => {
            format!("no command or folder named {first}; did you mean `dowse {meant}`?")
        }
        _ => format!("no such folder: {}", missing.display()),
    })
}

/// The subcommand `word` is probably a typo of.
fn suggest_subcommand(word: &str) -> Option<String> {
    use clap::CommandFactory as _;
    Cli::command()
        .get_subcommands()
        .map(|command| command.get_name().to_string())
        .map(|name| (edit_distance(word, &name), name))
        .filter(|(distance, name)| *distance <= 2 && *distance < name.len())
        .min()
        .map(|(_, name)| name)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous + usize::from(ca != *cb);
            previous = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(previous + 1);
        }
    }
    row[b.len()]
}

/// Run a subcommand; the arguments include the program name. Returns the
/// exit status.
pub fn run(args: impl IntoIterator<Item = OsString>) -> i32 {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return if error.use_stderr() { FAILED } else { FOUND };
        }
    };
    match execute(cli.command) {
        Ok(status) => status,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => FOUND,
        Err(error) => {
            eprintln!("dowse: {error}");
            FAILED
        }
    }
}

fn execute(command: CliCommand) -> io::Result<i32> {
    let request = match command {
        CliCommand::Guide => {
            io::stdout().write_all(GUIDE.as_bytes())?;
            return Ok(FOUND);
        }
        CliCommand::Search(search) => return run_search(search),
        CliCommand::Repos(repos) => match repos.action {
            None => {
                let json = repos.json;
                let frames = send(Request::Repos(repos.scope.into()))?;
                return print_frames(frames, json);
            }
            Some(ReposAction::Add { folders, tags }) => {
                let cwd = current_dir();
                Request::AddRepos {
                    folders: folders.into_iter().map(|folder| cwd.join(folder)).collect(),
                    tags,
                }
            }
            Some(ReposAction::Tag { repo, tags, remove }) => Request::Tag {
                repo,
                add: tags,
                remove,
            },
        },
        CliCommand::Index(index) => Request::Index {
            scope: index.scope.into(),
            wait: index.wait,
        },
        CliCommand::Status(json) => return print_frames(send(Request::Status)?, json.json),
        CliCommand::Quit => {
            match crate::ipc::Connection::connect(&crate::engine::config::default_root()) {
                Ok(connection) => {
                    return print_frames(connection.request(Request::Quit, &current_dir())?, false);
                }
                Err(_) => {
                    eprintln!("dowse is not running");
                    return Ok(FOUND);
                }
            }
        }
        CliCommand::Dev(dev) => match dev.action {
            DevAction::Logs {
                level,
                limit,
                follow,
                json,
            } => {
                let request = Request::Logs(LogsRequest {
                    after: 0,
                    level,
                    limit,
                    follow,
                });
                return print_frames(send(request)?, json);
            }
        },
    };
    print_frames(send(request)?, false)
}

fn current_dir() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_default()
}

/// Send a request to the running app, starting it when need be.
fn send(request: Request) -> io::Result<crate::ipc::Frames> {
    client::connect_or_start()?.request(request, &current_dir())
}

/// Print every frame of an answer as text, or as JSON with `json`.
fn print_frames(frames: crate::ipc::Frames, json: bool) -> io::Result<i32> {
    let style = style();
    let mut stdout = io::stdout().lock();
    for frame in frames {
        match frame? {
            Frame::Done => return Ok(FOUND),
            Frame::Error(error) => {
                eprintln!("dowse: {error}");
                return Ok(FAILED);
            }
            Frame::Message(message) => eprintln!("{message}"),
            Frame::Repos(repos) if json => writeln!(stdout, "{}", to_json(&repos))?,
            Frame::Repos(repos) => {
                let now = crate::diagnostics::log::now_ms();
                stdout.write_all(output::repos_text(&repos, now).as_bytes())?;
            }
            Frame::Status(status) if json => writeln!(stdout, "{}", to_json(&status))?,
            Frame::Status(status) => stdout.write_all(output::status_text(&status).as_bytes())?,
            Frame::Log(entry) if json => writeln!(stdout, "{}", to_json(&entry))?,
            Frame::Log(entry) => {
                stdout.write_all(output::log_line(&entry, style).as_bytes())?;
                stdout.flush()?;
            }
            Frame::Search(_) => {}
        }
    }
    Ok(FAILED)
}

fn to_json(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

fn run_search(search: SearchArgs) -> io::Result<i32> {
    let files_only = search.files_with_matches || search.count || search.quiet;
    let request = SearchRequest {
        query: SearchQuery {
            pattern: search.query.join(" "),
            case_sensitive: search.case_sensitive,
            whole_word: search.word,
            regex: search.regex,
            path_filter: String::new(),
        },
        scope: search.scope.into(),
        context: search.context,
        max_per_file: search.max_per_file,
        limit: search.limit,
        files_only,
        table: search.table.map(Into::into),
    };
    let style = style();
    let mut stdout = io::stdout().lock();
    for frame in send(Request::Search(request))? {
        match frame? {
            Frame::Search(response) => {
                if search.quiet {
                    return Ok(if response.files.is_empty() {
                        NOT_FOUND
                    } else {
                        FOUND
                    });
                }
                let body = if let Some(table) = &response.table {
                    table.clone()
                } else if search.json {
                    output::search_json(&response)
                } else if search.files_with_matches {
                    output::file_list(&response)
                } else if search.count {
                    output::file_counts(&response)
                } else {
                    output::search_text(&response, style)
                };
                stdout.write_all(body.as_bytes())?;
                stdout.flush()?;
                if !search.json {
                    let footer = output::search_footer(&response, search.stats, stderr_style());
                    eprint!("{footer}");
                }
                return Ok(if response.files.is_empty() {
                    NOT_FOUND
                } else {
                    FOUND
                });
            }
            Frame::Error(error) => {
                eprintln!("dowse: {error}");
                return Ok(FAILED);
            }
            Frame::Message(message) => eprintln!("{message}"),
            _ => {}
        }
    }
    Ok(FAILED)
}

/// Colour for stdout when it is a terminal and `NO_COLOR` is unset.
fn style() -> Style {
    Style {
        color: io::stdout().is_terminal() && colors_allowed(),
    }
}

fn stderr_style() -> Style {
    Style {
        color: io::stderr().is_terminal() && colors_allowed(),
    }
}

fn colors_allowed() -> bool {
    std::env::var_os("NO_COLOR").is_none() && client::enable_terminal_colors()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launcher_usage_lists_every_subcommand() {
        use clap::CommandFactory as _;
        for command in Cli::command().get_subcommands() {
            let name = command.get_name();
            assert!(
                crate::launch::USAGE.contains(&format!("\n  {name} ")),
                "{name} is missing from the usage"
            );
        }
    }

    #[test]
    fn launches_need_their_folders_and_catch_typos() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        std::fs::create_dir(cwd.join("api")).unwrap();
        let parse = |args: &[&str]| parse_launch(args.iter().map(OsString::from), cwd);
        assert!(parse(&["api"]).is_ok());
        assert!(parse(&["--remove", "gone"]).is_ok());
        assert_eq!(
            parse(&["serch", "x"]).unwrap_err(),
            "no command or folder named serch; did you mean `dowse search`?"
        );
        let error = parse(&["--add", "nowhere"]).unwrap_err();
        assert!(error.starts_with("no such folder: "), "{error}");
        assert!(parse(&["--frobnicate"]).is_err());
        assert_eq!(edit_distance("repo", "repos"), 1);
        assert_eq!(suggest_subcommand("x"), None);
    }

    #[test]
    fn subcommands_and_versions_go_to_the_command_line() {
        let arg = |text: &str| Some(OsString::from(text));
        assert!(wants_cli(arg("search").as_ref()));
        assert!(wants_cli(arg("--version").as_ref()));
        assert!(!wants_cli(arg("--add").as_ref()));
        assert!(!wants_cli(arg("./search").as_ref()));
        assert!(!wants_cli(None));
    }
}
