//! Icons: GPUI Kit's default component set plus the few extra Lucide icons
//! this app uses.

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

icon_assets!(pub ExtraIcons, [GitBranch, Tag, Trash, Plus, FolderGit2, Regex, WholeWord, RefreshCw, FileCode, Funnel, FolderSearch, TextSearch, Layers, Save, AppWindow, FilePlus, SquareArrowOutUpRight, Table, Rows3, Download, Command]);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
