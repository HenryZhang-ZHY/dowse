//! Long jobs the app runs in the background, such as clones and pulls: a
//! queue that starts them a few at a time per kind, tracks their progress,
//! cancels them and forgets old finished ones. Running them is the caller's;
//! this only keeps the books, so the app and the command line see the same.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Finished tasks kept for the task list.
pub const KEEP_FINISHED: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Clone,
    Pull,
}

impl TaskKind {
    /// How many run at once by default: clones mostly wait on the network.
    pub fn default_limit(self) -> usize {
        match self {
            TaskKind::Clone => 4,
            TaskKind::Pull => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum TaskState {
    Queued,
    Running,
    Done { message: String },
    Failed { error: String },
    Cancelled,
}

impl TaskState {
    pub fn is_finished(&self) -> bool {
        !matches!(self, TaskState::Queued | TaskState::Running)
    }
}

/// A task as the task list and `dowse tasks` show it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: u64,
    pub kind: TaskKind,
    /// What it works on, such as `alice/api`.
    pub title: String,
    #[serde(flatten)]
    pub state: TaskState,
    /// From 0 to 1, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f32>,
    /// What it is doing now, such as `Receiving objects · 12.00 MiB/s`.
    #[serde(default)]
    pub detail: String,
    /// Seconds since the Unix epoch.
    pub queued_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
}

struct Entry<J> {
    info: TaskInfo,
    /// Taken when the task starts.
    job: Option<J>,
    cancel: Arc<AtomicBool>,
}

/// A task ready to run: its id, its job and the flag that cancels it.
pub struct Started<J> {
    pub id: u64,
    pub kind: TaskKind,
    pub job: J,
    pub cancel: Arc<AtomicBool>,
}

pub struct TaskList<J> {
    entries: Vec<Entry<J>>,
    next_id: u64,
    limits: HashMap<TaskKind, usize>,
}

impl<J> Default for TaskList<J> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 1,
            limits: HashMap::new(),
        }
    }
}

impl<J> TaskList<J> {
    pub fn limit(&self, kind: TaskKind) -> usize {
        self.limits
            .get(&kind)
            .copied()
            .unwrap_or_else(|| kind.default_limit())
    }

    /// Run at most `limit` tasks of `kind` at once, at least one.
    pub fn set_limit(&mut self, kind: TaskKind, limit: usize) {
        self.limits.insert(kind, limit.max(1));
    }

    /// Queue `job`. Returns its id.
    pub fn push(&mut self, kind: TaskKind, title: impl Into<String>, job: J) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(Entry {
            info: TaskInfo {
                id,
                kind,
                title: title.into(),
                state: TaskState::Queued,
                fraction: None,
                detail: String::new(),
                queued_at: now_secs(),
                finished_at: None,
            },
            job: Some(job),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        id
    }

    /// Start as many queued tasks as the limits allow, oldest first.
    pub fn start_ready(&mut self) -> Vec<Started<J>> {
        let mut running: HashMap<TaskKind, usize> = HashMap::new();
        for entry in &self.entries {
            if entry.info.state == TaskState::Running {
                *running.entry(entry.info.kind).or_default() += 1;
            }
        }
        let limits: HashMap<TaskKind, usize> = [TaskKind::Clone, TaskKind::Pull]
            .into_iter()
            .map(|kind| (kind, self.limit(kind)))
            .collect();
        let mut started = Vec::new();
        for entry in &mut self.entries {
            if entry.info.state != TaskState::Queued {
                continue;
            }
            let kind = entry.info.kind;
            let count = running.entry(kind).or_default();
            if *count >= limits[&kind] {
                continue;
            }
            let Some(job) = entry.job.take() else {
                continue;
            };
            *count += 1;
            entry.info.state = TaskState::Running;
            started.push(Started {
                id: entry.info.id,
                kind,
                job,
                cancel: entry.cancel.clone(),
            });
        }
        started
    }

    pub fn progress(&mut self, id: u64, fraction: Option<f32>, detail: impl Into<String>) {
        if let Some(entry) = self.entry_mut(id)
            && entry.info.state == TaskState::Running
        {
            if let Some(fraction) = fraction {
                entry.info.fraction = Some(fraction.clamp(0.0, 1.0));
            }
            entry.info.detail = detail.into();
        }
    }

    /// The running task finished with `result`. A task whose cancel flag was
    /// set counts as cancelled, whatever it returned.
    pub fn finish(&mut self, id: u64, result: Result<String, String>) {
        if let Some(entry) = self.entry_mut(id) {
            if entry.info.state.is_finished() {
                return;
            }
            entry.info.state = if entry.cancel.load(Ordering::Relaxed) {
                TaskState::Cancelled
            } else {
                match result {
                    Ok(message) => {
                        entry.info.fraction = Some(1.0);
                        TaskState::Done { message }
                    }
                    Err(error) => TaskState::Failed { error },
                }
            };
            entry.info.detail.clear();
            entry.info.finished_at = Some(now_secs());
        }
        self.forget_old();
    }

    /// Cancel a task: a queued one at once, a running one when its runner
    /// sees the flag. Returns `false` when it had already finished.
    pub fn cancel(&mut self, id: u64) -> bool {
        let Some(entry) = self.entry_mut(id) else {
            return false;
        };
        match entry.info.state {
            TaskState::Queued => {
                entry.job = None;
                entry.info.state = TaskState::Cancelled;
                entry.info.finished_at = Some(now_secs());
                true
            }
            TaskState::Running => {
                entry.cancel.store(true, Ordering::Relaxed);
                entry.info.detail = "Cancelling…".into();
                true
            }
            _ => false,
        }
    }

    /// Cancel every task not yet finished. Returns how many.
    pub fn cancel_all(&mut self) -> usize {
        let ids: Vec<u64> = self
            .entries
            .iter()
            .filter(|entry| !entry.info.state.is_finished())
            .map(|entry| entry.info.id)
            .collect();
        ids.into_iter().filter(|id| self.cancel(*id)).count()
    }

    /// Forget finished tasks. Returns how many.
    pub fn clear_finished(&mut self) -> usize {
        let before = self.entries.len();
        self.entries.retain(|entry| !entry.info.state.is_finished());
        before - self.entries.len()
    }

    /// Whether some task is queued or running.
    pub fn has_active(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| !entry.info.state.is_finished())
    }

    /// How many tasks are queued or running.
    pub fn active_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| !entry.info.state.is_finished())
            .count()
    }

    /// Whether a queued task's job, or a running task's title, matches.
    pub fn any_active(&self, kind: TaskKind, title: &str) -> bool {
        self.entries.iter().any(|entry| {
            entry.info.kind == kind && entry.info.title == title && !entry.info.state.is_finished()
        })
    }

    /// Every task, oldest first.
    pub fn infos(&self) -> Vec<TaskInfo> {
        self.entries
            .iter()
            .map(|entry| entry.info.clone())
            .collect()
    }

    pub fn info(&self, id: u64) -> Option<&TaskInfo> {
        self.entries
            .iter()
            .find(|entry| entry.info.id == id)
            .map(|entry| &entry.info)
    }

    fn entry_mut(&mut self, id: u64) -> Option<&mut Entry<J>> {
        self.entries.iter_mut().find(|entry| entry.info.id == id)
    }

    /// Keep at most [`KEEP_FINISHED`] finished tasks, dropping the oldest.
    fn forget_old(&mut self) {
        let finished = self
            .entries
            .iter()
            .filter(|entry| entry.info.state.is_finished())
            .count();
        let mut excess = finished.saturating_sub(KEEP_FINISHED);
        self.entries.retain(|entry| {
            if excess > 0 && entry.info.state.is_finished() {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states(list: &TaskList<&str>) -> Vec<(String, TaskState)> {
        list.infos()
            .into_iter()
            .map(|info| (info.title, info.state))
            .collect()
    }

    #[test]
    fn starts_tasks_oldest_first_within_each_kinds_limit() {
        let mut list = TaskList::default();
        list.set_limit(TaskKind::Clone, 2);
        for name in ["a", "b", "c"] {
            list.push(TaskKind::Clone, name, name);
        }
        let pull = list.push(TaskKind::Pull, "p", "p");
        let started: Vec<&str> = list.start_ready().into_iter().map(|s| s.job).collect();
        assert_eq!(started, ["a", "b", "p"]);
        assert!(list.start_ready().is_empty(), "limits are full");

        list.finish(1, Ok("cloned".into()));
        let started: Vec<&str> = list.start_ready().into_iter().map(|s| s.job).collect();
        assert_eq!(started, ["c"]);
        assert_eq!(list.active_count(), 3);
        assert!(list.any_active(TaskKind::Pull, "p"));
        list.finish(pull, Err("offline".into()));
        assert!(!list.any_active(TaskKind::Pull, "p"));
        assert_eq!(
            list.info(pull).unwrap().state,
            TaskState::Failed {
                error: "offline".into()
            }
        );
        assert_eq!(list.info(1).unwrap().fraction, Some(1.0));
    }

    #[test]
    fn cancelling_drops_queued_tasks_and_flags_running_ones() {
        let mut list = TaskList::default();
        list.set_limit(TaskKind::Clone, 1);
        let running = list.push(TaskKind::Clone, "a", "a");
        let queued = list.push(TaskKind::Clone, "b", "b");
        let started = list.start_ready();
        assert!(list.cancel(queued));
        assert!(list.cancel(running));
        assert!(started[0].cancel.load(Ordering::Relaxed));
        assert_eq!(list.info(running).unwrap().state, TaskState::Running);
        // Whatever the runner returns, it was cancelled.
        list.finish(running, Ok("done anyway".into()));
        assert_eq!(
            states(&list),
            [
                ("a".into(), TaskState::Cancelled),
                ("b".into(), TaskState::Cancelled)
            ]
        );
        assert!(!list.cancel(running));
        assert!(
            list.start_ready().is_empty(),
            "a cancelled job never starts"
        );
        assert!(!list.has_active());
        assert_eq!(list.clear_finished(), 2);
        assert!(list.infos().is_empty());
    }

    #[test]
    fn progress_applies_to_running_tasks_only() {
        let mut list = TaskList::default();
        let id = list.push(TaskKind::Pull, "a", "a");
        list.progress(id, Some(0.5), "early");
        assert_eq!(list.info(id).unwrap().fraction, None);
        list.start_ready();
        list.progress(id, Some(1.5), "Receiving objects");
        list.progress(id, None, "Resolving deltas");
        let info = list.info(id).unwrap();
        assert_eq!(
            (info.fraction, info.detail.as_str()),
            (Some(1.0), "Resolving deltas")
        );
        assert_eq!(list.cancel_all(), 1);
    }

    #[test]
    fn keeps_a_bounded_history_of_finished_tasks() {
        let mut list = TaskList::default();
        list.set_limit(TaskKind::Pull, 1000);
        for n in 0..KEEP_FINISHED + 5 {
            list.push(TaskKind::Pull, n.to_string(), "job");
        }
        for started in list.start_ready() {
            list.finish(started.id, Ok(String::new()));
        }
        let infos = list.infos();
        assert_eq!(infos.len(), KEEP_FINISHED);
        assert_eq!(infos[0].title, "5", "the oldest go first");
    }

    #[test]
    fn task_infos_serialize_flat() {
        let mut list = TaskList::default();
        let id = list.push(TaskKind::Clone, "alice/api", ());
        list.start_ready();
        list.finish(id, Err("boom".into()));
        let json = serde_json::to_value(list.info(id).unwrap()).unwrap();
        assert_eq!(json["kind"], "clone");
        assert_eq!(json["state"], "failed");
        assert_eq!(json["error"], "boom");
        let back: TaskInfo = serde_json::from_value(json).unwrap();
        assert_eq!(&back, list.info(id).unwrap());
    }
}
