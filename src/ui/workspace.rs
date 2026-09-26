//! A window's workspace: untitled or saved to a `.tgrep-workspace` file,
//! switching to another one, saving it, and asking before an untitled one
//! with repositories is thrown away.

use std::path::{Path, PathBuf};

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::*;

use super::app::SearchApp;
use super::windows::{Opening, Windows};
use tgrep_gpui::engine::repo::{self, Scope};
use tgrep_gpui::engine::session::WindowSession;
use tgrep_gpui::engine::workspace;

impl SearchApp {
    /// The saved workspace this window shows, or `None` for an untitled one.
    pub(super) fn workspace_file(&self) -> Option<&Path> {
        self.workspace.as_deref()
    }

    pub(super) fn workspace_name(&self) -> String {
        match &self.workspace {
            Some(file) => workspace::name(file),
            None => "Untitled Workspace".into(),
        }
    }

    /// What the session remembers about this window.
    pub(super) fn session_state(&self) -> WindowSession {
        WindowSession {
            workspace: self.workspace.clone(),
            repos: match self.workspace {
                Some(_) => Vec::new(),
                None => self.members.iter().map(PathBuf::from).collect(),
            },
            scope: self.scope.tags().map(str::to_string).collect(),
        }
    }

    /// An untitled workspace that would be lost if the window closed.
    fn has_unsaved_repositories(&self) -> bool {
        self.workspace.is_none() && !self.members.is_empty()
    }

    // ----- switching -----------------------------------------------------------
    /// Show `opening` in this window, replacing the current workspace.
    pub(super) fn show_workspace(
        &mut self,
        opening: Opening,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.apply_workspace(opening, window, cx) {
            window.push_notification(Notification::error(error), cx);
        }
    }

    /// Switch to `opening`. Returns why a saved workspace could not be
    /// opened, in which case nothing changes.
    pub(super) fn apply_workspace(
        &mut self,
        opening: Opening,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let (file, repos, scope) = match opening {
            Opening::Untitled { repos, scope } => (None, repos, scope),
            Opening::Saved(file) => {
                let file = repo::identity(&file);
                match self.read_workspace(&file, cx) {
                    Ok(repos) => {
                        Windows::remember(&file, cx);
                        let scope = Windows::saved_scope(&file, cx);
                        (Some(file), repos, scope)
                    }
                    Err(error) => {
                        Windows::forget(&file, cx);
                        return Err(format!("Could not open {}: {error:#}", file.display()));
                    }
                }
            }
        };
        self.workspace = file.clone();
        Windows::set_workspace(window.window_handle(), file, cx);
        self.scope = Scope::new(scope);
        self.set_members(repos, cx);
        self.reset_search(cx);
        self.sync_tag_inputs(window, cx);
        self.update_title(window);
        self.schedule_search(false, cx);
        Ok(())
    }

    fn read_workspace(&self, file: &Path, cx: &mut Context<Self>) -> anyhow::Result<Vec<String>> {
        let paths = workspace::load(file)?;
        self.hub.update(cx, |hub, _| hub.register(&paths, false))
    }

    pub(super) fn update_title(&self, window: &mut Window) {
        window.set_window_title(&format!("{} — tgrep", self.workspace_name()));
    }

    /// Write the repository list where this workspace keeps it: its file, or
    /// the session for an untitled one.
    pub(super) fn persist_members(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(file) = &self.workspace {
            let paths: Vec<PathBuf> = self.members.iter().map(PathBuf::from).collect();
            if let Err(error) = workspace::save(file, &paths) {
                window.push_notification(
                    Notification::error(format!("Could not save the workspace: {error:#}")),
                    cx,
                );
            }
        }
        Windows::save(cx);
    }

    // ----- commands ------------------------------------------------------------

    /// Folders dropped on the window join its workspace; a dropped workspace
    /// file opens.
    pub(super) fn drop_paths(
        &mut self,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (files, folders): (Vec<PathBuf>, Vec<PathBuf>) = paths
            .iter()
            .cloned()
            .partition(|path| workspace::is_workspace_file(path));
        let folders: Vec<PathBuf> = folders.into_iter().filter(|path| path.is_dir()).collect();
        if !folders.is_empty() {
            self.add_repositories(folders, window, cx);
        }
        if let Some(file) = files.into_iter().next() {
            self.open_workspace_file(file, window, cx);
        }
    }

    pub(super) fn prompt_open_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(file) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.open_workspace_file(file, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Open a saved workspace in this window, or focus the window already
    /// showing it.
    pub(super) fn open_workspace_file(
        &mut self,
        file: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let file = repo::identity(&file);
        if let Some(handle) = Windows::window_for(&file, cx) {
            if handle != window.window_handle() {
                handle
                    .update(cx, |_, window, _| window.activate_window())
                    .ok();
            }
            return;
        }
        let keep = self.confirm_discard(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if !keep.await {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                this.show_workspace(Opening::Saved(file), window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Start a new, empty untitled workspace in this window.
    pub(super) fn new_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keep = self.confirm_discard(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if !keep.await {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                this.show_workspace(Opening::empty(), window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Ask for a file and save the workspace there. Resolves to whether it
    /// was saved.
    pub(super) fn save_workspace_as(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let directory = match &self.workspace {
            Some(file) => file.parent().map(Path::to_path_buf).unwrap_or_default(),
            None => {
                let dir = Windows::workspaces_dir(cx);
                std::fs::create_dir_all(&dir).ok();
                dir
            }
        };
        let suggested = match &self.workspace {
            Some(file) => workspace::name(file),
            None => "Workspace".into(),
        };
        let chosen = cx.prompt_for_new_path(
            &directory,
            Some(&format!("{suggested}.{}", workspace::EXTENSION)),
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(file))) = chosen.await else {
                return false;
            };
            this.update_in(cx, |this, window, cx| {
                this.save_workspace_to(file, window, cx)
            })
            .unwrap_or(false)
        })
    }

    fn save_workspace_to(
        &mut self,
        file: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let file = if workspace::is_workspace_file(&file) {
            file
        } else {
            let mut named = file.into_os_string();
            named.push(format!(".{}", workspace::EXTENSION));
            PathBuf::from(named)
        };
        let paths: Vec<PathBuf> = self.members.iter().map(PathBuf::from).collect();
        if let Err(error) = workspace::save(&file, &paths) {
            window.push_notification(
                Notification::error(format!("Could not save the workspace: {error:#}")),
                cx,
            );
            return false;
        }
        let file = repo::identity(&file);
        window.push_notification(format!("Saved {}", file.display()), cx);
        self.workspace = Some(file.clone());
        Windows::set_workspace(window.window_handle(), Some(file.clone()), cx);
        self.update_title(window);
        Windows::remember(&file, cx);
        cx.notify();
        true
    }

    /// Before an untitled workspace with repositories is replaced, ask whether
    /// to save it. Resolves to whether to go ahead.
    pub(super) fn confirm_discard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        if !self.has_unsaved_repositories() {
            return Task::ready(true);
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "Save the untitled workspace?",
            Some("Its repositories are forgotten if you don't save it. Their indexes and tags are kept."),
            &["Save…", "Don't Save", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| match answer.await {
            Ok(0) => {
                let Ok(saved) =
                    this.update_in(cx, |this, window, cx| this.save_workspace_as(window, cx))
                else {
                    return false;
                };
                saved.await
            }
            Ok(1) => true,
            _ => false,
        })
    }

    /// Whether the window may close now. Closing the last window quits, and
    /// the session keeps every workspace, so only a window closing while
    /// others stay open asks about an unsaved workspace.
    pub(super) fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.close_confirmed || Windows::count(cx) <= 1 || !self.has_unsaved_repositories() {
            return true;
        }
        let keep = self.confirm_discard(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if !keep.await {
                return;
            }
            this.update_in(cx, |this, window, _| {
                this.close_confirmed = true;
                window.remove_window();
            })
            .ok();
        })
        .detach();
        false
    }
}
