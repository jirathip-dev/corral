//! P3 auth + audit HTTP surface (W3).
//!
//! - `GET /host-key`  — host identity (X25519 public key, no path disclosure).
//! - `POST /register` — device registration: registration token (routing
//!   only) + device public key → `key_id` + read-only-default grants.
//! - `GET /audit`     — host admin: the hash-chained audit log with
//!   integrity verdict.
//!
//! `POST /drive` is served by W1's handler (`crate::api::drive`), which
//! keeps the documented order: parse → `DriveAuthorizer::verify` →
//! dispatch → audit append. Auth failures are never appended (AC5).
//! #354: the step-up route and the `/grants` grant-admin surface (GET
//! projection + POST set_grants/revoke) are retired with the mutating
//! plane they administered — the daemon now only serves signed reads, and
//! per-device grants are provisioned out-of-band (registry file), never
//! over HTTP.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use axum::routing::{get, post};

use crate::api::AppState;

use super::b64_decode_array_32;

pub fn auth_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/host-key", get(host_key))
        .route("/register", post(register))
        .route("/audit", get(audit))
}

/// GET /host-key — host identity is a curve25519 key, not a hostname.
/// Deliberately discloses no filesystem path (F11).
async fn host_key(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "algorithm": state.auth.host.algorithm(),
        "public_key": state.auth.host.public_key_b64(),
        "note": "host identity is an X25519 key; device writes are signed with per-device Ed25519 keys",
    }))
}

fn json_err(status: StatusCode, error: &str) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({ "error": error })))
}

/// POST /register {token, public_key, name?} -> {key_id, grants, expiry_ts}.
///
/// `name` is an optional human-readable device label (#209, purely
/// cosmetic — never used for auth): trimmed, control characters rejected,
/// truncated to 64 chars. Clients send their own device name (egui: local
/// hostname; iOS: UIDevice name) so the host can show which machine/phone
/// holds which key.
async fn register(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    let token = body.get("token").and_then(|v| v.as_str());
    let pubkey_b64 = body.get("public_key").and_then(|v| v.as_str());
    let Some(token) = token else {
        return json_err(StatusCode::BAD_REQUEST, "missing registration token");
    };
    let Some(pubkey_b64) = pubkey_b64 else {
        return json_err(StatusCode::BAD_REQUEST, "missing public_key");
    };
    let Some(public_key) = b64_decode_array_32(pubkey_b64) else {
        return json_err(
            StatusCode::BAD_REQUEST,
            "public_key must be base64 of 32 bytes",
        );
    };
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(raw) => Some(raw),
        // A non-string name is a malformed request, never silently dropped.
        None if body.get("name").is_some() => {
            return json_err(StatusCode::BAD_REQUEST, "name must be a string");
        }
        None => None,
    };
    match state
        .auth
        .registry
        .register_named(token, public_key, super::REGISTRATION_TTL, name)
    {
        Ok(rec) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "key_id": rec.key_id,
                "grants": rec.grants,
                "expiry_ts": rec.expiry_ts,
                "revoked": rec.revoked,
                "algorithm": "Ed25519",
                "note": "default grants are empty (read-only); the #354 daemon is read-only and grant administration over HTTP was removed",
            })),
        ),
        Err(e) => match e {
            super::RegisterError::BadToken => {
                json_err(StatusCode::UNAUTHORIZED, "bad registration token")
            }
            super::RegisterError::BadPublicKey => json_err(
                StatusCode::BAD_REQUEST,
                "public_key is not a valid Ed25519 point",
            ),
            super::RegisterError::BadName => json_err(
                StatusCode::BAD_REQUEST,
                "name must be a non-empty string without control characters",
            ),
            super::RegisterError::Persist(err) => json_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("registry persist failed: {err}"),
            ),
        },
    }
}

/// GET /audit — host admin (admin token). Returns the hash-chained log
/// with a live integrity verdict.
async fn audit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> (StatusCode, Json<serde_json::Value>) {
    if !state.auth.check_admin(&headers) {
        return json_err(StatusCode::UNAUTHORIZED, "admin token required");
    }
    let (entries, head, valid) = state.auth.audit.chain();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "head": head,
            "valid": valid,
            "entries": entries,
            "note": "grows on drive writes (executions + dispatch refusals); auth failures are never logged",
        })),
    )
}
