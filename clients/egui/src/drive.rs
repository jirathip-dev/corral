//! Signed-read drive client (P3 contract, P4-conformance normative): builds
//! the canonical envelope for the RETAINED read capability (`read_tail`),
//! signs it with the device Ed25519 key, POSTs to `/drive`, and owns the
//! retry policy:
//!
//! - **Idempotent retries**: one `request_id` per LOGICAL action (created
//!   once at tap time, reused for every retry of that action). The daemon's
//!   replay table dedupes: a transport/5xx/409 retry with the same id can
//!   never double-dispatch.
//! - **Typed refusals** surface as [`DriveFailure`] so the UI can render a
//!   typed error banner (`not_granted`, `bad_signature`, `unknown_agent`…).
//!
//! #354 read-only cut: every mutating capability (prompt / interrupt /
//! approve / kill / attach / start_worktree) and the step-up flow were
//! removed with their surfaces. The closed read set mirrors the iOS client:
//! `read_tail` is dispatched; `read_diff` remains only as a wire-decode case
//! (a transitional daemon's capability strings must never fail to decode).

use std::time::{SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signature, Signer, SigningKey};
use serde::{Deserialize, Serialize};

/// The canonical read capabilities (mirrors corrald `Capability` after the
/// #354 daemon cut). Only `read_tail` is ever dispatched by this client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    ReadTail,
    /// Retained as a wire-decode case only (the daemon still knows it); no
    /// egui surface reads a diff after the cut.
    ReadDiff,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadTail => "read_tail",
            Self::ReadDiff => "read_diff",
        }
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Mirrors corrald's `DriveEnvelope` field-for-field (fixed order), so the
/// canonical bytes a signature covers are byte-identical to the daemon's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveEnvelope {
    pub request_id: String,
    pub capability: Capability,
    pub target: String,
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<u64>,
}

/// The canonical bytes a signature must cover (mirrors
/// `corrald::drive::canonical_envelope_bytes`).
pub fn canonical_envelope_bytes(envelope: &DriveEnvelope) -> Vec<u8> {
    serde_json::to_vec(envelope).expect("envelope serializes")
}

/// Signed wire form.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignedDrive {
    pub key_id: String,
    pub signature: String,
    pub envelope: DriveEnvelope,
}

/// 200 body of a drive dispatch (mirrors `DriveResponse`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_kind: Option<String>,
    pub rev: u64,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
}

/// Typed refusal body (`{kind, message, request_id?}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Refusal {
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub request_id: Option<String>,
}

/// A logical read action as issued by the UI. `request_id` is generated
/// once per logical action and is STABLE across all retries of it.
#[derive(Debug, Clone, PartialEq)]
pub struct DriveIntent {
    pub request_id: String,
    pub capability: Capability,
    pub target: String,
    pub payload: serde_json::Value,
    pub rev: Option<u64>,
}

/// The bounded read_tail window the recents v1 drill-in requests. The
/// daemon caps responses at 200 lines / 32 KiB; the v1 tail is LIVE TAIL
/// ONLY (no load-earlier paging after the cut).
pub const READ_TAIL_LINES: u32 = 200;

impl DriveIntent {
    /// Bounded read_tail for the recents v1 drill-in.
    pub fn read_tail(agent_id: &str, rev: Option<u64>) -> Self {
        Self {
            request_id: new_request_id("tail"),
            capability: Capability::ReadTail,
            target: agent_id.to_string(),
            payload: serde_json::json!({ "kind": "read_tail", "lines": READ_TAIL_LINES }),
            rev,
        }
    }

    /// Refresh a cached tail from a source revision. The daemon forwards
    /// this cursor to herdr, which may return only lines newer than it.
    pub fn read_tail_since(agent_id: &str, since_rev: u64, rev: Option<u64>) -> Self {
        let mut intent = Self::read_tail(agent_id, rev);
        intent.payload["since_rev"] = serde_json::json!(since_rev);
        intent
    }
}

/// Result of a drive execution surfaced to the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum DriveOutcome {
    /// `ok: true` — dispatched once (or a byte-identical replay).
    /// `result` is the daemon's optional response payload: for `read_tail`
    /// it carries `{"lines": [...]}` (redacted, bounded ≤ 200 lines /
    /// 32 KiB — see [`parse_tail_lines`]).
    Ok {
        rev: u64,
        result: Option<serde_json::Value>,
    },
    /// Typed refusal (HTTP 4xx pre-dispatch, or `ok:false` at dispatch).
    Refused(DriveFailure),
}

/// Parse `DriveResponse.result` for `read_tail` (`{"lines": [...]}`) into
/// the tail cache shape. Tolerant: missing/wrong-shaped fields yield an
/// empty vec (the daemon bounds to 200 lines / 32 KiB and redacts before
/// the bytes leave it — the client never re-bounds, only renders).
pub fn parse_tail_lines(result: &serde_json::Value) -> Vec<String> {
    result
        .get("lines")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// #315: one CANONICAL semantic block from the daemon's read_tail result.
/// The daemon owns block boundaries AND kinds (including provenance-backed
/// `user` attribution and `unknown` for unprovenanced terminal text); the
/// client renders these verbatim and never re-classifies raw lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalBlock {
    pub kind: CanonicalBlockKind,
    pub text: String,
    /// The signed request id of the recorded Prompt dispatch behind a
    /// `user` block (provenance audit trail; absent otherwise).
    pub prompt_request_id: Option<String>,
}

/// #315: the canonical block kinds (mirrors the daemon's wire vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalBlockKind {
    User,
    Agent,
    Tool,
    System,
    Unknown,
}

impl CanonicalBlockKind {
    /// Decode the daemon's snake_case wire string. An unrecognized kind
    /// decodes to `Unknown` (forward compatible: a future daemon kind must
    /// not crash the board or get mis-rendered as a known role).
    pub fn from_wire(kind: &str) -> Self {
        match kind {
            "user" => Self::User,
            "agent" => Self::Agent,
            "tool" => Self::Tool,
            "system" => Self::System,
            _ => Self::Unknown,
        }
    }

    pub fn wire(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
            Self::Tool => "tool",
            Self::System => "system",
            Self::Unknown => "unknown",
        }
    }
}

/// Parse the additive `blocks` array from a read_tail result. Tolerant:
/// missing/malformed entries are skipped; an absent array yields an empty
/// vec (the caller falls back to the legacy `lines` surface for old
/// daemons — the wire change is backward compatible).
pub fn parse_tail_blocks(result: &serde_json::Value) -> Vec<CanonicalBlock> {
    result
        .get("blocks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let kind = entry.get("kind")?.as_str()?;
                    let text = entry.get("text")?.as_str()?;
                    Some(CanonicalBlock {
                        kind: CanonicalBlockKind::from_wire(kind),
                        text: text.to_string(),
                        prompt_request_id: entry
                            .get("prompt_request_id")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_tail_source_rev(result: &serde_json::Value) -> Option<u64> {
    result.get("source_rev").and_then(serde_json::Value::as_u64)
}

/// Typed refusal space, mirroring the conformance error table:
/// 400/401/404/403/409/422 + dispatch-level `ok:false` outcomes. The
/// approval/step-up rows are retained because the daemon's refusal table
/// still knows them (a transitional daemon may answer any read with any
/// row); no egui surface acts on them anymore.
#[derive(Debug, Clone, PartialEq)]
pub enum DriveFailure {
    BadRequest(String),
    UnknownCapability(String),
    Payload(String),
    MissingSignature,
    BadSignature(String),
    UnknownKey(String),
    Expired(String),
    Revoked(String),
    NotGranted(String),
    StepUpRequired,
    StepUpFailed(String),
    InFlight(String),
    UnknownAgent(String),
    StaleAgent(String),
    NoWaitingApproval,
    StaleApproval,
    HashMismatch,
    ChoiceNotInMenu,
    CannotApproveKind,
    /// Dispatch-level refusal (`ok:false` + error).
    Refused(String),
    /// Dispatch-level transport failure (`ok:false` + error).
    Failed(String),
    /// Transport/5xx — retried per policy, surfaced only when attempts
    /// are exhausted.
    Transport(String),
    /// HTTP 2xx/3xx but unparseable body.
    Malformed(String),
}

impl std::fmt::Display for DriveFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(m) => write!(f, "bad request: {m}"),
            Self::UnknownCapability(m) => write!(f, "unknown capability: {m}"),
            Self::Payload(m) => write!(f, "bad payload: {m}"),
            Self::MissingSignature => write!(f, "missing signature"),
            Self::BadSignature(m) => write!(f, "bad signature: {m}"),
            Self::UnknownKey(m) => write!(f, "unknown device key: {m}"),
            Self::Expired(m) => write!(f, "device key expired: {m}"),
            Self::Revoked(m) => write!(f, "device key revoked: {m}"),
            Self::NotGranted(m) => write!(f, "not granted: {m}"),
            Self::StepUpRequired => write!(f, "step-up required for destructive payload"),
            Self::StepUpFailed(m) => write!(f, "step-up failed: {m}"),
            Self::InFlight(m) => write!(f, "in flight: {m}"),
            Self::UnknownAgent(m) => write!(f, "unknown agent: {m}"),
            Self::StaleAgent(m) => write!(f, "stale agent: {m}"),
            Self::NoWaitingApproval => write!(f, "no waiting approval"),
            Self::StaleApproval => write!(f, "stale approval"),
            Self::HashMismatch => write!(f, "prompt hash mismatch (answered the wrong prompt)"),
            Self::ChoiceNotInMenu => write!(f, "choice not in menu"),
            Self::CannotApproveKind => write!(f, "cannot approve this waiting kind"),
            Self::Refused(m) => write!(f, "refused: {m}"),
            Self::Failed(m) => write!(f, "failed: {m}"),
            Self::Transport(m) => write!(f, "transport: {m}"),
            Self::Malformed(m) => write!(f, "malformed response: {m}"),
        }
    }
}

impl DriveFailure {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad_request",
            Self::UnknownCapability(_) => "unknown_capability",
            Self::Payload(_) => "payload",
            Self::MissingSignature => "missing_signature",
            Self::BadSignature(_) => "bad_signature",
            Self::UnknownKey(_) => "unknown_key",
            Self::Expired(_) => "expired",
            Self::Revoked(_) => "revoked",
            Self::NotGranted(_) => "not_granted",
            Self::StepUpRequired => "step_up_required",
            Self::StepUpFailed(_) => "step_up_failed",
            Self::InFlight(_) => "in_flight",
            Self::UnknownAgent(_) => "unknown_agent",
            Self::StaleAgent(_) => "stale_agent",
            Self::NoWaitingApproval => "no_waiting_approval",
            Self::StaleApproval => "stale_approval",
            Self::HashMismatch => "hash_mismatch",
            Self::ChoiceNotInMenu => "choice_not_in_menu",
            Self::CannotApproveKind => "cannot_approve_kind",
            Self::Refused(_) => "refused",
            Self::Failed(_) => "failed",
            Self::Transport(_) => "transport",
            Self::Malformed(_) => "malformed",
        }
    }

    /// Does this refusal invalidate the device's local grant knowledge?
    pub fn revokes_local_grant(&self) -> bool {
        matches!(self, Self::NotGranted(_))
    }

    /// Refusals that suggest the device key is no longer valid. `BadSignature`
    /// is the #249 rebuild/reinstall case: the board signs with fresh key
    /// material while the registered key_id still names the OLD key — the
    /// daemon's signature check fails, and a re-register (with the current
    /// key) is the fix.
    pub fn suggests_re_registration(&self) -> bool {
        matches!(
            self,
            Self::UnknownKey(_) | Self::Expired(_) | Self::Revoked(_) | Self::BadSignature(_)
        )
    }
}

// ---------------------------------------------------------------------------
// Retry policy
// ---------------------------------------------------------------------------

/// Max attempts per transport-failure burst, then the failure is surfaced.
pub const MAX_TRANSPORT_ATTEMPTS: u32 = 4;
/// Retry backoff base (doubles per attempt).
pub const RETRY_BASE_MS: u64 = 300;
/// A single drive attempt timeout.
pub const DRIVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Wire execution
// ---------------------------------------------------------------------------

pub struct DriveEndpoint {
    pub client: reqwest::Client,
    pub base_url: String,
    pub key_id: String,
    pub signing: SigningKey,
}

/// Sign the canonical envelope bytes with the device key (base64, RFC 4648
/// §4 with padding — matches the daemon's scheme).
pub fn sign_envelope(signing: &SigningKey, envelope: &DriveEnvelope) -> String {
    use base64::Engine;
    let sig: Signature = signing.sign(&canonical_envelope_bytes(envelope));
    base64::engine::general_purpose::STANDARD.encode(sig.to_bytes())
}

/// Fresh `request_id` for a logical action: stable per action, unique per
/// tap. Retries reuse it; the daemon dedupes on it.
pub fn new_request_id(what: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let mut rand = [0u8; 6];
    getrandom::fill(&mut rand).ok();
    let hex = rand.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!("corrald-ui:{what}:{now}:{hex}")
}

/// Execute a logical READ action end-to-end: sign → POST → retry policy.
/// Retries always reuse `intent.request_id` (idempotent by contract).
pub async fn execute_drive(endpoint: &DriveEndpoint, intent: &DriveIntent) -> DriveOutcome {
    let envelope = DriveEnvelope {
        request_id: intent.request_id.clone(),
        capability: intent.capability,
        target: intent.target.clone(),
        payload: intent.payload.clone(),
        rev: intent.rev,
    };
    let signed = SignedDrive {
        key_id: endpoint.key_id.clone(),
        signature: sign_envelope(&endpoint.signing, &envelope),
        envelope: envelope.clone(),
    };

    let mut attempt = 0u32;
    let mut backoff = RETRY_BASE_MS;
    loop {
        match drive_once(endpoint, &signed).await {
            Ok(outcome) => {
                if outcome.ok {
                    return DriveOutcome::Ok {
                        rev: outcome.rev,
                        result: outcome.result,
                    };
                }
                // Dispatch-level `ok:false` (unknown agent at dispatch,
                // transport failure): typed, no retry (the daemon already
                // stored it in the replay table).
                let message = outcome.error.unwrap_or_else(|| "unknown".to_string());
                let failure = if outcome.error_kind.as_deref() == Some("stale_agent") {
                    DriveFailure::StaleAgent(message)
                } else if outcome.error_kind.as_deref() == Some("unknown_agent") {
                    DriveFailure::UnknownAgent(message)
                } else if message.contains("transport")
                    || message.contains("rpc")
                    || message.contains("failed")
                {
                    DriveFailure::Failed(message)
                } else {
                    DriveFailure::Refused(message)
                };
                return DriveOutcome::Refused(failure);
            }
            Err(attempt_failure) => match &attempt_failure {
                DriveFailure::InFlight(_) | DriveFailure::Transport(_) => {
                    // A concurrent duplicate owns the request (or the
                    // network failed): retry with the SAME request_id — the
                    // daemon's replay table makes re-delivery a no-op.
                    if attempt < MAX_TRANSPORT_ATTEMPTS {
                        tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
                        backoff = (backoff * 2).min(5_000);
                        attempt += 1;
                        continue;
                    }
                    return DriveOutcome::Refused(attempt_failure.clone());
                }
                other => return DriveOutcome::Refused(other.clone()),
            },
        }
    }
}

/// One POST /drive. Returns the typed failure for 4xx refusals; `Ok` only
/// for a parseable 200 `DriveResponse`.
async fn drive_once(
    endpoint: &DriveEndpoint,
    signed: &SignedDrive,
) -> Result<DriveResponse, DriveFailure> {
    let url = format!("{}/drive", endpoint.base_url.trim_end_matches('/'));
    let response = match endpoint
        .client
        .post(&url)
        .json(signed)
        .timeout(DRIVE_TIMEOUT)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return Err(DriveFailure::Transport(e.to_string())),
    };
    let status = response.status();
    if status.is_success() {
        let body: DriveResponse = match response.json().await {
            Ok(b) => b,
            Err(e) => return Err(DriveFailure::Malformed(e.to_string())),
        };
        return Ok(body);
    }
    // Typed refusal: parse the `{kind, message}` body; fall back to the
    // status code if the body is not the contract shape.
    let refusal: Option<Refusal> = response.json().await.ok();
    let (kind, message) = refusal
        .map(|r| (r.kind, r.message))
        .unwrap_or_else(|| (status.as_u16().to_string(), "no refusal body".to_string()));
    Err(classify_refusal(status.as_u16(), &kind, &message))
}

/// Map a refused HTTP response onto the typed failure space. Exposed so
/// the conformance suite can assert the exact error table.
pub fn classify_refusal(status: u16, kind: &str, message: &str) -> DriveFailure {
    let message = message.to_string();
    match (status, kind) {
        (400, "bad_request") => DriveFailure::BadRequest(message),
        (400, "unknown_capability") => DriveFailure::UnknownCapability(message),
        (422, "payload") => DriveFailure::Payload(message),
        (400, "missing_signature") => DriveFailure::MissingSignature,
        (401, "bad_signature") => DriveFailure::BadSignature(message),
        (404, "unknown_key") => DriveFailure::UnknownKey(message),
        (403, "expired") => DriveFailure::Expired(message),
        (403, "revoked") => DriveFailure::Revoked(message),
        (403, "not_granted") => DriveFailure::NotGranted(message),
        (403, "step_up_required") => DriveFailure::StepUpRequired,
        (401, "step_up_failed") => DriveFailure::StepUpFailed(message),
        (409, "in_flight") => DriveFailure::InFlight(message),
        (404, "unknown_agent") => DriveFailure::UnknownAgent(message),
        (409, "stale_agent") => DriveFailure::StaleAgent(message),
        (409, "no_waiting_approval") => DriveFailure::NoWaitingApproval,
        (409, "stale_approval") => DriveFailure::StaleApproval,
        (409, "hash_mismatch") => DriveFailure::HashMismatch,
        (422, "choice_not_in_menu") => DriveFailure::ChoiceNotInMenu,
        (422, "cannot_approve_kind") => DriveFailure::CannotApproveKind,
        _ if status >= 500 => DriveFailure::Transport(format!("server error {status}: {message}")),
        _ => DriveFailure::Transport(format!("http {status} {kind}: {message}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #354 RED/GREEN probe — the retained capability surface is the closed
    /// READ set. Reintroducing a mutating capability (prompt/approve/kill/
    /// attach/start_worktree…) must fail this test AND the app-level
    /// dispatch probe before any UI could reach it.
    #[test]
    fn retained_capabilities_are_the_closed_read_set() {
        let names: Vec<&str> = [Capability::ReadTail, Capability::ReadDiff]
            .into_iter()
            .map(Capability::as_str)
            .collect();
        assert_eq!(names, ["read_tail", "read_diff"]);
        assert_eq!(Capability::ReadTail.as_str(), "read_tail");
        assert_eq!(Capability::ReadDiff.as_str(), "read_diff");
    }

    /// The only dispatchable intent this client can build is a bounded
    /// read_tail (v1: 200-line live tail, no load-earlier).
    #[test]
    fn read_tail_intent_is_bounded_and_since_cursor_carries_the_revision() {
        let intent = DriveIntent::read_tail("herdr:agent-a", Some(5));
        assert_eq!(intent.capability, Capability::ReadTail);
        assert_eq!(intent.target, "herdr:agent-a");
        assert_eq!(intent.payload["lines"], serde_json::json!(200));

        let since = DriveIntent::read_tail_since("herdr:agent-a", 42, Some(6));
        assert_eq!(since.payload["since_rev"], serde_json::json!(42));
        assert_eq!(since.rev, Some(6));
    }

    #[test]
    fn tail_parsers_are_tolerant_and_keep_source_rev() {
        let value = serde_json::json!({
            "lines": ["a", "b"],
            "blocks": [
                {"kind": "agent", "text": "a\nb"},
                {"kind": "tool", "text": "tool line"},
                {"kind": "mystery", "text": "x"},
                {"text": "no kind"}
            ],
            "source_rev": 7
        });
        assert_eq!(
            parse_tail_lines(&value),
            vec!["a".to_string(), "b".to_string()]
        );
        let blocks = parse_tail_blocks(&value);
        assert_eq!(blocks.len(), 3, "kind-less entries are skipped");
        assert_eq!(blocks[0].kind, CanonicalBlockKind::Agent);
        assert_eq!(
            blocks[2].kind,
            CanonicalBlockKind::Unknown,
            "unknown kinds stay unknown"
        );
        assert_eq!(parse_tail_source_rev(&value), Some(7));
        assert_eq!(
            parse_tail_lines(&serde_json::json!({})),
            Vec::<String>::new()
        );
        assert_eq!(parse_tail_blocks(&serde_json::json!({})), Vec::new());
        assert_eq!(parse_tail_source_rev(&serde_json::json!({})), None);
    }

    #[test]
    fn canonical_envelope_serializes_in_fixed_order() {
        let envelope = DriveEnvelope {
            request_id: "req-1".into(),
            capability: Capability::ReadTail,
            target: "herdr:agent-a".into(),
            payload: serde_json::json!({ "kind": "read_tail", "lines": 200 }),
            rev: None,
        };
        let bytes = canonical_envelope_bytes(&envelope);
        let text = String::from_utf8(bytes).unwrap();
        let first = text.find("request_id").unwrap();
        let capability = text.find("capability").unwrap();
        let target = text.find("target").unwrap();
        let payload = text.find("payload").unwrap();
        assert!(first < capability && capability < target && target < payload);
        assert!(!text.contains("\"rev\""));
    }

    #[test]
    fn classify_refusal_covers_the_kept_read_error_table() {
        assert!(matches!(
            classify_refusal(403, "not_granted", "capability not granted: read_tail"),
            DriveFailure::NotGranted(_)
        ));
        assert!(matches!(
            classify_refusal(401, "bad_signature", "sig"),
            DriveFailure::BadSignature(_)
        ));
        assert!(matches!(
            classify_refusal(404, "unknown_agent", "agent"),
            DriveFailure::UnknownAgent(_)
        ));
        assert!(matches!(
            classify_refusal(500, "", "boom"),
            DriveFailure::Transport(_)
        ));
        assert!(DriveFailure::NotGranted("x".into()).revokes_local_grant());
        assert!(DriveFailure::BadSignature("x".into()).suggests_re_registration());
    }
}
