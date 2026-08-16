//! Push payloads (D16) — exactly the two surfaces the phone renders:
//!
//! - **blocked**: the prompt (redacted at THIS boundary), its claim
//!   (`prompt_hash` + `approval_id`), the canned `choices[]`, kind and
//!   agent id. The lock-screen actions are bound to the payload's
//!   `prompt_hash`; a reply whose hash no longer matches the live claim is
//!   refused (typed, D13: whitelisted canned surface, no free-text).
//! - **done**: a plain completion notification — no category, no actions.
//!
//! ## Redaction
//!
//! The store already holds redacted prompt text (the herdr adapter
//! redacts at its boundary), but the push path re-applies
//! [`crate::core::redact::redact`] to every text field here — the
//! notifier must never assume its inputs are clean, and a test pins it.
//!
//! ## Wire shape
//!
//! The payload is the APNs body: `aps` (alert/category/thread) plus the
//! custom `type`/`agent_id`/… keys the iOS app parses. The DEBUG-only iOS
//! local-notification bridge embeds the same keys so one handler serves
//! both paths.
//!
//! ## Device-token registration request
//!
//! [`DeviceTokenRequest`] is the signed body for `POST /device-token` —
//! the same proof-of-possession shape as the step-up request (device key
//! signature over fixed-order canonical bytes, freshness enforced).

use serde::Serialize;
use serde_json::json;

use crate::auth::registry::now_secs;
use crate::core::model::{WaitingOnKind, Workspace};
use crate::core::redact::redact;

/// Max choices carried to the lock screen (the adapter already bounds to
/// 8; this is a second, payload-level bound so a pathological record can
/// never blow the 4 KiB APNs limit).
const MAX_CHOICES: usize = 8;

/// `POST /device-token` signed request. The signature covers the
/// fixed-order bytes of this struct (same discipline as the drive envelope
/// and the step-up request).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct DeviceTokenRequest {
    pub key_id: String,
    /// Opaque APNs device token (hex string from the app's
    /// `didRegisterForRemoteNotificationsWithDeviceToken`). Empty string
    /// clears the registration (per-device revocation).
    pub device_token: String,
    /// Device clock, seconds since epoch. The host enforces freshness
    /// `|now - ts| < 60s` — replaying an old signed request is refused.
    pub ts: u64,
}

/// Canonical bytes a device-token signature must cover.
pub fn canonical_device_token_bytes(request: &DeviceTokenRequest) -> Vec<u8> {
    serde_json::to_vec(request).expect("device-token request serializes")
}

/// APNs push body for a blocked agent (D16 surface 1).
///
/// `prompt` is redacted here, at the machine boundary, before any byte
/// leaves the daemon. `approval_id` + `prompt_hash` come from the store
/// record (itself derived by the adapter); the phone binds its reply to
/// this hash.
#[allow(clippy::too_many_arguments)]
pub fn blocked_payload(
    agent_id: &str,
    display_name: Option<&str>,
    workspace: &Workspace,
    kind: WaitingOnKind,
    prompt: &str,
    prompt_hash: &str,
    approval_id: &str,
    choices: &[String],
) -> serde_json::Value {
    let title = display_name.filter(|s| !s.is_empty()).unwrap_or(agent_id);
    let choices: Vec<&str> = choices
        .iter()
        .take(MAX_CHOICES)
        .map(String::as_str)
        .collect();
    json!({
        "aps": {
            "alert": {
                "title": redact(title),
                "body": redact(prompt),
            },
            "sound": "default",
            "category": "AGENT_BLOCKED",
            "thread-id": agent_id,
        },
        "type": "blocked",
        "agent_id": agent_id,
        "prompt_hash": prompt_hash,
        "approval_id": approval_id,
        "choices": choices,
        "kind": kind,
        "repo": workspace.repo,
        "branch": workspace.branch,
        "ts": now_secs(),
    })
}

/// APNs push body for a done agent (D16 surface 2): a plain completion
/// notification. No category → iOS renders it without action buttons.
pub fn done_payload(
    agent_id: &str,
    display_name: Option<&str>,
    workspace: &Workspace,
) -> serde_json::Value {
    let title = display_name.filter(|s| !s.is_empty()).unwrap_or(agent_id);
    let body = match (&workspace.repo, &workspace.branch) {
        (Some(repo), Some(branch)) => format!("{repo} · {branch} — done"),
        _ => "done".to_string(),
    };
    json!({
        "aps": {
            "alert": {
                "title": redact(title),
                "body": redact(&body),
            },
            "sound": "default",
            "thread-id": agent_id,
        },
        "type": "done",
        "agent_id": agent_id,
        "ts": now_secs(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> Workspace {
        Workspace {
            repo: Some("jirathip-k/corral".to_string()),
            branch: Some("g26/apns-push".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn canonical_device_token_bytes_are_fixed_order() {
        let req = DeviceTokenRequest {
            key_id: "dev_abc".to_string(),
            device_token: "a1b2c3".to_string(),
            ts: 1_700_000_000,
        };
        let bytes = canonical_device_token_bytes(&req);
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            r#"{"key_id":"dev_abc","device_token":"a1b2c3","ts":1700000000}"#,
            "field order key_id, device_token, ts (mirrored by the iOS client)"
        );
    }

    #[test]
    fn blocked_payload_carries_the_claim_and_bounds_choices() {
        let choices: Vec<String> = (0..20).map(|i| format!("choice {i}")).collect();
        let p = blocked_payload(
            "herdr:ses-1",
            Some("builder"),
            &workspace(),
            WaitingOnKind::Menu,
            "proceed?",
            "sha256:abc",
            "herdr:ses-1:sha256:abc",
            &choices,
        );
        assert_eq!(p["type"], "blocked");
        assert_eq!(p["agent_id"], "herdr:ses-1");
        assert_eq!(p["prompt_hash"], "sha256:abc");
        assert_eq!(p["approval_id"], "herdr:ses-1:sha256:abc");
        assert_eq!(p["kind"], "menu");
        assert_eq!(p["aps"]["category"], "AGENT_BLOCKED");
        assert_eq!(p["aps"]["alert"]["title"], "builder");
        assert_eq!(p["aps"]["alert"]["body"], "proceed?");
        assert_eq!(p["aps"]["thread-id"], "herdr:ses-1");
        assert_eq!(p["repo"], "jirathip-k/corral");
        assert_eq!(p["branch"], "g26/apns-push");
        assert_eq!(
            p["choices"].as_array().unwrap().len(),
            MAX_CHOICES,
            "choices are bounded to the lock-screen surface"
        );
    }

    #[test]
    fn blocked_payload_redacts_secret_shaped_prompt() {
        // The store holds redacted text by construction, but the push
        // boundary must not depend on that: inject a raw secret-shaped
        // prompt and assert the delivered body is clean (D13/D16).
        let p = blocked_payload(
            "herdr:ses-1",
            None,
            &workspace(),
            WaitingOnKind::ApproveTool,
            "approve deploy with token ghp_AbCdEf1234567890XyZ and key sk-ant-api03-0123456789abcdef0123456789abcdef",
            "sha256:abc",
            "herdr:ses-1:sha256:abc",
            &["y".to_string()],
        );
        let body = p["aps"]["alert"]["body"].as_str().unwrap();
        assert!(!body.contains("ghp_"), "PAT must not leave the machine");
        assert!(
            !body.contains("sk-ant-"),
            "Anthropic key must not leave the machine"
        );
        assert!(body.contains("[REDACTED]"), "redacted span marker present");
        assert_eq!(
            p["prompt_hash"], "sha256:abc",
            "hash covers the redacted prompt; claim untouched"
        );
    }

    #[test]
    fn done_payload_is_plain_without_actions() {
        let p = done_payload("herdr:ses-1", Some("builder"), &workspace());
        assert_eq!(p["type"], "done");
        assert_eq!(p["agent_id"], "herdr:ses-1");
        assert!(p.get("category").is_none(), "done has no action category");
        assert!(
            p["aps"].get("category").is_none(),
            "no category -> no buttons"
        );
        assert!(p["aps"]["alert"]["body"].as_str().unwrap().contains("done"));
    }
}
