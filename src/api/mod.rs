//! HTTP API served on loopback.
//!
//! - `GET /snapshot` — full current state with monotonic `rev`.
//! - `GET /events`  — SSE; resumes from `Last-Event-ID` (full snapshot when
//!   the cursor is too old, incremental `{rev, upd, del}` otherwise).
//! - `GET /healthz` — liveness.
//! - `POST /drive`  — P3 drive plane (writes): idempotent by `request_id`,
//!   capability-gated, signed by the device authorizer (see
//!   [`crate::api::drive`]).

pub mod drive;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use futures::stream::{self, Stream, StreamExt};
use tracing::info;

use crate::adapters::Adapter;
use crate::core::model::Resume;
use crate::core::store::Store;
use crate::drive::{AuditLog, DriveAuthorizer};

use self::drive::{drive, NoopAdapter, ReplayTable, StubAudit, StubAuthorizer};

/// Keepalive comment cadence so idle connections stay alive through NATs.
const KEEPALIVE: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    /// Drive-path dispatch target (W1 resolves agent_ids, never coordinates).
    pub adapter: Arc<dyn Adapter>,
    /// Signature/key/grant gate for every drive write (W3 provides the impl).
    pub authorizer: Arc<dyn DriveAuthorizer>,
    /// Append-only write log (W3 provides the impl; W1 calls it per write).
    pub audit: Arc<dyn AuditLog>,
    /// Idempotency table keyed by request_id (bounded, LRU-ish).
    pub replay: Arc<ReplayTable>,
}

impl Default for AppState {
    /// Read-path-only state: every drive dependency is a safe stub
    /// ([`NoopAdapter`], [`StubAuthorizer`], [`StubAudit`]) so read-only
    /// construction (tests, tooling) stays one struct away.
    fn default() -> Self {
        Self {
            store: Store::new(),
            adapter: Arc::new(NoopAdapter),
            authorizer: Arc::new(StubAuthorizer),
            audit: Arc::new(StubAudit::default()),
            replay: Arc::new(ReplayTable::default()),
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/snapshot", get(snapshot))
        .route("/events", get(events))
        .route("/drive", post(drive))
        .with_state(Arc::new(state))
}

async fn healthz() -> &'static str {
    "ok\n"
}

async fn snapshot(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let snap = state.store.snapshot().await;
    Json(serde_json::to_value(&snap).unwrap_or_else(|_| serde_json::json!({ "error": "encode" })))
}

/// SSE stream. The first frame is either a full `snapshot` or a `delta`
/// resume; afterwards every coalesced delta is pushed live.
async fn events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let store = state.store.clone();
    let last_rev = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    // Subscribe BEFORE resolving the cursor: any delta flushed in between is
    // still delivered (skipped below when already covered by the initial
    // payload), so no window is ever missed.
    let rx = store.subscribe();

    // Live: cursor is current — emit nothing (the SSE keep-alive suffices).
    // A fabricated empty delta would look like a state change to clients.
    let (initial, live_from_rev) = match store.resume_from(last_rev).await {
        Resume::Snapshot(snap) => {
            info!(rev = snap.rev, "SSE client joined with full snapshot");
            (vec![delta_event("snapshot", snap.rev, &snap)], snap.rev)
        }
        Resume::Deltas { deltas, live_from_rev } => {
            info!(replayed = deltas.len(), "SSE client resumed");
            (
                deltas
                    .iter()
                    .map(|d| delta_event("delta", d.rev, d))
                    .collect(),
                live_from_rev,
            )
        }
        Resume::Live { rev } => (Vec::new(), rev),
    };

    let live = stream::unfold(rx, move |mut rx| {
        let store = store.clone();
        async move {
            loop {
                match rx.recv().await {
                    Ok(delta) if delta.rev <= live_from_rev => continue,
                    Ok(delta) => {
                        return Some((Ok(delta_event("delta", delta.rev, &delta)), rx));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Client fell too far behind: full resnapshot.
                        let snap = store.snapshot().await;
                        return Some((Ok(delta_event("snapshot", snap.rev, &snap)), rx));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        }
    });

    let stream = stream::iter(initial.into_iter().map(Ok)).chain(live);
    Sse::new(stream).keep_alive(KeepAlive::new().interval(KEEPALIVE))
}

fn delta_event(kind: &str, rev: u64, data: &impl serde::Serialize) -> Event {
    Event::default()
        .event(kind)
        .id(rev.to_string())
        .json_data(data)
        .expect("delta/snapshot serializes")
}
