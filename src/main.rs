// Release builds are GUI applications: no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod cli;
mod editor;
mod format;
mod ui;

use cli::Command;

fn main() {
    let cwd = std::env::current_dir().unwrap_or_default();
    let command = match cli::parse(std::env::args_os().skip(1), &cwd) {
        Ok(Command::Help) => {
            print!("{}", cli::USAGE);
            return;
        }
        Ok(command) => command,
        Err(error) => {
            eprintln!("tgrep-gpui: {error}");
            std::process::exit(2);
        }
    };

    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            ui::init(cx);
            ui::start(command, cx);
        });
}
