//! Opening a search hit in the user's editor.
//!
//! `TGREP_GPUI_EDITOR` picks the command, with `{file}` and `{line}`
//! placeholders, for example `code -g {file}:{line}` or `nvim-qt +{line} {file}`.
//! Without it, the first editor found on `PATH` among VS Code, Cursor, Zed and
//! Sublime Text is used, and failing that the system's default application.

use std::path::{Path, PathBuf};
use std::process::Command;

pub const EDITOR_ENV: &str = "TGREP_GPUI_EDITOR";

/// How to open a file at a line.
#[derive(Debug, PartialEq, Eq)]
pub enum Launch {
    Command {
        program: PathBuf,
        args: Vec<String>,
    },
    /// Hand the file to the operating system's default application.
    System,
}

/// Editors probed on `PATH`, with their "go to line" argument form.
const KNOWN_EDITORS: &[(&str, &[&str])] = &[
    ("code", &["-g", "{file}:{line}"]),
    ("cursor", &["-g", "{file}:{line}"]),
    ("zed", &["{file}:{line}"]),
    ("subl", &["{file}:{line}"]),
];

pub fn resolve(file: &Path, line: usize) -> Launch {
    let file = tgrep_gpui::engine::workspace::display_path(file);
    if let Ok(template) = std::env::var(EDITOR_ENV)
        && let Some(launch) = from_template(&template, &file, line)
    {
        return launch;
    }
    for (name, args) in KNOWN_EDITORS {
        if let Ok(program) = which::which(name) {
            return Launch::Command {
                program,
                args: args
                    .iter()
                    .map(|arg| substitute(arg, &file, line))
                    .collect(),
            };
        }
    }
    Launch::System
}

fn from_template(template: &str, file: &str, line: usize) -> Option<Launch> {
    let mut words = split_words(template).into_iter();
    let program = words.next()?;
    let mut args: Vec<String> = words.map(|word| substitute(&word, file, line)).collect();
    if !template.contains("{file}") {
        args.push(file.to_string());
    }
    let program = which::which(&program).unwrap_or_else(|_| PathBuf::from(program));
    Some(Launch::Command { program, args })
}

fn substitute(word: &str, file: &str, line: usize) -> String {
    word.replace("{file}", file)
        .replace("{line}", &line.to_string())
}

/// Split on whitespace, keeping double-quoted runs together.
fn split_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut has_word = false;
    for ch in text.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                has_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has_word {
                    words.push(std::mem::take(&mut current));
                    has_word = false;
                }
            }
            c => {
                current.push(c);
                has_word = true;
            }
        }
    }
    if has_word {
        words.push(current);
    }
    words
}

/// Start the editor without waiting for it.
pub fn spawn(program: &Path, args: &[String]) -> std::io::Result<()> {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // Editors reached through a `.cmd` shim would otherwise flash a console.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn().map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quoted_words() {
        assert_eq!(
            split_words(r#""C:\Program Files\Ed\ed.exe" --line {line} {file}"#),
            vec![r"C:\Program Files\Ed\ed.exe", "--line", "{line}", "{file}"]
        );
        assert_eq!(split_words("  a  b "), vec!["a", "b"]);
    }

    #[test]
    fn templates_substitute_placeholders_and_default_to_appending_the_file() {
        let Some(Launch::Command { args, .. }) = from_template("ed +{line} {file}", "/x.rs", 7)
        else {
            panic!("expected a command");
        };
        assert_eq!(args, vec!["+7", "/x.rs"]);

        let Some(Launch::Command { args, .. }) = from_template("ed --wait", "/x.rs", 7) else {
            panic!("expected a command");
        };
        assert_eq!(args, vec!["--wait", "/x.rs"]);

        assert_eq!(from_template("   ", "/x.rs", 7), None);
    }
}
