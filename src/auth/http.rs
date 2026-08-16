//! P3 auth + audit HTTP surface (W3).
//!
//! - `GET /host-key`  — host identity (X25519 public key, no path disclosure).
//! - `POST /register` — device registration: registration token (routing
//!   only) + device public key → `key_id` + read-only-default grants.
//! - `POST /step-up`  — biometric step-up token for destructive payloads
//!   (single-use, 5 min TTL, bound to `key_id`).
//! - `POST /grants`   — host admin (admin token): grant promotion /
//!   revocation / expiry.
//! - `GET /audit`     — host admin: the hash-chained audit log with
//!   integrity verdict.
//!
//! `POST /drive` is served by W1's handler (`crate::api::drive`), which
//! keeps the documented order: parse → `DriveAuthorizer::verify` → step-up
//! gate (`[`STEP_UP_HEADER`]`) → dispatch → audit append. Auth and step-up
//! failures are never appended (AC5).

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;

use crate::api::AppState;

use super::step_up::{StepUpRequest, canonical_step_up_bytes};
use super::{STEP_UP_TTL, b64_decode_array_32, decode_b64, now_secs};
use super::registry::RegistryMutationError;

pub const STEP_UP_HEADER: &str = "X-Step-Up-Token";
/// Max accepted skew between a signed step-up request's `ts` and the host
/// clock (F14) — beyond this the request is treated as stale/replayed.
pub const STEP_UP_MAX_SKEW_SECS: u64 = 60;

pub fn auth_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/host-key", get(host_key))
        .route("/register", post(register))
        .route("/step-up", post(step_up))
        .route("/grants", post(grants))
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

/// POST /register {token, public_key} -> {key_id, grants, expiry_ts}.
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
        return json_err(StatusCode::BAD_REQUEST, "public_key must be base64 of 32 bytes");
    };
    match state
        .auth
        .registry
        .register(token, public_key, super::REGISTRATION_TTL)
    {
        Ok(rec) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "key_id": rec.key_id,
                "grants": rec.grants,
                "expiry_ts": rec.expiry_ts,
                "revoked": rec.revoked,
                "algorithm": "Ed25519",
                "note": "default grants are empty (read-only): drive capabilities are promoted by the host",
            })),
        ),
        Err(e) => match e {
            super::RegisterError::BadToken => {
                json_err(StatusCode::UNAUTHORIZED, "bad registration token")
            }
            super::RegisterError::BadPublicKey => {
                json_err(StatusCode::BAD_REQUEST, "public_key is not a valid Ed25519 point")
            }
            super::RegisterError::Persist(err) => {
                json_err(StatusCode::INTERNAL_SERVER_ERROR, &format!("registry persist failed: {err}"))
            }
        },
    }
}

/// POST /step-up {key_id, signature, request} -> {token, expires_ts}.
///
/// The signature proves possession of the device key: it covers
/// [`canonical_step_up_bytes`] of the request. The minted token is
/// single-use, expires in 5 minutes, and is bound to `key_id`. The
/// request's `ts` freshness is enforced (F14): `|now - ts| < 60s`.
/// Revoked/expired keys cannot mint (F9).
async fn step_up(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    let request: StepUpRequest = match serde_json::from_value(body.get("request").cloned().unwrap_or_default()) {
        Ok(r) => r,
        Err(_) => return json_err(StatusCode::BAD_REQUEST, "malformed step-up request"),
    };
    if request.purpose != "destructive" {
        return json_err(StatusCode::BAD_REQUEST, "unknown step-up purpose");
    }
    // Freshness: reject replayed or stale signed requests (F14).
    if now_secs().abs_diff(request.ts) > STEP_UP_MAX_SKEW_SECS {
        return json_err(StatusCode::BAD_REQUEST, "stale step-up request: |now - ts| > 60s");
    }
    let signature_b64 = match body.get("signature").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return json_err(StatusCode::BAD_REQUEST, "missing step-up signature"),
    };
    let rec = match state.auth.registry.get(&request.key_id) {
        Some(r) => r,
        None => return json_err(StatusCode::NOT_FOUND, "unknown device key"),
    };
    // A revoked or expired key cannot mint step-up tokens (F9) — mirror
    // verify()'s validity checks so a dead key cannot mint/exhaust state.
    if rec.revoked {
        return json_err(StatusCode::FORBIDDEN, "device key revoked");
    }
    if now_secs() >= rec.expiry_ts {
        return json_err(StatusCode::FORBIDDEN, "device key expired");
    }
    let sig = match decode_b64(signature_b64) {
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
    let message = canonical_step_up_bytes(&request);
    if public_key
        .verify_strict(&message, &ed25519_dalek::Signature::from_bytes(&sig))
        .is_err()
    {
        return json_err(StatusCode::UNAUTHORIZED, "bad step-up signature");
    }
    let token = state.auth.step_up.mint(&request.key_id, STEP_UP_TTL);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "token": token,
            "key_id": request.key_id,
            "ttl_secs": STEP_UP_TTL.as_secs(),
            "expires_ts": now_secs().saturating_add(STEP_UP_TTL.as_secs()),
        })),
    )
}

/// POST /grants — host admin (admin token), the v1 host-side promotion /
/// revocation mechanism.
///
/// Bodies:
/// - `{"action":"set_grants","key_id":"...","grants":["prompt", ...]}`
///   replaces the grant set (empty = read-only; default deny).
/// - `{"action":"revoke","key_id":"...","revoked":true|false}` flips the
///   revocation flag (checked on every verify).
async fn grants(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !state.auth.check_admin(&headers) {
        return json_err(StatusCode::UNAUTHORIZED, "admin token required");
    }
    let action = body.get("action").and_then(|v| v.as_str());
    let key_id = body.get("key_id").and_then(|v| v.as_str());
    let Some(key_id) = key_id else {
        return json_err(StatusCode::BAD_REQUEST, "missing key_id");
    };
    let result = match action {
        Some("set_grants") => {
            // Strict parse (F10): every element must be a known capability
            // string — a typo'd grant must fail loudly, never silently
            // produce an empty/partial set.
            let Some(list) = body.get("grants").and_then(|v| v.as_array()) else {
                return json_err(StatusCode::BAD_REQUEST, "grants must be an array of capability strings");
            };
            let mut grants = Vec::with_capacity(list.len());
            for g in list {
                let Some(s) = g.as_str() else {
                    return json_err(
                        StatusCode::BAD_REQUEST,
                        "grants must contain only capability strings",
                    );
                };
                match s.parse() {
                    Ok(cap) => grants.push(cap),
                    Err(_) => {
                        return json_err(
                            StatusCode::BAD_REQUEST,
                            &format!("unknown capability string: {s}"),
                        );
                    }
                }
            }
            state.auth.registry.set_grants(key_id, grants)
        }
        Some("revoke") => {
            let revoked = body.get("revoked").and_then(|v| v.as_bool()).unwrap_or(true);
            state.auth.registry.set_revoked(key_id, revoked)
        }
        other => return json_err(StatusCode::BAD_REQUEST, &format!("unknown action: {other:?}")),
    };
    match result {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true, "key_id": key_id }))),
        Err(e) => match e {
            // F8: unknown key vs persist failure are distinct, typed paths.
            RegistryMutationError::UnknownKey(k) => {
                json_err(StatusCode::NOT_FOUND, &format!("unknown key: {k}"))
            }
            RegistryMutationError::Persist(err) => json_err(
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
            "note": "grows only on drive writes (executions + dispatch refusals); auth failures and GETs are never logged",
        })),
    )
}
