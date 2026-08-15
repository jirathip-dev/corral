//! Revisioned agent store: single source of truth for the read path.
//!
//! Adapters apply [`Change`]s; the store keeps state current immediately and
//! publishes coalesced [`Delta`]s on a 250ms foreground / 2s background tick
//! ("foreground" = at least one SSE subscriber). Every delta bumps the global
//! monotonic `rev` and is retained in a bounded history ring so SSE clients
//! can resume via `Last-Event-ID`.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, Mutex, Notify};

use super::model::{Agent, Change, Delta, Resume, SCHEMA_VERSION, Snapshot};

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
    pending_upd: Vec<Agent>,
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
}

impl Default for Store {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            tx,
            notify: Arc::new(Notify::new()),
        }
    }
}

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one change. State updates immediately; publishing waits for the
    /// coalesce tick.
    pub async fn apply(&self, change: Change) {
        let mut inner = self.inner.lock().await;
        match change {
            Change::Upsert(agent) => {
                let agent_id = agent.agent_id.clone();
                inner.pending_upd.push((*agent).clone());
                inner.agents.insert(agent_id, *agent);
            }
            Change::Remove(agent_id) => {
                if inner.agents.remove(&agent_id).is_some() {
                    inner.pending_del.push(agent_id);
                }
            }
        }
        inner.pending_any = true;
        self.notify.notify_one();
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
            upd: std::mem::take(&mut inner.pending_upd),
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
        }
    }

    /// Resolve a client cursor. `Some(rev)` from `Last-Event-ID`, `None` if
    /// the client has no cursor (always a full snapshot). Flushes pending
    /// changes first so the returned boundary matches the live stream.
    pub async fn resume_from(&self, last_rev: Option<u64>) -> Resume {
        self.flush().await;
        let inner = self.inner.lock().await;
        let Some(last_rev) = last_rev else {
            return Resume::Snapshot(self.snapshot_locked(&inner));
        };
        let current = inner.rev;
        if last_rev >= current {
            return Resume::Live { rev: current };
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
