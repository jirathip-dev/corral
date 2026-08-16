//! Drive-plane contract (P3) — the seam the four P3 workstreams agree on
//! before they diverge:
//!
//! - W1 (drive endpoints) implements the `POST /drive` HTTP layer against
//!   these types; idempotency + capability gating live there.
//! - W2 (claim-based approvals) drives the `approve` flow with the typed
//!   [`ApprovePayload`] prompt_hash check (never trim the prompt).
//! - W3 (device signatures) implements [`DriveAuthorizer`] + [`AuditLog`];
//!   the authorizer gates every drive write before W1 dispatches.
//! - W4 (hardening) audits the boundary (redaction, loopback-only, audit
//!   growth on writes only).
//!
//! Contract rules:
//! - Additive only. New capabilities extend [`Capability`]; existing
//!   variants and field types never change shape.
//! - Signatures cover the canonical JSON bytes of the envelope
//!   ([`canonical_envelope_bytes`]) — deterministic because struct field
//!   order is fixed — so W1 and W3 agree on what a signature signs without
//!   sharing serialization code.
//! - Every write is idempotent by `request_id` (W1 keeps the replay table);
//!   responses always carry the new monotonic `rev`.
//! - The daemon never sends keys by coordinates (D8); `read_tail` is bounded
//!   (200 lines / 32 KiB, D5) and never prefetches.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Bounds for `read_tail` (D5): 200 lines / 32 KiB, never prefetch.
pub const READ_TAIL_MAX_LINES: u32 = 200;
pub const READ_TAIL_MAX_BYTES: usize = 32 * 1024;

/// The six canonical drive capabilities (D7). Server refuses anything not
/// in this set with a typed error, before dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Prompt,
    Interrupt,
    Approve,
    ReadTail,
    Kill,
    Attach,
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Prompt => "prompt",
            Self::Interrupt => "interrupt",
            Self::Approve => "approve",
            Self::ReadTail => "read_tail",
            Self::Kill => "kill",
            Self::Attach => "attach",
        })
    }
}

impl FromStr for Capability {
    type Err = UnknownCapability;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "prompt" => Ok(Self::Prompt),
            "interrupt" => Ok(Self::Interrupt),
            "approve" => Ok(Self::Approve),
            "read_tail" => Ok(Self::ReadTail),
            "kill" => Ok(Self::Kill),
            "attach" => Ok(Self::Attach),
            other => Err(UnknownCapability(other.to_string())),
        }
    }
}

/// Typed error for a capability string that is not part of the contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCapability(pub String);

impl fmt::Display for UnknownCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown capability: {}", self.0)
    }
}

impl std::error::Error for UnknownCapability {}

/// The typed per-capability payloads. Kept small on purpose: W1 parses the
/// envelope's `payload` JSON through these and rejects anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DrivePayload {
    /// Free-form text to send to the agent (W2 uses this for approval
    /// choices only when a prompt is waiting).
    Prompt {
        text: String,
    },
    ReadTail {
        lines: Option<u32>,
    },
    /// Claim-based approval reply (D8): the client echoes the exact
    /// `prompt_hash` it is answering. The host refuses when the current
    /// prompt's hash differs — this kills the wrong-question race.
    Approve {
        approval_id: String,
        prompt_hash: String,
        choice: String,
    },
}

impl DrivePayload {
    /// Parse the envelope payload JSON for `capability`. Unknown shape for a
    /// known capability is a typed error, never a silent no-op.
    pub fn parse(
        capability: Capability,
        payload: &serde_json::Value,
    ) -> Result<Self, PayloadError> {
        let typed: Self = serde_json::from_value(payload.clone()).map_err(|e| PayloadError {
            capability,
            detail: e.to_string(),
        })?;
        match (&typed, capability) {
            (Self::Prompt { .. }, Capability::Prompt)
            | (Self::ReadTail { .. }, Capability::ReadTail)
            | (Self::Approve { .. }, Capability::Approve) => Ok(typed),
            _ => Err(PayloadError {
                capability,
                detail: "payload kind does not match capability".to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadError {
    pub capability: Capability,
    pub detail: String,
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad payload for {}: {}", self.capability, self.detail)
    }
}

impl std::error::Error for PayloadError {}

/// A drive command as issued by a client (W1). `rev` is the client's last
/// seen monotonic rev (display/sanity only — ordering is the server's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveEnvelope {
    pub request_id: String,
    pub capability: Capability,
    /// Canonical `agent_id` (never a pane id — the adapter resolves it).
    pub target: String,
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<u64>,
}

/// Deterministic bytes a signature must cover (W3): the struct serializes
/// with fixed field order, so both sides agree without extra protocol.
pub fn canonical_envelope_bytes(envelope: &DriveEnvelope) -> Vec<u8> {
    serde_json::to_vec(envelope).expect("envelope serializes")
}

/// The signed wire form (W3 emits, W1 receives). `signature` is
/// hex/base64-encoded by W3's scheme; `key_id` selects the device key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDrive {
    pub key_id: String,
    pub signature: String,
    pub envelope: DriveEnvelope,
}

/// Result of a successful [`DriveAuthorizer::verify`]: the envelope plus the
/// identity that vouched for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedDrive {
    pub key_id: String,
    pub envelope: DriveEnvelope,
}

/// Typed authorization failures. W1 maps these onto typed HTTP errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// No signature / malformed signed envelope.
    MissingSignature,
    /// Signature failed verification against the named key.
    BadSignature,
    /// `key_id` is not a registered device key.
    UnknownKey,
    /// Key has passed its expiry.
    Expired,
    /// Key is on the host's revocation list.
    Revoked,
    /// The key is valid but lacks this capability grant (default deny).
    NotGranted(Capability),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSignature => write!(f, "missing signature"),
            Self::BadSignature => write!(f, "bad signature"),
            Self::UnknownKey => write!(f, "unknown device key"),
            Self::Expired => write!(f, "device key expired"),
            Self::Revoked => write!(f, "device key revoked"),
            Self::NotGranted(c) => write!(f, "capability not granted: {c}"),
        }
    }
}

impl std::error::Error for AuthError {}

/// The authorization seam. W1's drive handler calls [`DriveAuthorizer::verify`]
/// on every `POST /drive`; W3 implements it (device keys + grants + expiry +
/// revocation). Default deny: verify returns `NotGranted` unless the key
/// holds the capability.
pub trait DriveAuthorizer: fmt::Debug + Send + Sync {
    /// Verify signature + key validity + capability grant in one step.
    fn verify(&self, signed: &SignedDrive) -> Result<AuthorizedDrive, AuthError>;
}

/// What happened to one drive write; the audit log grows ONLY on writes
/// (reads and failed auth attempts are not appended — AC5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditOutcome {
    Executed,
    Refused(String),
    Failed(String),
}

/// One append-only audit entry (W3 signs the log; W4 verifies it grows only
/// on writes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub ts: u64,
    pub key_id: String,
    pub request_id: String,
    pub capability: String,
    pub target: String,
    pub outcome: AuditOutcome,
}

/// The audit seam. W1 calls [`AuditLog::append`] once per drive write; W3
/// provides the signed append-only implementation.
pub trait AuditLog: fmt::Debug + Send + Sync {
    fn append(&self, entry: &AuditEntry) -> Result<(), String>;
}

/// Response to a drive write (W1). `ok` + `error` are the typed outcome;
/// `rev` is the store's new monotonic rev after the write. For `read_tail`
/// `result` carries `{"lines": [...]}` — the adapter-returned tail, redacted
/// (D9) and bounded (D5: ≤ 200 lines / ≤ 32 KiB) before it left the machine;
/// an empty array means the agent had no output. All other capabilities
/// leave `result` unset.
#[derive(Debug, Clone, Serialize)]
pub struct DriveResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub rev: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_round_trips_and_parses() {
        for cap in [
            Capability::Prompt,
            Capability::Interrupt,
            Capability::Approve,
            Capability::ReadTail,
            Capability::Kill,
            Capability::Attach,
        ] {
            let s = cap.to_string();
            assert_eq!(s.parse::<Capability>().unwrap(), cap);
            let wire = serde_json::to_string(&cap).unwrap();
            assert_eq!(serde_json::from_str::<Capability>(&wire).unwrap(), cap);
        }
        assert_eq!(Capability::from_str("sudo").unwrap_err().0, "sudo");
    }

    #[test]
    fn canonical_bytes_are_deterministic() {
        let e = DriveEnvelope {
            request_id: "req-1".to_string(),
            capability: Capability::Prompt,
            target: "herdr:abc".to_string(),
            payload: serde_json::json!({ "text": "continue" }),
            rev: Some(7),
        };
        let a = canonical_envelope_bytes(&e);
        let b = serde_json::to_vec(&e).unwrap();
        assert_eq!(a, b, "canonical form is just the fixed-order struct bytes");
        // The signature covers the EXACT same bytes a verifier recomputes.
        let parsed: DriveEnvelope = serde_json::from_slice(&a).unwrap();
        assert_eq!(canonical_envelope_bytes(&parsed), a);
    }

    #[test]
    fn payload_parse_rejects_mismatched_kinds() {
        let ok = serde_json::json!({ "kind": "prompt", "text": "go" });
        assert_eq!(
            DrivePayload::parse(Capability::Prompt, &ok).unwrap(),
            DrivePayload::Prompt { text: "go".into() }
        );
        let wrong = serde_json::json!({ "kind": "prompt", "text": "go" });
        assert!(
            DrivePayload::parse(Capability::ReadTail, &wrong).is_err(),
            "prompt payload for read_tail must be refused"
        );
        let approve = serde_json::json!({
            "kind": "approve",
            "approval_id": "ap-1",
            "prompt_hash": "sha256:abc",
            "choice": "y"
        });
        assert_eq!(
            DrivePayload::parse(Capability::Approve, &approve).unwrap(),
            DrivePayload::Approve {
                approval_id: "ap-1".into(),
                prompt_hash: "sha256:abc".into(),
                choice: "y".into()
            }
        );
        let bad = serde_json::json!({ "kind": "interrupt" });
        assert!(DrivePayload::parse(Capability::Interrupt, &bad).is_err());
    }
}
