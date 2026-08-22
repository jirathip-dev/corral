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
//! A single watch per commondir is a deliberate constraint: notify 8.2.0's
//! fsevents backend *restarts the whole stream* (with
//! `kFSEventStreamEventIdSinceNow`) whenever `watch()` adds a path to an
//! existing watcher. The plane therefore creates a fresh watcher for each
//! commondir and registers exactly one path before keeping that stream alive;
//! later topology discovery adds another fresh watcher and never mutates an
//! existing stream. This avoids notify's stop/restart busy-yield path and
//! keeps event delivery independent across repositories. One watch per
//! commondir covers future worktrees too; the 60s sweep is the backstop for
//! anything outside it.
//!
//! Every fs event is mapped to the worktree(s) it concerns (most-specific
//! gitdir prefix, then `refs/heads/<branch>` → the worktree checked out on
//! that branch). Events for a path under `commondir/worktrees/` that matches
//! no known gitdir trigger a registry rescan, so `git worktree add` is
//! discovered within one event, not one sweep. Debounced 300ms per worktree;
//! each debounced batch re-reads one `git status --porcelain=v2 --branch`
//! snapshot (plus a subject read only when HEAD moved) and emits only on
//! change. Each reconcile cycle is measured against a 200ms budget and
//! logged with `warn!` when exceeded.
//!
//! ## Safety net (never the primary signal)
//!
//! A 60s `git status` sweep across all watched worktrees — one concurrent
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
use std::fmt::Display;
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
use crate::core::util::{canonicalize_existing_prefix, now_millis};

/// Per-worktree debounce window (the brief's 300ms).
const DEBOUNCE: Duration = Duration::from_millis(300);
/// Safety-net sweep cadence. FSEvents is the primary signal; this is only a
/// bounded backstop for events the OS coalesced or missed, so it must not be
/// a hot poll loop of its own.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
/// Registry-only safety-net cadence. FSEvents plus a one-shot startup rescan
/// are the primary topology signals; this is a bounded backstop for repos or
/// worktrees registered without a deliverable event.
const TOPOLOGY_INTERVAL: Duration = Duration::from_secs(10);
/// One-shot startup rescan after the watcher warm-up. This closes the race
/// where `git worktree add` completes before the initial FSEvents stream is
/// live, without polling repeatedly during idle operation.
const STARTUP_RESCAN_DELAY: Duration = Duration::from_millis(500);
/// Per-event processing budget; exceedances are logged (`warn!`).
const EVENT_BUDGET: Duration = Duration::from_millis(200);
/// Upper bound on a single `git` subprocess.
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
/// Concurrent `git` subprocesses during the sweep (one per worktree, bounded).
const MAX_CONCURRENT_PROBES: usize = 4;
/// Throttle on fsevents-triggered registry rescans (which may discover a new
/// commondir and create one fresh watcher).
const RESCAN_THROTTLE_MILLIS: u64 = 1000;
/// Delay before the one-shot retry of a registry rescan that found nothing:
/// `git worktree add` registers the entry *while* the events are still
/// arriving, so the first rescan can race the registration.
const RESCAN_RETRY_DELAY: Duration = Duration::from_millis(400);
/// Upper bound on fsevents frames coalesced into one watcher batch before
/// the loop yields again.
const FS_EVENT_BATCH_MAX: usize = 256;

/// Commands sent to the watcher task by fsevents handling and the safety-net
/// sweep. Registration is intentionally separate from rescanning: the sweep
/// already has the fresh topology, while an event-triggered retry needs both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatcherCommand {
    RegisterNew,
    RescanAndRegister,
}

/// Total `git` subprocess invocations since the test binary started (G21
/// acceptance 2: the head fields must add ZERO git calls — the probe tests
/// assert the per-probe delta stays at three).
#[cfg(test)]
static GIT_CALLS: AtomicU64 = AtomicU64::new(0);

/// Serializes the probe tests (G21 re-review F2): `GIT_CALLS` is shared
/// module state, so the delta assertion must not run while another test's
/// counted `run_git` invocations (via `probe_worktree` OR `rescan`) land in
/// the before/after window. Every test in the module that calls either
/// holds this lock for its whole body (`run_git` is module-private, so
/// nothing outside the module can increment the counter unguarded).
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
    subject: Option<String>,
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

/// git data plane: fsevents watcher (primary) + 60s status sweep (safety
/// net) over a repo's main checkout and herdr-managed linked worktrees.
#[derive(Debug)]
pub struct GitPlane {
    /// Explicit primary checkout roots — always watched.
    repo_roots: Vec<PathBuf>,
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
        Self::with_repo_roots(vec![repo_root], worktrees_root)
    }

    /// Watch every explicit primary checkout and the Herdr-managed linked
    /// worktree root. The default constructor keeps the historical single
    /// Corral root; the daemon uses this form after loading additional roots
    /// from the fleet registry.
    pub fn with_repo_roots(repo_roots: Vec<PathBuf>, worktrees_root: PathBuf) -> Self {
        let mut canonical_roots = Vec::new();
        for root in repo_roots {
            let root = fs::canonicalize(&root).unwrap_or(root);
            if !canonical_roots.contains(&root) {
                canonical_roots.push(root);
            }
        }
        let worktrees_root = fs::canonicalize(&worktrees_root).unwrap_or(worktrees_root);
        Self {
            repo_roots: canonical_roots,
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
    /// Canonicalization goes through [`canonicalize_existing_prefix`] rather
    /// than [`fs::canonicalize`], which fails outright on a path that does
    /// not exist and leaves the caller comparing a raw `/var/...` spelling
    /// against a `/private/var/...` root on macOS — so every missing path
    /// answered `false` while Linux answered `true` (#43).
    ///
    /// **Scope of that fix, stated precisely:** the only production caller is
    /// [`Self::rescan`], which filters `git worktree list --porcelain`
    /// output. Entries that EXIST arrive already canonicalized (the scan
    /// falls back to the raw spelling only when canonicalization fails,
    /// i.e. when the path is gone), and a missing one is caught two lines
    /// later by an `is_dir()` guard — so today this changes no observable
    /// behaviour either way. It is a latent
    /// correctness fix: it removes a trap for the next caller, for which the
    /// platform-dependent answer would be a real bug. Filesystem events do
    /// NOT come through here; they go through `handle_fs_event` →
    /// `map_event_path`.
    fn watches(&self, path: &Path) -> bool {
        let canon = canonicalize_existing_prefix(path);
        self.repo_roots.contains(&canon) || canon.starts_with(&self.worktrees_root)
    }

    // -- watcher (primary signal) -------------------------------------------

    async fn run_watcher(
        self: Arc<Self>,
        sink: PlaneSink,
        mut cmd_rx: mpsc::Receiver<WatcherCommand>,
        cmd_tx: mpsc::Sender<WatcherCommand>,
    ) {
        // A callback fan-in channel lets each commondir keep its own
        // RecommendedWatcher/FSEventStream alive without requiring a mutable
        // watcher to be selected from the event loop.
        let (event_tx, mut event_rx) =
            mpsc::unbounded_channel::<(PathBuf, notify::Result<Event>)>();
        let mut watchers = HashMap::<PathBuf, RecommendedWatcher>::new();

        // Warm the registry before creating streams. Every watcher below is
        // fresh and receives exactly one path, so notify 8.2.0 never has to
        // stop/restart an existing FSEventStream to add another commondir.
        self.rescan(&sink).await;
        if self.register_new_commondir_watchers(&mut watchers, |cd| {
            Self::new_commondir_watcher(cd, &event_tx)
        }) {
            info!(
                repos = ?self.repo_roots,
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
                item = event_rx.recv() => {
                    match item {
                        Some((_commondir, Ok(first))) => {
                            let mut batch = Vec::with_capacity(FS_EVENT_BATCH_MAX);
                            batch.push(first);
                            let mut stream_ended = false;
                            while batch.len() < FS_EVENT_BATCH_MAX {
                                match event_rx.try_recv() {
                                    Ok((_, Ok(event))) => batch.push(event),
                                    Ok((commondir, Err(e))) => {
                                        warn!(
                                            gitdir = %commondir.display(),
                                            error = %e,
                                            "git plane: fsevents error"
                                        );
                                    }
                                    Err(mpsc::error::TryRecvError::Empty) => break,
                                    Err(mpsc::error::TryRecvError::Disconnected) => {
                                        stream_ended = true;
                                        break;
                                    }
                                }
                            }
                            let affected =
                                self.handle_fs_event_batch(&batch, &sink, &cmd_tx).await;
                            for wt in affected {
                                self.debounce(wt, sink.clone());
                            }
                            if stream_ended {
                                info!("git plane: fsevents stream ended");
                                break;
                            }
                        }
                        Some((commondir, Err(e))) => {
                            warn!(gitdir = %commondir.display(), error = %e, "git plane: fsevents error");
                        }
                        None => {
                            info!("git plane: fsevents stream ended");
                            break;
                        }
                    }
                }
                msg = cmd_rx.recv() => {
                    match msg {
                        Some(first) => {
                            // Coalesce a burst of registration requests: only
                            // the strongest pending command matters, and
                            // registration itself is idempotent.
                            let mut command = first;
                            while let Ok(next) = cmd_rx.try_recv() {
                                if next == WatcherCommand::RescanAndRegister {
                                    command = next;
                                }
                            }
                            match command {
                                WatcherCommand::RegisterNew => {
                                    if self.register_new_commondir_watchers(&mut watchers, |cd| {
                                        Self::new_commondir_watcher(cd, &event_tx)
                                    }) {
                                        info!("git plane: fsevents watchers live");
                                    }
                                }
                                WatcherCommand::RescanAndRegister => {
                                    self.retry_scheduled.store(false, Ordering::Relaxed);
                                    // Unthrottled: a worktree or repository
                                    // registered while an earlier rescan was in
                                    // flight is caught here, then gets its own
                                    // immutable stream.
                                    self.rescan(&sink).await;
                                    if self.register_new_commondir_watchers(
                                        &mut watchers,
                                        |cd| Self::new_commondir_watcher(cd, &event_tx),
                                    ) {
                                        info!("git plane: fsevents watchers live");
                                    }
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

    /// Construct one watcher for one commondir. This is the only `watch` call
    /// for the returned watcher.
    fn new_commondir_watcher(
        commondir: &Path,
        event_tx: &mpsc::UnboundedSender<(PathBuf, notify::Result<Event>)>,
    ) -> notify::Result<RecommendedWatcher> {
        let callback_source = commondir.to_path_buf();
        let callback_tx = event_tx.clone();
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                let _ = callback_tx.send((callback_source.clone(), res));
            },
            Config::default(),
        )?;
        // This is the only `watch` call for this watcher. In notify 8.2.0 a
        // second call would stop and rebuild its entire FSEventStream.
        watcher.watch(commondir, RecursiveMode::Recursive)?;
        Ok(watcher)
    }

    /// Add one immutable watcher per known commondir (each scanned repo has
    /// its own; the herdr worktrees root holds worktrees of many repos — WS3
    /// F2). `true` when at least one new watcher is live.
    ///
    /// The factory is deliberately generic so the lifecycle contract can be
    /// tested without racing real FSEvents: a successful factory call creates
    /// one watcher and registers one path. Existing entries are never passed
    /// to the factory again, so no existing notify stream can enter
    /// `stop()`/`run()` merely because topology was rescanned.
    fn register_new_commondir_watchers<W, E, F>(
        &self,
        watchers: &mut HashMap<PathBuf, W>,
        mut create: F,
    ) -> bool
    where
        E: Display,
        F: FnMut(&Path) -> Result<W, E>,
    {
        let commondirs: Vec<PathBuf> = {
            let state = self.state.lock().unwrap();
            state.commondirs.clone()
        };

        let mut added = false;
        for cd in commondirs {
            if watchers.contains_key(&cd) {
                continue;
            }
            match create(&cd) {
                Ok(watcher) => {
                    watchers.insert(cd.clone(), watcher);
                    info!(gitdir = %cd.display(), "git plane: commondir watch live");
                    added = true;
                }
                Err(e) => {
                    warn!(gitdir = %cd.display(), error = %e, "git plane: commondir watch registration failed");
                }
            }
        }
        added
    }

    /// Resolve a batch of fs events to the worktrees they concern (rescanning
    /// the registry once when any path under `commondir/worktrees/` matches
    /// nothing — a worktree may have appeared).
    async fn handle_fs_event_batch(
        &self,
        events: &[Event],
        sink: &PlaneSink,
        cmd_tx: &mpsc::Sender<WatcherCommand>,
    ) -> Vec<PathBuf> {
        let mut affected: Vec<PathBuf> = Vec::new();
        let mut need_rescan = false;
        for event in events {
            for path in &event.paths {
                let canon = canonicalize_existing_prefix(path);
                let (worktrees, maybe_new) = self.map_event_path(&canon);
                need_rescan |= maybe_new;
                for wt in worktrees {
                    if !affected.contains(&wt) {
                        affected.push(wt);
                    }
                }
            }
        }
        if need_rescan {
            let added = self.maybe_rescan(sink).await;
            // The triggering events were mapped against the pre-rescan
            // state, so the new worktrees are not in `affected` — debounce
            // them now or their first observation would wait for the next
            // sweep (≤60s).
            for wt in &added {
                if !affected.contains(wt) {
                    affected.push(wt.clone());
                }
            }
            // The rescan may have discovered a new repository along with
            // its first worktree. Request registration regardless of the
            // added count; topology discovery must not depend on the retry
            // branch below.
            let _ = cmd_tx.try_send(WatcherCommand::RegisterNew);
            if added.is_empty() && !self.retry_scheduled.swap(true, Ordering::Relaxed) {
                // The rescan found nothing: `git worktree add` registers the
                // entry while its events are still arriving, so retry once
                // after the registration settles.
                let tx = cmd_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(RESCAN_RETRY_DELAY).await;
                    let _ = tx.send(WatcherCommand::RescanAndRegister).await;
                });
            }
        }
        affected
    }

    /// Test/debug wrapper for a single fs event.
    #[cfg(test)]
    async fn handle_fs_event(
        &self,
        event: &Event,
        sink: &PlaneSink,
        cmd_tx: &mpsc::Sender<WatcherCommand>,
    ) -> Vec<PathBuf> {
        self.handle_fs_event_batch(std::slice::from_ref(event), sink, cmd_tx)
            .await
    }

    /// Map one fs event path to the worktree(s) it concerns. The second
    /// return value is true when the path lives under `commondir/worktrees/`
    /// but matches no known gitdir — a worktree may have appeared, so the
    /// caller should rescan the registry.
    fn map_event_path(&self, path: &Path) -> (Vec<PathBuf>, bool) {
        let state = self.state.lock().unwrap();
        let mut best: Option<(usize, PathBuf)> = None;
        for (wt, st) in &state.worktrees {
            let Some(gd) = st.gitdir.as_ref() else {
                continue;
            };
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
                Some("index")
                | Some("HEAD")
                | Some("ORIG_HEAD")
                | Some("FETCH_HEAD")
                | Some("MERGE_HEAD")
                | Some("CHERRY_PICK_HEAD")
                | Some("REVERT_HEAD")
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
            let (cached_commit, cached_subject) = {
                let state = plane.state.lock().unwrap();
                state
                    .worktrees
                    .get(&wt)
                    .map(|st| (st.commit.clone(), st.subject.clone()))
                    .unwrap_or_default()
            };
            let probe =
                probe_worktree(&wt, cached_commit.as_deref(), cached_subject.as_deref()).await;
            plane.apply_probe(&wt, started, probe, &sink).await;
        });
    }

    // -- sweep (safety net, never the primary signal) -----------------------

    /// SAFETY NET: every `SWEEP_INTERVAL`, re-verify every watched worktree
    /// (one concurrent `git` subprocess per worktree) and emit only on
    /// change. Also rescans the registry so WorktreeAdded/WorktreeRemoved
    /// are detected even when fsevents missed them. The primary mechanism
    /// remains the fsevents watcher; this only catches up what it missed.
    async fn run_sweep(
        self: Arc<Self>,
        sink: PlaneSink,
        watcher_cmd_tx: mpsc::Sender<WatcherCommand>,
    ) {
        // Close the startup race once: a worktree added before the fsevents
        // stream is live is caught by this bounded one-shot rescan.
        tokio::time::sleep(STARTUP_RESCAN_DELAY).await;
        if self.stopped.load(Ordering::Relaxed) {
            info!("git plane: safety-net sweep exiting (sink closed)");
            return;
        }
        let added = self.rescan(&sink).await;
        if self.stopped.load(Ordering::Relaxed) {
            return;
        }
        let _ = watcher_cmd_tx.try_send(WatcherCommand::RegisterNew);
        for wt in added {
            self.debounce(wt, sink.clone());
        }

        let mut topology_ticker = tokio::time::interval(TOPOLOGY_INTERVAL);
        topology_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut status_ticker = tokio::time::interval(SWEEP_INTERVAL);
        status_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Consume the immediate first tick; the watcher already did the boot
        // rescan and the startup rescan above ran the same reconciliation.
        topology_ticker.tick().await;
        loop {
            tokio::select! {
                _ = topology_ticker.tick() => {
                    if self.stopped.load(Ordering::Relaxed) {
                        info!("git plane: safety-net sweep exiting (sink closed)");
                        break;
                    }
                    let added = self.rescan(&sink).await;
                    if self.stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    let _ = watcher_cmd_tx.try_send(WatcherCommand::RegisterNew);
                    for wt in added {
                        self.debounce(wt, sink.clone());
                    }
                }
                _ = status_ticker.tick() => {
                    if self.stopped.load(Ordering::Relaxed) {
                        info!("git plane: safety-net sweep exiting (sink closed)");
                        break;
                    }
                    self.rescan(&sink).await;
                    if self.stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    // `rescan` refreshes commondirs as well as worktrees. Ask
                    // the watcher task to add only topology it has not seen;
                    // it never reopens an existing stream.
                    let _ = watcher_cmd_tx.try_send(WatcherCommand::RegisterNew);
                    let worktrees: Vec<(PathBuf, Option<String>, Option<String>)> = {
                        let state = self.state.lock().unwrap();
                        state
                            .worktrees
                            .iter()
                            .map(|(wt, st)| (wt.clone(), st.commit.clone(), st.subject.clone()))
                            .collect()
                    };
                    let sweep_started = Instant::now();
                    let mut probes = futures::stream::iter(worktrees)
                        .map(|(wt, cached_commit, cached_subject)| async move {
                            // Budget clock starts at probe start (the sweep
                            // has no debounce) and measures through the emits.
                            let started = Instant::now();
                            let probe = probe_worktree(
                                &wt,
                                cached_commit.as_deref(),
                                cached_subject.as_deref(),
                            )
                            .await;
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
            st.subject = probe.subject.clone();
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
                state.worktrees.insert(
                    entry.path.clone(),
                    WorktreeState {
                        gitdir: entry.gitdir.clone(),
                        ..Default::default()
                    },
                );
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
        let mut sources = self.repo_roots.clone();
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
            let mut out = run_git(&source, &["worktree", "list", "--porcelain"])
                .await
                .ok();
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

// `canonicalize_existing_prefix` is shared with the integrator and Herdr
// attribution resolver so all path-keyed facts use the same alias rules.
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
fn resolve_possibly_escaped(raw: &Path) -> PathBuf {
    if raw.is_dir() {
        return raw.to_path_buf();
    }
    match unescape_worktree_path(raw.to_string_lossy().as_ref()) {
        Some(unescaped) => {
            let alt = PathBuf::from(unescaped);
            if alt.is_dir() { alt } else { raw.to_path_buf() }
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
        let abs = if path.is_absolute() {
            path
        } else {
            wt.join(path)
        };
        return fs::canonicalize(abs).ok();
    }
    None
}

/// One worktree snapshot. One `git status` subprocess; `git log` runs only
/// when the cached HEAD changed (or no cache exists). Never mutates anything
/// (`--no-optional-locks` so `status` cannot rewrite the index).
async fn probe_worktree(
    wt: &Path,
    cached_commit: Option<&str>,
    cached_subject: Option<&str>,
) -> Result<Probe, ProbeError> {
    if !wt.is_dir() {
        return Err(ProbeError::Gone);
    }
    let status = run_git(wt, &["status", "--porcelain=v2", "--branch"])
        .await
        .map_err(ProbeError::Git)?;
    let (branch, mut commit, status) = parse_status_v2(&status)?;
    let subject = if cached_commit == Some(commit.as_str()) {
        cached_subject.map(str::to_string)
    } else {
        // P4 G21: ONE invocation resolves both the head commit AND its
        // first-line subject when HEAD moved. The status probe already
        // carries `oid`, so no `rev-parse` is needed on the hot path.
        let head = run_git(wt, &["log", "-1", "--format=%H%n%s"])
            .await
            .map_err(ProbeError::Git)?;
        let mut lines = head.lines();
        commit = lines.next().unwrap_or_default().trim().to_string();
        lines
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Ok(Probe {
        branch,
        commit,
        subject,
        status,
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

/// Parse `git status --porcelain=v1 -b` (kept for precise regression
/// coverage only; production uses the v2 parser).
#[cfg(test)]
fn parse_status(output: &str) -> GitStatus {
    let mut status = GitStatus::default();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("##") {
            // `## main...origin/main [ahead 1, behind 2]` | `[gone]` | none.
            if let Some(bracket) = rest.split('[').nth(1).and_then(|s| s.strip_suffix(']')) {
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
        if line.starts_with('#') {
            continue;
        }
        parse_status_line(line, &mut status);
    }
    status
}

#[cfg(test)]
fn parse_status_line(line: &str, status: &mut GitStatus) {
    let bytes = line.as_bytes();
    if bytes.len() < 2 {
        return;
    }
    match (bytes[0], bytes[1]) {
        (b'?', _) => status.dirty_worktree = true, // untracked (v1 `??`, v2 `?`)
        (x, _) if x != b' ' => status.dirty_index = true,
        (_, y) if y != b' ' => status.dirty_worktree = true,
        _ => {}
    }
}

/// Parse one porcelain v2 status record. Unlike v1, the XY state is at bytes
/// 2..4 (`1 <XY> ...`), so the shared v1 parser must not be reused here.
fn parse_status_v2_record(line: &str, status: &mut GitStatus) {
    let bytes = line.as_bytes();
    if line.starts_with("? ") {
        status.dirty_worktree = true;
        return;
    }
    if bytes.len() >= 4
        && (bytes[0] == b'1' || bytes[0] == b'2' || bytes[0] == b'u')
        && bytes[1] == b' '
    {
        // Porcelain v2 uses `.` in place of the v1 ` ` clean marker.
        if bytes[2] != b'.' && bytes[2] != b' ' {
            status.dirty_index = true;
        }
        if bytes[3] != b'.' && bytes[3] != b' ' {
            status.dirty_worktree = true;
        }
    }
}

/// Parse `git status --porcelain=v2 --branch` into branch, HEAD oid and the
/// canonical summary. Unborn HEAD is a probe failure, matching the old
/// `rev-parse` + `git log` all-or-nothing semantics.
fn parse_status_v2(output: &str) -> Result<(String, String, GitStatus), ProbeError> {
    let mut branch = String::new();
    let mut commit = None;
    let mut status = GitStatus::default();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("# branch.oid ") {
            let oid = rest.trim();
            if oid != "(initial)" {
                commit = Some(oid.to_string());
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            let head = rest.trim();
            branch = if head == "(detached)" {
                "HEAD".to_string()
            } else {
                head.to_string()
            };
            continue;
        }
        if let Some(rest) = line.strip_prefix("# branch.ab ") {
            for part in rest.split_whitespace() {
                if let Some(n) = part.strip_prefix('+').and_then(|n| n.parse().ok()) {
                    status.ahead = n;
                } else if let Some(n) = part.strip_prefix('-').and_then(|n| n.parse().ok()) {
                    status.behind = n;
                }
            }
            continue;
        }
        if !line.starts_with('#') {
            parse_status_v2_record(line, &mut status);
        }
    }
    let commit = commit.ok_or_else(|| ProbeError::Git("unborn HEAD has no commit".to_string()))?;
    Ok((branch, commit, status))
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

    /// Spawn the fsevents watcher (primary) and the 60s sweep (safety net).
    /// Never blocks: all work happens on background tasks. Resets the
    /// sink-close flag so a supervised restart can re-arm cleanly (WS3 F4).
    fn start(self: Arc<Self>, sink: PlaneSink) {
        self.stopped.store(false, Ordering::Relaxed);
        let (watcher_cmd_tx, watcher_cmd_rx) = mpsc::channel::<WatcherCommand>(1);
        let watcher_sink = sink.clone();
        let watcher_plane = self.clone();
        let watcher_cmd_tx_for_events = watcher_cmd_tx.clone();
        tokio::spawn(async move {
            watcher_plane
                .run_watcher(watcher_sink, watcher_cmd_rx, watcher_cmd_tx_for_events)
                .await;
        });
        let sweep_plane = self.clone();
        tokio::spawn(async move {
            sweep_plane.run_sweep(sink, watcher_cmd_tx).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Poll `rx` until a `WorktreeRemoved` for `want` arrives or `timeout`
    /// elapses. Non-matching events (boot-scan `WorktreeAdded`, head facts,
    /// ...) are drained and ignored rather than failing the wait.
    async fn wait_for_removed(
        rx: &mut mpsc::Receiver<PlaneEvent>,
        want: &Path,
        timeout: Duration,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(PlaneEvent::Git(GitEvent::WorktreeRemoved { worktree })))
                    if worktree == want =>
                {
                    return true;
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => return false,
            }
        }
    }

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
        assert!(
            status.dirty_worktree,
            "unstaged + untracked must dirty the worktree"
        );
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
    fn parses_status_v2_branch_oid_and_dirty_flags() {
        let out = "# branch.oid abc123def\n\
                   # branch.head feat/topic\n\
                   # branch.upstream origin/feat/topic\n\
                   # branch.ab +1 -2\n\
                   1 M. path/a\n\
                   ? path/b\n";
        let (branch, commit, status) = parse_status_v2(out).expect("v2 status parses");
        assert_eq!(branch, "feat/topic");
        assert_eq!(commit, "abc123def");
        assert!(status.dirty_index);
        assert!(status.dirty_worktree);
        assert_eq!(status.ahead, 1);
        assert_eq!(status.behind, 2);
    }

    #[test]
    fn parses_status_v2_xy_at_correct_offset_and_untracked() {
        let header = "# branch.oid abc123def\n\
                      # branch.head feat/topic\n\
                      # branch.ab +0 -0\n";
        let cases = [
            (
                "1 .M N... 000000 000000 000000 000000 000000 path/a\n",
                false,
                true,
            ),
            (
                "1 MM N... 000000 000000 000000 000000 000000 path/a\n",
                true,
                true,
            ),
            (
                "1 M. N... 000000 000000 000000 000000 000000 path/a\n",
                true,
                false,
            ),
            ("? untracked\n", false, true),
        ];
        for (record, dirty_index, dirty_worktree) in cases {
            let out = format!("{header}{record}");
            let (_, _, status) = parse_status_v2(&out).expect("v2 status parses");
            assert_eq!(
                status.dirty_index, dirty_index,
                "dirty_index for {record:?}"
            );
            assert_eq!(
                status.dirty_worktree, dirty_worktree,
                "dirty_worktree for {record:?}"
            );
        }
    }

    #[test]
    fn parses_status_v2_detached_head() {
        let out = "# branch.oid abc123def\n# branch.head (detached)\n";
        let (branch, commit, status) = parse_status_v2(out).expect("detached v2 parses");
        assert_eq!(branch, "HEAD");
        assert_eq!(commit, "abc123def");
        assert_eq!(status, GitStatus::default());
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
        assert_eq!(
            entries[0].0,
            PathBuf::from("/Users/jirathip/Projects/herdr-board")
        );
        assert_eq!(entries[0].1.as_deref(), Some("main"));
        assert_eq!(entries[1].1.as_deref(), Some("ws1/git-plane"));
        assert_eq!(entries[2].1, None, "detached worktree has no branch");
    }

    #[test]
    fn commondir_watchers_are_fresh_additive_and_idempotent() {
        // This deterministic factory is the lifecycle seam for #115. The
        // parent implementation had one mutable watcher and would enter
        // PathsMut/stop/run when registration was revisited; this contract
        // requires one factory call per commondir for the whole generation.
        let plane = GitPlane::new(
            PathBuf::from("/fake/repo"),
            PathBuf::from("/fake/worktrees"),
        );
        let boot = [
            PathBuf::from("/fake/repo-one/.git"),
            PathBuf::from("/fake/repo-two/.git"),
        ];
        {
            let mut state = plane.state.lock().unwrap();
            state.commondirs = boot.to_vec();
        }

        // A Vec<PathBuf> stands in for a watcher and records the paths
        // installed into it. Production's factory puts exactly one path in
        // each RecommendedWatcher before it enters the event loop.
        let mut watchers = HashMap::<PathBuf, Vec<PathBuf>>::new();
        let installs = std::cell::RefCell::new(Vec::new());
        let mut create = |commondir: &Path| {
            installs.borrow_mut().push(commondir.to_path_buf());
            Ok::<Vec<PathBuf>, &'static str>(vec![commondir.to_path_buf()])
        };

        assert!(plane.register_new_commondir_watchers(&mut watchers, &mut create));
        assert_eq!(*installs.borrow(), boot);
        assert_eq!(watchers.len(), 2);
        for commondir in &boot {
            assert_eq!(watchers.get(commondir).unwrap(), &vec![commondir.clone()]);
        }

        // Re-running registration for the same boot topology must not touch
        // either existing watcher.
        assert!(!plane.register_new_commondir_watchers(&mut watchers, &mut create));
        assert_eq!(*installs.borrow(), boot);

        // A newly discovered repository gets one fresh watcher; the boot
        // entries remain immutable.
        let later = PathBuf::from("/fake/repo-three/.git");
        {
            let mut state = plane.state.lock().unwrap();
            state.commondirs.push(later.clone());
        }
        assert!(plane.register_new_commondir_watchers(&mut watchers, &mut create));
        assert_eq!(
            *installs.borrow(),
            vec![boot[0].clone(), boot[1].clone(), later.clone()]
        );
        assert_eq!(watchers.get(&later), Some(&vec![later.clone()]));

        // Worktree add/remove churn does not alter commondir topology, so
        // repeated discovery requests cannot restart any existing stream.
        for _ in 0..3 {
            assert!(!plane.register_new_commondir_watchers(&mut watchers, &mut create));
        }
        assert_eq!(installs.borrow().len(), 3);

        // A failed new registration remains retryable, but it must not cause
        // the already-live watchers to be recreated on the next discovery
        // request. This is the partial-failure case that used to rebuild the
        // entire shared stream.
        let failed = PathBuf::from("/fake/repo-four/.git");
        {
            let mut state = plane.state.lock().unwrap();
            state.commondirs.push(failed.clone());
        }
        let attempts = std::cell::RefCell::new(Vec::new());
        let mut fail = |commondir: &Path| {
            attempts.borrow_mut().push(commondir.to_path_buf());
            Err::<Vec<PathBuf>, &'static str>("test registration failure")
        };
        assert!(!plane.register_new_commondir_watchers(&mut watchers, &mut fail));
        assert!(!plane.register_new_commondir_watchers(&mut watchers, &mut fail));
        assert_eq!(*attempts.borrow(), vec![failed.clone(), failed.clone()]);
        assert_eq!(watchers.len(), 3, "failed path is not marked live");
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
        assert_eq!(
            entries[1].0,
            PathBuf::from("/tmp/wt\twith\ttab"),
            "tab in path survives raw"
        );
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
        assert_eq!(
            resolve_possibly_escaped(Path::new("/nonexistent/raw")),
            PathBuf::from("/nonexistent/raw")
        );
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
        assert!(
            plane.watches(&worktree),
            "raw spelling matches via canonicalization"
        );
        assert!(
            plane.watches(&link.join("wts/herdr-board/corral-p2-ws2")),
            "symlinked spelling matches"
        );
        assert!(
            plane.watches(&fs::canonicalize(&repo).unwrap()),
            "main checkout matches"
        );

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
    /// `watches()` itself never sees filesystem events — its only production
    /// caller is `rescan()`, where a missing entry is masked anyway by the
    /// `is_dir()` guard on the following line. What this pins is the
    /// *helper's* answer for the missing-path case, which `handle_fs_event`
    /// does depend on: a delete event names a path that is already gone.
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

        // Pin the RECONSTRUCTED PATH, not just the prefix match. Everything
        // above goes through `watches()`, which is a `starts_with` test, so
        // any permutation of the re-appended tail still passes — a helper
        // that walked the tail forwards instead of in reverse would resolve
        // `wts/a/b/c/d` to `<wts>/d/c/b/a` and satisfy every assertion here.
        assert_eq!(
            canonicalize_existing_prefix(&wts.join("a/b/c/d")),
            fs::canonicalize(&wts).unwrap().join("a/b/c/d"),
            "missing components are re-appended in their original order"
        );

        // The negative direction must survive the fix: a missing path
        // OUTSIDE both roots stays out of scope. A helper that resolved
        // everything to the worktrees root would pass the assertions above
        // and fail this one.
        assert!(
            !plane.watches(&root.join("outside/also-missing")),
            "a missing path outside both roots is not watched"
        );
        // NOTE: this one resolves via `/`, which exists — it does NOT
        // exercise the "no existing ancestor" fallback, and an earlier
        // version of this comment claimed it did. That arm is unreachable
        // for an absolute `..`-free Unix path; it fires for an empty path, a
        // bare relative component, or a path CONTAINING (not merely ending
        // in) `..` — see the helper's own doc, which is the authority here.
        assert!(
            !plane.watches(Path::new("/nonexistent-root-level-path/x")),
            "an absolute missing path outside both roots is not watched"
        );
        // The actual fallback arm: no existing ancestor to canonicalize.
        assert_eq!(
            canonicalize_existing_prefix(Path::new("")),
            PathBuf::from(""),
            "an empty path falls back to the raw spelling"
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
        fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", wt_gitdir.display()),
        )
        .unwrap();
        assert_eq!(
            resolve_gitdir(&linked).expect("file form resolves"),
            fs::canonicalize(&wt_gitdir).unwrap()
        );
        // Relative target form.
        let rel = root.join("rel-wt");
        fs::create_dir_all(&rel).unwrap();
        let rel_gitdir = commondir.join("worktrees/rel");
        fs::create_dir_all(&rel_gitdir).unwrap();
        fs::write(rel.join(".git"), "gitdir: ../commondir/worktrees/rel\n").unwrap();
        assert_eq!(
            resolve_gitdir(&rel).expect("relative form resolves"),
            fs::canonicalize(&rel_gitdir).unwrap()
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// Construct a `GitPlane` over made-up, nonexistent roots and let the
    /// caller populate its registry state directly. `map_event_path` never
    /// touches the filesystem, so these unit tests need no temp dirs — only
    /// the state it reads.
    ///
    /// The caller's mutation is then RECONCILED against the invariants
    /// `rescan` actually maintains, because a fixture that cannot occur in
    /// production silently disarms the code under test. Concretely: with
    /// `commondirs` left empty, the `state.commondirs.contains(gd)` skip in
    /// `map_event_path` — the guard that stops the main checkout swallowing
    /// every commondir-relative event — is dead, and deleting it goes
    /// undetected.
    ///
    /// Enforced here rather than in each test so the invariant cannot drift:
    ///
    /// - `commondirs == main_checkouts.keys()` (`rescan` assigns it exactly so)
    /// - every main checkout is itself a tracked worktree, with
    ///   `gitdir == commondir` (it is the first porcelain entry)
    /// - every `by_branch` value is a tracked worktree path (`rescan`
    ///   `retain`s exactly that)
    fn plane_with_state(mutate: impl FnOnce(&mut PlaneState)) -> GitPlane {
        let plane = GitPlane::new(
            PathBuf::from("/nonexistent-gitplane-repo-root"),
            PathBuf::from("/nonexistent-gitplane-wts-root"),
        );
        {
            let mut state = plane.state.lock().unwrap();
            mutate(&mut state);

            state.commondirs = state.main_checkouts.keys().cloned().collect();
            for (commondir, main) in state.main_checkouts.clone() {
                state.worktrees.entry(main).or_insert(WorktreeState {
                    gitdir: Some(commondir),
                    ..Default::default()
                });
            }

            let tracked: HashSet<PathBuf> = state.worktrees.keys().cloned().collect();
            for ((_, branch), path) in &state.by_branch {
                assert!(
                    tracked.contains(path),
                    "fixture is unreachable in production: by_branch[{branch}] -> {} is not a \
                     tracked worktree, but rescan retains only tracked paths",
                    path.display()
                );
            }
        }
        plane
    }

    #[test]
    fn map_event_path_prefers_most_specific_gitdir_when_nested() {
        // Two gitdirs where one is a path-prefix of the other: the event
        // must resolve to the worktree whose gitdir is the longer (more
        // specific) match, not whichever the map happens to iterate first.
        //
        // Repeated on a FRESH PlaneState each iteration on purpose. With two
        // entries, a naive `if best.is_none()` first-match implementation is
        // a coin flip against HashMap iteration order — measured at a 35%
        // miss rate over 60 single-shot runs. `RandomState` reseeds per
        // HashMap instance, so each iteration is an independent draw and 64
        // of them make a first-match regression a certainty rather than a
        // gamble. The test is pure (no I/O), so the loop is nearly free.
        let outer_gitdir = PathBuf::from("/fake/commondir/worktrees/outer");
        let inner_gitdir = outer_gitdir.join("nested-gitdir");
        let outer_wt = PathBuf::from("/fake/wts/outer");
        let inner_wt = PathBuf::from("/fake/wts/inner");
        for iteration in 0..64 {
            let plane = plane_with_state(|state| {
                state.worktrees.insert(
                    outer_wt.clone(),
                    WorktreeState {
                        gitdir: Some(outer_gitdir.clone()),
                        ..Default::default()
                    },
                );
                state.worktrees.insert(
                    inner_wt.clone(),
                    WorktreeState {
                        gitdir: Some(inner_gitdir.clone()),
                        ..Default::default()
                    },
                );
            });
            let (worktrees, maybe_new) = plane.map_event_path(&inner_gitdir.join("HEAD"));
            assert_eq!(
                worktrees,
                vec![inner_wt.clone()],
                "iteration {iteration}: the longer (more specific) gitdir prefix wins over the \
                 shorter one it nests inside"
            );
            assert!(!maybe_new);
        }
    }

    #[test]
    fn map_event_path_maps_refs_heads_to_the_worktree_on_that_branch() {
        let commondir = PathBuf::from("/fake/repo/.git");
        let main = PathBuf::from("/fake/repo");
        let wt = PathBuf::from("/fake/wts/feature");
        let plane = plane_with_state(|state| {
            state.main_checkouts.insert(commondir.clone(), main);
            // The branch's worktree must be tracked — `rescan` drops any
            // by_branch entry whose value is not a known worktree.
            state.worktrees.insert(
                wt.clone(),
                WorktreeState {
                    gitdir: Some(commondir.join("worktrees/feature")),
                    ..Default::default()
                },
            );
            state
                .by_branch
                .insert((commondir.clone(), "feat/x".to_string()), wt.clone());
        });
        let (worktrees, maybe_new) = plane.map_event_path(&commondir.join("refs/heads/feat/x"));
        assert_eq!(
            worktrees,
            vec![wt],
            "refs/heads/<branch> maps to the worktree checked out on that branch"
        );
        assert!(!maybe_new);
    }

    #[test]
    fn map_event_path_unmatched_worktrees_path_signals_rescan() {
        // A path under commondir/worktrees/ that matches no known gitdir:
        // the worktree may have just appeared (or just been removed), so
        // the caller must rescan — no worktree is resolved directly.
        let commondir = PathBuf::from("/fake/repo/.git");
        let main = PathBuf::from("/fake/repo");
        let plane = plane_with_state(|state| {
            state.main_checkouts.insert(commondir.clone(), main);
        });
        let (worktrees, maybe_new) = plane.map_event_path(&commondir.join("worktrees/brand-new"));
        assert!(
            worktrees.is_empty(),
            "no worktree is resolved for an unmatched worktrees/ path"
        );
        assert!(
            maybe_new,
            "a commondir/worktrees/ path matching no known gitdir signals a rescan"
        );
    }

    #[test]
    fn map_event_path_unrelated_path_maps_to_nothing() {
        let plane = plane_with_state(|_| {});
        let (worktrees, maybe_new) = plane.map_event_path(Path::new("/completely/unrelated/path"));
        assert!(worktrees.is_empty());
        assert!(!maybe_new);
    }

    #[tokio::test]
    async fn fs_event_batch_coalesces_burst_to_one_worktree() {
        let commondir = PathBuf::from("/fake/repo/.git");
        let wt = PathBuf::from("/fake/wts/wt");
        let gitdir = commondir.join("worktrees/wt");
        let plane = plane_with_state(|state| {
            state.worktrees.insert(
                wt.clone(),
                WorktreeState {
                    gitdir: Some(gitdir.clone()),
                    ..Default::default()
                },
            );
        });
        let (sink, _rx) = crate::core::plane_channel();
        let (cmd_tx, _cmd_rx) = mpsc::channel::<WatcherCommand>(1);
        let events: Vec<Event> = (0..8)
            .map(|_| {
                Event::new(notify::EventKind::Remove(notify::event::RemoveKind::Folder))
                    .add_path(gitdir.join("HEAD"))
            })
            .collect();
        let affected = plane.handle_fs_event_batch(&events, &sink, &cmd_tx).await;
        assert_eq!(
            affected,
            vec![wt],
            "a burst of events for one worktree must coalesce to one debounce target"
        );
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
        assert!(
            git(&["config", "user.email", "plane@test.local"])
                .status
                .success()
        );
        assert!(git(&["config", "user.name", "Plane Test"]).status.success());
        fs::write(root.join("README.md"), "hello\n").unwrap();
        assert!(git(&["add", "README.md"]).status.success());
        assert!(git(&["commit", "-m", "initial"]).status.success());
        let sha = git(&["rev-parse", "HEAD"]).stdout;
        let sha = String::from_utf8_lossy(&sha).trim().to_string();
        (root, sha)
    }

    #[tokio::test]
    async fn probe_returns_sha_and_first_line_subject_in_two_git_calls() {
        // F2: serialize against the other probe test — GIT_CALLS is shared.
        let _guard = PROBE_LOCK.lock().await;
        // G21 acceptance 2: `status --porcelain=v2 --branch` carries the head
        // oid and branch; the subject is read by a single `git log` only when
        // HEAD is uncached, so a fresh probe is two git invocations, never
        // four.
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
        assert!(
            git(&[
                "commit",
                "--allow-empty",
                "-m",
                "second line\n\nbody paragraph"
            ])
            .status
            .success()
        );
        let sha = String::from_utf8_lossy(&git(&["rev-parse", "HEAD"]).stdout)
            .trim()
            .to_string();

        let before = GIT_CALLS.load(Ordering::Relaxed);
        let probe = probe_worktree(&root, None, None)
            .await
            .expect("probe succeeds");
        let delta = GIT_CALLS.load(Ordering::Relaxed) - before;
        assert_eq!(
            delta, 2,
            "one status + one subject read when HEAD is uncached (probe = 2)"
        );
        assert_eq!(
            probe.commit, sha,
            "probe resolves the same HEAD sha as rev-parse"
        );
        assert_eq!(
            probe.subject.as_deref(),
            Some("second line"),
            "subject is the commit's first line"
        );
        assert_eq!(probe.branch, "main");
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn probe_reuses_cached_subject_without_log_on_unchanged_head() {
        let _guard = PROBE_LOCK.lock().await;
        let (root, _) = scratch_repo("head-cache");
        let first = probe_worktree(&root, None, None)
            .await
            .expect("first probe");
        let before = GIT_CALLS.load(Ordering::Relaxed);
        let second = probe_worktree(&root, Some(first.commit.as_str()), first.subject.as_deref())
            .await
            .expect("cached probe");
        let delta = GIT_CALLS.load(Ordering::Relaxed) - before;
        assert_eq!(delta, 1, "unchanged HEAD needs only one status call");
        assert_eq!(second, first);
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
        assert!(
            probe_worktree(&root, None, None).await.is_err(),
            "unborn HEAD must fail the probe"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// #46/#50: a delete event for a known, now-removed worktree must map
    /// to that worktree and drive a `WorktreeRemoved` all the way through —
    /// mapping, debounce, probe (`ProbeError::Gone`), emit. This is the case
    /// #46 touched (`fs::canonicalize` -> `canonicalize_existing_prefix`)
    /// and the one with no coverage before this test.
    #[tokio::test(flavor = "multi_thread")]
    async fn handle_fs_event_emits_worktree_removed_for_deleted_worktree() {
        // F2: serialize against the probe-delta tests — `rescan()` below
        // calls `run_git`, which increments the same shared `GIT_CALLS`
        // static the probe tests measure a before/after delta over.
        let _guard = PROBE_LOCK.lock().await;
        let root = std::env::temp_dir().join(format!(
            "corral-gitplane-delete-{}-{}",
            std::process::id(),
            now_millis()
        ));
        let repo = root.join("repo");
        let wts = root.join("wts");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&wts).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .expect("git subprocess runs")
        };
        assert!(git(&repo, &["init", "-b", "main"]).status.success());
        assert!(
            git(&repo, &["config", "user.email", "plane@test.local"])
                .status
                .success()
        );
        assert!(
            git(&repo, &["config", "user.name", "Plane Test"])
                .status
                .success()
        );
        fs::write(repo.join("README.md"), "hello\n").unwrap();
        assert!(git(&repo, &["add", "README.md"]).status.success());
        assert!(git(&repo, &["commit", "-m", "initial"]).status.success());

        let wt = wts.join("wt1");
        assert!(
            git(
                &repo,
                &[
                    "worktree",
                    "add",
                    &wt.to_string_lossy(),
                    "-b",
                    "feat/delete"
                ],
            )
            .status
            .success()
        );
        let wt = fs::canonicalize(&wt).unwrap();

        let plane = Arc::new(GitPlane::new(repo.clone(), wts.clone()));
        let (sink, mut rx) = crate::core::plane_channel();
        plane.rescan(&sink).await;

        let gitdir = resolve_gitdir(&wt).expect("gitdir resolves before removal");
        assert!(
            git(
                &repo,
                &["worktree", "remove", "--force", &wt.to_string_lossy()],
            )
            .status
            .success(),
            "git worktree remove cleans up both the working dir and the admin gitdir"
        );
        assert!(!wt.exists(), "precondition: worktree directory is gone");
        assert!(!gitdir.exists(), "precondition: gitdir is gone");

        let (cmd_tx, _cmd_rx) = mpsc::channel::<WatcherCommand>(1);
        let event = Event::new(notify::EventKind::Remove(notify::event::RemoveKind::Folder))
            .add_path(gitdir);
        let affected = plane.handle_fs_event(&event, &sink, &cmd_tx).await;
        assert_eq!(
            affected,
            vec![wt.clone()],
            "delete event for the gitdir maps to the removed worktree"
        );

        for w in affected {
            plane.debounce(w, sink.clone());
        }
        assert!(
            wait_for_removed(&mut rx, &wt, Duration::from_secs(2)).await,
            "WorktreeRemoved for {wt:?} within 2s of the delete event"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The highest-value regression test for #46: the whole #43 family of
    /// bugs only appears when the raw and canonical spellings of a path
    /// differ. Built like `watches_accepts_symlinked_roots_and_canonical_spellings`
    /// — a `real/` tree plus a `link/` symlink to it — the plane is
    /// constructed through the symlinked path, then the delete event names
    /// the RAW spelling (through `link/`) of the now-gone gitdir. Before #46
    /// (bare `fs::canonicalize`, which fails outright on a missing path and
    /// falls back to the raw spelling) this path would not have resolved,
    /// because the raw spelling never `starts_with` the canonical gitdir
    /// recorded in the registry at scan time.
    #[tokio::test(flavor = "multi_thread")]
    async fn handle_fs_event_resolves_raw_symlinked_delete_path() {
        // F2: serialize against the probe-delta tests — see the guard note
        // on `handle_fs_event_emits_worktree_removed_for_deleted_worktree`.
        let _guard = PROBE_LOCK.lock().await;
        let root = std::env::temp_dir().join(format!(
            "corral-gitplane-delete-symlink-{}-{}",
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

        let git = |dir: &Path, args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .expect("git subprocess runs")
        };
        assert!(git(&repo, &["init", "-b", "main"]).status.success());
        assert!(
            git(&repo, &["config", "user.email", "plane@test.local"])
                .status
                .success()
        );
        assert!(
            git(&repo, &["config", "user.name", "Plane Test"])
                .status
                .success()
        );
        fs::write(repo.join("README.md"), "hello\n").unwrap();
        assert!(git(&repo, &["add", "README.md"]).status.success());
        assert!(git(&repo, &["commit", "-m", "initial"]).status.success());

        let wt_raw = wts.join("wt1");
        assert!(
            git(
                &repo,
                &[
                    "worktree",
                    "add",
                    &wt_raw.to_string_lossy(),
                    "-b",
                    "feat/delete",
                ],
            )
            .status
            .success()
        );
        let wt = fs::canonicalize(&wt_raw).unwrap();

        // Construct the plane through the SYMLINKED path (link/repo,
        // link/wts) — `GitPlane::new` canonicalizes both internally, so the
        // registry it builds is keyed on the canonical `real/...` spelling.
        let plane = Arc::new(GitPlane::new(link.join("repo"), link.join("wts")));
        let (sink, mut rx) = crate::core::plane_channel();
        plane.rescan(&sink).await;

        let canonical_real = fs::canonicalize(&real).unwrap();
        let gitdir = resolve_gitdir(&wt).expect("gitdir resolves before removal");
        let rel = gitdir
            .strip_prefix(&canonical_real)
            .expect("gitdir lives under the real root");
        // The RAW spelling: the same file, reached through the symlink
        // instead of the canonical root.
        let raw_event_path = link.join(rel);
        assert_ne!(
            raw_event_path, gitdir,
            "precondition: raw and canonical spellings differ"
        );

        assert!(
            git(
                &repo,
                &["worktree", "remove", "--force", &wt_raw.to_string_lossy()],
            )
            .status
            .success()
        );
        assert!(!gitdir.exists(), "precondition: gitdir is gone");
        assert!(
            !raw_event_path.exists(),
            "precondition: raw spelling is gone too"
        );

        let (cmd_tx, _cmd_rx) = mpsc::channel::<WatcherCommand>(1);
        let event = Event::new(notify::EventKind::Remove(notify::event::RemoveKind::Folder))
            .add_path(raw_event_path);
        let affected = plane.handle_fs_event(&event, &sink, &cmd_tx).await;
        assert_eq!(
            affected,
            vec![wt.clone()],
            "raw symlinked delete path still maps to the canonical worktree"
        );

        for w in affected {
            plane.debounce(w, sink.clone());
        }
        assert!(
            wait_for_removed(&mut rx, &wt, Duration::from_secs(2)).await,
            "WorktreeRemoved for {wt:?} within 2s of the raw-spelling delete event"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
