// Release builds are GUI applications: no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod editor;
mod format;
mod fuzzy;
#[cfg(windows)]
mod shell;
mod ui;

use dowse::engine::config::ConfigDir;
use dowse::ipc::{self, Launch};
use dowse::launch::{self, Command};

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if dowse::cli::wants_cli(args.get(1)) {
        attach_console();
        std::process::exit(dowse::cli::run(args));
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let command = match dowse::cli::parse_launch(args.into_iter().skip(1), &cwd) {
        Ok(Command::Help) => {
            attach_console();
            print!("{}", launch::USAGE);
            return;
        }
        Ok(command) => command,
        Err(error) => {
            attach_console();
            eprintln!("dowse: {error}");
            std::process::exit(2);
        }
    };

    let config_root = dowse::engine::config::default_root();
    if std::env::var_os(launch::TAKE_OVER_ENV).is_some() {
        // SAFETY: no other thread runs yet. The programs this one starts
        // are not taking over.
        unsafe { std::env::remove_var(launch::TAKE_OVER_ENV) };
        ipc::wait_until_gone(&config_root, std::time::Duration::from_secs(30));
    }
    let commands = match ipc::claim(&config_root, &command) {
        Launch::Forwarded => return,
        Launch::Primary(commands) => commands,
    };
    dowse::diagnostics::log::init(&ConfigDir::new(&config_root).logs_dir());
    log::info!(
        "dowse {} starting, pid {}, settings in {}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        config_root.display()
    );

    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            ui::init(config_root, cx);
            ui::start(command, commands, cx);
        });
}

/// Release builds on Windows have no console of their own; write `--help`,
/// errors and subcommands' output to the console that started them. The
/// terminal does not wait for a GUI program, though; `dowse.com` does this
/// properly.
fn attach_console() {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        // SAFETY: no pointers; without a parent console output is just lost.
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}
