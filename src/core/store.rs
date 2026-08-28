//! Revisioned agent store: single source of truth for the read path.
//!
//! Adapters apply [`Change`]s; the store keeps state current immediately and
//! publishes coalesced [`Delta`]s on a 250ms foreground / 2s background tick
//! ("foreground" = at least one SSE subscriber). Every delta bumps the global
//! monotonic `rev` and is retained in a bounded history ring so SSE clients
//! can resume via `Last-Event-ID`.
//!
//! D23: the `Change::Upsert` arm of [`Store::apply`] is ALSO the single
//! choke point where status transitions land (every caller is the herdr
//! adapter; the integrator merges workspace fields via [`Store::update_where`]
//! and never touches `state`). A state change (old != new) is pushed into the
//! persistent [`crate::history::HistoryRing`] — zero polling, zero extra git
//! calls, one page-cache write syscall.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify, broadcast, watch};

use super::model::{Agent, Change, Delta, Resume, SCHEMA_VERSION, Snapshot};
use crate::history::{HistoryEvent, HistoryRing, RotationPolicy};

/// How many coalesced delta batches to retain for SSE resume.
const HISTORY_CAP: usize = 1024;
/// Broadcast capacity for live deltas.
const BROADCAST_CAP: usize = 256;
/// Coalesce window while at least one SSE subscriber is connected.
const FOREGROUND_TICK: Duration = Duration::from_millis(250);
/// Coalesce window when nobody is watching.
const BACKGROUND_TICK: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
struct Inner {
    agents: BTreeMap<String, Agent>,
    rev: u64,
    /// (rev, delta) history, oldest first. Bounded by HISTORY_CAP.
    history: VecDeque<(u64, Delta)>,
    /// Pending upserts deduped by agent_id so an event storm within one
    /// coalesce window cannot accumulate unbounded clones of the same agent.
    pending_upd: BTreeMap<String, Agent>,
    pending_del: Vec<String>,
    /// Whether any change arrived while nobody was subscribed (drives the
    /// coalesce tick choice without touching the broadcast channel).
    pending_any: bool,
}

/// Cloneable handle into the store.
#[derive(Clone)]
pub struct Store {
    inner: Arc<Mutex<Inner>>,
    tx: broadcast::Sender<Delta>,
    notify: Arc<Notify>,
    /// Change-version signal: bumped once per applied change batch (WS3's
    /// convergence trigger — a `watch`, NOT a broadcast receiver, so it
    /// cannot pin `subscriber_count`/the gh plane's cadence).
    version: watch::Sender<u64>,
    /// D23: persistent status-transition history (in-memory only when no dir
    /// was configured; see [`HistoryRing::open`]).
    history: HistoryRing,
}

impl Default for Store {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        let (version, _) = watch::channel(0);
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            tx,
            notify: Arc::new(Notify::new()),
            version,
            history: HistoryRing::in_memory(RotationPolicy::default()),
        }
    }
}

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    /// Daemon entry: history persists under `dir` (see [`crate::history`]).
    /// `Store::new()` stays in-memory-only for tests and read-path defaults.
    pub fn with_history_dir(dir: PathBuf) -> Self {
        Self {
            history: HistoryRing::open(dir, RotationPolicy::default()),
            ..Self::default()
        }
    }

    /// D23: the status-transition history ring (persistent when the store
    /// was built with [`Store::with_history_dir`]).
    pub fn history(&self) -> HistoryRing {
        self.history.clone()
    }

    /// Apply one change. State updates immediately; publishing waits for the
    /// coalesce tick. Upserts within one window dedupe by agent_id (the
    /// latest record wins), so memory stays bounded by the agent count even
    /// under a burst.
    ///
    /// D23: an actual state transition (old != new, or a first-seen agent)
    /// is recorded into the history ring while the lock is held, so ring
    /// order matches apply order exactly.
    pub async fn apply(&self, change: Change) {
        self.apply_if(change, || true).await;
    }

    /// Apply an upsert only if the synchronous predicate agrees while the
    /// Store lock is held. This is the write-side counterpart to
    /// [`Store::remove_if`]: an adapter can validate its in-memory mapping at
    /// the same commit point as the row mutation, closing a read/modify/write
    /// gap without awaiting while either lock is held.
    pub async fn upsert_if(&self, agent: Agent, should_apply: impl FnOnce() -> bool) -> bool {
        self.apply_if(Change::upsert(agent), should_apply).await
    }

    async fn apply_if(&self, change: Change, should_apply: impl FnOnce() -> bool) -> bool {
        let mut inner = self.inner.lock().await;
        if !should_apply() {
            return false;
        }
        let mut event: Option<HistoryEvent> = None;
        match change {
            Change::Upsert(agent) => {
                let agent_id = agent.agent_id.clone();
                let old_state = inner.agents.get(&agent_id).map(|a| a.state);
                if old_state != Some(agent.state) {
                    event = Some(HistoryEvent {
                        ts: now_millis(),
                        pane_id: agent.attachment.as_ref().map(|a| a.reference.clone()),
                        agent_id: Some(agent_id.clone()),
                        old_status: old_state,
                        new_status: agent.state,
                        source: agent.source.clone(),
                        repo: agent.workspace.repo.clone(),
                    });
                }
                // A remove followed by a replacement in one coalescing
                // window is one live upsert, not a delete followed by an
                // upsert. A client that applies `del` after `upd` would erase
                // the replacement if the stale delete remained queued.
                inner.pending_del.retain(|pending| pending != &agent_id);
                inner.pending_upd.insert(agent_id.clone(), (*agent).clone());
                inner.agents.insert(agent_id, *agent);
            }
            Change::Remove(agent_id) => {
                if inner.agents.remove(&agent_id).is_some() {
                    // A same-window upsert is subsumed by the removal.
                    inner.pending_upd.remove(&agent_id);
                    if !inner.pending_del.contains(&agent_id) {
                        inner.pending_del.push(agent_id);
                    }
                }
            }
        }
        inner.pending_any = true;
        self.notify.notify_one();
        let next = self.version.borrow().wrapping_add(1);
        let _ = self.version.send(next);
        if let Some(event) = event {
            self.history.push(event);
        }
        true
    }

    /// Atomically read-compare-apply a transformation to every record
    /// satisfying `f`. Runs under ONE lock acquisition: the predicate, the
    /// merge (`g` over a clone of the CURRENT record), the changed-check and
    /// the pending-batch insert all happen before the lock is released, so a
    /// concurrent writer cannot slip a newer record in between (WS3 F3 —
    /// the integrator must never overwrite a fresher `ts`/`seq`).
    ///
    /// Returns how many records actually changed; unchanged records are not
    /// re-published. The coalescer owns the rev exactly as with [`Store::apply`].
    pub async fn update_where(&self, f: impl Fn(&Agent) -> bool, g: impl Fn(&mut Agent)) -> usize {
        let mut inner = self.inner.lock().await;
        let mut changed_records: Vec<Agent> = Vec::new();
        for agent in inner.agents.values_mut() {
            if !f(agent) {
                continue;
            }
            let mut next = agent.clone();
            g(&mut next);
            if next == *agent {
                continue;
            }
            *agent = next.clone();
            changed_records.push(next);
        }
        let changed = changed_records.len();
        for next in changed_records {
            inner.pending_upd.insert(next.agent_id.clone(), next);
        }
        if changed > 0 {
            inner.pending_any = true;
            self.notify.notify_one();
            let next = self.version.borrow().wrapping_add(1);
            let _ = self.version.send(next);
        }
        changed
    }

    /// Subscribe to the change-version signal (WS3 F1: the integrator
    /// re-applies cached facts when the store changes, so an agent created
    /// with zero subsequent plane events still converges). A `watch`, not a
    /// broadcast receiver — it never counts toward `subscriber_count`.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.version.subscribe()
    }

    /// Number of live SSE subscribers (drives foreground/background tick).
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Coalesce and publish. Runs forever; the only exit is the shutdown
    /// signal being dropped by [`Store::shutdown`]'s owner dropping the guard.
    pub async fn run_coalescer(&self) {
        loop {
            self.notify.notified().await;
            loop {
                let sleep = {
                    let inner = self.inner.lock().await;
                    if !inner.pending_any {
                        break;
                    }
                    if self.subscriber_count() > 0 {
                        FOREGROUND_TICK
                    } else {
                        BACKGROUND_TICK
                    }
                };
                tokio::time::sleep(sleep).await;
                self.flush().await;
            }
        }
    }

    /// Publish pending changes as one delta batch with a fresh rev.
    pub async fn flush(&self) -> Option<Delta> {
        let mut inner = self.inner.lock().await;
        if !inner.pending_any {
            return None;
        }
        inner.pending_any = false;
        let delta = Delta {
            rev: inner.rev + 1,
            upd: std::mem::take(&mut inner.pending_upd)
                .into_values()
                .collect(),
            del: std::mem::take(&mut inner.pending_del),
        };
        inner.rev = delta.rev;
        inner.history.push_back((delta.rev, delta.clone()));
        while inner.history.len() > HISTORY_CAP {
            inner.history.pop_front();
        }
        drop(inner);
        let _ = self.tx.send(delta.clone());
        Some(delta)
    }

    /// Fetch a single record (adapters use this for gated updates like
    /// waiting_on, which depend on the current state).
    pub async fn get(&self, agent_id: &str) -> Option<Agent> {
        self.inner.lock().await.agents.get(agent_id).cloned()
    }

    /// Remove a row only when the synchronous predicate agrees while the
    /// Store lock is held. The predicate is deliberately synchronous: callers
    /// may inspect another in-memory state machine at this exact mutation
    /// point, but must never await while either lock is held.
    pub async fn remove_if(&self, agent_id: &str, should_remove: impl FnOnce() -> bool) -> bool {
        let mut inner = self.inner.lock().await;
        if !inner.agents.contains_key(agent_id) || !should_remove() {
            return false;
        }
        if inner.agents.remove(agent_id).is_none() {
            return false;
        }
        inner.pending_upd.remove(agent_id);
        if !inner.pending_del.contains(&agent_id.to_string()) {
            inner.pending_del.push(agent_id.to_string());
        }
        inner.pending_any = true;
        self.notify.notify_one();
        let next = self.version.borrow().wrapping_add(1);
        let _ = self.version.send(next);
        true
    }

    /// Read-only lookup of every record satisfying `f`.
    ///
    /// Deliberately does NOT flush pending changes: the coalescer owns the
    /// rev, so a flush here would turn the plane integrator's event handling
    /// into a second tick. The integrator (WS3) uses this to map path/repo
    /// facts onto agent records without holding a broadcast receiver — a
    /// permanent receiver would pin `subscriber_count` and keep the gh plane
    /// on its foreground cadence forever.
    pub async fn matching(&self, f: impl Fn(&Agent) -> bool) -> Vec<Agent> {
        self.inner
            .lock()
            .await
            .agents
            .values()
            .filter(|agent| f(agent))
            .cloned()
            .collect()
    }

    /// Point-in-time snapshot. Flushes pending changes first so `rev` and the
    /// agent map are mutually consistent.
    pub async fn snapshot(&self) -> Snapshot {
        self.flush().await;
        let inner = self.inner.lock().await;
        Snapshot {
            schema_version: SCHEMA_VERSION,
            rev: inner.rev,
            generated_at: now_millis(),
            agents: inner.agents.clone(),
            fleet_health: Vec::new(),
        }
    }

    /// Resolve a client cursor. `Some(rev)` from `Last-Event-ID`, `None` if
    /// the client has no cursor (always a full snapshot). Flushes pending
    /// changes first so the returned boundary matches the live stream.
    ///
    /// Cursor semantics: equal to current -> go live; older but covered by
    /// the history ring -> replay deltas; older than the ring, or NEWER than
    /// current (a daemon restart resets `rev` to 0, so a future cursor means
    /// the client is on a dead epoch) -> full snapshot. A future cursor must
    /// never silently go live, or the client keeps its stale epoch forever.
    pub async fn resume_from(&self, last_rev: Option<u64>) -> Resume {
        self.flush().await;
        let inner = self.inner.lock().await;
        let Some(last_rev) = last_rev else {
            return Resume::Snapshot(self.snapshot_locked(&inner));
        };
        let current = inner.rev;
        if last_rev == current {
            return Resume::Live { rev: current };
        }
        if last_rev > current {
            // Cursor from the future: dead epoch (daemon restart). Resnapshot
            // so the client re-anchors instead of waiting forever.
            return Resume::Snapshot(self.snapshot_locked(&inner));
        }
        let oldest = inner.history.front().map(|(rev, _)| *rev);
        let Some(oldest) = oldest else {
            return Resume::Snapshot(self.snapshot_locked(&inner));
        };
        if last_rev < oldest {
            // Cursor too old: history no longer covers it.
            return Resume::Snapshot(self.snapshot_locked(&inner));
        }
        let deltas: Vec<Delta> = inner
            .history
            .iter()
            .filter(|(rev, _)| *rev > last_rev)
            .map(|(_, d)| d.clone())
            .collect();
        Resume::Deltas {
            deltas,
            live_from_rev: current,
        }
    }

    fn snapshot_locked(&self, inner: &Inner) -> Snapshot {
        Snapshot {
            schema_version: SCHEMA_VERSION,
            rev: inner.rev,
            generated_at: now_millis(),
            agents: inner.agents.clone(),
            fleet_health: Vec::new(),
        }
    }

    /// Subscribe to live deltas.
    pub fn subscribe(&self) -> broadcast::Receiver<Delta> {
        self.tx.subscribe()
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
