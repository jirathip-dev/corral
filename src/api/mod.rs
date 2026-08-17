//! HTTP API served on loopback.
//!
//! - `GET /snapshot` — full current state with monotonic `rev`.
//! - `GET /events`  — SSE; resumes from `Last-Event-ID` (full snapshot when
//!   the cursor is too old, incremental `{rev, upd, del}` otherwise).
//! - `GET /history` — D23: status-transition events from the persistent
//!   ring, oldest first, `?since=<ts>` / `?limit=<n>` filtered.
//! - `GET /healthz` — liveness.
//! - `POST /drive`  — P3 drive plane (writes): idempotent by `request_id`,
//!   capability-gated, signed by the device authorizer, step-up-gated for
//!   destructive payloads (see [`crate::api::drive`]).
//! - P3 auth surface (W3, [`crate::auth::http`]): `GET /host-key`,
//!   `POST /register`, `POST /step-up`, `POST /grants`, `GET /audit`.
//! - `POST /device-token` — D16 push registration: the device signs a
//!   [`DeviceTokenRequest`](crate::push::payload::DeviceTokenRequest) with
//!   its key (same proof-of-possession shape as `/step-up`); the token is
//!   stored on the registry record (revocable per-device).
//!
//! ## Push notifier arming
//!
//! [`router()`] arms the APNs notifier once per process (main calls it
//! exactly once; tests never set `CORRAL_APNS_*` so they stay unarmed).
//! See [`crate::push`] for the D16 architecture and the provisioning
//! inputs (the `.p8` push key is Guy's).

pub mod drive;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use futures::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use tracing::info;

use crate::adapters::Adapter;
use crate::auth::AuthPlane;
use crate::core::model::Resume;
use crate::core::store::Store;
use crate::push::payload::{DeviceTokenRequest, canonical_device_token_bytes};

use self::drive::{drive, NoopAdapter, ReplayTable};

/// Keepalive comment cadence so idle connections stay alive through NATs.
const KEEPALIVE: Duration = Duration::from_secs(15);
/// Default / cap for one `/history` page.
const HISTORY_DEFAULT_LIMIT: usize = 1000;
const HISTORY_MAX_LIMIT: usize = 5000;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    /// P3 auth plane (W3): host identity, device registry, authorizer,
    /// step-up gate, hash-chained audit log. The drive handler reaches the
    /// contract seams through `auth.authorizer` / `auth.audit`.
    pub auth: Arc<AuthPlane>,
    /// Drive-path dispatch target (W1 resolves agent_ids, never coordinates).
    pub adapter: Arc<dyn Adapter>,
    /// Idempotency table keyed by request_id (bounded, LRU-ish).
    pub replay: Arc<ReplayTable>,
}

impl Default for AppState {
    /// Read-path-only state: a fresh auth plane over a throwaway temp dir
    /// and a no-op adapter, so read-only construction (tests, tooling)
    /// stays one struct away. Write paths use the real plane.
    fn default() -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "corral-appstate-default-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let auth = Arc::new(AuthPlane::load_or_create(dir).expect("default auth plane"));
        Self {
            store: Store::new(),
            auth,
            adapter: Arc::new(NoopAdapter),
            replay: Arc::new(ReplayTable::default()),
        }
    }
}

pub fn router(state: AppState) -> Router {
    // D16: arm the APNs push notifier once per process. main calls router
    // exactly once; tests never set CORRAL_APNS_* so they stay unarmed.
    // Disabled (unconfigured / bad p8) -> the daemon runs exactly as
    // before, with a startup warning. Kept out of main.rs so the daemon
    // entrypoint stays untouched (see crate::push for the architecture).
    if let Some(notifier) =
        crate::push::Notifier::from_env(state.store.clone(), state.auth.registry.clone())
    {
        notifier.start();
    } else {
        info!("push notifier not configured (set CORRAL_APNS_* to enable APNs)");
    }
    Router::new()
        .route("/healthz", get(healthz))
        .route("/snapshot", get(snapshot))
        .route("/events", get(events))
        .route("/history", get(history))
        .route("/drive", post(drive))
        .route("/device-token", post(device_token))
        .merge(crate::auth::http::auth_routes())
        .with_state(Arc::new(state))
}

async fn healthz() -> &'static str {
    "ok\n"
}

/// `GET /history` query parameters. `since` is epoch millis (inclusive);
/// `limit` bounds the page. Both optional.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HistoryQuery {
    pub since: Option<u64>,
    pub limit: Option<usize>,
}

/// D23 live feed: status-transition events from the persistent ring,
/// oldest first. `?since=<ts>` filters to events at or after `ts`;
/// `?limit=<n>` caps the page (default 1000, hard cap 5000).
async fn history(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HistoryQuery>,
) -> Json<serde_json::Value> {
    let limit = params.limit.unwrap_or(HISTORY_DEFAULT_LIMIT).min(HISTORY_MAX_LIMIT);
    let events = state.store.history().query(params.since, Some(limit));
    Json(serde_json::json!({ "events": events }))
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

/// Max accepted skew between a signed device-token request's `ts` and the
/// host clock — same freshness rule as the step-up request (replay-proof).
const DEVICE_TOKEN_MAX_SKEW_SECS: u64 = 60;

/// `POST /device-token {key_id, signature, request}` → `{ok, key_id}`.
///
/// Proof of possession, mirroring `POST /step-up` (src/auth/http.rs): the
/// signature covers [`canonical_device_token_bytes`] of the request
/// (fixed-order `{key_id, device_token, ts}`), verified against the
/// registered device key; freshness `|now - ts| < 60s`; revoked/expired
/// keys are refused. An empty `device_token` clears the registration
/// (per-device revocation). The token itself is opaque (hex APNs token
/// from the app) and is persisted on the registry record — the notifier's
/// delivery filter (`DeviceRecord::push_eligible`).
async fn device_token(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    let request: DeviceTokenRequest =
        match serde_json::from_value(body.get("request").cloned().unwrap_or_default()) {
            Ok(r) => r,
            Err(_) => return json_err(StatusCode::BAD_REQUEST, "malformed device-token request"),
        };
    if now_secs().abs_diff(request.ts) > DEVICE_TOKEN_MAX_SKEW_SECS {
        return json_err(
            StatusCode::BAD_REQUEST,
            "stale device-token request: |now - ts| > 60s",
        );
    }
    let signature_b64 = match body.get("signature").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return json_err(StatusCode::BAD_REQUEST, "missing device-token signature"),
    };
    let rec = match state.auth.registry.get(&request.key_id) {
        Some(r) => r,
        None => return json_err(StatusCode::NOT_FOUND, "unknown device key"),
    };
    if rec.revoked {
        return json_err(StatusCode::FORBIDDEN, "device key revoked");
    }
    if now_secs() >= rec.expiry_ts {
        return json_err(StatusCode::FORBIDDEN, "device key expired");
    }
    let sig = match crate::auth::decode_b64(signature_b64) {
        Some(s) => match s.try_into() {
            Ok(sig) => sig,
            Err(_) => return json_err(StatusCode::BAD_REQUEST, "signature must be 64 bytes"),
        },
        None => return json_err(StatusCode::BAD_REQUEST, "signature must be base64"),
    };
    let public_key = match ed25519_dalek::VerifyingKey::from_bytes(&rec.public_key) {
        Ok(pk) => pk,
        Err(_) => return json_err(StatusCode::INTERNAL_SERVER_ERROR, "corrupt registry key"),
    };
    let message = canonical_device_token_bytes(&request);
    if public_key
        .verify_strict(&message, &ed25519_dalek::Signature::from_bytes(&sig))
        .is_err()
    {
        return json_err(StatusCode::UNAUTHORIZED, "bad device-token signature");
    }
    match state
        .auth
        .registry
        .set_device_token(&request.key_id, Some(&request.device_token))
    {
        Ok(()) => {
            info!(
                key_id = %request.key_id,
                registered = !request.device_token.is_empty(),
                "device push token updated"
            );
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "key_id": request.key_id,
                    "push_registered": !request.device_token.is_empty(),
                })),
            )
        }
        Err(e) => match e {
            crate::auth::registry::RegistryMutationError::UnknownKey(k) => {
                json_err(StatusCode::NOT_FOUND, &format!("unknown key: {k}"))
            }
            crate::auth::registry::RegistryMutationError::Persist(err) => json_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("registry persist failed: {err}"),
            ),
        },
    }
}

fn json_err(status: StatusCode, error: &str) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({ "error": error })))
}

fn now_secs() -> u64 {
    crate::auth::registry::now_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::registry::now_secs;
    use crate::auth::test_support;
    use tower::ServiceExt;

    /// A signed device-token request the handler must accept.
    fn signed_request(
        registry: &crate::auth::registry::DeviceRegistry,
        signing: &ed25519_dalek::SigningKey,
        pubkey: [u8; 32],
        device_token: &str,
    ) -> serde_json::Value {
        let token = registry.registration_token();
        let rec = registry
            .register(&token, pubkey, std::time::Duration::from_secs(3600))
            .expect("register");
        let request = DeviceTokenRequest {
            key_id: rec.key_id.clone(),
            device_token: device_token.to_string(),
            ts: now_secs(),
        };
        let signature = test_support::sign_bytes(signing, &canonical_device_token_bytes(&request));
        serde_json::json!({
            "key_id": rec.key_id,
            "signature": signature,
            "request": request,
        })
    }

    #[tokio::test]
    async fn device_token_registers_and_clears() {
        let state = AppState::default();
        let app = router(state.clone());
        let (signing, pubkey) = test_support::keypair();

        // Register a token.
        let body = signed_request(&state.auth.registry, &signing, pubkey, "a1b2c3d4e5f6");
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/device-token")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let key_id = body["key_id"].as_str().unwrap();
        assert_eq!(
            state.auth.registry.get(key_id).unwrap().device_token.as_deref(),
            Some("a1b2c3d4e5f6")
        );

        // Empty token clears (revocation).
        let body = signed_request(&state.auth.registry, &signing, pubkey, "");
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/device-token")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            state.auth.registry.get(key_id).unwrap().device_token,
            None,
            "empty token clears the push registration"
        );
    }

    #[tokio::test]
    async fn device_token_requires_a_valid_signature_and_freshness() {
        let state = AppState::default();
        let app = router(state.clone());
        let (signing, pubkey) = test_support::keypair();
        let rec = {
            let token = state.auth.registry.registration_token();
            state
                .auth
                .registry
                .register(&token, pubkey, std::time::Duration::from_secs(3600))
                .unwrap()
        };

        // Tampered signature -> 401.
        let request = DeviceTokenRequest {
            key_id: rec.key_id.clone(),
            device_token: "abc".to_string(),
            ts: now_secs(),
        };
        let forged = serde_json::json!({
            "key_id": rec.key_id,
            "signature": test_support::sign_bytes(&signing, b"tampered"),
            "request": request,
        });
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/device-token")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&forged).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            state.auth.registry.get(&rec.key_id).unwrap().device_token,
            None,
            "forged registration must not land"
        );

        // Stale ts -> 400.
        let stale = DeviceTokenRequest {
            key_id: rec.key_id.clone(),
            device_token: "abc".to_string(),
            ts: now_secs() - 3600,
        };
        let signature = test_support::sign_bytes(&signing, &canonical_device_token_bytes(&stale));
        let stale_body = serde_json::json!({
            "key_id": rec.key_id,
            "signature": signature,
            "request": stale,
        });
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/device-token")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&stale_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    /// Unknown key / revoked key refuse without touching the registry.
    #[tokio::test]
    async fn device_token_refuses_unknown_and_revoked_keys() {
        let state = AppState::default();
        let app = router(state.clone());
        let (signing, pubkey) = test_support::keypair();
        let rec = {
            let token = state.auth.registry.registration_token();
            state
                .auth
                .registry
                .register(&token, pubkey, std::time::Duration::from_secs(3600))
                .unwrap()
        };
        state.auth.registry.set_revoked(&rec.key_id, true).unwrap();

        let request = DeviceTokenRequest {
            key_id: rec.key_id.clone(),
            device_token: "abc".to_string(),
            ts: now_secs(),
        };
        let signature = test_support::sign_bytes(&signing, &canonical_device_token_bytes(&request));
        let body = serde_json::json!({
            "key_id": rec.key_id,
            "signature": signature,
            "request": request,
        });
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/device-token")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "revoked key cannot register push");
    }
}
