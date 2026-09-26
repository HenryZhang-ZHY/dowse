//! Answering the command line. `dowse search`, `dowse repos` and the rest
//! reach the running app over its socket (see [`dowse::ipc`]), so they share
//! its warm indexes and file watchers, and what they do shows in the
//! developer tools.
//!
//! Repositories a request covers are opened in the hub as they would be for
//! a window, and stay open for a while after the last request, so an agent's
//! next query is fast too. With no windows open, the app keeps running for
//! the command line until it has been idle for as long.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use async_channel::Sender;
use gpui_kit::*;

use super::hub::{IndexActivity, RepoHub};
use super::windows::Windows;
use dowse::diagnostics::log as app_log;
use dowse::diagnostics::metrics::{Origin, SearchRecord, metrics};
use dowse::engine::index::{IndexStatus, RepoIndex};
use dowse::engine::query::CompiledQuery;
use dowse::engine::repo::{self, RepoInfo};
use dowse::engine::search::{self, SearchLimits};
use dowse::engine::table::ResultTable;
use dowse::ipc::protocol::{
    AppStatus, Frame, LogsRequest, RepoStatus, Request, RequestEnvelope, ScopeSpec, SearchRequest,
    SearchResponse,
};

/// How long repositories opened for the command line stay open after their
/// last request, and how long a windowless app waits for the next request
/// before quitting.
const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How often idle repositories are closed.
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);
/// How often a request waiting on the hub looks again.
const WAIT_STEP: Duration = Duration::from_millis(25);
/// How long a search waits for its repositories to load.
const LOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// How often `dowse dev logs --follow` looks for new records.
const FOLLOW_INTERVAL: Duration = Duration::from_millis(200);

pub(super) struct Remote {
    started: Instant,
    last_request: Option<Instant>,
    /// Repositories held open for the command line, by id, with when a
    /// request last used each.
    leases: HashMap<String, Instant>,
    _sweep: Task<()>,
}

impl Global for Remote {}

impl Remote {
    pub(super) fn init(cx: &mut App) {
        let sweep = cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(SWEEP_INTERVAL).await;
                cx.update(Self::sweep);
            }
        });
        cx.set_global(Self {
            started: Instant::now(),
            last_request: None,
            leases: HashMap::new(),
            _sweep: sweep,
        });
    }

    /// The app started for the command line, with no windows: count that as
    /// a request, so it waits for the next one before quitting.
    pub(super) fn started_in_background(cx: &mut App) {
        cx.global_mut::<Self>().last_request = Some(Instant::now());
    }

    /// Whether the app should keep running with no windows open.
    pub(super) fn keeps_app_running(cx: &App) -> bool {
        cx.global::<Self>()
            .last_request
            .is_some_and(|last| last.elapsed() < IDLE_TIMEOUT)
    }

    /// Close repositories no request used lately, and quit a windowless app
    /// nobody asks anything of.
    fn sweep(cx: &mut App) {
        let remote = cx.global_mut::<Self>();
        let idle: Vec<String> = remote
            .leases
            .iter()
            .filter(|(_, used)| used.elapsed() >= IDLE_TIMEOUT)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &idle {
            remote.leases.remove(id);
        }
        if !idle.is_empty() {
            let hub = RepoHub::global(cx);
            hub.update(cx, |hub, cx| {
                for id in &idle {
                    hub.release(id, cx);
                }
            });
        }
        if Windows::count(cx) == 0 && !Self::keeps_app_running(cx) {
            log::info!("no windows and no command-line requests for a while; quitting");
            cx.quit();
        }
    }

    /// Hold `repos` open for the command line, opening those not yet open.
    fn lease(repos: &[Arc<RepoInfo>], cx: &mut App) {
        let now = Instant::now();
        let mut opening = Vec::new();
        let remote = cx.global_mut::<Self>();
        for repo in repos {
            if remote.leases.insert(repo.id.clone(), now).is_none() {
                opening.push(repo.id.clone());
            }
        }
        if !opening.is_empty() {
            let hub = RepoHub::global(cx);
            hub.update(cx, |hub, cx| {
                for id in &opening {
                    hub.acquire(id, cx);
                }
            });
        }
    }
}

/// Carry out a command-line request, answering on `reply`.
pub(super) fn handle(envelope: RequestEnvelope, reply: Sender<Frame>, cx: &mut App) {
    cx.global_mut::<Remote>().last_request = Some(Instant::now());
    log::info!("command line: {}", describe(&envelope.request));
    metrics().count_request(kind(&envelope.request));
    let cwd = envelope.cwd;
    let result = match envelope.request {
        Request::Search(request) => {
            search(request, &cwd, reply.clone(), cx);
            Ok(Vec::new())
        }
        Request::Repos(scope) => repos(&scope, &cwd, cx).map(|repos| vec![Frame::Repos(repos)]),
        Request::Status => Ok(vec![Frame::Status(status(cx))]),
        Request::Metrics => Ok(vec![Frame::Metrics(Box::new(metrics().snapshot()))]),
        Request::AddRepos { folders, tags } => add_repos(&folders, &tags, cx),
        Request::Tag { repo, add, remove } => tag(&repo, &add, &remove, &cwd, cx),
        Request::Index { scope, wait } => {
            index(&scope, wait, &cwd, reply.clone(), cx);
            Ok(Vec::new())
        }
        Request::Logs(request) => {
            logs(request, reply.clone(), cx);
            Ok(Vec::new())
        }
        Request::Quit => {
            reply
                .send_blocking(Frame::Message("dowse quit".into()))
                .ok();
            reply.send_blocking(Frame::Done).ok();
            // Let the connection write the answer first.
            cx.spawn(async move |cx| {
                cx.background_executor().timer(WAIT_STEP).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
            Ok(Vec::new())
        }
    };
    // Requests that answer later took `reply` along; the rest answer now.
    match result {
        Ok(frames) if frames.is_empty() => {}
        Ok(frames) => {
            for frame in frames {
                reply.send_blocking(frame).ok();
            }
            reply.send_blocking(Frame::Done).ok();
        }
        Err(error) => {
            reply.send_blocking(Frame::Error(error)).ok();
        }
    }
}

/// A request's kind, for counting.
fn kind(request: &Request) -> &'static str {
    match request {
        Request::Search(_) => "search",
        Request::Repos(_) => "repos",
        Request::Status => "status",
        Request::AddRepos { .. } => "repos add",
        Request::Tag { .. } => "repos tag",
        Request::Index { .. } => "index",
        Request::Logs(_) => "dev logs",
        Request::Metrics => "dev metrics",
        Request::Quit => "quit",
    }
}

/// A request in a few words, for the log.
fn describe(request: &Request) -> String {
    match request {
        Request::Search(search) => format!("search {:?}", search.query.pattern),
        Request::Repos(_) => "repos".into(),
        Request::Status => "status".into(),
        Request::AddRepos { folders, .. } => format!("add {} folders", folders.len()),
        Request::Tag { repo, .. } => format!("tag {repo}"),
        Request::Index { .. } => "index".into(),
        Request::Logs(_) => "logs".into(),
        Request::Quit => "quit".into(),
        Request::Metrics => "metrics".into(),
    }
}

/// The repositories `scope` covers, or why there are none.
fn select(scope: &ScopeSpec, cwd: &Path, cx: &App) -> Result<Vec<Arc<RepoInfo>>, String> {
    let hub = RepoHub::global(cx);
    let repos = scope.select(
        hub.read(cx).library(),
        cwd,
        &Windows::config(cx),
        &Windows::recent(cx),
    )?;
    if repos.is_empty() {
        return Err(if scope.is_everything() {
            "dowse knows no repositories yet; add some with `dowse repos add <folder>`".into()
        } else {
            "no repositories match; `dowse repos` lists them with their tags".into()
        });
    }
    Ok(repos)
}

// ----- search -------------------------------------------------------------------

fn search(request: SearchRequest, cwd: &Path, reply: Sender<Frame>, cx: &mut App) {
    let fail = |error: String| {
        reply.send_blocking(Frame::Error(error)).ok();
    };
    let compiled = match CompiledQuery::new(&request.query) {
        Ok(compiled) => compiled,
        Err(error) => return fail(format!("invalid query: {error}")),
    };
    let repos = match select(&request.scope, cwd, cx) {
        Ok(repos) => repos,
        Err(error) => return fail(error),
    };
    Remote::lease(&repos, cx);
    let ids: Vec<String> = repos.iter().map(|repo| repo.id.clone()).collect();
    cx.spawn(async move |cx| {
        wait_until_loaded(&ids, cx).await;
        let hub = cx.update(|cx| RepoHub::global(cx));
        let (sources, notes) = cx.update(|cx| {
            let hub = hub.read(cx);
            (hub.search_sources(&ids).0, source_notes(hub, &ids))
        });
        let limits = SearchLimits {
            context_lines: request.context,
            max_lines_per_file: request.max_per_file.max(1),
            ..SearchLimits::default()
        };
        let response = cx
            .background_spawn(async move {
                let outcome = search::search(&sources, &compiled, &limits, &AtomicBool::new(false));
                metrics().record_search(SearchRecord::new(
                    Origin::Cli,
                    &request.query.pattern,
                    &outcome,
                ));
                let mut response = SearchResponse::new(&outcome, request.limit, request.files_only);
                if let Some(format) = request.table {
                    let visible: Vec<usize> = (0..outcome.files.len()).collect();
                    let table = ResultTable::new(&outcome.files, &visible, &compiled.matcher);
                    let order: Vec<usize> = (0..table.rows.len()).collect();
                    response.table = Some(table.export(&order, format));
                }
                response.notes = notes;
                response
            })
            .await;
        reply.send(Frame::Search(Box::new(response))).await.ok();
        reply.send(Frame::Done).await.ok();
    })
    .detach();
}

/// Wait until none of `ids` is still reading its index or walking its
/// folder, so a search covers every file.
async fn wait_until_loaded(ids: &[String], cx: &mut AsyncApp) {
    let deadline = Instant::now() + LOAD_TIMEOUT;
    while Instant::now() < deadline {
        let loading = cx.update(|cx| {
            let hub = RepoHub::global(cx);
            let hub = hub.read(cx);
            ids.iter().any(|id| {
                hub.view(id)
                    .is_some_and(|view| view.activity == IndexActivity::Loading)
            })
        });
        if !loading {
            return;
        }
        cx.background_executor().timer(WAIT_STEP).await;
    }
}

/// What a reader of search results should know about the repositories
/// searched.
fn source_notes(hub: &RepoHub, ids: &[String]) -> Vec<String> {
    ids.iter()
        .filter_map(|id| hub.view(id))
        .filter_map(|view| {
            let name = &view.info.name;
            match &view.activity {
                IndexActivity::Missing => Some(format!("{name}: folder not found, not searched")),
                IndexActivity::Failed(error) => Some(format!(
                    "{name}: indexing failed ({error}); searched what could be read"
                )),
                IndexActivity::Idle(IndexStatus::Missing | IndexStatus::Unusable)
                | IndexActivity::Queued
                | IndexActivity::Building => {
                    Some(format!("{name}: no index yet, so its files were scanned"))
                }
                IndexActivity::Loading => Some(format!("{name}: still loading, not searched")),
                IndexActivity::Idle(IndexStatus::Ready { .. }) => None,
            }
        })
        .collect()
}

// ----- repositories -------------------------------------------------------------

fn repos(scope: &ScopeSpec, cwd: &Path, cx: &App) -> Result<Vec<RepoStatus>, String> {
    let repos = select(scope, cwd, cx)?;
    let hub = RepoHub::global(cx);
    let hub = hub.read(cx);
    Ok(repos.iter().map(|info| repo_status(hub, info)).collect())
}

fn repo_status(hub: &RepoHub, info: &RepoInfo) -> RepoStatus {
    let (activity, changed_files) = match hub.view(&info.id) {
        Some(view) => (Some(view.activity), view.changed_files),
        None => (None, 0),
    };
    // What the index on disk says, whether or not the app has it open.
    let on_disk = RepoIndex::open(&info.root)
        .ok()
        .map(|index| index.index_status());
    let (index, error) = match &activity {
        None if on_disk.is_none() => ("not-found", None),
        None => ("closed", None),
        Some(IndexActivity::Loading) => ("loading", None),
        Some(IndexActivity::Queued) => ("queued", None),
        Some(IndexActivity::Building) => ("building", None),
        Some(IndexActivity::Missing) => ("not-found", None),
        Some(IndexActivity::Failed(error)) => ("failed", Some(error.clone())),
        Some(IndexActivity::Idle(IndexStatus::Missing)) => ("missing", None),
        Some(IndexActivity::Idle(IndexStatus::Unusable)) => ("unusable", None),
        Some(IndexActivity::Idle(IndexStatus::Ready { .. })) => ("ready", None),
    };
    let (files, indexed_at_ms) = match on_disk {
        Some(IndexStatus::Ready { files, updated_at }) => (
            Some(files),
            updated_at
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|elapsed| elapsed.as_millis() as u64),
        ),
        _ => (None, None),
    };
    RepoStatus {
        name: info.name.clone(),
        path: info.root.clone(),
        branch: info.branch.clone(),
        tags: info.tags.clone(),
        index: index.into(),
        error,
        files,
        indexed_at_ms,
        changed_files,
    }
}

fn add_repos(
    folders: &[std::path::PathBuf],
    tags: &[String],
    cx: &mut App,
) -> Result<Vec<Frame>, String> {
    let hub = RepoHub::global(cx);
    let ids = hub
        .update(cx, |hub, _| hub.register(folders, true))
        .map_err(|error| format!("{error:#}"))?;
    if ids.is_empty() {
        return Err("found no folders to add".into());
    }
    if !tags.is_empty() {
        hub.update(cx, |hub, cx| {
            for id in &ids {
                let mut merged = hub.library_tags(id);
                for tag in tags {
                    if !merged.contains(tag) {
                        merged.push(tag.clone());
                    }
                }
                hub.set_tags(id, merged, cx)?;
            }
            anyhow::Ok(())
        })
        .map_err(|error| format!("{error:#}"))?;
    }
    let names: Vec<String> = ids
        .iter()
        .map(|id| hub.read(cx).library_name(id).unwrap_or_else(|| id.clone()))
        .collect();
    Ok(vec![Frame::Message(format!(
        "{} in the library: {}",
        crate::format::plural(names.len(), "repository", "repositories"),
        names.join(", ")
    ))])
}

fn tag(
    repo: &str,
    add: &[String],
    remove: &[String],
    cwd: &Path,
    cx: &mut App,
) -> Result<Vec<Frame>, String> {
    let hub = RepoHub::global(cx);
    let id = {
        let library = hub.read(cx).library();
        let by_path = repo::identity(&cwd.join(repo));
        library
            .repos
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(repo))
            .or_else(|| library.repos.iter().find(|entry| entry.path == by_path))
            .map(|entry| entry.path.to_string_lossy().into_owned())
            .ok_or_else(|| format!("no repository named {repo}; `dowse repos` lists them"))?
    };
    let mut tags = hub.read(cx).library_tags(&id);
    tags.retain(|tag| !remove.contains(tag));
    for tag in add {
        if !tags.contains(tag) {
            tags.push(tag.clone());
        }
    }
    hub.update(cx, |hub, cx| hub.set_tags(&id, tags.clone(), cx))
        .map_err(|error| format!("{error:#}"))?;
    Ok(vec![Frame::Message(if tags.is_empty() {
        format!("{repo} has no tags")
    } else {
        format!("{repo} is tagged {}", tags.join(" "))
    })])
}

fn index(scope: &ScopeSpec, wait: bool, cwd: &Path, reply: Sender<Frame>, cx: &mut App) {
    let repos = match select(scope, cwd, cx) {
        Ok(repos) => repos,
        Err(error) => {
            reply.send_blocking(Frame::Error(error)).ok();
            return;
        }
    };
    Remote::lease(&repos, cx);
    let ids: Vec<String> = repos.iter().map(|repo| repo.id.clone()).collect();
    cx.spawn(async move |cx| {
        wait_until_loaded(&ids, cx).await;
        cx.update(|cx| {
            RepoHub::global(cx).update(cx, |hub, cx| {
                for id in &ids {
                    hub.queue_index(id, cx);
                }
            })
        });
        let names = cx.update(|cx| {
            let hub = RepoHub::global(cx);
            let hub = hub.read(cx);
            ids.iter()
                .map(|id| hub.library_name(id).unwrap_or_else(|| id.clone()))
                .collect::<Vec<_>>()
        });
        reply
            .send(Frame::Message(format!("indexing {}", names.join(", "))))
            .await
            .ok();
        let mut pending = ids.clone();
        while wait && !pending.is_empty() {
            cx.background_executor().timer(WAIT_STEP * 8).await;
            let finished = cx.update(|cx| {
                let hub = RepoHub::global(cx);
                let hub = hub.read(cx);
                let mut finished = Vec::new();
                pending.retain(|id| {
                    let Some(view) = hub.view(id) else {
                        return false;
                    };
                    if view.is_busy() {
                        return true;
                    }
                    finished.push(format!("{}: {}", view.info.name, view.index_summary()));
                    false
                });
                finished
            });
            for line in finished {
                if reply.send(Frame::Message(line)).await.is_err() {
                    return;
                }
            }
        }
        reply.send(Frame::Done).await.ok();
    })
    .detach();
}

// ----- the app ------------------------------------------------------------------

fn status(cx: &App) -> AppStatus {
    let config = Windows::config(cx);
    let hub = RepoHub::global(cx);
    let hub = hub.read(cx);
    let remote = cx.global::<Remote>();
    let windows = Windows::count(cx);
    let quits_in_ms = (windows == 0)
        .then(|| remote.last_request)
        .flatten()
        .map(|last| IDLE_TIMEOUT.saturating_sub(last.elapsed()).as_millis() as u64);
    let (building, queued) = hub.build_queue();
    AppStatus {
        version: env!("CARGO_PKG_VERSION").into(),
        pid: std::process::id(),
        uptime_ms: remote.started.elapsed().as_millis() as u64,
        log_file: app_log::log_file(&config.logs_dir()),
        config_dir: config.root().to_path_buf(),
        windows,
        quits_in_ms,
        library_repos: hub.library().repos.len(),
        open_repos: hub.open_count(),
        building,
        queued,
    }
}

fn logs(request: LogsRequest, reply: Sender<Frame>, cx: &mut App) {
    cx.spawn(async move |cx| {
        let buffer = app_log::buffer();
        let mut after = request.after;
        let mut limit = request.limit.max(1);
        loop {
            for entry in buffer.since(after, request.level, limit) {
                after = entry.seq;
                if reply.send(Frame::Log(entry)).await.is_err() {
                    return;
                }
            }
            if !request.follow {
                break;
            }
            limit = usize::MAX;
            cx.background_executor().timer(FOLLOW_INTERVAL).await;
            if reply.is_closed() {
                return;
            }
        }
        reply.send(Frame::Done).await.ok();
    })
    .detach();
}
