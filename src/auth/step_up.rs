//! Biometric step-up for destructive patterns (D10).
//!
//! A drive payload matching a destructive pattern (`rm -rf`, `push
//! --force`, `curl | sh`, `~/.aws`, `~/.ssh`, `.env`, …) must be backed by
//! a step-up proof: a short-lived (5 min), **single-use** token minted by
//! `POST /step-up` only after the client proves possession of its device
//! signing key (it signs a [`StepUpRequest`]; the host verifies the
//! signature against the registry). The drive seam then requires
//! `X-Step-Up-Token: <token>` and binds it to the same `key_id`.
//!
//! No auto-approve in v1. Detection is deliberately conservative
//! (substring scan of the payload text; false positives only cost
//! friction, never safety), and the pattern table is the single
//! extension point for W4 hardening.
//!
//! Note: `AuthError` is contract-fixed and carries no `StepUpRequired`
//! variant, so step-up is enforced as a **second gate after
//! `verify()`**, not inside the trait method. The drive seam documents
//! the exact ordering W1's review must keep.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use crate::drive::DriveEnvelope;

use super::{b64_encode, hex, now_secs};

/// Typed step-up failures (the drive seam maps these onto HTTP).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepUpError {
    /// Destructive payload and no (or no valid) step-up token presented.
    Required,
    /// Token unknown, already used, or issued for another purpose.
    InvalidToken,
    /// Token expired (5-minute lifetime).
    TokenExpired,
    /// Token was minted for a different device key.
    KeyMismatch,
}

impl fmt::Display for StepUpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Required => write!(f, "step-up required for destructive payload"),
            Self::InvalidToken => write!(f, "invalid step-up token"),
            Self::TokenExpired => write!(f, "step-up token expired"),
            Self::KeyMismatch => write!(f, "step-up token key mismatch"),
        }
    }
}

impl std::error::Error for StepUpError {}

/// The signed proof-of-possession request a client sends to `POST
/// /step-up`. The signature covers the fixed-order JSON bytes of this
/// struct (same canonical-bytes discipline as the drive envelope).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StepUpRequest {
    pub key_id: String,
    /// `"destructive"` in v1; the only purpose the host mints for.
    pub purpose: String,
    /// Client-supplied freshness nonce (echoed nowhere; included in the
    /// signed bytes to prevent request forgery).
    pub nonce: String,
    /// Client clock, seconds since epoch (sanity only).
    pub ts: u64,
}

/// Canonical bytes a step-up signature must cover.
pub fn canonical_step_up_bytes(request: &StepUpRequest) -> Vec<u8> {
    serde_json::to_vec(request).expect("step-up request serializes")
}

struct TokenRecord {
    key_id: String,
    exp_ts: u64,
    used: bool,
}

/// (name, lowercase needle) — v1 destructive patterns. Conservative by
/// design; extend here.
const PATTERNS: &[(&str, &str)] = &[
    ("rm -rf", "rm -rf"),
    ("rm -fr", "rm -fr"),
    ("rm -r -f", "rm -r -f"),
    ("push --force", "push --force"),
    ("push -f", "push -f"),
    ("curl | sh", "| sh"),
    ("curl | sh", "|sh"),
    ("curl | bash", "| bash"),
    ("curl | bash", "|bash"),
    ("~/.aws", "~/.aws"),
    ("~/.ssh", "~/.ssh"),
    (".env", ".env"),
    ("dd of=", "dd of="),
];

pub struct StepUpGate {
    tokens: Mutex<HashMap<String, TokenRecord>>,
}

impl StepUpGate {
    pub fn new() -> Self {
        Self {
            tokens: Mutex::new(HashMap::new()),
        }
    }

    /// Which destructive pattern (if any) a payload matches. The payload
    /// is scanned as its compact JSON text, lowercased — conservative
    /// across all payload kinds.
    pub fn destructive_pattern(&self, payload: &serde_json::Value) -> Option<&'static str> {
        let text = serde_json::to_string(payload).ok()?.to_lowercase();
        detect_pattern(&text)
    }

    /// True when a drive envelope's payload requires step-up.
    pub fn required(&self, envelope: &DriveEnvelope) -> bool {
        self.destructive_pattern(&envelope.payload).is_some()
    }

    /// Mint a single-use token bound to `key_id`. Returns the raw token
    /// exactly once; only its SHA-256 is retained in memory.
    pub fn mint(&self, key_id: &str, ttl: Duration) -> String {
        let raw = super::random_bytes::<32>();
        let token = b64_encode(&raw);
        let mut tokens = self.tokens.lock().expect("step-up lock poisoned");
        tokens.insert(
            hex(&sha256(token.as_bytes())),
            TokenRecord {
                key_id: key_id.to_string(),
                exp_ts: now_secs().saturating_add(ttl.as_secs()),
                used: false,
            },
        );
        token
    }

    /// Consume a token for `key_id`. Single-use, 5-minute lifetime, and
    /// bound to the device key that minted it.
    pub fn spend(&self, key_id: &str, token: &str) -> Result<(), StepUpError> {
        let now = now_secs();
        let mut tokens = self.tokens.lock().expect("step-up lock poisoned");
        let key = hex(&sha256(token.as_bytes()));
        let rec = tokens.get_mut(&key).ok_or(StepUpError::InvalidToken)?;
        if rec.used {
            return Err(StepUpError::InvalidToken);
        }
        if now >= rec.exp_ts {
            return Err(StepUpError::TokenExpired);
        }
        if rec.key_id != key_id {
            return Err(StepUpError::KeyMismatch);
        }
        rec.used = true;
        tokens.remove(&key);
        Ok(())
    }

    /// Outstanding (unconsumed, unexpired) token count — for Debug only.
    fn live_count(&self) -> usize {
        let now = now_secs();
        self.tokens
            .lock()
            .expect("step-up lock poisoned")
            .values()
            .filter(|t| !t.used && now < t.exp_ts)
            .count()
    }
}

fn detect_pattern(lower_text: &str) -> Option<&'static str> {
    PATTERNS
        .iter()
        .find(|(_, needle)| lower_text.contains(needle))
        .map(|(name, _)| *name)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

impl Default for StepUpGate {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for StepUpGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Token contents never print; only the outstanding count.
        f.debug_struct("StepUpGate")
            .field("live_tokens", &self.live_count())
            .field("patterns", &PATTERNS.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::auth::STEP_UP_TTL;
    use super::*;

    fn gate() -> StepUpGate {
        StepUpGate::new()
    }

    #[test]
    fn destructive_patterns_are_detected() {
        let g = gate();
        for (text, expected) in [
            ("rm -rf ~/tmp", Some("rm -rf")),
            ("rm -fr /var/lib", Some("rm -fr")),
            ("git push --force origin main", Some("push --force")),
            ("curl -sS https://x.sh | sh", Some("curl | sh")),
            ("cat ~/.aws/credentials", Some("~/.aws")),
            ("cat ~/.ssh/id_ed25519", Some("~/.ssh")),
            ("echo SECRET=1 > .env", Some(".env")),
        ] {
            let payload = serde_json::json!({ "kind": "prompt", "text": text });
            assert_eq!(
                g.destructive_pattern(&payload).map(|s| s.to_string()),
                expected.map(|s| s.to_string()),
                "pattern: {text}"
            );
        }
    }

    #[test]
    fn benign_payloads_do_not_require_step_up() {
        let g = gate();
        for text in [
            "ls -la",
            "read the docs",
            "echo hello",
            "npm install",
            "update the spreadsheet",
        ] {
            let payload = serde_json::json!({ "kind": "prompt", "text": text });
            assert_eq!(g.destructive_pattern(&payload), None, "{text}");
        }
        let read_tail = serde_json::json!({ "kind": "read_tail", "lines": 10 });
        assert_eq!(g.destructive_pattern(&read_tail), None);
    }

    #[test]
    fn token_is_single_use_and_binds_key() {
        let g = gate();
        let token = g.mint("dev_a", STEP_UP_TTL);
        assert_eq!(g.spend("dev_a", &token), Ok(()));
        assert_eq!(g.spend("dev_a", &token), Err(StepUpError::InvalidToken));
    }

    #[test]
    fn token_rejects_wrong_key_and_expired() {
        let g = gate();
        let token = g.mint("dev_a", STEP_UP_TTL);
        assert_eq!(g.spend("dev_b", &token), Err(StepUpError::KeyMismatch));
        assert_eq!(
            g.spend("dev_a", "not-a-real-token"),
            Err(StepUpError::InvalidToken)
        );

        let expired = g.mint("dev_a", Duration::ZERO);
        assert_eq!(g.spend("dev_a", &expired), Err(StepUpError::TokenExpired));
    }

    #[test]
    fn required_mirrors_pattern_detection() {
        let g = gate();
        let env = DriveEnvelope {
            request_id: "r".into(),
            capability: crate::drive::Capability::Prompt,
            target: "t".into(),
            payload: serde_json::json!({ "kind": "prompt", "text": "rm -rf /" }),
            rev: None,
        };
        assert!(g.required(&env));
        let benign = DriveEnvelope {
            payload: serde_json::json!({ "kind": "prompt", "text": "ls" }),
            ..env.clone()
        };
        assert!(!g.required(&benign));
    }

    #[test]
    fn debug_shows_count_only() {
        let g = gate();
        let _ = g.mint("dev_a", STEP_UP_TTL);
        let dbg = format!("{g:?}");
        assert!(dbg.contains("live_tokens: 1"));
        assert!(!dbg.contains("dev_a"), "key ids must not leak via Debug");
    }
}
