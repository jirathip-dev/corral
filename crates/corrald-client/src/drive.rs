//! Drive-plane wire types, mirroring `src/drive/mod.rs` on main
//! field-for-field. The signature covers [`canonical_envelope_bytes`] —
//! deterministic because struct field order is fixed — so client and daemon
//! agree on what a signature signs without sharing serialization code.
//!
//! The canonical-bytes discipline is load-bearing: the pinned-literal test
//! in `tests/` and the R3/R4 conformance scenarios prove the client
//! reproduces the EXACT bytes the daemon's `serde_json::to_vec` produces.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Bounds for `read_tail` (D5): 200 lines / 32 KiB, never prefetch.
pub const READ_TAIL_MAX_LINES: u32 = 200;
pub const READ_TAIL_MAX_BYTES: usize = 32 * 1024;

/// The six canonical drive capabilities (D7).
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

/// The typed per-capability payloads. `serde(tag = "kind")` guarantees the
/// exact `{"kind": ...}` shapes the daemon's `DrivePayload::parse` accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DrivePayload {
    Prompt {
        text: String,
    },
    ReadTail {
        lines: Option<u32>,
    },
    /// Claim-based approval reply (D8): echo the exact `prompt_hash` of the
    /// snapshot's `waiting_on.prompt`.
    Approve {
        approval_id: String,
        prompt_hash: String,
        choice: String,
    },
}

impl DrivePayload {
    /// Build the JSON payload value a drive envelope carries. Serializing
    /// through the typed enum guarantees the same object shapes (and thus
    /// the same canonical bytes) the daemon produces.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("payload serializes")
    }
}

/// A drive command as issued by a client. **Field order is part of the
/// wire contract**: `canonical_envelope_bytes` is `serde_json::to_vec` on
/// this struct, so declaration order MUST mirror `src/drive/mod.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveEnvelope {
    pub request_id: String,
    pub capability: Capability,
    /// Canonical `agent_id` (never a pane id — the daemon resolves it).
    pub target: String,
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<u64>,
}

/// Deterministic bytes a signature must cover: the fixed-order struct
/// serialization. Identical to the daemon's
/// `src/drive::canonical_envelope_bytes` by construction.
pub fn canonical_envelope_bytes(envelope: &DriveEnvelope) -> Vec<u8> {
    serde_json::to_vec(envelope).expect("envelope serializes")
}

/// The signed wire form. `signature` is base64 of the Ed25519 signature;
/// `key_id` selects the device key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDrive {
    pub key_id: String,
    pub signature: String,
    pub envelope: DriveEnvelope,
}

/// Response to a drive write. `ok:false` + `error` is a dispatch-level
/// refusal that still rides HTTP 200 (and is audited); auth/approval
/// refusals are non-200 with a typed body (see [`crate::errors`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The store's new monotonic rev after the write.
    pub rev: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}
