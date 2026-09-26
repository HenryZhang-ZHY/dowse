//! Launching the desktop app, modelled on VS Code's `code`:
//!
//! - `dowse` restores the last session.
//! - `dowse <folder>...` opens the folders as a new untitled workspace;
//!   a `.dowse-workspace` file opens that workspace.
//! - `dowse --add <folder>...` adds the folders to the last focused
//!   window's workspace; `--remove` takes them out.
//!
//! A folder holding several git repositories stands for all of them.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::workspace;

pub const USAGE: &str = "\
Usage: dowse [OPTIONS] [PATH...]
       dowse <COMMAND> [ARGS...]

Search code across many repositories, in the desktop app or from the
command line.

  PATH                  Folders open as a new untitled workspace; a folder
                        holding git repositories adds each of them. A
                        .dowse-workspace file opens that workspace.

Options:
  -a, --add <FOLDER>...     Add folders to the last focused window's workspace
      --remove <FOLDER>...  Remove folders from the last focused window's workspace
  -h, --help                Show this help
  -V, --version             Show the version

Commands (`dowse <COMMAND> --help` tells more):
  search   Search the repositories dowse knows, in GitHub code search syntax
  repos    List the repositories dowse knows, add them and tag them
  index    Bring the indexes of repositories up to date
  status   Show what the running app is doing
  quit     Quit the running app, closing its windows
  dev      The running app's logs and metrics
  guide    Print the guide to using dowse, written for coding agents
";

/// What the command line asks for. Paths are absolute.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    /// Open folders as one new untitled workspace, and each workspace file
    /// in a window. With neither, just restore the session.
    #[default]
    Start,
    Open {
        folders: Vec<PathBuf>,
        workspaces: Vec<PathBuf>,
    },
    Add(Vec<PathBuf>),
    Remove(Vec<PathBuf>),
    /// Run without windows, for the command line. Not for people to type:
    /// `dowse search` and the other subcommands start the app this way when
    /// it is not running.
    Background,
    Help,
}

/// The flag behind [`Command::Background`].
pub const BACKGROUND_FLAG: &str = "--background";

/// Parse the arguments after the program name, resolving relative paths
/// against `cwd`.
pub fn parse(args: impl IntoIterator<Item = OsString>, cwd: &Path) -> Result<Command, String> {
    #[derive(PartialEq)]
    enum Mode {
        Open,
        Add,
        Remove,
    }
    let mut mode = Mode::Open;
    let mut paths = Vec::new();
    let mut flags_done = false;
    let mut background = false;
    for arg in args {
        if !flags_done && let Some(flag) = arg.to_str().filter(|arg| arg.starts_with('-')) {
            let next = match flag {
                "--" => {
                    flags_done = true;
                    continue;
                }
                "-h" | "--help" => return Ok(Command::Help),
                BACKGROUND_FLAG => {
                    background = true;
                    continue;
                }
                "-a" | "--add" => Mode::Add,
                "--remove" => Mode::Remove,
                other => return Err(format!("unknown option {other}\n\n{USAGE}")),
            };
            if mode != Mode::Open && mode != next {
                return Err(format!("--add and --remove cannot be combined\n\n{USAGE}"));
            }
            mode = next;
            continue;
        }
        paths.push(cwd.join(PathBuf::from(arg)));
    }
    if background {
        return if mode == Mode::Open && paths.is_empty() {
            Ok(Command::Background)
        } else {
            Err(format!(
                "{BACKGROUND_FLAG} takes nothing else

{USAGE}"
            ))
        };
    }
    Ok(match mode {
        Mode::Add if paths.is_empty() => return Err(format!("--add needs a folder\n\n{USAGE}")),
        Mode::Remove if paths.is_empty() => {
            return Err(format!("--remove needs a folder\n\n{USAGE}"));
        }
        Mode::Add => Command::Add(paths),
        Mode::Remove => Command::Remove(paths),
        Mode::Open if paths.is_empty() => Command::Start,
        Mode::Open => {
            let (workspaces, folders) = paths
                .into_iter()
                .partition(|path| workspace::is_workspace_file(path));
            Command::Open {
                folders,
                workspaces,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(OsString::from), Path::new("/cwd"))
    }

    #[test]
    fn parses_like_code() {
        assert_eq!(run(&[]), Ok(Command::Start));
        assert_eq!(run(&["-h"]), Ok(Command::Help));
        assert_eq!(
            run(&["api", "/abs/web", "team.dowse-workspace"]),
            Ok(Command::Open {
                folders: vec!["/cwd/api".into(), "/abs/web".into()],
                workspaces: vec!["/cwd/team.dowse-workspace".into()],
            })
        );
        assert_eq!(
            run(&["api", "--add", "web"]),
            Ok(Command::Add(vec!["/cwd/api".into(), "/cwd/web".into()]))
        );
        assert_eq!(
            run(&["-a", "api"]),
            Ok(Command::Add(vec!["/cwd/api".into()]))
        );
        assert_eq!(
            run(&["--remove", "api"]),
            Ok(Command::Remove(vec!["/cwd/api".into()]))
        );
        assert_eq!(
            run(&["--", "-odd"]),
            Ok(Command::Open {
                folders: vec!["/cwd/-odd".into()],
                workspaces: vec![],
            })
        );
    }

    #[test]
    fn background_runs_the_app_without_windows() {
        assert_eq!(run(&["--background"]), Ok(Command::Background));
        assert!(run(&["--background", "api"]).is_err());
        assert!(run(&["--background", "--add", "api"]).is_err());
    }

    #[test]
    fn rejects_bad_combinations() {
        assert!(run(&["--add"]).is_err());
        assert!(run(&["--remove"]).is_err());
        assert!(run(&["-a", "x", "--remove", "y"]).is_err());
        assert!(run(&["--frobnicate"]).is_err());
    }
}
