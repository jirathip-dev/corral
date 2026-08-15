//! git data plane (WS1): fsevents-driven watcher over the repo's `.git`
//! state, emitting canonical [`GitEvent`]s keyed by WORKTREE PATH.
//!
//! ## Primary signal (push, zero polling)
//!
//! fsevents (macOS) watches the git metadata of the main checkout and every
//! linked worktree through ONE recursive watch on the repo's commondir (the
//! main checkout's `.git` directory, unless a `commondir` file redirects
//! it). The commondir holds everything that matters: the main checkout's
//! `HEAD`/`index`, the shared `refs/` + `packed-refs`, and the
//! `worktrees/<name>` gitdirs of every linked worktree (both the `.git`
//! **directory** form of the main checkout and the `.git` **file** form of
//! linked worktrees, whose `gitdir: <path>` target always lives under the
//! commondir — resolved by reading the file, never by guessing).
//!
//! A single watch is a deliberate constraint: notify's fsevents backend
//! *restarts the whole stream* (with `kFSEventStreamEventIdSinceNow`) on
//! every added path, so adding per-worktree watches later would silently
//! drop events for anything that changed during the restart. One watch,
//! registered once at boot, covers future worktrees too; the 10s sweep is
//! the backstop for anything outside it.
//!
//! Every fs event is mapped to the worktree(s) it concerns (most-specific
//! gitdir prefix, then `refs/heads/<branch>` → the worktree checked out on
//! that branch). Events for a path under `commondir/worktrees/` that matches
//! no known gitdir trigger a registry rescan, so `git worktree add` is
//! discovered within one event, not one sweep. Debounced 300ms per worktree;
//! each debounced batch re-reads HEAD + `git status` and emits only on
//! change. Each reconcile cycle is measured against a 200ms budget and
//! logged with `warn!` when exceeded.
//!
//! ## Safety net (never the primary signal)
//!
//! A 10s `git status` sweep across all watched worktrees — one concurrent
//! `git` subprocess per worktree — re-verifies head + status and emits only
//! when something changed. The sweep also rescans the worktree registry
//! (`git worktree list --porcelain`), so WorktreeAdded/WorktreeRemoved are
//! still detected when fsevents missed them. The PRIMARY mechanism remains
//! fsevents; the sweep is a documented catch-up only.
//!
//! ## Contract notes
//!
//! - Events are keyed by worktree path (`/Users/.../corral-p2-ws2`), never
//!   by `.git` internals.
//! - The plane is strictly read-only: only `status` / `rev-parse` /
//!   `worktree list` subprocesses (`--no-optional-locks`), never a mutation.
//! - Boot: the first registry scan reports the current worktree set as
//!   WorktreeAdded facts (path-keyed and idempotent for the consumer — WS3
//!   upserts on the path), so a worktree created during the boot scan can
//!   never be lost to inventory suppression. Head/status first observations
//!   also emit, so consumers converge the snapshot immediately.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::core::events::{GitEvent, GitStatus, Plane, PlaneEvent, PlaneSink};
use crate::core::util::now_millis;

/// Per-worktree debounce window (the brief's 300ms).
const DEBOUNCE: Duration = Duration::from_millis(300);
/// Safety-net sweep cadence.
const SWEEP_INTERVAL: Duration = Duration::from_secs(10);
/// Per-event processing budget; exceedances are logged (`warn!`).
const EVENT_BUDGET: Duration = Duration::from_millis(200);
/// Upper bound on a single `git` subprocess.
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
/// Concurrent `git` subprocesses during the sweep (one per worktree, bounded).
const MAX_CONCURRENT_PROBES: usize = 4;
/// Throttle on fsevents-triggered registry rescans (they re-add watchers).
const RESCAN_THROTTLE_MILLIS: u64 = 1000;
/// Delay before the one-shot retry of a registry rescan that found nothing:
/// `git worktree add` registers the entry *while* the events are still
/// arriving, so the first rescan can race the registration.
const RESCAN_RETRY_DELAY: Duration = Duration::from_millis(400);

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// One entry of `git worktree list --porcelain` (watched set only).
#[derive(Debug, Clone)]
struct WorktreeEntry {
    /// Worktree root (the event key).
    path: PathBuf,
    /// Resolved gitdir (`.git` dir, or the target of the `.git` file).
    gitdir: Option<PathBuf>,
    /// Current branch (`None` when detached).
    branch: Option<String>,
}

/// Per-worktree facts the plane tracks, keyed by the WORKTREE PATH.
#[derive(Debug, Clone, Default)]
struct WorktreeState {
    gitdir: Option<PathBuf>,
    branch: Option<String>,
    commit: Option<String>,
    status: Option<GitStatus>,
}

#[derive(Debug, Default)]
struct PlaneState {
    /// worktree root path -> state.
    worktrees: HashMap<PathBuf, WorktreeState>,
    /// branch name (`ws1/git-plane`) -> worktree root path.
    by_branch: HashMap<String, PathBuf>,
    /// Shared object/refs directory (usually the main checkout's `.git`).
    commondir: Option<PathBuf>,
    /// Worktrees currently inside their debounce window.
    pending: HashSet<PathBuf>,
}

/// Snapshot of one worktree, re-read on every reconcile.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Probe {
    branch: String,
    commit: String,
    status: GitStatus,
}

#[derive(Debug)]
enum ProbeError {
    /// The worktree directory no longer exists (removal).
    Gone,
    /// `git` subprocess failure (spawn / exit / timeout).
    Git(String),
}

// ---------------------------------------------------------------------------
// Plane
// ---------------------------------------------------------------------------

/// git data plane: fsevents watcher (primary) + 10s status sweep (safety
/// net) over a repo's main checkout and herdr-managed linked worktrees.
#[derive(Debug)]
pub struct GitPlane {
    /// Main checkout (repo root) — always watched.
    repo_root: PathBuf,
    /// Root of the herdr-managed worktrees; linked worktrees under it are
    /// watched too.
    worktrees_root: PathBuf,
    state: Mutex<PlaneState>,
    /// Millis of the last fsevents-triggered rescan (throttle).
    last_rescan: AtomicU64,
    /// One-shot retry of a registry rescan is scheduled (dedup).
    retry_scheduled: AtomicBool,
}

impl GitPlane {
    /// Watch the main checkout at `repo_root` and every linked worktree
    /// under `worktrees_root`.
    pub fn new(repo_root: PathBuf, worktrees_root: PathBuf) -> Self {
        let repo_root = fs::canonicalize(&repo_root).unwrap_or(repo_root);
        let worktrees_root = fs::canonicalize(&worktrees_root).unwrap_or(worktrees_root);
        Self {
            repo_root,
            worktrees_root,
            state: Mutex::new(PlaneState::default()),
            last_rescan: AtomicU64::new(0),
            retry_scheduled: AtomicBool::new(false),
        }
    }

    /// True for the main checkout itself and any worktree under the herdr
    /// worktrees root.
    fn watches(&self, path: &Path) -> bool {
        path == self.repo_root || path.starts_with(&self.worktrees_root)
    }

    // -- watcher (primary signal) -------------------------------------------

    async fn run_watcher(self: Arc<Self>, sink: PlaneSink) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<()>();
        let mut watcher = match RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                let _ = tx.send(res);
            },
            Config::default(),
        ) {
            Ok(w) => w,
            Err(e) => {
                warn!(
                    error = %e,
                    "git plane: fsevents unavailable — degraded to sweep-only safety net"
                );
                return;
            }
        };
        // Warm the registry (resolves the commondir), then register the ONE
        // recursive watch. Registering more watches later would restart the
        // fsevents stream and drop events, so this never happens again.
        self.rescan(&sink).await;
        let mut watch_registered = self.register_commondir_watch(&mut watcher);
        if watch_registered {
            info!(
                repo = %self.repo_root.display(),
                root = %self.worktrees_root.display(),
                "git plane: fsevents watcher live"
            );
        }
        loop {
            tokio::select! {
                res = rx.recv() => {
                    match res {
                        Some(Ok(event)) => {
                            let affected = self.handle_fs_event(&event, &sink, &cmd_tx).await;
                            for wt in affected {
                                self.debounce(wt, sink.clone());
                            }
                        }
                        Some(Err(e)) => warn!(error = %e, "git plane: fsevents error"),
                        None => {
                            info!("git plane: fsevents stream ended");
                            break;
                        }
                    }
                }
                msg = cmd_rx.recv() => {
                    match msg {
                        Some(()) => {
                            self.retry_scheduled.store(false, Ordering::Relaxed);
                            // Unthrottled: a worktree registered *while* an
                            // earlier rescan was in flight is caught here.
                            self.rescan(&sink).await;
                            if !watch_registered {
                                watch_registered = self.register_commondir_watch(&mut watcher);
                                if watch_registered {
                                    info!("git plane: fsevents watcher live");
                                }
                            }
                        }
                        None => {
                            info!("git plane: command channel closed");
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Register the recursive watch on the commondir (if known). `true` when
    /// the watch is live. A single watch is a deliberate constraint: notify's
    /// fsevents backend restarts the whole stream — `kFSEventStreamEventId
    /// SinceNow` — on every added path, dropping events that change during
    /// the restart.
    fn register_commondir_watch(&self, watcher: &mut RecommendedWatcher) -> bool {
        let cd = {
            let state = self.state.lock().unwrap();
            state.commondir.clone()
        };
        match cd {
            None => false,
            Some(cd) => match watcher.watch(&cd, RecursiveMode::Recursive) {
                Ok(()) => true,
                Err(e) => {
                    warn!(gitdir = %cd.display(), error = %e, "git plane: commondir watch registration failed");
                    false
                }
            },
        }
    }

    /// Resolve an fs event to the worktrees it concerns (rescanning the
    /// registry when a path under `commondir/worktrees/` matches nothing —
    /// a worktree may have appeared).
    async fn handle_fs_event(
        &self,
        event: &Event,
        sink: &PlaneSink,
        cmd_tx: &mpsc::UnboundedSender<()>,
    ) -> Vec<PathBuf> {
        let mut affected: Vec<PathBuf> = Vec::new();
        let mut need_rescan = false;
        for path in &event.paths {
            let canon = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            let (worktrees, maybe_new) = self.map_event_path(&canon);
            need_rescan |= maybe_new;
            for wt in worktrees {
                if !affected.contains(&wt) {
                    affected.push(wt);
                }
            }
        }
        if need_rescan {
            let added = self.maybe_rescan(sink).await;
            if !added && !self.retry_scheduled.swap(true, Ordering::Relaxed) {
                // The rescan found nothing: `git worktree add` registers the
                // entry while its events are still arriving, so retry once
                // after the registration settles.
                let tx = cmd_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(RESCAN_RETRY_DELAY).await;
                    let _ = tx.send(());
                });
            }
        }
        affected
    }

    /// Map one fs event path to the worktree(s) it concerns. The second
    /// return value is true when the path lives under `commondir/worktrees/`
    /// but matches no known gitdir — a worktree may have appeared, so the
    /// caller should rescan the registry.
    fn map_event_path(&self, path: &Path) -> (Vec<PathBuf>, bool) {
        let state = self.state.lock().unwrap();
        let mut best: Option<(usize, PathBuf)> = None;
        for (wt, st) in &state.worktrees {
            let Some(gd) = st.gitdir.as_ref() else { continue };
            // The main checkout's gitdir is the commondir itself; its paths
            // are resolved below so `refs/heads/<b>` can reach the worktree
            // actually checked out on that branch.
            if state.commondir.as_ref() == Some(gd) {
                continue;
            }
            if !path.starts_with(gd) {
                continue;
            }
            let len = gd.as_os_str().len();
            if best.as_ref().is_none_or(|(bl, _)| len > *bl) {
                best = Some((len, wt.clone()));
            }
        }
        if let Some((_, wt)) = best {
            return (vec![wt], false);
        }
        let Some(cd) = state.commondir.as_ref() else {
            return (vec![], false);
        };
        let Ok(rel) = path.strip_prefix(cd) else {
            return (vec![], false);
        };
        let rel = rel.to_string_lossy();
        let all: Vec<PathBuf> = state.worktrees.keys().cloned().collect();
        let mut parts = rel.split('/');
        match parts.next() {
            // refs/heads/<b> and logs/refs/heads/<b> → the worktree(s)
            // checked out on that branch; other shared refs (remotes/tags/
            // stash) → every worktree.
            Some("refs") | Some("logs") => {
                let rest: Vec<&str> = parts.collect();
                let branch = match rest.as_slice() {
                    ["heads", tail @ ..] => Some(tail.join("/")),
                    ["refs", "heads", tail @ ..] => Some(tail.join("/")),
                    _ => None,
                };
                if let Some(branch) = branch {
                    if let Some(wt) = state.by_branch.get(&branch) {
                        return (vec![wt.clone()], false);
                    }
                    return (all, false); // branch not checked out anywhere watched
                }
                match rest.as_slice() {
                    // logs/HEAD and the logs dir itself → the main checkout.
                    ["HEAD"] | [] => (vec![self.repo_root.clone()], false),
                    _ => (all, false),
                }
            }
            Some("packed-refs") => (all, false),
            // A dir under worktrees/ that no known gitdir covers: new (or
            // just removed) linked worktree.
            Some("worktrees") => (vec![], true),
            // commondir root files belong to the main checkout.
            Some("index") | Some("HEAD") | Some("ORIG_HEAD") | Some("FETCH_HEAD")
            | Some("MERGE_HEAD") | Some("CHERRY_PICK_HEAD") | Some("REVERT_HEAD")
            | Some("config") => (vec![self.repo_root.clone()], false),
            // objects/, hooks/, info/, ... — no worktree state to re-read.
            _ => (vec![], false),
        }
    }

    /// Throttled registry rescan triggered by an fs event. Returns true when
    /// the rescan discovered previously-unknown worktrees.
    async fn maybe_rescan(&self, sink: &PlaneSink) -> bool {
        let now = now_millis();
        let last = self.last_rescan.load(Ordering::Relaxed);
        if now.saturating_sub(last) < RESCAN_THROTTLE_MILLIS {
            return false;
        }
        self.last_rescan.store(now, Ordering::Relaxed);
        let added = self.rescan(sink).await;
        !added.is_empty()
    }

    /// Debounce one worktree: at most one reconcile per `DEBOUNCE` window,
    /// measured from the first event of a burst.
    fn debounce(self: &Arc<Self>, wt: PathBuf, sink: PlaneSink) {
        let spawn = {
            let mut state = self.state.lock().unwrap();
            state.pending.insert(wt.clone())
        };
        if !spawn {
            return;
        }
        let plane = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(DEBOUNCE).await;
            {
                let mut state = plane.state.lock().unwrap();
                state.pending.remove(&wt);
            }
            let probe = probe_worktree(&wt).await;
            plane.apply_probe(&wt, probe, &sink).await;
        });
    }

    // -- sweep (safety net, never the primary signal) -----------------------

    /// SAFETY NET: every `SWEEP_INTERVAL`, re-verify every watched worktree
    /// (one concurrent `git` subprocess per worktree) and emit only on
    /// change. Also rescans the registry so WorktreeAdded/WorktreeRemoved
    /// are detected even when fsevents missed them. The primary mechanism
    /// remains the fsevents watcher; this only catches up what it missed.
    async fn run_sweep(&self, sink: PlaneSink) {
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            self.rescan(&sink).await;
            let worktrees: Vec<PathBuf> = {
                let state = self.state.lock().unwrap();
                state.worktrees.keys().cloned().collect()
            };
            let started = Instant::now();
            let mut probes = futures::stream::iter(worktrees)
                .map(|wt| async move {
                    let probe = probe_worktree(&wt).await;
                    (wt, probe)
                })
                .buffer_unordered(MAX_CONCURRENT_PROBES);
            while let Some((wt, probe)) = probes.next().await {
                self.apply_probe(&wt, probe, &sink).await;
            }
            debug!(
                took_ms = started.elapsed().as_millis() as u64,
                "git plane: safety-net sweep complete"
            );
        }
    }

    // -- reconcile ----------------------------------------------------------

    /// Diff a fresh probe against the cached facts for `wt` and emit only
    /// the events that actually changed. Runs for both the watcher's
    /// debounced batches and the sweep, so the emit surface is identical.
    async fn apply_probe(&self, wt: &Path, probe: Result<Probe, ProbeError>, sink: &PlaneSink) {
        let started = Instant::now();
        let probe = match probe {
            Ok(p) => p,
            Err(ProbeError::Gone) => {
                let removed = {
                    let mut state = self.state.lock().unwrap();
                    state.worktrees.remove(wt).is_some()
                };
                if removed {
                    info!(worktree = %wt.display(), "git plane: worktree removed (directory gone)");
                    let _ = sink
                        .send(PlaneEvent::Git(GitEvent::WorktreeRemoved {
                            worktree: wt.to_path_buf(),
                        }))
                        .await;
                }
                return;
            }
            Err(ProbeError::Git(e)) => {
                debug!(worktree = %wt.display(), error = %e, "git plane: probe failed (transient)");
                return;
            }
        };
        let mut events: Vec<GitEvent> = Vec::new();
        {
            let mut state = self.state.lock().unwrap();
            let st = state.worktrees.entry(wt.to_path_buf()).or_default();
            let commit_changed = st.commit.as_deref() != Some(probe.commit.as_str());
            let branch_changed = st.branch.as_deref() != Some(probe.branch.as_str());
            let head_changed = commit_changed || branch_changed;
            if head_changed {
                events.push(GitEvent::HeadMoved {
                    worktree: wt.to_path_buf(),
                    branch: probe.branch.clone(),
                    commit: probe.commit.clone(),
                });
                if commit_changed && !branch_changed && probe.branch != "HEAD" {
                    events.push(GitEvent::CommitOnBranch {
                        worktree: wt.to_path_buf(),
                        branch: probe.branch.clone(),
                        commit: probe.commit.clone(),
                    });
                }
            }
            if st.status.as_ref() != Some(&probe.status) {
                events.push(GitEvent::DirtyChanged {
                    worktree: wt.to_path_buf(),
                    status: probe.status.clone(),
                });
            }
            st.branch = Some(probe.branch.clone());
            st.commit = Some(probe.commit.clone());
            st.status = Some(probe.status.clone());
        }
        let took = started.elapsed();
        for event in events {
            info!(
                source = "git",
                worktree = %wt.display(),
                event = event_kind(&event),
                took_ms = took.as_millis() as u64,
                "git plane event emitted"
            );
            let _ = sink.send(PlaneEvent::Git(event)).await;
        }
        if took > EVENT_BUDGET {
            warn!(
                worktree = %wt.display(),
                took_ms = took.as_millis() as u64,
                budget_ms = EVENT_BUDGET.as_millis() as u64,
                "git plane event over budget"
            );
        }
    }

    // -- registry -----------------------------------------------------------

    /// Rescan `git worktree list --porcelain`, diff against the tracked
    /// set, emit WorktreeAdded/WorktreeRemoved (the first scan reports the
    /// current registry — path-keyed, idempotent for the consumer), and
    /// refresh gitdirs + the branch map. Returns the added worktree paths.
    async fn rescan(&self, sink: &PlaneSink) -> Vec<PathBuf> {
        let entries = match self.scan_worktrees().await {
            Ok(entries) => entries,
            Err(e) => {
                warn!(error = %e, "git plane: worktree registry scan failed");
                return Vec::new();
            }
        };
        let mut tracked: HashMap<PathBuf, WorktreeEntry> = HashMap::new();
        let mut by_branch: HashMap<String, PathBuf> = HashMap::new();
        for entry in entries {
            if !self.watches(&entry.path) {
                continue;
            }
            if !entry.path.is_dir() {
                // Listed but the directory is gone (pre-prune state): the
                // worktree is effectively removed.
                continue;
            }
            if let Some(branch) = &entry.branch {
                by_branch.insert(branch.clone(), entry.path.clone());
            }
            tracked.insert(entry.path.clone(), entry);
        }
        let commondir = tracked
            .get(&self.repo_root)
            .and_then(|e| e.gitdir.as_ref())
            .map(|gd| {
                let marker = gd.join("commondir");
                match fs::read_to_string(&marker) {
                    Ok(content) => {
                        let target = PathBuf::from(content.trim());
                        if target.is_absolute() {
                            target
                        } else {
                            gd.join(target)
                        }
                    }
                    Err(_) => gd.clone(),
                }
            })
            .and_then(|cd| fs::canonicalize(cd).ok());
        let (added, removed, added_paths) = {
            let mut state = self.state.lock().unwrap();
            let mut added: Vec<WorktreeEntry> = Vec::new();
            let mut removed: Vec<PathBuf> = Vec::new();
            for (path, entry) in &tracked {
                match state.worktrees.get_mut(path) {
                    Some(st) => st.gitdir = entry.gitdir.clone(),
                    None => {
                        added.push(entry.clone());
                    }
                }
            }
            for path in state.worktrees.keys() {
                if !tracked.contains_key(path) {
                    removed.push(path.clone());
                }
            }
            for entry in &added {
                state
                    .worktrees
                    .insert(entry.path.clone(), WorktreeState { gitdir: entry.gitdir.clone(), ..Default::default() });
            }
            for path in &removed {
                state.worktrees.remove(path);
            }
            if let Some(cd) = commondir {
                state.commondir = Some(cd);
            }
            state.by_branch = by_branch;
            let added_paths: Vec<PathBuf> = added.iter().map(|e| e.path.clone()).collect();
            (added, removed, added_paths)
        };
        for entry in &added {
            info!(worktree = %entry.path.display(), "git plane: worktree added");
            let _ = sink
                .send(PlaneEvent::Git(GitEvent::WorktreeAdded {
                    worktree: entry.path.clone(),
                }))
                .await;
        }
        for path in &removed {
            info!(worktree = %path.display(), "git plane: worktree removed");
            let _ = sink
                .send(PlaneEvent::Git(GitEvent::WorktreeRemoved {
                    worktree: path.clone(),
                }))
                .await;
        }
        added_paths
    }

    async fn scan_worktrees(&self) -> Result<Vec<WorktreeEntry>, String> {
        let out = run_git(&self.repo_root, &["worktree", "list", "--porcelain"]).await?;
        let mut entries = Vec::new();
        let mut cur: Option<(PathBuf, Option<String>)> = None;
        for line in out.lines() {
            let line = line.trim();
            if line.is_empty() {
                if let Some((path, branch)) = cur.take() {
                    entries.push(WorktreeEntry {
                        gitdir: resolve_gitdir(&path),
                        path,
                        branch,
                    });
                }
                continue;
            }
            if let Some(p) = line.strip_prefix("worktree ") {
                cur = Some((PathBuf::from(p.trim()), None));
                continue;
            }
            let Some((_, branch)) = cur.as_mut() else {
                continue;
            };
            if let Some(b) = line.strip_prefix("branch refs/heads/") {
                *branch = Some(b.trim().to_string());
            }
        }
        if let Some((path, branch)) = cur.take() {
            entries.push(WorktreeEntry {
                gitdir: resolve_gitdir(&path),
                path,
                branch,
            });
        }
        Ok(entries)
    }
}

// ---------------------------------------------------------------------------
// git plumbing (read-only)
// ---------------------------------------------------------------------------

/// Resolve the gitdir of a worktree root. Handles both forms:
/// - `.git` as a DIRECTORY (main checkout), and
/// - `.git` as a FILE carrying `gitdir: <path>` (linked worktrees), where
///   the target may be relative to the worktree root.
fn resolve_gitdir(wt: &Path) -> Option<PathBuf> {
    let dot_git = wt.join(".git");
    if dot_git.is_dir() {
        return fs::canonicalize(&dot_git).ok();
    }
    if dot_git.is_file() {
        let content = fs::read_to_string(&dot_git).ok()?;
        let target = content
            .lines()
            .find_map(|line| line.strip_prefix("gitdir: "))?;
        let path = PathBuf::from(target);
        let abs = if path.is_absolute() { path } else { wt.join(path) };
        return fs::canonicalize(abs).ok();
    }
    None
}

/// One worktree snapshot. Two `git` subprocesses; never mutates anything
/// (`--no-optional-locks` so `status` cannot rewrite the index).
async fn probe_worktree(wt: &Path) -> Result<Probe, ProbeError> {
    if !wt.is_dir() {
        return Err(ProbeError::Gone);
    }
    let branch =
        run_git(wt, &["rev-parse", "--abbrev-ref", "HEAD"]).await.map_err(ProbeError::Git)?;
    let commit = run_git(wt, &["rev-parse", "HEAD"]).await.map_err(ProbeError::Git)?;
    let status =
        run_git(wt, &["status", "--porcelain=v1", "-b"]).await.map_err(ProbeError::Git)?;
    Ok(Probe {
        branch: branch.trim().to_string(),
        commit: commit.trim().to_string(),
        status: parse_status(&status),
    })
}

async fn run_git(wt: &Path, args: &[&str]) -> Result<String, String> {
    let output = tokio::time::timeout(
        GIT_TIMEOUT,
        Command::new("git")
            .arg("-C")
            .arg(wt)
            .arg("--no-optional-locks")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| format!("git {args:?} timed out"))?
    .map_err(|e| format!("spawn git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse `git status --porcelain=v1 -b` into the canonical summary.
fn parse_status(output: &str) -> GitStatus {
    let mut status = GitStatus::default();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("##") {
            // `## main...origin/main [ahead 1, behind 2]` | `[gone]` | none.
            if let Some(bracket) = rest
                .split('[')
                .nth(1)
                .and_then(|s| s.strip_suffix(']'))
            {
                for part in bracket.split(',') {
                    let mut it = part.split_whitespace();
                    match it.next() {
                        Some("ahead") => {
                            if let Some(n) = it.next().and_then(|n| n.parse().ok()) {
                                status.ahead = n;
                            }
                        }
                        Some("behind") => {
                            if let Some(n) = it.next().and_then(|n| n.parse().ok()) {
                                status.behind = n;
                            }
                        }
                        _ => {}
                    }
                }
            }
            continue;
        }
        let bytes = line.as_bytes();
        if bytes.len() < 2 {
            continue;
        }
        match (bytes[0], bytes[1]) {
            (b'?', b'?') => status.dirty_worktree = true, // untracked
            (x, _) if x != b' ' => status.dirty_index = true,
            (_, y) if y != b' ' => status.dirty_worktree = true,
            _ => {}
        }
    }
    status
}

fn event_kind(event: &GitEvent) -> &'static str {
    match event {
        GitEvent::HeadMoved { .. } => "head_moved",
        GitEvent::DirtyChanged { .. } => "dirty_changed",
        GitEvent::WorktreeAdded { .. } => "worktree_added",
        GitEvent::WorktreeRemoved { .. } => "worktree_removed",
        GitEvent::CommitOnBranch { .. } => "commit_on_branch",
    }
}

impl Plane for GitPlane {
    fn source(&self) -> &'static str {
        "git"
    }

    /// Spawn the fsevents watcher (primary) and the 10s sweep (safety net).
    /// Never blocks: all work happens on background tasks.
    fn start(self: Arc<Self>, sink: PlaneSink) {
        let watcher_sink = sink.clone();
        let watcher_plane = self.clone();
        tokio::spawn(async move {
            watcher_plane.run_watcher(watcher_sink).await;
        });
        let sweep_plane = self.clone();
        tokio::spawn(async move {
            sweep_plane.run_sweep(sink).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_clean_branch_header() {
        let status = parse_status("## ws1/git-plane\n");
        assert_eq!(
            status,
            GitStatus {
                dirty_index: false,
                dirty_worktree: false,
                ahead: 0,
                behind: 0,
            }
        );
    }

    #[test]
    fn parses_status_ahead_behind_and_dirty_flags() {
        let out = "## corral-p2...origin/feat/corral-p2 [ahead 1, behind 2]\n\
                   M  src/adapters/git_plane.rs\n\
                    M src/adapters/herdr.rs\n\
                   ?? docs/todo.md\n";
        let status = parse_status(out);
        assert!(status.dirty_index, "staged `M ` must dirty the index");
        assert!(status.dirty_worktree, "unstaged + untracked must dirty the worktree");
        assert_eq!(status.ahead, 1);
        assert_eq!(status.behind, 2);
    }

    #[test]
    fn parses_status_detached_and_gone_upstream() {
        assert_eq!(
            parse_status("## HEAD (no branch)\n"),
            GitStatus::default(),
            "detached HEAD has no upstream counts"
        );
        assert_eq!(
            parse_status("## main...origin/main [gone]\n"),
            GitStatus::default(),
            "deleted upstream counts as 0/0"
        );
        assert_eq!(
            parse_status("## main...origin/main [behind 3]\n"),
            GitStatus {
                dirty_index: false,
                dirty_worktree: false,
                ahead: 0,
                behind: 3,
            }
        );
    }

    #[test]
    fn parses_worktree_list_porcelain() {
        let out = "worktree /Users/jirathip/Projects/herdr-board\n\
                   HEAD 88c2e09a2b75f4966f2e9e7e5bd331b5aa5a65a1\n\
                   branch refs/heads/main\n\
                   \n\
                   worktree /Users/jirathip/.herdr/worktrees/herdr-board/corral-p2\n\
                   HEAD 4cdd2dc9e2fbfecbb4719b51c93d262a9fba7c73\n\
                   branch refs/heads/ws1/git-plane\n\
                   \n\
                   worktree /tmp/detached-wt\n\
                   HEAD 3f3d7f9298773ba15d047e159691e7f072d970b4\n\
                   detached\n";
        let mut entries = Vec::new();
        let mut cur: Option<(PathBuf, Option<String>)> = None;
        for line in out.lines() {
            let line = line.trim();
            if line.is_empty() {
                if let Some((path, branch)) = cur.take() {
                    entries.push((path, branch));
                }
                continue;
            }
            if let Some(p) = line.strip_prefix("worktree ") {
                cur = Some((PathBuf::from(p.trim()), None));
                continue;
            }
            let Some((_, branch)) = cur.as_mut() else { continue };
            if let Some(b) = line.strip_prefix("branch refs/heads/") {
                *branch = Some(b.trim().to_string());
            }
        }
        if let Some((path, branch)) = cur.take() {
            entries.push((path, branch));
        }
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].0, PathBuf::from("/Users/jirathip/Projects/herdr-board"));
        assert_eq!(entries[0].1.as_deref(), Some("main"));
        assert_eq!(entries[1].1.as_deref(), Some("ws1/git-plane"));
        assert_eq!(entries[2].1, None, "detached worktree has no branch");
    }

    #[test]
    fn resolves_gitdir_for_dir_and_file_forms() {
        let root = std::env::temp_dir().join(format!(
            "corral-gitplane-resolve-{}-{}",
            std::process::id(),
            now_millis()
        ));
        // Main-checkout form: `.git` is a directory.
        let main = root.join("main");
        fs::create_dir_all(main.join(".git")).unwrap();
        let main_gitdir = resolve_gitdir(&main).expect("dir form resolves");
        assert_eq!(main_gitdir, fs::canonicalize(main.join(".git")).unwrap());
        // Linked-worktree form: `.git` is a file pointing at a gitdir.
        let commondir = root.join("commondir");
        let wt_gitdir = commondir.join("worktrees/wt1");
        fs::create_dir_all(&wt_gitdir).unwrap();
        let linked = root.join("linked");
        fs::create_dir_all(&linked).unwrap();
        fs::write(linked.join(".git"), format!("gitdir: {}\n", wt_gitdir.display())).unwrap();
        assert_eq!(resolve_gitdir(&linked).expect("file form resolves"), fs::canonicalize(&wt_gitdir).unwrap());
        // Relative target form.
        let rel = root.join("rel-wt");
        fs::create_dir_all(&rel).unwrap();
        let rel_gitdir = commondir.join("worktrees/rel");
        fs::create_dir_all(&rel_gitdir).unwrap();
        fs::write(rel.join(".git"), "gitdir: ../commondir/worktrees/rel\n").unwrap();
        assert_eq!(resolve_gitdir(&rel).expect("relative form resolves"), fs::canonicalize(&rel_gitdir).unwrap());
        let _ = fs::remove_dir_all(&root);
    }
}
