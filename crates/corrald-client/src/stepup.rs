//! Step-up proof of possession (D10): a signed request minting the
//! single-use token destructive drives need. Mirrors
//! `src/auth/step_up.rs` on main field-for-field — the signature covers
//! [`canonical_step_up_bytes`], the fixed-order struct serialization.
//!
//! Freshness is host-enforced (`|now - ts| < 60s`), so `ts` must be the
//! client's current epoch-seconds at signing time.

use serde::{Deserialize, Serialize};

/// The signed proof-of-possession request sent to `POST /step-up`.
/// **Field order is part of the wire contract** (must mirror the daemon).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepUpRequest {
    pub key_id: String,
    /// `"destructive"` in v1 — the only purpose the host mints for.
    pub purpose: String,
    /// Client-supplied freshness nonce (included in the signed bytes).
    pub nonce: String,
    /// Client clock, seconds since epoch. Host-enforced: requests with
    /// `|host_now - ts| > 60s` are refused as stale.
    pub ts: u64,
}

impl StepUpRequest {
    /// Fresh request with the current wall clock and a fresh nonce.
    pub fn new(key_id: impl Into<String>, nonce: impl Into<String>) -> Self {
        Self {
            key_id: key_id.into(),
            purpose: "destructive".to_string(),
            nonce: nonce.into(),
            ts: now_secs(),
        }
    }
}

/// Canonical bytes a step-up signature must cover — identical to the
/// daemon's `canonical_step_up_bytes` by construction.
pub fn canonical_step_up_bytes(request: &StepUpRequest) -> Vec<u8> {
    serde_json::to_vec(request).expect("step-up request serializes")
}

/// Seconds since Unix epoch (the wire `ts` unit the host enforces).
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Response of `POST /step-up`: a single-use, 5-minute, key-bound token.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StepUpToken {
    pub token: String,
    pub key_id: String,
    pub ttl_secs: u64,
    pub expires_ts: u64,
}
