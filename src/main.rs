// Release builds are GUI applications: no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod editor;
mod format;
mod ui;

use std::path::PathBuf;

use gpui_kit::component::Root;
use gpui_kit::*;

fn main() {
    // `tgrep-gpui [FOLDER...]` adds each folder (or the git repositories
    // inside it) to the saved repositories.
    let folders: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();

    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            ui::init(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1280.), px(820.)),
                    cx,
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some("tgrep".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(720.), px(480.))),
                ..Default::default()
            };
            cx.spawn(async move |cx| {
                cx.open_window(options, |window, cx| {
                    let view = cx.new(|cx| ui::SearchApp::new(folders, window, cx));
                    // `Root` hosts notifications, dialogs and tooltips.
                    cx.new(|cx| Root::new(view, window, cx))
                })
                .expect("failed to open the main window");
                cx.update(|cx| cx.activate(true));
            })
            .detach();
        });
}
