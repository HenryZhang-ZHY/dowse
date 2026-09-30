//! Running background tasks, shared by every window: clones and pulls, a
//! few at a time each, on threads of their own so a large clone never holds
//! up anything else. Also pulls the repositories that ask for it on their
//! interval. The books are kept by [`TaskList`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::*;

use super::app::SearchApp;
use super::hub::{IndexJob, RepoHub};
use dowse::engine::github::{self, CloneMode};
use dowse::engine::settings::TaskSettings;
use dowse::engine::sync::{self, Interval, PullOutcome};
use dowse::engine::tasks::{Started, TaskInfo, TaskKind, TaskList, TaskState};

/// How often repositories are checked for a pull that is due.
const SCHEDULE_INTERVAL: Duration = Duration::from_secs(60);
/// The first check after starting, once windows have opened.
const FIRST_SCHEDULE: Duration = Duration::from_secs(20);
/// Progress is passed on at most this often.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

/// What a task does.
#[derive(Clone)]
pub(super) enum Job {
    Clone(CloneJob),
    Pull { id: String },
}

#[derive(Clone)]
pub(super) struct CloneJob {
    /// `owner/name`.
    pub(super) full_name: String,
    pub(super) destination: PathBuf,
    pub(super) mode: CloneMode,
    /// Added to the new repository.
    pub(super) tags: Vec<String>,
    pub(super) pull_every: Option<Interval>,
    /// The window whose workspace the clone joins.
    pub(super) window: Option<(AnyWindowHandle, WeakEntity<SearchApp>)>,
}

/// How a repository's last pull went.
#[derive(Clone, Debug)]
pub(super) struct PullRecord {
    pub(super) at: SystemTime,
    pub(super) ok: bool,
    pub(super) summary: String,
}

enum Message {
    Progress(Option<f32>, String),
    Finished(Result<Outcome, String>),
}

enum Outcome {
    Cloned(PathBuf),
    Pulled(PullOutcome),
}

pub(super) struct TaskHub {
    list: TaskList<Job>,
    /// Every task's job, to run it again.
    jobs: HashMap<u64, Job>,
    /// The latest pull of each repository, by id.
    pulls: HashMap<String, PullRecord>,
    runners: HashMap<u64, Task<()>>,
    _schedule: Task<()>,
}

struct GlobalTasks(Entity<TaskHub>);

impl Global for GlobalTasks {}

impl TaskHub {
    pub(super) fn init(cx: &mut App) {
        let settings = RepoHub::try_global(cx)
            .map(|hub| hub.read(cx).settings().tasks.clone())
            .unwrap_or_default();
        let mut list = TaskList::default();
        for kind in [TaskKind::Clone, TaskKind::Pull] {
            list.set_limit(kind, settings.limit(kind));
        }
        let hub = cx.new(|cx| Self {
            list,
            jobs: HashMap::new(),
            pulls: HashMap::new(),
            runners: HashMap::new(),
            _schedule: Self::schedule(cx),
        });
        cx.set_global(GlobalTasks(hub));
    }

    pub(super) fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalTasks>().0.clone()
    }

    /// Whether tasks are queued or running, so the app should not quit.
    pub(super) fn keeps_app_running(cx: &App) -> bool {
        cx.try_global::<GlobalTasks>()
            .is_some_and(|tasks| tasks.0.read(cx).list.has_active())
    }

    // ----- reading ---------------------------------------------------------------

    pub(super) fn infos(&self) -> Vec<TaskInfo> {
        self.list.infos()
    }

    pub(super) fn active_count(&self) -> usize {
        self.list.active_count()
    }

    pub(super) fn limit(&self, kind: TaskKind) -> usize {
        self.list.limit(kind)
    }

    pub(super) fn last_pull(&self, id: &str) -> Option<&PullRecord> {
        self.pulls.get(id)
    }

    /// Whether `full_name` is being cloned, or waits to be.
    pub(super) fn is_cloning(&self, full_name: &str) -> bool {
        self.list.any_active(TaskKind::Clone, full_name)
    }

    pub(super) fn is_pulling(&self, id: &str) -> bool {
        self.list.any_active(TaskKind::Pull, id)
    }

    // ----- changing ----------------------------------------------------------------

    /// Run `limit` tasks of `kind` at once, and remember it in the settings.
    /// It applies even when the settings cannot be saved.
    pub(super) fn change_limit(kind: TaskKind, limit: usize, cx: &mut App) -> Result<(), String> {
        let mut settings = RepoHub::global(cx).read(cx).settings().tasks.clone();
        settings.set_limit(kind, limit);
        Self::global(cx).update(cx, |this, cx| this.apply_settings(&settings, cx));
        RepoHub::global(cx)
            .update(cx, |hub, cx| hub.set_task_settings(settings, cx))
            .map_err(|error| format!("Could not save the settings: {error:#}"))
    }

    /// Run as many of each kind at once as `settings` say.
    pub(super) fn apply_settings(&mut self, settings: &TaskSettings, cx: &mut Context<Self>) {
        for kind in [TaskKind::Clone, TaskKind::Pull] {
            self.list.set_limit(kind, settings.limit(kind));
        }
        self.start_ready(cx);
        cx.notify();
    }

    /// Queue clones, skipping those already queued. Returns the new tasks' ids.
    pub(super) fn clone_repos(&mut self, jobs: Vec<CloneJob>, cx: &mut Context<Self>) -> Vec<u64> {
        let mut ids = Vec::new();
        for job in jobs {
            if self.is_cloning(&job.full_name) {
                continue;
            }
            let name = job.full_name.clone();
            ids.push(self.push(TaskKind::Clone, name.clone(), name, Job::Clone(job)));
        }
        self.start_ready(cx);
        ids
    }

    /// Queue pulls of repositories by id, skipping those already queued.
    pub(super) fn pull(&mut self, ids: &[String], cx: &mut Context<Self>) -> Vec<u64> {
        let mut tasks = Vec::new();
        for id in ids {
            if !self.is_pulling(id) {
                let name = RepoHub::global(cx)
                    .read(cx)
                    .library_name(id)
                    .unwrap_or_else(|| id.clone());
                let job = Job::Pull { id: id.clone() };
                tasks.push(self.push(TaskKind::Pull, id.clone(), name, job));
            }
        }
        self.start_ready(cx);
        tasks
    }

    fn push(&mut self, kind: TaskKind, key: String, title: String, job: Job) -> u64 {
        let id = self.list.push(kind, key, title, job.clone());
        self.jobs.insert(id, job);
        id
    }

    pub(super) fn cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        self.cancel_one(id, cx);
    }

    /// Cancel a task. Returns `false` when it had already finished.
    pub(super) fn cancel_one(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        let cancelled = self.list.cancel(id);
        if cancelled {
            cx.notify();
        }
        cancelled
    }

    pub(super) fn cancel_all(&mut self, cx: &mut Context<Self>) -> usize {
        let count = self.list.cancel_all();
        cx.notify();
        count
    }

    /// Run a failed or cancelled task again, as a new task.
    pub(super) fn retry(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(info) = self.list.info(id).cloned() else {
            return;
        };
        let Some(job) = self.jobs.get(&id).cloned() else {
            return;
        };
        if !info.state.is_finished() || self.list.any_active(info.kind, &info.key) {
            return;
        }
        self.push(info.kind, info.key, info.title, job);
        self.start_ready(cx);
    }

    pub(super) fn clear_finished(&mut self, cx: &mut Context<Self>) {
        self.list.clear_finished();
        let kept: Vec<u64> = self.list.infos().iter().map(|info| info.id).collect();
        self.jobs.retain(|id, _| kept.contains(id));
        cx.notify();
    }

    // ----- running -------------------------------------------------------------------

    fn start_ready(&mut self, cx: &mut Context<Self>) {
        for started in self.list.start_ready() {
            let id = started.id;
            log::info!("starting task {id}: {}", self.title(id));
            let runner = self.run(started, cx);
            self.runners.insert(id, runner);
        }
        cx.notify();
    }

    fn title(&self, id: u64) -> String {
        self.list
            .info(id)
            .map(|info| info.title.clone())
            .unwrap_or_default()
    }

    /// Run the job on a thread of its own: a clone can take an hour, and
    /// the background executor's threads are for short work.
    fn run(&mut self, started: Started<Job>, cx: &mut Context<Self>) -> Task<()> {
        let Started {
            id, job, cancel, ..
        } = started;
        let (sender, receiver) = async_channel::unbounded::<Message>();
        let work = job.clone();
        std::thread::Builder::new()
            .name(format!("task-{id}"))
            .spawn(move || {
                let result = work_on(&work, &cancel, &sender);
                sender.send_blocking(Message::Finished(result)).ok();
            })
            .expect("a thread for the task");

        cx.spawn(async move |this, cx| {
            let result = loop {
                match receiver.recv().await {
                    Ok(Message::Progress(fraction, detail)) => {
                        this.update(cx, |this, cx| {
                            this.list.progress(id, fraction, detail);
                            cx.notify();
                        })
                        .ok();
                    }
                    Ok(Message::Finished(result)) => break result,
                    Err(_) => break Err("the task stopped unexpectedly".into()),
                }
            };
            cx.update(|cx| Self::finished(id, job, result, cx));
        })
    }

    fn finished(id: u64, job: Job, result: Result<Outcome, String>, cx: &mut App) {
        let repos = RepoHub::global(cx);
        let tasks = Self::global(cx);
        let message = match (&job, result) {
            (_, Err(error)) => Err(error),
            (Job::Clone(clone), Ok(Outcome::Cloned(folder))) => {
                adopt_clone(clone, &folder, &repos, cx)
                    .map(|_| format!("cloned into {}", folder.display()))
            }
            (Job::Pull { id: repo }, Ok(Outcome::Pulled(outcome))) => {
                let summary = outcome.summary();
                repos.update(cx, |hub, cx| {
                    if matches!(outcome, PullOutcome::Updated { .. }) && hub.is_open(repo) {
                        hub.queue_index(repo, IndexJob::Update, cx);
                    }
                });
                Ok(summary)
            }
            _ => Err("the task did something unexpected".into()),
        };
        let state = tasks.update(cx, |this, cx| {
            match &message {
                Ok(message) => log::info!("task {id} ({}): {message}", this.title(id)),
                Err(error) => log::warn!("task {id} ({}) failed: {error}", this.title(id)),
            }
            this.list.finish(id, message.clone());
            this.runners.remove(&id);
            this.start_ready(cx);
            this.list.info(id).map(|info| info.state.clone())
        });
        if let Job::Pull { id: repo } = &job
            && !matches!(state, Some(TaskState::Cancelled))
        {
            let now = SystemTime::now();
            repos.update(cx, |hub, _| hub.note_pulled(repo, now));
            let record = PullRecord {
                at: now,
                ok: message.is_ok(),
                summary: match message {
                    Ok(summary) => summary,
                    Err(error) => error,
                },
            };
            tasks.update(cx, |this, cx| {
                this.pulls.insert(repo.clone(), record);
                cx.notify();
            });
        }
    }

    // ----- pulling on a schedule -------------------------------------------------------

    fn schedule(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FIRST_SCHEDULE).await;
            loop {
                if this.update(cx, |this, cx| this.pull_due(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(SCHEDULE_INTERVAL).await;
            }
        })
    }

    /// Queue a pull of every repository whose interval has passed.
    fn pull_due(&mut self, cx: &mut Context<Self>) {
        let now = SystemTime::now();
        let due: Vec<String> = RepoHub::global(cx)
            .read(cx)
            .library()
            .repos
            .iter()
            .filter(|entry| {
                entry
                    .pull_every
                    .is_some_and(|every| every.is_due(entry.pulled_at(), now))
            })
            .filter(|entry| entry.path.join(".git").exists())
            .map(|entry| entry.path.to_string_lossy().into_owned())
            .collect();
        if !due.is_empty() {
            log::info!("pulling {} repositories that are due", due.len());
            self.pull(&due, cx);
        }
    }
}

/// Do `job`, passing progress on through `sender`.
fn work_on(
    job: &Job,
    cancel: &AtomicBool,
    sender: &async_channel::Sender<Message>,
) -> Result<Outcome, String> {
    match job {
        Job::Clone(clone) => {
            let mut last = Instant::now() - PROGRESS_INTERVAL;
            github::clone(
                &clone.full_name,
                &clone.destination,
                clone.mode,
                cancel,
                |progress| {
                    if last.elapsed() >= PROGRESS_INTERVAL || progress.fraction >= 1.0 {
                        last = Instant::now();
                        let detail = if progress.detail.is_empty() {
                            progress.phase
                        } else {
                            format!("{} · {}", progress.phase, progress.detail)
                        };
                        sender
                            .send_blocking(Message::Progress(Some(progress.fraction), detail))
                            .ok();
                    }
                },
            )
            .map(|()| Outcome::Cloned(clone.destination.clone()))
            .map_err(|error| format!("{error:#}"))
        }
        Job::Pull { id } => {
            sender
                .send_blocking(Message::Progress(None, "Fetching…".into()))
                .ok();
            sync::pull(Path::new(id), cancel)
                .map(Outcome::Pulled)
                .map_err(|error| format!("{error:#}"))
        }
    }
}

/// Register a finished clone, and add it to the workspace of the window
/// that asked for it when that window is still open.
fn adopt_clone(
    clone: &CloneJob,
    folder: &Path,
    repos: &Entity<RepoHub>,
    cx: &mut App,
) -> Result<String, String> {
    let id = repos
        .update(cx, |hub, cx| {
            hub.adopt(folder, &clone.tags, clone.pull_every, cx)
        })
        .map_err(|error| format!("cloned, but could not save it: {error:#}"))?;
    if let Some((handle, app)) = &clone.window {
        let id = id.clone();
        let app = app.clone();
        handle
            .update(cx, |_, window, cx| {
                app.update(cx, |app, cx| app.adopt_repositories(vec![id], window, cx))
                    .ok();
            })
            .ok();
    }
    Ok(id)
}
