//! Icons: GPUI Kit's default component set plus the few extra Lucide icons
//! this app uses, and dowse's own mark.

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

icon_assets!(pub ExtraIcons, [GitBranch, Tag, Trash, Plus, FolderGit2, Regex, WholeWord, RefreshCw, FileCode, Funnel, FolderSearch, TextSearch, Layers, Save, AppWindow, FilePlus, SquareArrowOutUpRight, Table, Rows3, Download, Command, Bug, ClipboardCopy, ListChecks, CloudDownload, Hammer, Timer, MousePointerClick, CircleStop, ListX, GitFork, Archive, Lock, Users, GitCommitHorizontal]);

/// dowse's mark, a circle cut at the waterline. Drawn in the text colour,
/// like the icons.
pub const BRAND_MARK: &str = "brand/dowse.svg";

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == BRAND_MARK {
            return Ok(Some(Cow::Borrowed(include_bytes!("../assets/dowse.svg"))));
        }
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        if BRAND_MARK.starts_with(path) {
            paths.push(BRAND_MARK.into());
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
