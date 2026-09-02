//! The HTTP client surface: every frozen corrald endpoint, typed.
//!
//! - [`CorralClient`] — the read path (`/healthz`, `/snapshot`, `/events`),
//!   the auth surface (`/host-key`, `/register`, `/step-up`), the admin
//!   surface (`/grants`, `/audit`), and a single `POST /drive`.
//! - [`DriveClient`] — signed writes on top of [`CorralClient`] with a
//!   client-side `request_id` replay table and idempotent retries: the same
//!   signed envelope is resent after transport failures / `409 in_flight`,
//!   and the daemon's replay table serves the first response byte-identical,
//!   so exactly one dispatch ever happens.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;

use crate::drive::{Capability, DriveEnvelope, DriveResponse, SignedDrive};
use crate::errors::{ApiError, parse_drive_refusal};
use crate::keypair::DeviceKeypair;
use crate::model::{BuildIdentity, Snapshot};
use crate::sse::{SseEvent, SseStream};
use crate::stepup::{StepUpRequest, StepUpToken};

/// HTTP header carrying a step-up token on `POST /drive` (mirrors the
/// daemon's `src/auth/http.rs::STEP_UP_HEADER`).
pub const STEP_UP_HEADER: &str = "X-Step-Up-Token";

/// Host identity published by `GET /host-key` (X25519 — NOT a hostname).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HostKey {
    pub algorithm: String,
    pub public_key: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// Result of `POST /register`: a registered device key with its grants.
/// Default grants are EMPTY (read-only, D13); drive capabilities are
/// promoted by the host via `POST /grants`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RegisteredDevice {
    pub key_id: String,
    /// Tolerant parse: unknown capability strings are skipped so a future
    /// daemon grant never breaks decoding.
    #[serde(default, deserialize_with = "capabilities")]
    pub grants: Vec<Capability>,
    pub expiry_ts: u64,
    #[serde(default)]
    pub revoked: bool,
    #[serde(default)]
    pub algorithm: Option<String>,
}

/// One device projected by the host-admin `GET /grants` read surface
/// (#209). Public keys and push tokens stay host-side and never cross this
/// wire shape; `name` is the optional cosmetic label the device supplied
/// at registration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GrantDevice {
    pub key_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "capabilities")]
    pub grants: Vec<Capability>,
    #[serde(default)]
    pub revoked: bool,
    #[serde(default)]
    pub expiry_ts: u64,
    #[serde(default)]
    pub created_ts: u64,
}

/// The host-admin `GET /grants` envelope (#209): every registered device
/// with its current grant set + revocation state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AdminGrantsView {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub devices: Vec<GrantDevice>,
}

/// The hash-chained audit log as served by `GET /audit` (admin token).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AuditData {
    pub head: String,
    pub valid: bool,
    /// Opaque to the client: `{ts, key_id, request_id, capability, target,
    /// outcome, prev, hash}` per entry — the admin surface, not part of the
    /// read contract.
    pub entries: Vec<serde_json::Value>,
    #[serde(default)]
    pub note: Option<String>,
}

impl AuditData {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Deserialize a grant list tolerantly (unknown capability strings are
/// skipped — additive-only alignment with the frozen contract).
fn capabilities<'de, D>(deserializer: D) -> Result<Vec<Capability>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Vec<String> = Vec::deserialize(deserializer)?;
    Ok(raw.iter().filter_map(|s| s.parse().ok()).collect())
}

/// A client of the frozen corrald HTTP surface. Clone is cheap (a new
/// request builder under the hood is fine to share).
#[derive(Debug, Clone)]
pub struct CorralClient {
    base: String,
    http: reqwest::Client,
}

impl CorralClient {
    /// `base` is the daemon origin, e.g. `http://127.0.0.1:8474`.
    pub fn new(base: impl AsRef<str>) -> Result<Self, ApiError> {
        let base = base.as_ref().trim_end_matches('/');
        // Validate the origin shape (scheme + host) up front.
        let parsed =
            reqwest::Url::parse(&format!("{base}/")).map_err(|e| ApiError::Url(e.to_string()))?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(ApiError::Url("base URL must be http(s)".to_string()));
        }
        Ok(Self {
            base: base.to_string(),
            http: reqwest::Client::new(),
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    /// GET /healthz — liveness.
    pub async fn healthz(&self) -> Result<(), ApiError> {
        let response = self.http.get(self.endpoint("healthz")).send().await?;
        if !response.status().is_success() {
            return Err(ApiError::Plain {
                status: response.status(),
                error: "healthz failed".to_string(),
            });
        }
        Ok(())
    }

    /// GET /version — non-secret host build/protocol identity.
    pub async fn version(&self) -> Result<BuildIdentity, ApiError> {
        let response = self.http.get(self.endpoint("version")).send().await?;
        if !response.status().is_success() {
            return Err(ApiError::Plain {
                status: response.status(),
                error: "version failed".to_string(),
            });
        }
        Ok(response.json().await?)
    }

    /// GET /snapshot — full current state with the monotonic `rev`.
    pub async fn snapshot(&self) -> Result<Snapshot, ApiError> {
        let response = self.http.get(self.endpoint("snapshot")).send().await?;
        if !response.status().is_success() {
            return Err(ApiError::Plain {
                status: response.status(),
                error: "snapshot failed".to_string(),
            });
        }
        Ok(response.json().await?)
    }

    /// GET /events — the reconnecting SSE stream. `last_rev` becomes the
    /// `Last-Event-ID` header; the daemon replies with a snapshot (missing /
    /// stale / future cursor), a delta replay (covered cursor), or goes
    /// straight to live (current cursor).
    pub fn events(&self, last_rev: Option<u64>) -> SseStream {
        SseStream::new(self.http.clone(), self.endpoint("events"), last_rev)
    }

    /// GET /host-key — host identity (X25519 public key).
    pub async fn host_key(&self) -> Result<HostKey, ApiError> {
        let response = self.http.get(self.endpoint("host-key")).send().await?;
        if !response.status().is_success() {
            return Err(ApiError::Plain {
                status: response.status(),
                error: "host-key failed".to_string(),
            });
        }
        Ok(response.json().await?)
    }

    /// POST /register — enroll a device public key (base64) with the host's
    /// registration token. Returns the `key_id` + read-only-default grants.
    pub async fn register(
        &self,
        registration_token: &str,
        public_key_b64: &str,
    ) -> Result<RegisteredDevice, ApiError> {
        self.register_named(registration_token, public_key_b64, None)
            .await
    }

    /// [`Self::register`] with an optional device display name (#209,
    /// cosmetic only — the daemon stores it as the device's label in the
    /// host-admin Devices/Grants surfaces).
    pub async fn register_named(
        &self,
        registration_token: &str,
        public_key_b64: &str,
        name: Option<&str>,
    ) -> Result<RegisteredDevice, ApiError> {
        let mut body = serde_json::Map::new();
        body.insert("token".to_string(), serde_json::json!(registration_token));
        body.insert("public_key".to_string(), serde_json::json!(public_key_b64));
        if let Some(name) = name {
            body.insert("name".to_string(), serde_json::json!(name));
        }
        let response = self
            .http
            .post(self.endpoint("register"))
            .json(&serde_json::Value::Object(body))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(self.plain_error(response).await);
        }
        Ok(response.json().await?)
    }

    /// POST /step-up — mint a single-use, 5-minute step-up token bound to
    /// the device key, proving possession by signing the canonical
    /// [`StepUpRequest`] bytes.
    pub async fn step_up(
        &self,
        request: &StepUpRequest,
        signature_b64: &str,
    ) -> Result<StepUpToken, ApiError> {
        let response = self
            .http
            .post(self.endpoint("step-up"))
            .json(&json!({
                "key_id": request.key_id,
                "signature": signature_b64,
                "request": request,
            }))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(self.plain_error(response).await);
        }
        Ok(response.json().await?)
    }

    /// POST /grants (admin) — replace a device's grant set (empty =
    /// read-only; default deny).
    pub async fn grants_set(
        &self,
        admin_token: &str,
        key_id: &str,
        grants: &[Capability],
    ) -> Result<(), ApiError> {
        let wire: Vec<String> = grants.iter().map(|c| c.to_string()).collect();
        self.grants(
            admin_token,
            json!({
                "action": "set_grants",
                "key_id": key_id,
                "grants": wire,
            }),
        )
        .await
    }

    /// POST /grants (admin) — flip a device's revocation flag.
    pub async fn grants_revoke(
        &self,
        admin_token: &str,
        key_id: &str,
        revoked: bool,
    ) -> Result<(), ApiError> {
        self.grants(
            admin_token,
            json!({
                "action": "revoke",
                "key_id": key_id,
                "revoked": revoked,
            }),
        )
        .await
    }

    /// GET /grants (admin, #209) — every registered device with its
    /// current grant set, revocation state, and cosmetic display name.
    /// This is the read surface the Devices/Grants UIs render; it
    /// deliberately exposes no public keys or push tokens.
    pub async fn grants_list(&self, admin_token: &str) -> Result<AdminGrantsView, ApiError> {
        let response = self
            .http
            .get(self.endpoint("grants"))
            .bearer_auth(admin_token)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(self.plain_error(response).await);
        }
        Ok(response.json().await?)
    }

    async fn grants(&self, admin_token: &str, body: serde_json::Value) -> Result<(), ApiError> {
        let response = self
            .http
            .post(self.endpoint("grants"))
            .bearer_auth(admin_token)
            .json(&body)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(self.plain_error(response).await);
        }
        let _: serde_json::Value = response.json().await?;
        Ok(())
    }

    /// GET /audit (admin) — the hash-chained audit log with its integrity
    /// verdict. Grows only on drive writes.
    pub async fn audit(&self, admin_token: &str) -> Result<AuditData, ApiError> {
        let response = self
            .http
            .get(self.endpoint("audit"))
            .bearer_auth(admin_token)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(self.plain_error(response).await);
        }
        Ok(response.json().await?)
    }

    /// POST /drive — one signed envelope. Non-2xx refusals are classified
    /// into [`ApiError::Drive`] (typed kind + message + request_id). A 200
    /// with `ok:false` is a dispatch-level refusal and returns as a normal
    /// [`DriveResponse`] (audited server-side).
    ///
    /// No retry happens here — see [`DriveClient`] for idempotent
    /// `request_id` retries.
    pub async fn drive(
        &self,
        signed: &SignedDrive,
        step_up_token: Option<&str>,
    ) -> Result<DriveResponse, ApiError> {
        let mut request = self.http.post(self.endpoint("drive")).json(signed);
        if let Some(token) = step_up_token {
            request = request.header(STEP_UP_HEADER, token);
        }
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.bytes().await?;
            return Err(ApiError::Drive(parse_drive_refusal(status, &body)));
        }
        Ok(response.json().await?)
    }

    async fn plain_error(&self, response: reqwest::Response) -> ApiError {
        let status = response.status();
        let error = response
            .text()
            .await
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|v| {
                v.get("error")
                    .and_then(|e| e.as_str())
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| format!("HTTP {status}"));
        ApiError::Plain { status, error }
    }
}

/// Retry policy for [`DriveClient`]: how many attempts and how long to wait
/// between them.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(2),
        }
    }
}

/// Signed-drive writer with a client-side replay table and idempotent
/// retries.
///
/// Semantics (mirroring the daemon's replay contract):
///
/// - The first 200 `DriveResponse` for a `request_id` is stored locally and
///   returned verbatim for any later submission of the same id (client-side
///   dedup, same rule as the daemon's table).
/// - Retries resend the SAME signed envelope (Ed25519 is deterministic, so
///   the signature is byte-identical). Only transport failures and
///   `409 in_flight` are retried — deterministic refusals
///   (`bad_signature`, `not_granted`, `step_up_required`, approval 409s,
///   ...) are returned immediately, since the daemon does not occupy the
///   replay slot for them and a resend cannot change the outcome.
/// - A step-up-gated retry that lands on `step_up_failed` returns
///   [`ApiError::AmbiguousWrite`]: per the daemon's ordering the token was
///   spent before the replay claim, so the earlier attempt may have
///   dispatched. Do not resubmit; re-read the snapshot to learn the rev.
pub struct DriveClient {
    client: CorralClient,
    keypair: DeviceKeypair,
    replay: Mutex<HashMap<String, DriveResponse>>,
    policy: RetryPolicy,
}

impl DriveClient {
    pub fn new(client: CorralClient, keypair: DeviceKeypair) -> Self {
        Self {
            client,
            keypair,
            replay: Mutex::new(HashMap::new()),
            policy: RetryPolicy::default(),
        }
    }

    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn key_id(&self) -> &str {
        self.keypair.key_id()
    }

    /// Number of stored replay responses (bounded by the caller's usage;
    /// clear with [`Self::clear_replay`]).
    pub fn replay_len(&self) -> usize {
        self.replay.lock().expect("replay lock").len()
    }

    pub fn clear_replay(&self) {
        self.replay.lock().expect("replay lock").clear();
    }

    /// Submit (or replay) one signed envelope. `step_up_token` is required
    /// for destructive payloads (see [`ApiError::Drive`] with kind
    /// [`crate::errors::DriveErrorKind::StepUpRequired`]).
    pub async fn drive(
        &self,
        envelope: &DriveEnvelope,
        step_up_token: Option<&str>,
    ) -> Result<DriveResponse, ApiError> {
        if let Some(stored) = self
            .replay
            .lock()
            .expect("replay lock")
            .get(&envelope.request_id)
        {
            return Ok(stored.clone());
        }

        let signed = SignedDrive {
            key_id: self.keypair.key_id().to_string(),
            signature: self.keypair.sign_envelope(envelope),
            envelope: envelope.clone(),
        };

        let mut backoff = Duration::ZERO;
        for attempt in 0..self.policy.max_attempts {
            if !backoff.is_zero() {
                tokio::time::sleep(backoff).await;
            }
            match self.client.drive(&signed, step_up_token).await {
                Ok(response) => {
                    self.replay
                        .lock()
                        .expect("replay lock")
                        .insert(envelope.request_id.clone(), response.clone());
                    return Ok(response);
                }
                Err(ApiError::Transport(_)) => {
                    // Outcome unknown: retry the same envelope; the daemon's
                    // replay table guarantees at most one dispatch.
                    backoff = next_backoff(backoff, self.policy);
                }
                Err(ApiError::Drive(refusal))
                    if refusal.kind == Some(crate::errors::DriveErrorKind::InFlight) =>
                {
                    // A concurrent duplicate is dispatching; the table will
                    // hold the response shortly.
                    backoff = next_backoff(backoff, self.policy);
                }
                Err(ApiError::Drive(refusal))
                    if refusal.kind == Some(crate::errors::DriveErrorKind::StepUpFailed)
                        && step_up_token.is_some()
                        && attempt > 0 =>
                {
                    // Earlier attempt consumed the token; outcome unknown.
                    return Err(ApiError::AmbiguousWrite {
                        request_id: envelope.request_id.clone(),
                    });
                }
                Err(e) => return Err(e),
            }
        }
        // Exhausted retries on transport/in_flight: report the last refusal
        // style honestly.
        Err(ApiError::Plain {
            status: reqwest::StatusCode::BAD_GATEWAY,
            error: format!(
                "drive {}: retries exhausted after {} attempts (outcome unknown; resubmit \
                 with the same request_id and the replay table answers)",
                envelope.request_id, self.policy.max_attempts
            ),
        })
    }
}

fn next_backoff(current: Duration, policy: RetryPolicy) -> Duration {
    if current.is_zero() {
        policy.base_backoff
    } else {
        (current * 2).min(policy.max_backoff)
    }
}

/// Convenience: a fresh signed envelope for a typed payload.
pub fn envelope(
    request_id: impl Into<String>,
    capability: Capability,
    target: impl Into<String>,
    payload: crate::drive::DrivePayload,
    rev: Option<u64>,
) -> DriveEnvelope {
    DriveEnvelope {
        request_id: request_id.into(),
        capability,
        target: target.into(),
        payload: payload.to_json(),
        rev,
    }
}

/// Build a snapshot-led consumer: fetch the current snapshot, then follow
/// deltas from `SseStream`, returning both. (Small helper for W2's board
/// bootstrapping.)
pub async fn snapshot_and_stream(client: &CorralClient) -> Result<(Snapshot, SseStream), ApiError> {
    let snapshot = client.snapshot().await?;
    let stream = client.events(Some(snapshot.rev));
    Ok((snapshot, stream))
}

/// Test helper: read events until a predicate holds or the timeout hits.
pub async fn wait_for_event(
    stream: &mut SseStream,
    mut predicate: impl FnMut(&SseEvent) -> bool,
    timeout: Duration,
) -> Result<SseEvent, ApiError> {
    let result = tokio::time::timeout(timeout, async {
        loop {
            match stream.next().await {
                Some(Ok(event)) if predicate(&event) => return Ok(event),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(e),
                None => {
                    return Err(ApiError::Plain {
                        status: reqwest::StatusCode::OK,
                        error: "event stream ended".to_string(),
                    });
                }
            }
        }
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => Err(ApiError::Plain {
            status: reqwest::StatusCode::OK,
            error: "timed out waiting for an event".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::DrivePayload;

    #[test]
    fn base_url_normalization() {
        let c = CorralClient::new("http://127.0.0.1:8474/").unwrap();
        assert_eq!(c.base(), "http://127.0.0.1:8474");
        assert_eq!(c.endpoint("snapshot"), "http://127.0.0.1:8474/snapshot");
        assert_eq!(c.endpoint("/snapshot"), "http://127.0.0.1:8474/snapshot");
        assert!(CorralClient::new("not a url").is_err());
        assert!(CorralClient::new("ftp://x").is_err());
    }

    #[test]
    fn envelope_builder_produces_typed_payload() {
        let env = envelope(
            "r-1",
            Capability::ReadTail,
            "herdr:pane:wQ:p1",
            DrivePayload::ReadTail { lines: Some(50) },
            Some(3),
        );
        assert_eq!(
            env.payload,
            serde_json::json!({
                "kind": "read_tail",
                "lines": 50
            })
        );
    }
}
