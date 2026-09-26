//! The command line as a console program, for Windows: installed as
//! `dowse.com` beside the app's `dowse.exe`, it is what a terminal runs for
//! `dowse`, since Windows prefers `.com` to `.exe`. Subcommands run here;
//! a launch of the desktop app is checked here, then handed to the app,
//! which this leaves running.

use std::process::{Command, Stdio};

use dowse::launch;

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if dowse::cli::wants_cli(args.get(1)) {
        std::process::exit(dowse::cli::run(args));
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    match dowse::cli::parse_launch(args.iter().skip(1).cloned(), &cwd) {
        Ok(launch::Command::Help) => {
            print!("{}", launch::USAGE);
            return;
        }
        Ok(_) => {}
        Err(error) => {
            eprintln!("dowse: {error}");
            std::process::exit(2);
        }
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let app = exe.with_file_name(format!("dowse{}", std::env::consts::EXE_SUFFIX));
    // The app outlives this command: it must not hold the pipe a caller may
    // be reading this command's output from.
    dowse::cli::client::keep_std_handles_to_ourselves();
    let launched = Command::new(&app)
        .args(&args[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(error) = launched {
        eprintln!("dowse: cannot start {}: {error}", app.display());
        std::process::exit(2);
    }
}
