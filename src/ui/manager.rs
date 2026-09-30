//! The repositories page's state and behaviour: its sections, filtering and
//! selecting the workspace's repositories to act on several at once, and
//! listing an owner's GitHub repositories to clone. Rendering is in
//! `repos_page.rs`; the clones and pulls themselves run in [`TaskHub`].

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::*;

use super::app::{Page, SearchApp};
use super::hub::{IndexJob, RepoView};
use super::tasks::{CloneJob, TaskHub};
use super::windows::Windows;
use crate::format;
use dowse::engine::github::{self, CloneMode, ListProgress, RemoteFilter, RemoteRepo};
use dowse::engine::repo;
use dowse::engine::session::CloneDefaults;
use dowse::engine::settings::{IndexLocation, IndexSettings};
use dowse::engine::sync::Interval;
use dowse::engine::tasks::{TaskKind, TaskState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Section {
    Workspace,
    GitHub,
    Tasks,
    Settings,
}

impl Section {
    pub(super) const ALL: [Section; 4] = [
        Section::Workspace,
        Section::GitHub,
        Section::Tasks,
        Section::Settings,
    ];
}

/// The repositories page of one window.
pub(super) struct Manager {
    pub(super) section: Section,
    /// Filters the workspace's repositories.
    pub(super) filter: Entity<InputState>,
    /// Workspace repositories selected for a bulk action, by id.
    pub(super) selected: BTreeSet<String>,
    /// The repository whose details are shown.
    pub(super) expanded: Option<String>,
    /// Tags to add to (or, with `-`, remove from) the selected repositories.
    pub(super) bulk_tags: Entity<InputState>,
    /// The workspace's repositories as a virtualized list: the rows the
    /// filter keeps, then the notes below them.
    pub(super) list: ListState,
    /// The rows of the last render, which the list draws from.
    pub(super) shown: Vec<RepoView>,
    pub(super) shown_ids: Vec<String>,
    /// Whether each repository is a git repository, by id: asked of the
    /// disk once, not on every frame.
    git: RefCell<HashMap<String, bool>>,
    /// Explorer's menu for dowse, read from the registry once.
    #[cfg(windows)]
    explorer: RefCell<Option<(PathBuf, crate::shell::State)>>,
    pub(super) github: GithubPanel,
    _subscriptions: Vec<Subscription>,
}

/// Listing an owner's repositories on GitHub and choosing some to clone.
pub(super) struct GithubPanel {
    pub(super) owner: Entity<InputState>,
    pub(super) filter: Entity<InputState>,
    pub(super) destination: Entity<InputState>,
    pub(super) tags: Entity<InputState>,
    pub(super) mode: CloneMode,
    pub(super) pull_every: Option<Interval>,
    pub(super) forks: bool,
    pub(super) archived: bool,
    pub(super) listing: Listing,
    /// Chosen repositories, by `owner/name`.
    pub(super) selected: BTreeSet<String>,
    /// Indexes into the listing that the filter keeps.
    pub(super) visible: Vec<usize>,
    pub(super) scroll: UniformListScrollHandle,
    /// The signed-in user, once known.
    pub(super) me: Option<String>,
    load_task: Option<Task<()>>,
    /// Stops the listing running.
    cancel: Option<Arc<AtomicBool>>,
}

/// An owner's repositories on GitHub, as far as they are listed.
#[derive(Default)]
pub(super) struct Listing {
    pub(super) state: ListingState,
    /// Whose they are: as typed until GitHub says.
    pub(super) owner: String,
    /// Most recently pushed first.
    pub(super) repos: Vec<RemoteRepo>,
    pub(super) progress: ListProgress,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum ListingState {
    #[default]
    NotLoaded,
    Loading,
    Loaded,
    /// Stopped before the end; `repos` holds what came before.
    Stopped,
    Failed(String),
}

impl Listing {
    pub(super) fn repos(&self) -> &[RemoteRepo] {
        &self.repos
    }

    pub(super) fn is_loading(&self) -> bool {
        self.state == ListingState::Loading
    }
}

/// What a listing running in the background says.
enum ListingEvent {
    Me(String),
    Page(Vec<RemoteRepo>, ListProgress),
    /// The error, when it failed.
    Done(Option<String>),
}

impl GithubPanel {
    /// Stop the listing running, if one is.
    fn stop_listing(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.load_task = None;
    }
}

impl Drop for GithubPanel {
    fn drop(&mut self) {
        self.stop_listing();
    }
}

/// Where a GitHub repository stands on this machine.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum RemoteStatus {
    Available,
    /// Its folder under the destination is a git repository already.
    Cloned,
    Queued,
    Cloning(Option<f32>),
    Failed(String),
}

impl Manager {
    pub(super) fn new(window: &mut Window, cx: &mut Context<SearchApp>) -> Self {
        let defaults = Windows::clone_defaults(cx);
        let input =
            |placeholder: &str, value: String, window: &mut Window, cx: &mut Context<SearchApp>| {
                let placeholder = placeholder.to_string();
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value)
                })
            };
        let filter = input(
            "Filter by name, path, branch or tag",
            String::new(),
            window,
            cx,
        );
        let bulk_tags = input("Tags to add, or -tag to remove", String::new(), window, cx);
        let owner = input(
            "Owner, or empty for your own",
            defaults.owner.clone().unwrap_or_default(),
            window,
            cx,
        );
        let remote_filter = input(
            "Filter by name, description or language",
            String::new(),
            window,
            cx,
        );
        let destination = input(
            "Folder to clone into, as <folder>/<owner>/<name>",
            defaults
                .root
                .as_ref()
                .map(|root| root.display().to_string())
                .unwrap_or_default(),
            window,
            cx,
        );
        let tags = input(
            "Tags for the clones, e.g. mirror",
            String::new(),
            window,
            cx,
        );

        let subscriptions = vec![
            cx.subscribe_in(&filter, window, |_, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&bulk_tags, window, |this, input, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let text = input.read(cx).value().to_string();
                    this.tag_selected(&text, window, cx);
                }
            }),
            cx.subscribe_in(&owner, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.load_github(window, cx);
                }
            }),
            cx.subscribe_in(&remote_filter, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refilter_github(cx);
                }
            }),
        ];
        Self {
            section: Section::Workspace,
            filter,
            selected: BTreeSet::new(),
            expanded: None,
            bulk_tags,
            list: ListState::new(0, ListAlignment::Top, px(600.)),
            shown: Vec::new(),
            shown_ids: Vec::new(),
            git: RefCell::default(),
            #[cfg(windows)]
            explorer: RefCell::default(),
            github: GithubPanel {
                owner,
                filter: remote_filter,
                destination,
                tags,
                mode: defaults.mode,
                pull_every: None,
                forks: false,
                archived: false,
                listing: Listing::default(),
                selected: BTreeSet::new(),
                visible: Vec::new(),
                scroll: UniformListScrollHandle::new(),
                me: None,
                load_task: None,
                cancel: None,
            },
            _subscriptions: subscriptions,
        }
    }
}

impl Manager {
    /// Whether the repository `id` is a git repository.
    pub(super) fn is_git(&self, id: &str) -> bool {
        *self
            .git
            .borrow_mut()
            .entry(id.to_string())
            .or_insert_with(|| std::path::Path::new(id).join(".git").exists())
    }

    /// This program, and whether Explorer's menu starts it.
    #[cfg(windows)]
    pub(super) fn explorer_state(&self) -> Option<(PathBuf, crate::shell::State)> {
        let mut cached = self.explorer.borrow_mut();
        if cached.is_none() {
            let exe = std::env::current_exe().ok()?;
            let state = crate::shell::state(&exe);
            *cached = Some((exe, state));
        }
        cached.clone()
    }

    #[cfg(windows)]
    pub(super) fn forget_explorer_state(&self) {
        self.explorer.borrow_mut().take();
    }

    /// Look at the disk and registry again, as when the page is shown.
    pub(super) fn forget_disk_state(&self) {
        self.git.borrow_mut().clear();
        #[cfg(windows)]
        self.forget_explorer_state();
    }
}

impl SearchApp {
    pub(super) fn show_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.manager.section = section;
        self.manager.forget_disk_state();
        if section == Section::GitHub
            && self.manager.github.listing.state == ListingState::NotLoaded
        {
            self.start_loading_github(cx);
        }
        cx.notify();
    }

    /// Show the repositories page at `section`.
    pub(super) fn open_section(
        &mut self,
        section: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_page(Page::Repositories, window, cx);
        self.show_section(section, cx);
    }

    // ----- the workspace's repositories ---------------------------------------------

    /// Pull the repositories the scope includes.
    pub(super) fn pull_scope(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repos = self.repo_views(cx);
        let ids: Vec<String> = self
            .in_scope(&repos)
            .map(|repo| repo.info.id.clone())
            .collect();
        self.pull_repos(ids, window, cx);
    }

    /// The workspace's repositories the filter keeps, sorted by name.
    pub(super) fn managed_repos(&self, cx: &App) -> Vec<RepoView> {
        let text = self.manager.filter.read(cx).value().to_lowercase();
        let words: Vec<&str> = text.split_whitespace().collect();
        self.repo_views(cx)
            .into_iter()
            .filter(|view| {
                let haystack = format!(
                    "{} {} {}",
                    view.info.name,
                    view.info.id,
                    view.info.effective_tags().join(" ")
                )
                .to_lowercase();
                words.iter().all(|word| haystack.contains(word))
            })
            .collect()
    }

    /// The selected repositories still in the workspace.
    pub(super) fn selected_ids(&self) -> Vec<String> {
        self.manager
            .selected
            .iter()
            .filter(|id| self.members.contains(id))
            .cloned()
            .collect()
    }

    pub(super) fn toggle_selected(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.manager.selected.remove(id) {
            self.manager.selected.insert(id.to_string());
        }
        cx.notify();
    }

    /// Select every repository the filter keeps, or none.
    pub(super) fn select_all_shown(&mut self, select: bool, cx: &mut Context<Self>) {
        if select {
            let ids: Vec<String> = self
                .managed_repos(cx)
                .into_iter()
                .map(|view| view.info.id.clone())
                .collect();
            self.manager.selected.extend(ids);
        } else {
            self.manager.selected.clear();
        }
        cx.notify();
    }

    pub(super) fn toggle_expanded_repo(&mut self, id: &str, cx: &mut Context<Self>) {
        self.manager.expanded = match &self.manager.expanded {
            Some(open) if open == id => None,
            _ => Some(id.to_string()),
        };
        cx.notify();
    }

    pub(super) fn pull_repos(
        &mut self,
        ids: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pullable: Vec<String> = ids
            .into_iter()
            .filter(|id| std::path::Path::new(id).join(".git").exists())
            .collect();
        if pullable.is_empty() {
            window.push_notification("Only git repositories can be pulled.", cx);
            return;
        }
        let count = pullable.len();
        TaskHub::global(cx).update(cx, |tasks, cx| tasks.pull(&pullable, cx));
        if count > 1 {
            window.push_notification(
                format!(
                    "Pulling {}",
                    format::plural(count, "repository", "repositories")
                ),
                cx,
            );
        }
    }

    pub(super) fn index_repos(&mut self, ids: &[String], job: IndexJob, cx: &mut Context<Self>) {
        self.hub.update(cx, |hub, cx| {
            for id in ids {
                hub.queue_index(id, job, cx);
            }
        });
    }

    pub(super) fn set_pull_every(
        &mut self,
        ids: &[String],
        every: Option<Interval>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self.hub.update(cx, |hub, cx| {
            ids.iter()
                .try_for_each(|id| hub.set_pull_every(id, every, cx))
        });
        if let Err(error) = result {
            window.push_notification(
                Notification::error(format!("Could not save repositories: {error:#}")),
                cx,
            );
        }
    }

    /// Keep the repositories' indexes at `location`, or where the app's
    /// setting says with `None`; those that change place move.
    pub(super) fn set_index_location(
        &mut self,
        ids: &[String],
        location: Option<IndexLocation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self
            .hub
            .update(cx, |hub, cx| hub.set_index_location(ids, location, cx));
        if let Err(error) = result {
            window.push_notification(
                Notification::error(format!("Could not save repositories: {error:#}")),
                cx,
            );
        }
    }

    /// Change where indexes are kept; those that change place move.
    pub(super) fn set_index_settings(
        &mut self,
        settings: IndexSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self
            .hub
            .update(cx, |hub, cx| hub.set_index_settings(settings, cx));
        if let Err(error) = result {
            window.push_notification(
                Notification::error(format!("Could not save the settings: {error:#}")),
                cx,
            );
        }
    }

    /// Pick the folder external indexes go under.
    pub(super) fn browse_external_index_dir(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Keep Indexes Here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let mut settings = this.hub.read(cx).index_settings().clone();
                settings.external_dir = Some(path);
                this.set_index_settings(settings, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Add the tags in `text` to the selected repositories; `-tag` removes one.
    fn tag_selected(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selected_ids();
        let (removed, added): (Vec<String>, Vec<String>) = repo::parse_tags(text)
            .into_iter()
            .partition(|tag| tag.starts_with('-'));
        let removed: Vec<String> = removed
            .iter()
            .map(|tag| tag.trim_start_matches('-').to_string())
            .filter(|tag| !tag.is_empty())
            .collect();
        if ids.is_empty() || (added.is_empty() && removed.is_empty()) {
            return;
        }
        let result = self.hub.update(cx, |hub, cx| {
            ids.iter().try_for_each(|id| {
                let mut tags = hub.library_tags(id);
                tags.retain(|tag| !removed.contains(tag));
                for tag in &added {
                    if !tags.contains(tag) {
                        tags.push(tag.clone());
                    }
                }
                hub.set_tags(id, tags, cx)
            })
        });
        match result {
            Ok(()) => {
                self.manager
                    .bulk_tags
                    .update(cx, |input, cx| input.set_value("", window, cx));
                window.push_notification(
                    format!(
                        "Tagged {}",
                        format::plural(ids.len(), "repository", "repositories")
                    ),
                    cx,
                );
            }
            Err(error) => window.push_notification(
                Notification::error(format!("Could not save repositories: {error:#}")),
                cx,
            ),
        }
    }

    pub(super) fn apply_bulk_tags(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.manager.bulk_tags.read(cx).value().to_string();
        self.tag_selected(&text, window, cx);
    }

    pub(super) fn remove_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selected_ids();
        if ids.is_empty() {
            return;
        }
        let count = ids.len();
        for id in &ids {
            self.manager.selected.remove(id);
        }
        self.remove_ids(&ids, window, cx);
        window.push_notification(
            format!(
                "Removed {} from this workspace",
                format::plural(count, "repository", "repositories")
            ),
            cx,
        );
    }

    // ----- GitHub ---------------------------------------------------------------------

    pub(super) fn load_github(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.start_loading_github(cx);
    }

    fn start_loading_github(&mut self, cx: &mut Context<Self>) {
        let owner = self
            .manager
            .github
            .owner
            .read(cx)
            .value()
            .trim()
            .to_string();
        let owner = (!owner.is_empty()).then_some(owner);
        let github = &mut self.manager.github;
        github.stop_listing();
        let cancel = Arc::new(AtomicBool::new(false));
        github.cancel = Some(cancel.clone());
        github.listing = Listing {
            state: ListingState::Loading,
            owner: owner.clone().unwrap_or_else(|| "your account".into()),
            ..Listing::default()
        };
        github.selected.clear();
        github.visible.clear();
        let need_me = github.me.is_none();
        let mut defaults = Windows::clone_defaults(cx);
        defaults.owner = owner.clone();
        Windows::set_clone_defaults(defaults, cx);

        let (sender, received) = async_channel::unbounded();
        if need_me {
            let sender = sender.clone();
            cx.background_spawn(async move {
                if let Ok(me) = github::current_user() {
                    sender.send_blocking(ListingEvent::Me(me)).ok();
                }
            })
            .detach();
        }
        cx.background_spawn(async move {
            let listed = github::list_each(owner.as_deref(), None, &cancel, |repos, progress| {
                sender
                    .send_blocking(ListingEvent::Page(repos, progress))
                    .ok();
            });
            sender
                .send_blocking(ListingEvent::Done(
                    listed.err().map(|error| format!("{error:#}")),
                ))
                .ok();
        })
        .detach();
        // Dropping this (for another listing, or the window) stops listening;
        // `cancel` stops the listing itself.
        self.manager.github.load_task = Some(cx.spawn(async move |this, cx| {
            while let Ok(event) = received.recv().await {
                // Take whatever else has arrived too, and show it at once.
                let mut events = vec![event];
                while let Ok(event) = received.try_recv() {
                    events.push(event);
                }
                let shown = this.update(cx, |this, cx| {
                    for event in events {
                        this.on_listing_event(event);
                    }
                    this.refilter_github(cx);
                });
                if shown.is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn on_listing_event(&mut self, event: ListingEvent) {
        let github = &mut self.manager.github;
        let listing = &mut github.listing;
        match event {
            ListingEvent::Me(me) => github.me = Some(me),
            ListingEvent::Page(repos, progress) => {
                if listing.state != ListingState::Loading {
                    return;
                }
                if let Some(first) = repos.first().filter(|_| listing.repos.is_empty()) {
                    listing.owner = first.owner.clone();
                }
                listing.repos.extend(repos);
                github::sort_by_pushed(&mut listing.repos);
                listing.progress = progress;
            }
            ListingEvent::Done(error) => {
                if listing.state != ListingState::Loading {
                    return;
                }
                if listing.repos.is_empty()
                    && let Some(me) = &github.me
                    && listing.owner == "your account"
                {
                    listing.owner = me.clone();
                }
                listing.state = match error {
                    None => ListingState::Loaded,
                    Some(error) => ListingState::Failed(error),
                };
                github.cancel = None;
            }
        }
    }

    /// Stop listing, keeping the repositories listed so far.
    pub(super) fn stop_github(&mut self, cx: &mut Context<Self>) {
        let github = &mut self.manager.github;
        if github.listing.state == ListingState::Loading {
            github.stop_listing();
            github.listing.state = ListingState::Stopped;
            cx.notify();
        }
    }

    pub(super) fn remote_filter(&self, cx: &App) -> RemoteFilter {
        RemoteFilter {
            text: self.manager.github.filter.read(cx).value().to_string(),
            forks: self.manager.github.forks,
            archived: self.manager.github.archived,
        }
    }

    pub(super) fn refilter_github(&mut self, cx: &mut Context<Self>) {
        let filter = self.remote_filter(cx);
        let github = &mut self.manager.github;
        github.visible = github
            .listing
            .repos()
            .iter()
            .enumerate()
            .filter(|(_, repo)| filter.matches(repo))
            .map(|(index, _)| index)
            .collect();
        cx.notify();
    }

    pub(super) fn set_remote_option(
        &mut self,
        forks: Option<bool>,
        archived: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        if let Some(forks) = forks {
            self.manager.github.forks = forks;
        }
        if let Some(archived) = archived {
            self.manager.github.archived = archived;
        }
        self.refilter_github(cx);
    }

    pub(super) fn toggle_remote(&mut self, full_name: &str, cx: &mut Context<Self>) {
        let selected = &mut self.manager.github.selected;
        if !selected.remove(full_name) {
            selected.insert(full_name.to_string());
        }
        cx.notify();
    }

    /// Select every repository shown that can be cloned, or none.
    pub(super) fn select_all_remote(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select {
            self.manager.github.selected.clear();
            cx.notify();
            return;
        }
        let statuses = self.remote_statuses(cx);
        let root = self.destination_root(cx);
        let github = &self.manager.github;
        let repos = github.listing.repos();
        let names: Vec<String> = github
            .visible
            .iter()
            .map(|index| &repos[*index])
            .filter(|repo| {
                matches!(
                    self.remote_status(repo, &statuses, root.as_ref()),
                    RemoteStatus::Available | RemoteStatus::Failed(_)
                )
            })
            .map(|repo| repo.full_name.clone())
            .collect();
        self.manager.github.selected.extend(names);
        cx.notify();
    }

    /// The folder typed as the clones' destination.
    pub(super) fn destination_root(&self, cx: &App) -> Option<PathBuf> {
        let text = self
            .manager
            .github
            .destination
            .read(cx)
            .value()
            .trim()
            .to_string();
        (!text.is_empty()).then(|| PathBuf::from(text))
    }

    pub(super) fn browse_destination(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Clone Here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.manager.github.destination.update(cx, |input, cx| {
                    input.set_value(path.display().to_string(), window, cx)
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Clone tasks' states, by `owner/name`: the latest task for each.
    pub(super) fn remote_statuses(&self, cx: &App) -> HashMap<String, RemoteStatus> {
        let mut statuses = HashMap::new();
        for info in TaskHub::global(cx).read(cx).infos() {
            if info.kind != TaskKind::Clone {
                continue;
            }
            let status = match &info.state {
                TaskState::Queued => RemoteStatus::Queued,
                TaskState::Running => RemoteStatus::Cloning(info.fraction),
                TaskState::Done { .. } => RemoteStatus::Cloned,
                TaskState::Failed { error } => RemoteStatus::Failed(error.clone()),
                TaskState::Cancelled => continue,
            };
            statuses.insert(info.key.clone(), status);
        }
        statuses
    }

    /// Where `repo` stands: being cloned, cloned, or neither.
    pub(super) fn remote_status(
        &self,
        repo: &RemoteRepo,
        statuses: &HashMap<String, RemoteStatus>,
        root: Option<&PathBuf>,
    ) -> RemoteStatus {
        if let Some(status) = statuses.get(&repo.full_name)
            && *status != RemoteStatus::Cloned
        {
            return status.clone();
        }
        let present = root.is_some_and(|root| {
            let folder = github::clone_destination(root, &repo.full_name);
            self.manager.is_git(&folder.to_string_lossy())
        });
        if present || statuses.get(&repo.full_name) == Some(&RemoteStatus::Cloned) {
            RemoteStatus::Cloned
        } else {
            RemoteStatus::Available
        }
    }

    /// Queue clones of the chosen repositories. Ones already on disk join the
    /// library and workspace at once.
    pub(super) fn clone_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.destination_root(cx) else {
            window.push_notification(
                Notification::error("Choose a folder to clone into first."),
                cx,
            );
            return;
        };
        let github = &self.manager.github;
        let chosen: Vec<RemoteRepo> = github
            .listing
            .repos()
            .iter()
            .filter(|repo| github.selected.contains(&repo.full_name))
            .cloned()
            .collect();
        if chosen.is_empty() {
            return;
        }
        let extra_tags = repo::parse_tags(&github.tags.read(cx).value());
        let (mode, pull_every) = (github.mode, github.pull_every);
        Windows::set_clone_defaults(
            CloneDefaults {
                root: Some(root.clone()),
                mode,
                owner: Windows::clone_defaults(cx).owner,
            },
            cx,
        );

        let target = Some((window.window_handle(), cx.entity().downgrade()));
        let mut jobs = Vec::new();
        let mut present = Vec::new();
        for remote in &chosen {
            let destination = github::clone_destination(&root, &remote.full_name);
            let mut tags = vec![format!("owner:{}", remote.owner)];
            tags.extend(extra_tags.iter().cloned());
            if destination.join(".git").exists() {
                present.push((destination, tags));
                continue;
            }
            jobs.push(CloneJob {
                full_name: remote.full_name.clone(),
                destination,
                mode,
                tags,
                pull_every,
                window: target.clone(),
            });
        }
        let mut adopted = Vec::new();
        for (folder, tags) in present {
            let result = self
                .hub
                .update(cx, |hub, cx| hub.adopt(&folder, &tags, pull_every, cx));
            match result {
                Ok(id) => adopted.push(id),
                Err(error) => log::warn!("could not add {}: {error:#}", folder.display()),
            }
        }
        let already = adopted.len();
        if !adopted.is_empty() {
            self.adopt_repositories(adopted, window, cx);
        }
        let queued = TaskHub::global(cx).update(cx, |tasks, cx| tasks.clone_repos(jobs, cx));
        self.manager.github.selected.clear();
        let mut message = match queued.len() {
            0 => String::new(),
            n => format!(
                "Cloning {}",
                format::plural(n, "repository", "repositories")
            ),
        };
        if already > 0 {
            if !message.is_empty() {
                message.push_str("; ");
            }
            message.push_str(&format!(
                "added {} already on disk",
                format::plural(already, "repository", "repositories")
            ));
        }
        if !message.is_empty() {
            window.push_notification(message, cx);
        }
        cx.notify();
    }
}
