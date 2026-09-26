//! Reaching the running app, and starting it in the background when it is
//! not running.

use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::engine::config;
use crate::ipc::Connection;
use crate::launch::BACKGROUND_FLAG;

/// How long a freshly started app gets to start answering.
const START_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_INTERVAL: Duration = Duration::from_millis(50);

/// Connect to the app running with the current settings, starting it
/// without windows when none runs.
pub fn connect_or_start() -> io::Result<Connection> {
    let root = config::default_root();
    if let Ok(connection) = Connection::connect(&root) {
        return Ok(connection);
    }
    let app = app_executable()?;
    eprintln!("dowse: starting the app in the background");
    start_in_background(&app)?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        std::thread::sleep(RETRY_INTERVAL);
        match Connection::connect(&root) {
            Ok(connection) => return Ok(connection),
            Err(_) if Instant::now() < deadline => continue,
            Err(error) => {
                let log =
                    crate::diagnostics::log::log_file(&config::ConfigDir::new(&root).logs_dir());
                return Err(io::Error::other(format!(
                    "started {} but it did not answer ({error}); its log is {}",
                    app.display(),
                    log.display()
                )));
            }
        }
    }
}

/// The desktop app: this program, or on Windows the `dowse.exe` beside the
/// console program `dowse.com` (or `dowse-cli.exe`, as Cargo builds it).
fn app_executable() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let stem = exe
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let is_console_program = stem == "dowse-cli"
        || exe
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("com"));
    if !is_console_program {
        return Ok(exe);
    }
    let app = exe.with_file_name(format!("dowse{}", std::env::consts::EXE_SUFFIX));
    if app.is_file() {
        Ok(app)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("cannot find the app at {}", app.display()),
        ))
    }
}

/// Start `app` without windows, detached from this terminal, so it outlives
/// this command.
fn start_in_background(app: &std::path::Path) -> io::Result<()> {
    keep_std_handles_to_ourselves();
    let mut command = Command::new(app);
    command
        .arg(BACKGROUND_FLAG)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Its own process group, so a Ctrl+C in this terminal leaves it be.
        command.process_group(0);
    }
    command.spawn().map(drop)
}

/// Stop programs started from here inheriting this process's standard
/// handles. On Windows a child inherits every inheritable handle, so an app
/// started in the background would hold the pipe a caller reads this
/// command's output from, and the caller would wait for it to quit. Handles
/// given to a child explicitly are unaffected.
pub fn keep_std_handles_to_ourselves() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetStdHandle(handle: u32) -> isize;
            fn SetHandleInformation(object: isize, mask: u32, flags: u32) -> i32;
        }
        const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
        for handle in [-10i32, -11, -12] {
            // SAFETY: no pointers; an invalid handle just fails.
            unsafe {
                SetHandleInformation(GetStdHandle(handle as u32), HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// Let the terminal interpret colour codes. On Windows the console needs to
/// be asked; returns whether it agreed.
pub fn enable_terminal_colors() -> bool {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetStdHandle(handle: u32) -> isize;
            fn GetConsoleMode(console: isize, mode: *mut u32) -> i32;
            fn SetConsoleMode(console: isize, mode: u32) -> i32;
        }
        const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
        const STD_ERROR_HANDLE: u32 = -12i32 as u32;
        const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
        let mut enabled = false;
        for handle in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: `mode` outlives the calls; invalid handles just fail.
            unsafe {
                let console = GetStdHandle(handle);
                let mut mode = 0;
                if GetConsoleMode(console, &mut mode) != 0 {
                    enabled |=
                        SetConsoleMode(console, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0;
                }
            }
        }
        enabled
    }
    #[cfg(not(windows))]
    {
        true
    }
}
