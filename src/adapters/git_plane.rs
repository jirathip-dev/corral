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
//!   `log` / `worktree list` subprocesses (`--no-optional-locks`), never a
//!   mutation.
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

/// Total `git` subprocess invocations since the test binary started (G21
/// acceptance 2: the head fields must add ZERO git calls — the probe tests
/// assert the per-probe delta stays at three).
#[cfg(test)]
static GIT_CALLS: AtomicU64 = AtomicU64::new(0);

/// Serializes the probe tests (G21 re-review F2): `GIT_CALLS` is shared
/// module state, so the delta assertion must not run while another probe
/// test's counted invocations land in the before/after window. Both probe
/// tests hold this lock for their whole body — no other test in the module
/// (or anywhere — `run_git` is module-private) increments the counter.
#[cfg(test)]
static PROBE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    /// (commondir, branch name) -> worktree root path. Keyed per repo so two
    /// repos checked out on the same branch name cannot collide (WS3 F2).
    by_branch: HashMap<(PathBuf, String), PathBuf>,
    /// Shared object/refs dirs — one per scanned repo (usually the repo's
    /// main-checkout `.git`). The watcher registers a recursive watch on
    /// each (WS3 F2: the herdr worktrees root holds worktrees of many repos).
    commondirs: Vec<PathBuf>,
    /// commondir -> main checkout path (commondir root files like `HEAD`/
    /// `index` belong to the main checkout).
    main_checkouts: HashMap<PathBuf, PathBuf>,
    /// Commondirs with an active fsevents watch (watches are added once).
    watched: HashSet<PathBuf>,
    /// Worktrees currently inside their debounce window.
    pending: HashSet<PathBuf>,
    /// Worktree paths a rescan skip has already been warned about (once per
    /// continuous skip period, so a legitimate out-of-scope worktree cannot
    /// spam the log every sweep).
    skip_warned: HashSet<PathBuf>,
}

/// Snapshot of one worktree, re-read on every reconcile.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Probe {
    branch: String,
    commit: String,
    /// First line of the commit message (P4 G21), read by the SAME `git log`
    /// invocation that resolves `commit` — zero extra subprocesses.
    subject: Option<String>,
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
    /// Sink-close signal (WS3 F4): set when a send fails, so the watcher and
    /// sweep loops exit and a supervised restart can re-arm `start()` without
    /// duplicating loops.
    stopped: AtomicBool,
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
            stopped: AtomicBool::new(false),
        }
    }

    /// True for the main checkout itself and any worktree under the herdr
    /// worktrees root. The input is canonicalized best-effort so a symlinked
    /// `HOME` (standard APFS firmlink setups) cannot split the raw porcelain
    /// paths from the canonicalized roots (WS3 F5).
    ///
    /// The canonicalization goes through [`canonicalize_existing_prefix`]
    /// rather than [`fs::canonicalize`] because the interesting inputs are
    /// often paths that do NOT exist — a worktree that was just removed, or
    /// one whose creation event arrives before the directory is visible.
    /// `fs::canonicalize` fails outright on those, and the raw fallback then
    /// compares a `/var/...` spelling against a `/private/var/...` root on
    /// macOS, so every missing path answered `false` (#43).
    fn watches(&self, path: &Path) -> bool {
        let canon = canonicalize_existing_prefix(path);
        canon == self.repo_root || canon.starts_with(&self.worktrees_root)
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
        // Warm the registry (resolves the commondirs), then register the
        // watches. Each commondir gets ONE recursive watch; registering more
        // watches later would restart the fsevents stream and drop events,
        // so per-commondir watches are added once, in one boot batch.
        self.rescan(&sink).await;
        let mut watch_registered = self.register_commondir_watches(&mut watcher);
        if watch_registered {
            info!(
                repo = %self.repo_root.display(),
                root = %self.worktrees_root.display(),
                "git plane: fsevents watchers live"
            );
        }
        loop {
            if self.stopped.load(Ordering::Relaxed) {
                info!("git plane: fsevents watcher exiting (sink closed)");
                break;
            }
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
                                watch_registered = self.register_commondir_watches(&mut watcher);
                                if watch_registered {
                                    info!("git plane: fsevents watchers live");
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

    /// Register one recursive watch per known commondir (each scanned repo
    /// has its own; the herdr worktrees root holds worktrees of many repos —
    /// WS3 F2). `true` when at least one watch is live. Watches are added
    /// once per commondir, in a boot batch, because notify's fsevents
    /// backend restarts the whole stream — `kFSEventStreamEventIdSinceNow` —
    /// on every added path, dropping events that change during the restart.
    fn register_commondir_watches(&self, watcher: &mut RecommendedWatcher) -> bool {
        let mut any = false;
        let commondirs: Vec<PathBuf> = {
            let state = self.state.lock().unwrap();
            state.commondirs.clone()
        };
        for cd in commondirs {
            {
                let state = self.state.lock().unwrap();
                if state.watched.contains(&cd) {
                    continue; // already watched
                }
            }
            match watcher.watch(&cd, RecursiveMode::Recursive) {
                Ok(()) => {
                    self.state.lock().unwrap().watched.insert(cd.clone());
                    any = true;
                    info!(gitdir = %cd.display(), "git plane: commondir watch live");
                }
                Err(e) => {
                    warn!(gitdir = %cd.display(), error = %e, "git plane: commondir watch registration failed");
                }
            }
        }
        any
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
            // The triggering events were mapped against the pre-rescan
            // state, so the new worktrees are not in `affected` — debounce
            // them now or their first observation would wait for the next
            // sweep (≤10s).
            for wt in &added {
                if !affected.contains(wt) {
                    affected.push(wt.clone());
                }
            }
            if added.is_empty() && !self.retry_scheduled.swap(true, Ordering::Relaxed) {
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
            // The main checkout's gitdir is its repo's commondir; its paths
            // are resolved below so `refs/heads/<b>` can reach the worktree
            // actually checked out on that branch.
            if state.commondirs.contains(gd) {
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
        // Commondir-root paths: try each repo's commondir. refs/heads/<b>
        // map to the worktree checked out on that branch (per repo — two
        // repos on the same branch name cannot collide); commondir root
        // files (HEAD/index/config/...) belong to that repo's main checkout;
        // a path under commondir/worktrees/ matching no known gitdir means a
        // worktree may have appeared — rescan.
        for (cd, main) in &state.main_checkouts {
            let Ok(rel) = path.strip_prefix(cd) else {
                continue;
            };
            let rel = rel.to_string_lossy();
            let mut parts = rel.split('/');
            match parts.next() {
                // refs/heads/<b> and logs/refs/heads/<b> → the worktree(s)
                // checked out on that branch; other shared refs (remotes/
                // tags/stash) → every worktree.
                Some("refs") | Some("logs") => {
                    let rest: Vec<&str> = parts.collect();
                    let branch = match rest.as_slice() {
                        ["heads", tail @ ..] => Some(tail.join("/")),
                        ["refs", "heads", tail @ ..] => Some(tail.join("/")),
                        _ => None,
                    };
                    if let Some(branch) = branch {
                        if let Some(wt) = state.by_branch.get(&(cd.clone(), branch)) {
                            return (vec![wt.clone()], false);
                        }
                        return (state.worktrees.keys().cloned().collect(), false); // branch not checked out anywhere watched
                    }
                    match rest.as_slice() {
                        // logs/HEAD and the logs dir itself → the main checkout.
                        ["HEAD"] | [] => return (vec![main.clone()], false),
                        _ => return (state.worktrees.keys().cloned().collect(), false),
                    }
                }
                Some("packed-refs") => return (state.worktrees.keys().cloned().collect(), false),
                // A dir under worktrees/ that no known gitdir covers: new (or
                // just removed) linked worktree.
                Some("worktrees") => return (vec![], true),
                // commondir root files belong to the main checkout.
                Some("index") | Some("HEAD") | Some("ORIG_HEAD") | Some("FETCH_HEAD")
                | Some("MERGE_HEAD") | Some("CHERRY_PICK_HEAD") | Some("REVERT_HEAD")
                | Some("config") => return (vec![main.clone()], false),
                // objects/, hooks/, info/, ... — no worktree state to re-read.
                _ => return (vec![], false),
            }
        }
        (vec![], false)
    }

    /// Throttled registry rescan triggered by an fs event. Returns the
    /// paths of previously-unknown worktrees the rescan discovered (the
    /// caller must debounce them).
    async fn maybe_rescan(&self, sink: &PlaneSink) -> Vec<PathBuf> {
        let now = now_millis();
        let last = self.last_rescan.load(Ordering::Relaxed);
        if now.saturating_sub(last) < RESCAN_THROTTLE_MILLIS {
            return Vec::new();
        }
        self.last_rescan.store(now, Ordering::Relaxed);
        self.rescan(sink).await
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
            // Budget clock starts after the debounce (the 300ms window is
            // outside the 200ms per-event budget) but covers the probe —
            // the dominant cost — through the emits.
            let started = Instant::now();
            let probe = probe_worktree(&wt).await;
            plane.apply_probe(&wt, started, probe, &sink).await;
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
            if self.stopped.load(Ordering::Relaxed) {
                info!("git plane: safety-net sweep exiting (sink closed)");
                break;
            }
            self.rescan(&sink).await;
            if self.stopped.load(Ordering::Relaxed) {
                break;
            }
            let worktrees: Vec<PathBuf> = {
                let state = self.state.lock().unwrap();
                state.worktrees.keys().cloned().collect()
            };
            let sweep_started = Instant::now();
            let mut probes = futures::stream::iter(worktrees)
                .map(|wt| async move {
                    // Budget clock starts at probe start (the sweep has no
                    // debounce) and measures through the emits.
                    let started = Instant::now();
                    let probe = probe_worktree(&wt).await;
                    (wt, started, probe)
                })
                .buffer_unordered(MAX_CONCURRENT_PROBES);
            while let Some((wt, started, probe)) = probes.next().await {
                self.apply_probe(&wt, started, probe, &sink).await;
            }
            debug!(
                took_ms = sweep_started.elapsed().as_millis() as u64,
                "git plane: safety-net sweep complete"
            );
        }
    }

    // -- reconcile ----------------------------------------------------------

    /// Diff a fresh probe against the cached facts for `wt` and emit only
    /// the events that actually changed. Runs for both the watcher's
    /// debounced batches and the sweep, so the emit surface is identical.
    /// `started` is the per-event budget clock (probe start for the sweep,
    /// after the debounce for fs events) — the 200ms budget check covers
    /// the probe through the emits.
    async fn apply_probe(
        &self,
        wt: &Path,
        started: Instant,
        probe: Result<Probe, ProbeError>,
        sink: &PlaneSink,
    ) {
        let probe = match probe {
            Ok(p) => p,
            Err(ProbeError::Gone) => {
                let removed = {
                    let mut state = self.state.lock().unwrap();
                    state.worktrees.remove(wt).is_some()
                };
                if removed {
                    info!(worktree = %wt.display(), "git plane: worktree removed (directory gone)");
                    if sink
                        .send(PlaneEvent::Git(GitEvent::WorktreeRemoved {
                            worktree: wt.to_path_buf(),
                        }))
                        .await
                        .is_err()
                    {
                        self.stopped.store(true, Ordering::Relaxed);
                    }
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
                    subject: probe.subject.clone(),
                });
                if commit_changed && !branch_changed && probe.branch != "HEAD" {
                    events.push(GitEvent::CommitOnBranch {
                        worktree: wt.to_path_buf(),
                        branch: probe.branch.clone(),
                        commit: probe.commit.clone(),
                        subject: probe.subject.clone(),
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
            // WS3 F4: a dead sink means the integrator is gone — mark the
            // plane stopped so the loops exit and a supervised restart can
            // re-arm without duplicating them.
            if sink.send(PlaneEvent::Git(event)).await.is_err() {
                self.stopped.store(true, Ordering::Relaxed);
                return;
            }
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

    /// Rescan every scanned repo, diff against the tracked set, emit
    /// WorktreeAdded/WorktreeRemoved (the first scan reports the current
    /// registry — path-keyed, idempotent for the consumer), and refresh
    /// gitdirs, the branch map and the per-repo commondir topology. Returns
    /// the added worktree paths.
    async fn rescan(&self, sink: &PlaneSink) -> Vec<PathBuf> {
        let scan = self.scan_all_worktrees().await;
        let mut tracked: HashMap<PathBuf, WorktreeEntry> = HashMap::new();
        let mut skipped: HashSet<PathBuf> = HashSet::new();
        for entry in scan.entries {
            if !self.watches(&entry.path) {
                skipped.insert(entry.path.clone());
                continue;
            }
            if !entry.path.is_dir() {
                // Listed but the directory is gone (pre-prune state): the
                // worktree is effectively removed.
                skipped.insert(entry.path.clone());
                continue;
            }
            tracked.insert(entry.path.clone(), entry);
        }
        {
            // A skipped entry means the plane will never emit facts for it —
            // make the drop visible (once per continuous skip period).
            let mut state = self.state.lock().unwrap();
            state.skip_warned.retain(|p| skipped.contains(p));
            for path in &skipped {
                if state.skip_warned.insert(path.clone()) {
                    warn!(
                        worktree = %path.display(),
                        "git plane: worktree listed but skipped (outside the watched set or directory missing)"
                    );
                }
            }
        }
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
            // WS3 F2: per-repo commondir topology + branch map (keyed per
            // repo so equal branch names cannot collide).
            state.main_checkouts = scan.main_checkouts.clone();
            state.commondirs = scan.main_checkouts.keys().cloned().collect();
            state.by_branch = scan.by_branch.clone();
            let tracked_paths: HashSet<PathBuf> = state.worktrees.keys().cloned().collect();
            state
                .by_branch
                .retain(|_, path| tracked_paths.contains(path));
            let added_paths: Vec<PathBuf> = added.iter().map(|e| e.path.clone()).collect();
            (added, removed, added_paths)
        };
        for entry in &added {
            info!(worktree = %entry.path.display(), "git plane: worktree added");
            if sink
                .send(PlaneEvent::Git(GitEvent::WorktreeAdded {
                    worktree: entry.path.clone(),
                }))
                .await
                .is_err()
            {
                self.stopped.store(true, Ordering::Relaxed);
                return added_paths;
            }
        }
        for path in &removed {
            info!(worktree = %path.display(), "git plane: worktree removed");
            if sink
                .send(PlaneEvent::Git(GitEvent::WorktreeRemoved {
                    worktree: path.clone(),
                }))
                .await
                .is_err()
            {
                self.stopped.store(true, Ordering::Relaxed);
                return added_paths;
            }
        }
        added_paths
    }
}

/// Per-repo scan result: merged worktree entries plus the repo topology
/// (commondir -> main checkout) for watcher registration and event mapping.
#[derive(Debug, Default)]
struct ScanResult {
    entries: Vec<WorktreeEntry>,
    /// commondir -> main checkout path, one per scanned repo.
    main_checkouts: HashMap<PathBuf, PathBuf>,
    /// (commondir, branch) -> worktree root path.
    by_branch: HashMap<(PathBuf, String), PathBuf>,
}

impl GitPlane {
    /// WS3 F2: enumerate worktrees PER REPO. `git worktree list` from the
    /// main checkout covers only that repo; the herdr worktrees root holds
    /// one container dir per repo (`<root>/<repo>/<label>`) and the
    /// containers are NOT repos themselves — so each container is probed
    /// with `git worktree list --porcelain`, falling back to its first
    /// worktree child when the container is not a git repo. The first
    /// porcelain entry of each probe is that repo's main checkout; its
    /// gitdir resolves to the repo's commondir. Results are merged and
    /// deduped by canonical path (WS3 F5: one spelling for registry keys,
    /// emitted events and commondir lookups).
    async fn scan_all_worktrees(&self) -> ScanResult {
        let mut sources = vec![self.repo_root.clone()];
        if let Ok(entries) = fs::read_dir(&self.worktrees_root) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    sources.push(entry.path());
                }
            }
        }
        let mut result = ScanResult::default();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for source in sources {
            let mut out = run_git(&source, &["worktree", "list", "--porcelain"]).await.ok();
            if out.is_none() {
                // herdr containers are not repos; probe their worktree
                // children until one answers.
                if let Ok(entries) = fs::read_dir(&source) {
                    for entry in entries.flatten() {
                        let child = entry.path();
                        if child.is_dir() {
                            out = run_git(&child, &["worktree", "list", "--porcelain"])
                                .await
                                .ok();
                            if out.is_some() {
                                break;
                            }
                        }
                    }
                }
            }
            let Some(out) = out else {
                warn!(source = %source.display(), "git plane: worktree scan failed for repo source");
                continue;
            };
            let parsed = parse_worktree_list(&out);
            let commondir = parsed
                .first()
                .map(|(p, _)| resolve_possibly_escaped(p))
                .and_then(|main| resolve_gitdir(&main))
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
            let main_path = parsed
                .first()
                .map(|(p, _)| resolve_possibly_escaped(p))
                .map(|p| fs::canonicalize(&p).unwrap_or(p));
            for (raw, branch) in parsed {
                let resolved = resolve_possibly_escaped(&raw);
                let path = fs::canonicalize(&resolved).unwrap_or(resolved);
                if !seen.insert(path.clone()) {
                    continue;
                }
                if let (Some(branch), Some(cd)) = (&branch, &commondir) {
                    result
                        .by_branch
                        .insert((cd.clone(), branch.clone()), path.clone());
                }
                result.entries.push(WorktreeEntry {
                    gitdir: resolve_gitdir(&path),
                    path,
                });
            }
            if let (Some(cd), Some(main)) = (commondir, main_path) {
                result.main_checkouts.insert(cd, main);
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------
// git plumbing (read-only)
// ---------------------------------------------------------------------------

/// Parse `git worktree list --porcelain` into `(path, branch)` pairs. The
/// path is kept RAW (`git` prints it verbatim — see `parse_worktree_path`);
/// branch is `None` for detached worktrees.
fn parse_worktree_list(output: &str) -> Vec<(PathBuf, Option<String>)> {
    let mut entries = Vec::new();
    let mut cur: Option<(PathBuf, Option<String>)> = None;
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            if let Some(entry) = cur.take() {
                entries.push(entry);
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
    if let Some(entry) = cur.take() {
        entries.push(entry);
    }
    entries
}

/// Resolve a porcelain worktree path to the path that actually exists.
///
/// `git worktree list --porcelain` prints paths RAW — verified against the
/// git source (`printf("worktree %s%c", wt->path, ...)` in
/// `builtin/worktree.c`, stable across versions; a space or tab in a path
/// comes through as the literal byte, confirmed with `od -c`). So the raw
/// path is used as-is whenever it exists on disk — a literal `\t` or `\f`
/// sequence in a real path must never be rewritten.
///
/// The unescape below is a DEFENSIVE fallback for the (currently
/// hypothetical) case where a git build starts C-escaping the path: the
/// raw form then won't exist on disk, and the unescaped variant is tried.
/// If the raw path exists, it always wins.
/// Canonicalize the longest EXISTING prefix of `path`, then re-append the
/// components that do not exist yet.
///
/// [`fs::canonicalize`] fails outright on a missing path, which leaves the
/// caller comparing a raw spelling against canonicalized roots. On macOS
/// those disagree by construction — `/var` is a symlink to `/private/var`
/// and `/tmp` to `/private/tmp` — so a path under a watched root answered
/// `false` purely because it did not exist yet (#43). A filesystem watcher
/// asks about exactly those paths: a worktree just removed, or one whose
/// creation event arrives before the directory is visible.
///
/// Walking up cannot loop: each step drops one component, and a path with
/// no parent (`/`, or a bare relative component) returns the input.
fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path;
    loop {
        if let Ok(base) = fs::canonicalize(cursor) {
            let mut out = base;
            // `tail` was pushed leaf-first while walking up; re-append in
            // reverse so the original order is restored.
            for part in tail.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (cursor.parent(), cursor.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                cursor = parent;
            }
            // No existing ancestor resolved (or a rootless/relative path):
            // fall back to the raw spelling, exactly as before.
            _ => return path.to_path_buf(),
        }
    }
}

fn resolve_possibly_escaped(raw: &Path) -> PathBuf {
    if raw.is_dir() {
        return raw.to_path_buf();
    }
    match unescape_worktree_path(raw.to_string_lossy().as_ref()) {
        Some(unescaped) => {
            let alt = PathBuf::from(unescaped);
            if alt.is_dir() {
                alt
            } else {
                raw.to_path_buf()
            }
        }
        None => raw.to_path_buf(),
    }
}

/// Decode git's C-style quoting (`\b \t \n \v \f \r \0 \" \\ \NNN` octal),
/// used only as the fallback above. Octal escapes are raw UTF-8 BYTES (git
/// quotes `café` as `caf\303\251`), so the output is decoded as UTF-8 at
/// the end. Unknown escapes are kept literally — a path containing a real
/// backslash must never be corrupted by this. Returns `None` when the
/// string has no backslash, is malformed, or does not decode as UTF-8.
fn unescape_worktree_path(s: &str) -> Option<String> {
    if !s.contains('\\') {
        return None;
    }
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next()? {
            '0' => out.push(0),
            'b' => out.push(0x08),
            't' => out.push(b'\t'),
            'n' => out.push(b'\n'),
            'v' => out.push(0x0b),
            'f' => out.push(0x0c),
            'r' => out.push(b'\r'),
            '"' => out.push(b'"'),
            '\\' => out.push(b'\\'),
            ' ' => out.push(b' '),
            d if d.is_ascii_digit() => {
                let mut octal = d.to_digit(8)?;
                for _ in 0..2 {
                    octal = octal * 8 + chars.next()?.to_digit(8)?;
                }
                out.push(u8::try_from(octal).ok()?);
            }
            other => {
                // Not a git escape sequence: keep the backslash literally.
                out.push(b'\\');
                let mut buf = [0u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    String::from_utf8(out).ok()
}

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

/// One worktree snapshot. Three `git` subprocesses; never mutates anything
/// (`--no-optional-locks` so `status` cannot rewrite the index).
async fn probe_worktree(wt: &Path) -> Result<Probe, ProbeError> {
    if !wt.is_dir() {
        return Err(ProbeError::Gone);
    }
    let branch =
        run_git(wt, &["rev-parse", "--abbrev-ref", "HEAD"]).await.map_err(ProbeError::Git)?;
    // P4 G21: ONE invocation resolves both the head commit AND its first-line
    // subject (`%H%n%s`), so `head_sha`/`head_subject` on the snapshot cost
    // zero extra git calls. Fails like `rev-parse HEAD` on an unborn HEAD
    // (exit 128), keeping the unborn case a clean probe failure.
    //
    // Parity caveat (G21 re-review F3): on a PARTIAL/TREELESS clone whose
    // HEAD commit object is missing, `git log -1` may attempt a lazy fetch
    // (bounded by GIT_TIMEOUT) where `rev-parse HEAD` would fail fast —
    // acceptable for herdr-managed worktrees, and the probe stays
    // all-or-nothing (a head-read failure suppresses the whole fact, like
    // any other probe failure).
    let head =
        run_git(wt, &["log", "-1", "--format=%H%n%s"]).await.map_err(ProbeError::Git)?;
    let mut lines = head.lines();
    let commit = lines.next().unwrap_or_default().trim().to_string();
    let subject = lines
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let status =
        run_git(wt, &["status", "--porcelain=v1", "-b"]).await.map_err(ProbeError::Git)?;
    Ok(Probe {
        branch: branch.trim().to_string(),
        commit,
        subject,
        status: parse_status(&status),
    })
}

async fn run_git(wt: &Path, args: &[&str]) -> Result<String, String> {
    #[cfg(test)]
    GIT_CALLS.fetch_add(1, Ordering::Relaxed);
    let output = tokio::time::timeout(
        GIT_TIMEOUT,
        Command::new("git")
            .arg("-C")
            .arg(wt)
            .arg("--no-optional-locks")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // A timed-out `git` must not orphan the subprocess: `Child` is
            // not killed on drop unless asked, and the timeout branch below
            // drops the future (and with it the Child).
            .kill_on_drop(true)
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
    /// Never blocks: all work happens on background tasks. Resets the
    /// sink-close flag so a supervised restart can re-arm cleanly (WS3 F4).
    fn start(self: Arc<Self>, sink: PlaneSink) {
        self.stopped.store(false, Ordering::Relaxed);
        // The previous generation's watcher is gone; its watch registrations
        // died with it, so the fresh watcher must re-register every commondir.
        self.state.lock().unwrap().watched.clear();
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
        let entries = parse_worktree_list(out);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].0, PathBuf::from("/Users/jirathip/Projects/herdr-board"));
        assert_eq!(entries[0].1.as_deref(), Some("main"));
        assert_eq!(entries[1].1.as_deref(), Some("ws1/git-plane"));
        assert_eq!(entries[2].1, None, "detached worktree has no branch");
    }

    #[test]
    fn parses_porcelain_paths_with_special_characters() {
        // git prints worktree paths RAW (no C-escaping) — a space or tab is
        // the literal byte, verified against `od -c` on real output. The
        // parser must round-trip them without mangling.
        let out = "worktree /Users/jirathip/.herdr/worktrees/herdr-board/wt one\n\
                   branch refs/heads/feat/space\n\
                   \n\
                   worktree /tmp/wt\twith\ttab\n\
                   branch refs/heads/feat/tab\n\
                   \n\
                   worktree /tmp/wt\"quote\n\
                   branch refs/heads/feat/quote\n";
        let entries = parse_worktree_list(out);
        assert_eq!(
            entries[0].0,
            PathBuf::from("/Users/jirathip/.herdr/worktrees/herdr-board/wt one"),
            "space in path survives raw"
        );
        assert_eq!(entries[1].0, PathBuf::from("/tmp/wt\twith\ttab"), "tab in path survives raw");
        assert_eq!(entries[2].0, PathBuf::from("/tmp/wt\"quote"));
    }

    #[test]
    fn unescapes_c_style_paths_as_fallback_only() {
        // Defensive fallback: decode git's C-style escapes only for paths
        // that do not exist raw (git does not emit these today).
        assert_eq!(
            unescape_worktree_path(r"/tmp/wt\ one").as_deref(),
            Some("/tmp/wt one"),
            "escaped space decodes"
        );
        assert_eq!(
            unescape_worktree_path(r"/tmp/wt\ttab").as_deref(),
            Some("/tmp/wt\ttab"),
            "escaped tab decodes"
        );
        assert_eq!(
            unescape_worktree_path(r"/tmp/caf\303\251").as_deref(),
            Some("/tmp/caf\u{e9}"),
            "octal escape decodes"
        );
        // A REAL path containing a backslash sequence must never be
        // rewritten: raw existence wins.
        let root = std::env::temp_dir().join(format!(
            "corral-gitplane-esc-{}-{}",
            std::process::id(),
            now_millis()
        ));
        let raw = root.join("wt\\ttab"); // literal backslash + "ttab"
        fs::create_dir_all(&raw).unwrap();
        let real = fs::canonicalize(&raw).unwrap();
        assert_eq!(
            resolve_possibly_escaped(&real),
            real,
            "existing raw path is used as-is, never unescaped"
        );
        // Non-existent raw path with escape-like content → unescaped
        // variant wins only when it exists.
        assert_eq!(resolve_possibly_escaped(Path::new("/nonexistent/raw")), PathBuf::from("/nonexistent/raw"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn watches_accepts_symlinked_roots_and_canonical_spellings() {
        // WS3 F5: a symlinked HOME must not split the canonicalized roots
        // from the raw porcelain/cwd spellings of the same worktrees — every
        // spelling of a watched worktree must match `watches()`.
        let root = std::env::temp_dir().join(format!(
            "corral-gitplane-watches-{}-{}",
            std::process::id(),
            now_millis()
        ));
        let real = root.join("real");
        let repo = real.join("repo");
        let wts = real.join("wts");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&wts).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let plane = GitPlane::new(link.join("repo"), link.join("wts"));

        let worktree = wts.join("herdr-board/corral-p2-ws2");
        fs::create_dir_all(&worktree).unwrap();
        let canonical_wt = fs::canonicalize(&worktree).unwrap();
        assert!(plane.watches(&canonical_wt), "canonical spelling matches");
        assert!(plane.watches(&worktree), "raw spelling matches via canonicalization");
        assert!(
            plane.watches(&link.join("wts/herdr-board/corral-p2-ws2")),
            "symlinked spelling matches"
        );
        assert!(plane.watches(&fs::canonicalize(&repo).unwrap()), "main checkout matches");

        // Out-of-scope: a directory that EXISTS and is under neither root.
        let outside = root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        assert!(!plane.watches(&outside), "out-of-scope path does not match");
        let _ = fs::remove_dir_all(&root);
    }

    /// #43 regression. Both directions of the MISSING-path case, which was
    /// previously unpinned — and which used to answer `false` on macOS for
    /// every input, because `fs::canonicalize` fails on a path that does not
    /// exist and the raw `/var/...` spelling never matched a
    /// `/private/var/...` root.
    ///
    /// This is not an edge case for a filesystem watcher: a worktree that
    /// was just removed, or one whose creation event arrives before the
    /// directory is visible, is exactly what `watches()` gets asked about.
    #[test]
    fn watches_resolves_paths_that_do_not_exist_yet() {
        let root = std::env::temp_dir().join(format!(
            "corral-gitplane-missing-{}-{}",
            std::process::id(),
            now_millis()
        ));
        let real = root.join("real");
        let repo = real.join("repo");
        let wts = real.join("wts");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&wts).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let plane = GitPlane::new(link.join("repo"), link.join("wts"));

        // A worktree under the watched root that does NOT exist on disk.
        // Both spellings must match: the raw one (`/var/...` on macOS) and
        // the symlinked one. Before #43 both answered false.
        let missing = wts.join("not-created-yet");
        assert!(
            !missing.exists(),
            "precondition: the path must not exist, or this proves nothing"
        );
        assert!(
            plane.watches(&missing),
            "a missing path under the worktrees root is still watched"
        );
        assert!(
            plane.watches(&link.join("wts/not-created-yet")),
            "the symlinked spelling of that missing path is watched too"
        );

        // Deeper, and with several missing components — the helper has to
        // walk up more than one level to find an existing ancestor.
        assert!(
            plane.watches(&wts.join("a/b/c/deeply-missing")),
            "several missing components still resolve to the watched root"
        );

        // The negative direction must survive the fix: a missing path
        // OUTSIDE both roots stays out of scope. A helper that resolved
        // everything to the worktrees root would pass the assertions above
        // and fail this one.
        assert!(
            !plane.watches(&root.join("outside/also-missing")),
            "a missing path outside both roots is not watched"
        );
        assert!(
            !plane.watches(Path::new("/nonexistent-root-level-path/x")),
            "a missing path with no existing ancestor is not watched"
        );

        let _ = fs::remove_dir_all(&root);
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

    /// A throwaway repo with one committed file, returning (root, HEAD sha).
    fn scratch_repo(tag: &str) -> (PathBuf, String) {
        let root = std::env::temp_dir().join(format!(
            "corral-gitplane-probe-{}-{}-{tag}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .expect("git subprocess runs")
        };
        assert!(git(&["init", "-b", "main"]).status.success());
        assert!(git(&["config", "user.email", "plane@test.local"]).status.success());
        assert!(git(&["config", "user.name", "Plane Test"]).status.success());
        fs::write(root.join("README.md"), "hello\n").unwrap();
        assert!(git(&["add", "README.md"]).status.success());
        assert!(git(&["commit", "-m", "initial"]).status.success());
        let sha = git(&["rev-parse", "HEAD"]).stdout;
        let sha = String::from_utf8_lossy(&sha).trim().to_string();
        (root, sha)
    }

    #[tokio::test]
    async fn probe_returns_sha_and_first_line_subject_in_three_git_calls() {
        // F2: serialize against the other probe test — GIT_CALLS is shared.
        let _guard = PROBE_LOCK.lock().await;
        // G21 acceptance 2: the head fields ride the commit probe — the
        // subject is read by the SAME `git log` that resolves the sha, so a
        // probe is exactly three git invocations (branch, head+subject,
        // status), never four.
        let (root, _) = scratch_repo("head-fields");
        // Multi-line message: the subject is the FIRST line only.
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .expect("git subprocess runs")
        };
        assert!(git(&["commit", "--allow-empty", "-m", "second line\n\nbody paragraph"]).status.success());
        let sha = String::from_utf8_lossy(&git(&["rev-parse", "HEAD"]).stdout).trim().to_string();

        let before = GIT_CALLS.load(Ordering::Relaxed);
        let probe = probe_worktree(&root).await.expect("probe succeeds");
        let delta = GIT_CALLS.load(Ordering::Relaxed) - before;
        assert_eq!(delta, 3, "head_sha/head_subject must add zero git calls (probe = 3)");
        assert_eq!(probe.commit, sha, "probe resolves the same HEAD sha as rev-parse");
        assert_eq!(probe.subject.as_deref(), Some("second line"), "subject is the commit's first line");
        assert_eq!(probe.branch, "main");
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn probe_survives_unborn_head_with_null_equivalents() {
        // F2: serialize against the other probe test — GIT_CALLS is shared.
        let _guard = PROBE_LOCK.lock().await;
        // Unborn/empty checkout: HEAD has no commit, so the head probe fails
        // like `rev-parse HEAD` did — the plane emits no head facts and the
        // snapshot's head_sha/head_subject stay null (acceptance 1).
        let root = std::env::temp_dir().join(format!(
            "corral-gitplane-unborn-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&root).unwrap();
        let init = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["init", "-b", "main"])
            .output()
            .expect("git subprocess runs");
        assert!(init.status.success());
        assert!(probe_worktree(&root).await.is_err(), "unborn HEAD must fail the probe");
        let _ = fs::remove_dir_all(&root);
    }
}
