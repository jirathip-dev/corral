//! HTTP API served on loopback.
//!
//! - `GET /snapshot` — full current state with monotonic `rev`.
//! - `GET /events`  — SSE; resumes from `Last-Event-ID` (full snapshot when
//!   the cursor is too old, incremental `{rev, upd, del}` otherwise).
//! - `GET /history` — D23: status-transition events from the persistent
//!   ring, oldest first, `?since=<ts>` / `?limit=<n>` filtered.
//! - `GET /healthz` — liveness.
//! - `GET /issues` — #113/#237: read-only repo-level issue view for the
//!   board's issue browser and worktree action (live `workspace.repo`
//!   categories plus fleet-ops CLI validated fleet-name keys; configless —
//!   no `fleets.json` is read).
//! - `GET /fleets` — #237: the fleet-ops CLI validated identity catalog
//!   (name, gh_repo, orch, workers, paused) for the board's read-only Fleets
//!   tab. NOT a registry projection: no `local`/`worktree_dir`/`models`/path
//!   fields. A fleet-ops CLI failure is HTTP 200 `status="error"`.
//! - `POST /drive`  — P3 drive plane (writes): idempotent by `request_id`,
//!   capability-gated, signed by the device authorizer, step-up-gated for
//!   destructive payloads (see [`crate::api::drive`]).
//! - P3 auth surface (W3, [`crate::auth::http`]): `GET /host-key`,
//!   `POST /register`, `POST /step-up`, `GET /grants` (#137 host-admin
//!   device/grants projection), `POST /grants`, `GET /audit`.
//! - `POST /device-token` — D16 push registration: the device signs a
//!   [`DeviceTokenRequest`](crate::push::payload::DeviceTokenRequest) with
//!   its key (same proof-of-possession shape as `/step-up`); the token is
//!   stored on the registry record (revocable per-device).
//! - `POST /grants-read` — #101: signed self-service grants read. The
//!   device signs a
//!   [`GrantsReadRequest`](crate::push::payload::GrantsReadRequest) with its
//!   key; the daemon verifies exactly like `/device-token` and returns that
//!   key's CURRENT grants + expiry — host promotions reach the phone without
//!   a device reset.
//!
//! ## Push notifier arming
//!
//! `main.rs` arms the APNs notifier once per process (it calls
//! [`Notifier::from_env`](crate::push::Notifier::from_env) + `start`
//! before building the router). [`router()`] itself never touches the
//! environment: it is also the test constructor, and arming as a side
//! effect of it made every API test read `CORRAL_APNS_*` from the ambient
//! environment (N6). See [`crate::push`] for the D16 architecture and the
//! provisioning inputs (the `.p8` push key is Guy's).

pub(crate) mod cors;
pub mod drive;
pub mod fleets;
pub mod issues;
pub(crate) mod plugin;
pub(crate) mod repo;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use futures::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use tracing::info;

use crate::adapters::Adapter;
use crate::auth::AuthPlane;
use crate::core::model::{Resume, Snapshot};
use crate::core::store::Store;
use crate::push::payload::{
    DeviceTokenRequest, GrantsReadRequest, canonical_device_token_bytes,
    canonical_grants_read_bytes,
};

use self::drive::{NoopAdapter, ReplayTable, drive};

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
    /// #113: read-only repo-level issues cache (written by the integrator,
    /// served by [`issues::issues`]). The browser and the worktree action
    /// read this; nothing here mutates GitHub.
    pub issues: Arc<issues::IssuesCache>,
    /// #237: the fleet-ops CLI validated identity path (configless corral).
    /// The daemon never reads fleets.json; every actionable fleet identity
    /// (issue-free / issue-linked start targets, `/fleets` catalog) comes
    /// from this provider. Tests inject a stub; production shells
    /// `herdr-fleet list`.
    pub fleets: Arc<dyn crate::fleet::cli::FleetOpsProvider>,
    /// #215: exact-origin allowlist for the read plane's CORS headers
    /// (`--cors-origin` / `CORRALD_CORS_ORIGIN`). Empty (the default) =
    /// no CORS headers at all — the daemon behaves exactly as before.
    /// See [`cors`] for the never-widen rules.
    pub cors_origins: Vec<String>,
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
            issues: Arc::new(issues::IssuesCache::default()),
            // #237: the production provider shells the fleet-ops CLI; tests
            // replace it with a stub so the hermetic suite never depends on
            // a live `herdr-fleet` on PATH.
            fleets: Arc::new(crate::fleet::cli::CliFleetOpsProvider),
            cors_origins: Vec::new(),
        }
    }
}

pub fn router(state: AppState) -> Router {
    // The push notifier is armed by main.rs BEFORE calling this, never as
    // a side effect here: router() is also the test constructor (N6), and
    // reading CORRAL_APNS_* from every test's ambient env would race the
    // config tests and arm a live notifier on machines that export it.
    //
    // #215: the CORS layer sits on the READ routes only, so a browser page
    // from an allowlisted origin can read the live board; the write plane
    // (/drive, device-token, grants-read, auth routes) never emits CORS
    // headers — a cross-origin signed write is blocked by the browser.
    let state = Arc::new(state);
    let read = Router::new()
        .route("/healthz", get(healthz))
        .route("/snapshot", get(snapshot))
        .route("/events", get(events))
        .route("/history", get(history))
        .route("/issues", get(issues::issues))
        .route("/fleets", get(fleets::fleets))
        .route("/plugins", get(plugin::view))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            cors::cors,
        ));
    let write = Router::new()
        .route("/drive", post(drive))
        .route("/plugins/action", post(plugin::action))
        .route("/device-token", post(device_token))
        .route("/grants-read", post(grants_read))
        .merge(crate::auth::http::auth_routes());
    read.merge(write).with_state(state)
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
    let limit = params
        .limit
        .unwrap_or(HISTORY_DEFAULT_LIMIT)
        .min(HISTORY_MAX_LIMIT);
    let events = state.store.history().query(params.since, Some(limit));
    Json(serde_json::json!({ "events": events }))
}

async fn snapshot(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let snap = state.store.snapshot().await;
    let snap = snapshot_with_fleet_health(&state, snap).await;
    Json(serde_json::to_value(&snap).unwrap_or_else(|_| serde_json::json!({ "error": "encode" })))
}

/// #210: fold the read-only per-fleet health aggregation onto a snapshot.
/// The fleet-ops CLI identity path may be unavailable (a herdr-free
/// machine, the CLI went away): the snapshot then simply carries no
/// fleet-health rows — the strip reads as "no registry", never as a
/// fabricated roster. The aggregation never touches wallet/provider state.
async fn snapshot_with_fleet_health(state: &AppState, mut snap: Snapshot) -> Snapshot {
    match state.fleets.list() {
        Ok(identities) => {
            let last_seen = state.adapter.last_seen_millis();
            snap.fleet_health = crate::fleet::health::aggregate(
                &identities,
                &snap.agents,
                &last_seen,
                crate::core::util::now_millis(),
            );
        }
        Err(error) => {
            info!(error = %error, "fleet-ops identity path unavailable; snapshot carries no fleet health");
        }
    }
    snap
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
            let snap = snapshot_with_fleet_health(&state, snap).await;
            info!(rev = snap.rev, "SSE client joined with full snapshot");
            (vec![delta_event("snapshot", snap.rev, &snap)], snap.rev)
        }
        Resume::Deltas {
            deltas,
            live_from_rev,
        } => {
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
        let state = state.clone();
        async move {
            loop {
                match rx.recv().await {
                    Ok(delta) if delta.rev <= live_from_rev => continue,
                    Ok(delta) => {
                        return Some((Ok(delta_event("delta", delta.rev, &delta)), rx));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Client fell too far behind: full resnapshot.
                        let snap = snapshot_with_fleet_health(&state, store.snapshot().await).await;
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
    // N13: the token is spliced into the APNs URL verbatim
    // (…/3/device/<token>), so a stored token must be a plain lowercase
    // hex id — 32–200 chars, like APNs' 64-char tokens. Anything else
    // (path segments, query strings, unbounded length) is refused before
    // it is persisted, so a registered device cannot redirect the provider
    // URL or grow registry.json without limit. An empty token is the
    // documented revocation path and is allowed.
    if !request.device_token.is_empty()
        && (!(32..=200).contains(&request.device_token.len())
            || !request
                .device_token
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
    {
        return json_err(
            StatusCode::BAD_REQUEST,
            "device token must be 32-200 lowercase hex characters",
        );
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

/// Max accepted skew between a signed grants-read request's `ts` and the
/// host clock — same freshness rule as device-token/step-up (replay-proof).
const GRANTS_READ_MAX_SKEW_SECS: u64 = DEVICE_TOKEN_MAX_SKEW_SECS;

/// `POST /grants-read {key_id, signature, request}` → `{ok, key_id, grants,
/// expiry_ts, revoked}`.
///
/// #101 signed self-service grants read. Auth mirrors `POST /device-token`
/// EXACTLY: the signature covers [`canonical_grants_read_bytes`] of the
/// request (fixed-order `{key_id, request, ts}`), verified against the
/// registered device key; freshness `|now - ts| < 60s`; revoked/expired
/// keys are refused. On success the handler returns the CURRENT registry
/// record — never a cached copy — so a host-side promotion reaches the
/// phone without admin involvement or a device reset. No new key material,
/// no token storage.
async fn grants_read(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    let request: GrantsReadRequest =
        match serde_json::from_value(body.get("request").cloned().unwrap_or_default()) {
            Ok(r) => r,
            Err(_) => return json_err(StatusCode::BAD_REQUEST, "malformed grants-read request"),
        };
    if now_secs().abs_diff(request.ts) > GRANTS_READ_MAX_SKEW_SECS {
        return json_err(
            StatusCode::BAD_REQUEST,
            "stale grants-read request: |now - ts| > 60s",
        );
    }
    let signature_b64 = match body.get("signature").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return json_err(StatusCode::BAD_REQUEST, "missing grants-read signature"),
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
    let message = canonical_grants_read_bytes(&request);
    if public_key
        .verify_strict(&message, &ed25519_dalek::Signature::from_bytes(&sig))
        .is_err()
    {
        return json_err(StatusCode::UNAUTHORIZED, "bad grants-read signature");
    }
    info!(key_id = %request.key_id, grants = ?rec.grants, "grants-read served");
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "key_id": rec.key_id,
            "grants": rec.grants,
            "expiry_ts": rec.expiry_ts,
            "revoked": rec.revoked,
        })),
    )
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

    /// A valid-format APNs token (64 lowercase hex chars — the N13 shape).
    const VALID_TOKEN: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

    /// Drive one signed `/device-token` request for `key_id` and return the
    /// HTTP status — shared by the register/clear and N13 rejection tests.
    async fn post_device_token(
        app: &Router,
        key_id: &str,
        signing: &ed25519_dalek::SigningKey,
        device_token: &str,
    ) -> StatusCode {
        let request = DeviceTokenRequest {
            key_id: key_id.to_string(),
            device_token: device_token.to_string(),
            ts: now_secs(),
        };
        let signature = test_support::sign_bytes(signing, &canonical_device_token_bytes(&request));
        let body = serde_json::json!({
            "key_id": key_id,
            "signature": signature,
            "request": request,
        });
        app.clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/device-token")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn device_token_registers_and_clears() {
        let state = AppState::default();
        let app = router(state.clone());
        let (signing, pubkey) = test_support::keypair();

        // Register a token.
        let body = signed_request(&state.auth.registry, &signing, pubkey, VALID_TOKEN);
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
            state
                .auth
                .registry
                .get(key_id)
                .unwrap()
                .device_token
                .as_deref(),
            Some(VALID_TOKEN)
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
                    .body(axum::body::Body::from(
                        serde_json::to_vec(&stale_body).unwrap(),
                    ))
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
        assert_eq!(
            res.status(),
            StatusCode::FORBIDDEN,
            "revoked key cannot register push"
        );
    }

    /// A signed grants-read request the handler must accept (register →
    /// sign the canonical `{key_id, request, ts}` bytes).
    fn signed_grants_read_request(
        registry: &crate::auth::registry::DeviceRegistry,
        signing: &ed25519_dalek::SigningKey,
        pubkey: [u8; 32],
    ) -> serde_json::Value {
        let token = registry.registration_token();
        let rec = registry
            .register(&token, pubkey, std::time::Duration::from_secs(3600))
            .expect("register");
        let request = GrantsReadRequest {
            key_id: rec.key_id.clone(),
            request: "grants-read".to_string(),
            ts: now_secs(),
        };
        let signature = test_support::sign_bytes(signing, &canonical_grants_read_bytes(&request));
        serde_json::json!({
            "key_id": rec.key_id,
            "signature": signature,
            "request": request,
        })
    }

    /// POST one `/grants-read` body and return (status, parsed JSON body) —
    /// the body is read so tests can assert no grants leak on error paths.
    async fn post_grants_read(
        app: &Router,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/grants-read")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    /// #101 THE regression test: a host-side promotion must be visible to
    /// the device on a signed self-service read. Register (read-only
    /// default → grants `[]`), promote via the registry, then `POST
    /// /grants-read` with a valid signature → 200 with the CURRENT grants
    /// and expiry — never a cached snapshot. RED on unfixed code: the
    /// route does not exist → 404 with no grants body.
    #[tokio::test]
    async fn grants_read_returns_current_grants() {
        let state = AppState::default();
        let app = router(state.clone());
        let (signing, pubkey) = test_support::keypair();
        let body = signed_grants_read_request(&state.auth.registry, &signing, pubkey);
        let key_id = body["key_id"].as_str().unwrap().to_string();
        assert_eq!(
            state.auth.registry.get(&key_id).unwrap().grants,
            Vec::<crate::drive::Capability>::new(),
            "precondition: a fresh registration is read-only"
        );

        // Host-side promotion (what `POST /grants` does, admin-only).
        let promoted = vec![
            crate::drive::Capability::ReadTail,
            crate::drive::Capability::Prompt,
            crate::drive::Capability::Interrupt,
            crate::drive::Capability::Approve,
        ];
        state
            .auth
            .registry
            .set_grants(&key_id, promoted.clone())
            .unwrap();
        let expiry_ts = state.auth.registry.get(&key_id).unwrap().expiry_ts;

        let (status, body) = post_grants_read(&app, body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["grants"]
                .as_array()
                .map(|g| g
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect::<Vec<_>>())
                .unwrap_or_default(),
            vec![
                "read_tail".to_string(),
                "prompt".to_string(),
                "interrupt".to_string(),
                "approve".to_string(),
            ],
            "the device sees its CURRENT grants, not the cached []"
        );
        assert_eq!(body["expiry_ts"].as_u64(), Some(expiry_ts));
        assert_eq!(body["revoked"].as_bool(), Some(false));
        assert_eq!(body["ok"].as_bool(), Some(true));
    }

    /// #101 rejection matrix — every failure must refuse with the right
    /// status AND leak no grants: forged signature → 401, stale `ts` →
    /// 400, unknown key → 404, revoked key → 403, expired key → 403.
    #[tokio::test]
    async fn grants_read_rejects_forged_signature_stale_ts_and_unknown_key() {
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
        let fresh = GrantsReadRequest {
            key_id: rec.key_id.clone(),
            request: "grants-read".to_string(),
            ts: now_secs(),
        };

        // Forged signature -> 401, no grants.
        let forged = serde_json::json!({
            "key_id": rec.key_id,
            "signature": test_support::sign_bytes(&signing, b"tampered"),
            "request": fresh.clone(),
        });
        let (status, body) = post_grants_read(&app, forged).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.get("grants").is_none(), "forged read leaks no grants");

        // Stale ts -> 400, no grants.
        let stale = GrantsReadRequest {
            key_id: rec.key_id.clone(),
            request: "grants-read".to_string(),
            ts: now_secs() - 3600,
        };
        let signature = test_support::sign_bytes(&signing, &canonical_grants_read_bytes(&stale));
        let (status, body) = post_grants_read(
            &app,
            serde_json::json!({
                "key_id": rec.key_id,
                "signature": signature,
                "request": stale,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.get("grants").is_none(), "stale read leaks no grants");

        // Unknown key -> 404, no grants (the registry lookup uses the
        // signed request's key_id, so the request itself is the unknown).
        let unknown = GrantsReadRequest {
            key_id: "dev_unknown".to_string(),
            request: "grants-read".to_string(),
            ts: now_secs(),
        };
        let signature = test_support::sign_bytes(&signing, &canonical_grants_read_bytes(&unknown));
        let (status, body) = post_grants_read(
            &app,
            serde_json::json!({
                "key_id": "dev_unknown",
                "signature": signature,
                "request": unknown,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.get("grants").is_none(), "unknown key leaks no grants");

        // Revoked key -> 403, no grants (signature is valid; the refusal
        // happens before verification, mirroring /device-token).
        let (signing2, pubkey2) = test_support::keypair();
        let revoked_rec = {
            let token = state.auth.registry.registration_token();
            state
                .auth
                .registry
                .register(&token, pubkey2, std::time::Duration::from_secs(3600))
                .unwrap()
        };
        state
            .auth
            .registry
            .set_revoked(&revoked_rec.key_id, true)
            .unwrap();
        let request = GrantsReadRequest {
            key_id: revoked_rec.key_id.clone(),
            request: "grants-read".to_string(),
            ts: now_secs(),
        };
        let signature = test_support::sign_bytes(&signing2, &canonical_grants_read_bytes(&request));
        let (status, body) = post_grants_read(
            &app,
            serde_json::json!({
                "key_id": revoked_rec.key_id,
                "signature": signature,
                "request": request,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.get("grants").is_none(), "revoked key leaks no grants");

        // Expired key (TTL 0 → expiry == now) -> 403, no grants.
        let (signing3, pubkey3) = test_support::keypair();
        let expired_rec = {
            let token = state.auth.registry.registration_token();
            state
                .auth
                .registry
                .register(&token, pubkey3, std::time::Duration::from_secs(0))
                .unwrap()
        };
        let request = GrantsReadRequest {
            key_id: expired_rec.key_id.clone(),
            request: "grants-read".to_string(),
            ts: now_secs(),
        };
        let signature = test_support::sign_bytes(&signing3, &canonical_grants_read_bytes(&request));
        let (status, body) = post_grants_read(
            &app,
            serde_json::json!({
                "key_id": expired_rec.key_id,
                "signature": signature,
                "request": request,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.get("grants").is_none(), "expired key leaks no grants");
    }

    /// N13: a device token is spliced into the APNs URL verbatim, so
    /// anything that is not 32–200 lowercase hex chars must be refused
    /// before it is persisted — path traversal, query strings, uppercase
    /// hex, or unbounded length.
    #[tokio::test]
    async fn device_token_rejects_non_hex_or_path_traversing_tokens() {
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

        for bad in [
            "../evil/../secret", // path traversal
            "a1b2c3d4e5f6?x=1",  // query string
            "A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2", // uppercase
            "short",             // too short
            &"a".repeat(300),    // too long
        ] {
            assert_eq!(
                post_device_token(&app, &rec.key_id, &signing, bad).await,
                StatusCode::BAD_REQUEST,
                "token {bad:?} must be rejected"
            );
        }
        assert_eq!(
            state.auth.registry.get(&rec.key_id).unwrap().device_token,
            None,
            "no rejected token is persisted"
        );

        // A valid-format token is accepted (the same signer).
        assert_eq!(
            post_device_token(&app, &rec.key_id, &signing, VALID_TOKEN).await,
            StatusCode::OK
        );
    }
}
