//! The window's side of repositories: which ones it uses, their tag inputs,
//! and the search scope. The repositories themselves live in the shared
//! [`RepoHub`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::*;

use super::app::SearchApp;
use super::hub::{HubEvent, IndexJob, RepoHub, RepoView};
use super::windows::Windows;
use dowse::engine::repo::{self, RepoInfo};
use dowse::engine::search::SearchSource;

/// A repository's tag input on the repositories page.
pub(super) struct TagInput {
    pub(super) input: Entity<InputState>,
    _subscription: Subscription,
}

impl SearchApp {
    // ----- membership ----------------------------------------------------------

    /// Use exactly the repositories `ids`, in that order: acquire the new ones
    /// before releasing the old, so repositories in both stay open.
    pub(super) fn set_members(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        let mut members: Vec<String> = Vec::new();
        for id in ids {
            if !members.contains(&id) {
                members.push(id);
            }
        }
        let old = std::mem::replace(&mut self.members, members);
        let members = self.members.clone();
        self.hub.update(cx, |hub, cx| {
            for id in members.iter().filter(|id| !old.contains(id)) {
                hub.acquire(id, cx);
            }
            for id in old.iter().filter(|id| !members.contains(id)) {
                hub.release(id, cx);
            }
        });
        self.prefer_scope(cx);
        cx.notify();
    }

    /// Add every repository found at `paths` (see [`repo::discover`]).
    pub(super) fn add_repositories(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let registered = self.hub.update(cx, |hub, _| hub.register(&paths, true));
        let ids = match registered {
            Ok(ids) => ids,
            Err(error) => {
                window.push_notification(
                    Notification::error(format!("Could not save repositories: {error:#}")),
                    cx,
                );
                return;
            }
        };
        let added: Vec<String> = ids
            .into_iter()
            .filter(|id| !self.members.contains(id))
            .collect();
        if added.is_empty() {
            window.push_notification("Those repositories are already in this workspace.", cx);
            return;
        }
        let message = match added.as_slice() {
            [one] => format!("Added {one}"),
            many => format!("Added {} repositories", many.len()),
        };
        let mut members = self.members.clone();
        members.extend(added);
        self.set_members(members, cx);
        self.sync_tag_inputs(window, cx);
        self.persist_members(window, cx);
        window.push_notification(message, cx);
    }

    pub(super) fn prompt_for_repositories(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.add_repositories(paths, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Take a repository out of this workspace. Its index and tags are kept.
    pub(super) fn remove_repository(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remove_ids(&[id.to_string()], window, cx);
    }

    /// Take the repositories at `paths` out of this workspace: each folder,
    /// and the repositories directly inside it.
    pub(super) fn remove_folders(
        &mut self,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut ids: Vec<String> = Vec::new();
        for path in paths {
            let mut folders = vec![path.clone()];
            if path.is_dir() {
                folders.extend(repo::discover(path));
            }
            ids.extend(
                folders
                    .iter()
                    .map(|folder| repo::identity(folder).to_string_lossy().into_owned()),
            );
        }
        let removed = ids.iter().filter(|id| self.members.contains(id)).count();
        if removed == 0 {
            window.push_notification("Those repositories are not in this workspace.", cx);
            return;
        }
        self.remove_ids(&ids, window, cx);
        window.push_notification(
            format!(
                "Removed {}",
                crate::format::plural(removed, "repository", "repositories")
            ),
            cx,
        );
    }

    fn remove_ids(&mut self, ids: &[String], window: &mut Window, cx: &mut Context<Self>) {
        let members: Vec<String> = self
            .members
            .iter()
            .filter(|member| !ids.contains(member))
            .cloned()
            .collect();
        if members.len() == self.members.len() {
            return;
        }
        self.set_members(members, cx);
        self.sync_tag_inputs(window, cx);
        self.persist_members(window, cx);
        self.schedule_search(false, cx);
    }

    /// Let go of every repository, as the window closes.
    pub(super) fn release_repositories(&mut self, cx: &mut App) {
        let members = std::mem::take(&mut self.members);
        self.hub.update(cx, |hub, cx| {
            for id in &members {
                hub.release(id, cx);
            }
        });
    }

    // ----- reading -------------------------------------------------------------

    /// The window's repositories, sorted by name.
    pub(super) fn repo_views(&self, cx: &App) -> Vec<RepoView> {
        self.hub.read(cx).views(&self.members)
    }

    /// The repositories among `repos` that the scope includes.
    pub(super) fn in_scope<'a>(
        &'a self,
        repos: &'a [RepoView],
    ) -> impl Iterator<Item = &'a RepoView> + 'a {
        repos.iter().filter(|repo| self.scope.includes(&repo.info))
    }

    fn in_scope_ids(&self, cx: &App) -> Vec<String> {
        let hub = self.hub.read(cx);
        self.members
            .iter()
            .filter(|id| hub.info(id).is_some_and(|info| self.scope.includes(&info)))
            .cloned()
            .collect()
    }

    /// Whether searches in this window read the repository.
    fn is_searched(&self, id: &str, cx: &App) -> bool {
        self.members.iter().any(|member| member == id)
            && self
                .hub
                .read(cx)
                .info(id)
                .is_some_and(|info| self.scope.includes(&info))
    }

    /// What a search over the current scope reads, and whether some
    /// repository in scope is still loading.
    pub(super) fn search_sources(&self, cx: &App) -> (Vec<SearchSource>, bool) {
        let ids = self.in_scope_ids(cx);
        self.hub.read(cx).search_sources(&ids)
    }

    // ----- hub events ----------------------------------------------------------

    pub(super) fn on_hub_event(
        &mut self,
        _: &Entity<RepoHub>,
        event: &HubEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            HubEvent::ReleaseCorpus(id) => {
                // A queued search runs again once the new index loads; tabs
                // in the background search again when shown.
                if self.is_searched(id, cx) {
                    let active = self.active_tab;
                    for (index, tab) in self.tabs.iter_mut().enumerate() {
                        if tab.search_task.is_some() {
                            tab.cancel();
                            tab.stale |= index != active;
                        }
                    }
                }
            }
            HubEvent::CorpusChanged(id) => {
                if self.is_searched(id, cx) {
                    self.schedule_search(true, cx);
                }
            }
            HubEvent::MetadataChanged => {
                if !self.members.is_empty() {
                    self.sync_tag_inputs(window, cx);
                    self.prefer_scope(cx);
                    self.schedule_search(true, cx);
                }
            }
        }
    }

    // ----- tags ------------------------------------------------------------------

    /// Give every repository a tag input, and show tags changed elsewhere in
    /// inputs nobody is typing in.
    pub(super) fn sync_tag_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let infos: Vec<Arc<RepoInfo>> = {
            let hub = self.hub.read(cx);
            self.members.iter().filter_map(|id| hub.info(id)).collect()
        };
        self.tag_inputs
            .retain(|id, _| infos.iter().any(|info| &info.id == id));
        for info in infos {
            let text = info.tags.join(" ");
            match self.tag_inputs.get(&info.id) {
                Some(tag_input) => {
                    let input = tag_input.input.clone();
                    let typing = input.read(cx).focus_handle(cx).is_focused(window);
                    let current = repo::parse_tags(&input.read(cx).value());
                    if !typing && current != info.tags {
                        input.update(cx, |input, cx| input.set_value(text, window, cx));
                    }
                }
                None => {
                    let input = cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("Add tags: mirror dev owner:alice project:billing")
                            .default_value(text)
                    });
                    let id = info.id.clone();
                    let subscription =
                        cx.subscribe_in(&input, window, move |this, input, event, window, cx| {
                            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                                let text = input.read(cx).value().to_string();
                                this.set_repository_tags(&id, &text, window, cx);
                            }
                        });
                    self.tag_inputs.insert(
                        info.id.clone(),
                        TagInput {
                            input,
                            _subscription: subscription,
                        },
                    );
                }
            }
        }
    }

    fn set_repository_tags(
        &mut self,
        id: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tags = repo::parse_tags(text);
        if let Some(tag_input) = self.tag_inputs.get(id) {
            let normalized = tags.join(" ");
            tag_input
                .input
                .update(cx, |input, cx| input.set_value(normalized, window, cx));
        }
        let saved = self.hub.update(cx, |hub, cx| hub.set_tags(id, tags, cx));
        if let Err(error) = saved {
            window.push_notification(
                Notification::error(format!("Could not save repositories: {error:#}")),
                cx,
            );
        }
    }

    // ----- scope -----------------------------------------------------------------

    pub(super) fn toggle_scope_tag(&mut self, tag: &str, cx: &mut Context<Self>) {
        self.scope.toggle(tag);
        self.scope_changed(cx);
    }

    pub(super) fn clear_scope(&mut self, cx: &mut Context<Self>) {
        self.scope.clear();
        self.scope_changed(cx);
    }

    fn scope_changed(&mut self, cx: &mut Context<Self>) {
        Windows::save(cx);
        self.prefer_scope(cx);
        self.schedule_search(false, cx);
    }

    /// Have the repositories this window searches indexed first.
    pub(super) fn prefer_scope(&mut self, cx: &mut Context<Self>) {
        let ids = self.in_scope_ids(cx);
        self.hub.update(cx, |hub, _| hub.prefer(ids));
    }

    /// Do `job` to the index of every repository in scope.
    pub(super) fn queue_scope_indexes(&mut self, job: IndexJob, cx: &mut Context<Self>) {
        let ids = self.in_scope_ids(cx);
        self.hub.update(cx, |hub, cx| {
            for id in &ids {
                hub.queue_index(id, job, cx);
            }
        });
    }

    /// Bring one repository's index up to date.
    pub(super) fn queue_index(&mut self, id: &str, cx: &mut Context<Self>) {
        self.hub
            .update(cx, |hub, cx| hub.queue_index(id, IndexJob::Update, cx));
    }
}

/// Tag inputs by repository id.
pub(super) type TagInputs = HashMap<String, TagInput>;
