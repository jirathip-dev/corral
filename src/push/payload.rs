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
//! custom `type`/`agent_id`/`host_id`/… keys the iOS app parses. The
//! DEBUG-only iOS local-notification bridge embeds the same keys so one
//! handler serves both paths.
//!
//! ## Composite target (#397)
//!
//! Every payload carries the daemon's stable X25519 host identity as
//! `host_id` (the same base64 key `GET /host-key` publishes and the phone
//! pins per host profile). The notification TARGET is the composite
//! `(host_id, agent_id)`: `agent_id` stays the raw wire id for the selected
//! host's read requests, while `aps.thread-id` is namespaced
//! `host_id::agent_id` so equal raw agent ids on two independent hosts
//! group as separate notification threads on the phone. The iOS side
//! routes taps by matching `host_id` against the pinned host key of
//! exactly one profile — a display name/URL is never a routing identity.
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
/// Apple's hard payload cap: anything larger is refused with 400
/// PayloadTooLarge.
const APNS_PAYLOAD_LIMIT: usize = 4096;
/// The notifier's own budget: the WHOLE serialized payload must fit under
/// this (N3) — a long prompt must be truncated before it leaves the
/// machine, never silently dropped by Apple.
const PAYLOAD_BUDGET: usize = 3891;
/// Max bytes per choice string (bounds the choice LENGTH as well as the
/// count — a pathological multi-KB choice must not eat the payload budget).
const MAX_CHOICE_BYTES: usize = 200;
/// Max bytes for the alert title (display_name is agent-supplied).
const MAX_TITLE_BYTES: usize = 200;
/// Body budget the shrink loop starts from; reduced until the serialized
/// payload fits under [`PAYLOAD_BUDGET`].
const BODY_BUDGET: usize = 2048;
/// Floor for the shrink loop: with choices/title bounded above, a body at
/// this floor guarantees the total always fits.
const MIN_BODY_BUDGET: usize = 256;
/// Marker appended when a field is truncated for size.
const TRUNCATION_MARKER: &str = "…";

/// Truncate `s` to at most `max_bytes` bytes on a UTF-8 char boundary,
/// appending [`TRUNCATION_MARKER`] when anything was cut. Idempotent: a
/// string that already fits (including an earlier truncation) is returned
/// unchanged, so shrinking the budget in the payload loop never doubles
/// the marker.
fn truncate_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes.saturating_sub(TRUNCATION_MARKER.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &s[..end], TRUNCATION_MARKER)
}

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

/// `POST /grants-read` signed request (#101). The signature covers the
/// fixed-order bytes of this struct (same proof-of-possession discipline as
/// the device-token and step-up requests): the device refreshes its CURRENT
/// grants + expiry without admin involvement or a new key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct GrantsReadRequest {
    pub key_id: String,
    /// Short purpose string (e.g. "grants-read") — part of the signed
    /// envelope so the body cannot be re-targeted.
    pub request: String,
    /// Device clock, seconds since epoch. The host enforces freshness
    /// `|now - ts| < 60s` — replaying an old signed request is refused.
    pub ts: u64,
}

/// Canonical bytes a grants-read signature must cover.
pub fn canonical_grants_read_bytes(request: &GrantsReadRequest) -> Vec<u8> {
    serde_json::to_vec(request).expect("grants-read request serializes")
}

/// APNs push body for a blocked agent (D16 surface 1).
///
/// `host_id` is the daemon's pinned X25519 host identity (#397) — the
/// composite target `(host_id, agent_id)` keeps equal raw agent ids on
/// independent hosts distinct on the phone. `prompt` is redacted at the
/// machine boundary before any byte leaves the daemon, and the
/// `approval_id` + `prompt_hash` pair comes from the store record
/// (derived by the adapter); the phone binds its reply to this hash.
#[allow(clippy::too_many_arguments)]
pub fn blocked_payload(
    agent_id: &str,
    host_id: &str,
    display_name: Option<&str>,
    workspace: &Workspace,
    kind: WaitingOnKind,
    prompt: &str,
    prompt_hash: &str,
    approval_id: &str,
    choices: &[String],
) -> serde_json::Value {
    let title = truncate_bytes(
        &redact(display_name.filter(|s| !s.is_empty()).unwrap_or(agent_id)),
        MAX_TITLE_BYTES,
    );
    // #397: the aps thread-id is the composite target — equal raw agent
    // ids from two hosts never share a Notification Center thread.
    let thread_id = composite_thread_id(host_id, agent_id);
    let choices: Vec<String> = choices
        .iter()
        .take(MAX_CHOICES)
        .map(|c| truncate_bytes(&redact(c), MAX_CHOICE_BYTES))
        .collect();
    // N3: a pathological prompt (embedded diff / multi-paragraph tool
    // description) must be TRUNCATED before it leaves the machine — not
    // silently dropped by Apple's 400 PayloadTooLarge. Measure the whole
    // serialized payload and shrink the body until it fits the budget.
    // truncate_bytes is idempotent, so re-truncating at a smaller budget
    // never doubles the marker.
    let mut budget = BODY_BUDGET;
    loop {
        let body = truncate_bytes(&redact(prompt), budget);
        let candidate = json!({
            "aps": {
                "alert": {
                    "title": &title,
                    "body": body,
                },
                "sound": "default",
                "category": "AGENT_BLOCKED",
                "thread-id": thread_id,
            },
            "type": "blocked",
            "host_id": host_id,
            "agent_id": agent_id,
            "prompt_hash": prompt_hash,
            "approval_id": approval_id,
            "choices": &choices,
            "kind": kind,
            "repo": workspace.repo,
            "branch": workspace.branch,
            "ts": now_secs(),
        });
        if serde_json::to_vec(&candidate)
            .map(|b| b.len())
            .unwrap_or(usize::MAX)
            <= PAYLOAD_BUDGET
            || budget <= MIN_BODY_BUDGET
        {
            // The budget loop is the guarantee; this is the belt-and-braces
            // check that Apple's hard cap is never approached.
            debug_assert!(
                serde_json::to_vec(&candidate)
                    .map(|b| b.len())
                    .unwrap_or(usize::MAX)
                    < APNS_PAYLOAD_LIMIT
            );
            break candidate;
        }
        budget = budget * 3 / 4;
    }
}

/// APNs push body for a done agent (D16 surface 2): a plain completion
/// notification. No category → iOS renders it without action buttons.
pub fn done_payload(
    agent_id: &str,
    host_id: &str,
    display_name: Option<&str>,
    workspace: &Workspace,
) -> serde_json::Value {
    let title = truncate_bytes(
        &redact(display_name.filter(|s| !s.is_empty()).unwrap_or(agent_id)),
        MAX_TITLE_BYTES,
    );
    let body = match (&workspace.repo, &workspace.branch) {
        (Some(repo), Some(branch)) => format!("{repo} · {branch} — done"),
        _ => "done".to_string(),
    };
    json!({
        "aps": {
            "alert": {
                "title": title,
                "body": redact(&body),
            },
            "sound": "default",
            "thread-id": composite_thread_id(host_id, agent_id),
        },
        "type": "done",
        "host_id": host_id,
        "agent_id": agent_id,
        "ts": now_secs(),
    })
}

/// #397: the composite `aps.thread-id` of a notification target. The two
/// halves are joined with the same `::` separator the iOS composite
/// identity uses; host keys are fixed-width base64 so the agent id — which
/// may itself contain `:` — can never be confused for the host half.
fn composite_thread_id(host_id: &str, agent_id: &str) -> String {
    format!("{host_id}::{agent_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #397 fixture host identity: base64 of 32 bytes — the wire form of
    /// the daemon's X25519 public key (any 32-byte value is a valid
    /// X25519 public-key byte string; fixture only).
    const TEST_HOST_ID: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    const OTHER_HOST_ID: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";

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
            TEST_HOST_ID,
            Some("builder"),
            &workspace(),
            WaitingOnKind::Menu,
            "proceed?",
            "sha256:abc",
            "herdr:ses-1:sha256:abc",
            &choices,
        );
        assert_eq!(p["type"], "blocked");
        assert_eq!(p["host_id"], TEST_HOST_ID);
        assert_eq!(p["agent_id"], "herdr:ses-1");
        assert_eq!(p["prompt_hash"], "sha256:abc");
        assert_eq!(p["approval_id"], "herdr:ses-1:sha256:abc");
        assert_eq!(p["kind"], "menu");
        assert_eq!(p["aps"]["category"], "AGENT_BLOCKED");
        assert_eq!(p["aps"]["alert"]["title"], "builder");
        assert_eq!(p["aps"]["alert"]["body"], "proceed?");
        assert_eq!(
            p["aps"]["thread-id"],
            format!("{TEST_HOST_ID}::herdr:ses-1")
        );
        assert_eq!(p["repo"], "jirathip-k/corral");
        assert_eq!(p["branch"], "g26/apns-push");
        assert_eq!(
            p["choices"].as_array().unwrap().len(),
            MAX_CHOICES,
            "choices are bounded to the lock-screen surface"
        );
    }

    #[test]
    fn payloads_namespace_thread_and_host_by_composite_target() {
        // #397: the same raw agent id on two hosts is the SAME target's
        // agent half but a DIFFERENT composite target — distinct host_id
        // values and distinct thread ids, raw agent_id preserved.
        let choices = vec!["y".to_string()];
        let p = blocked_payload(
            "herdr:ses-dup",
            TEST_HOST_ID,
            None,
            &workspace(),
            WaitingOnKind::Menu,
            "proceed?",
            "sha256:abc",
            "herdr:ses-dup:sha256:abc",
            &choices,
        );
        let q = blocked_payload(
            "herdr:ses-dup",
            OTHER_HOST_ID,
            None,
            &workspace(),
            WaitingOnKind::Menu,
            "proceed?",
            "sha256:abc",
            "herdr:ses-dup:sha256:abc",
            &choices,
        );
        assert_eq!(p["agent_id"], "herdr:ses-dup", "raw agent id preserved");
        assert_eq!(q["agent_id"], "herdr:ses-dup", "raw agent id preserved");
        assert_ne!(p["host_id"], q["host_id"]);
        assert_ne!(p["aps"]["thread-id"], q["aps"]["thread-id"]);
        let d = done_payload("herdr:ses-dup", TEST_HOST_ID, None, &workspace());
        let e = done_payload("herdr:ses-dup", OTHER_HOST_ID, None, &workspace());
        assert_ne!(d["aps"]["thread-id"], e["aps"]["thread-id"]);
    }

    #[test]
    fn blocked_payload_redacts_secret_shaped_prompt() {
        // The store holds redacted text by construction, but the push
        // boundary must not depend on that: inject a raw secret-shaped
        // prompt and assert the delivered body is clean (D13/D16).
        let p = blocked_payload(
            "herdr:ses-1",
            TEST_HOST_ID,
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
        let p = done_payload("herdr:ses-1", TEST_HOST_ID, Some("builder"), &workspace());
        assert_eq!(p["type"], "done");
        assert_eq!(p["host_id"], TEST_HOST_ID);
        assert_eq!(p["agent_id"], "herdr:ses-1");
        assert!(p.get("category").is_none(), "done has no action category");
        assert!(
            p["aps"].get("category").is_none(),
            "no category -> no buttons"
        );
        assert!(p["aps"]["alert"]["body"].as_str().unwrap().contains("done"));
    }

    #[test]
    fn oversized_prompt_truncates_to_fit_the_apns_budget() {
        // N3: a 10 KiB prompt (embedded diff, multi-paragraph tool output —
        // routine) must yield a payload Apple accepts, with a truncation
        // marker — never a silent 400 PayloadTooLarge drop.
        let huge = "a".repeat(10 * 1024);
        let p = blocked_payload(
            "herdr:ses-1",
            TEST_HOST_ID,
            Some("builder"),
            &workspace(),
            WaitingOnKind::ApproveTool,
            &huge,
            "sha256:big",
            "herdr:ses-1:sha256:big",
            &["y".to_string(), "n".to_string()],
        );
        let bytes = serde_json::to_vec(&p).expect("payload serializes");
        assert!(
            bytes.len() <= PAYLOAD_BUDGET,
            "10 KiB prompt must shrink the payload under {PAYLOAD_BUDGET} bytes, got {}",
            bytes.len()
        );
        assert!(bytes.len() < APNS_PAYLOAD_LIMIT, "under the APNs hard cap");
        let body = p["aps"]["alert"]["body"].as_str().unwrap();
        assert!(
            body.contains(TRUNCATION_MARKER),
            "truncated prompt carries the marker"
        );
        assert!(
            body.len() < huge.len(),
            "the body was actually cut, not copied verbatim"
        );
        assert_eq!(
            p["prompt_hash"], "sha256:big",
            "the claim is untouched by truncation"
        );
    }

    #[test]
    fn oversized_choices_are_truncated_to_the_per_choice_budget() {
        // N3: the COUNT of choices was already bounded; the LENGTH of each
        // choice is bounded here so 8 pathological choices cannot eat the
        // whole payload budget.
        let huge_choice = "z".repeat(10 * 1024);
        let choices: Vec<String> = vec![huge_choice.clone(); 20];
        let p = blocked_payload(
            "herdr:ses-1",
            TEST_HOST_ID,
            None,
            &workspace(),
            WaitingOnKind::Menu,
            "proceed?",
            "sha256:abc",
            "herdr:ses-1:sha256:abc",
            &choices,
        );
        let body_choices = p["choices"].as_array().unwrap();
        assert_eq!(body_choices.len(), MAX_CHOICES, "count still bounded");
        for c in body_choices {
            let c = c.as_str().unwrap();
            assert!(c.len() <= MAX_CHOICE_BYTES + TRUNCATION_MARKER.len());
            assert!(c.contains(TRUNCATION_MARKER), "oversized choice truncated");
        }
        let bytes = serde_json::to_vec(&p).unwrap();
        assert!(bytes.len() <= PAYLOAD_BUDGET, "still fits the budget");
    }

    #[test]
    fn truncate_bytes_is_utf8_safe_and_idempotent() {
        // Multi-byte char boundaries must never be split, and re-truncating
        // at a smaller budget must not double the marker.
        let s = "é".repeat(100); // 200 bytes, all multi-byte chars
        let t = truncate_bytes(&s, 60);
        assert!(t.is_char_boundary(t.len()), "cut on a char boundary");
        assert!(t.ends_with(TRUNCATION_MARKER));
        assert_eq!(t.matches(TRUNCATION_MARKER).count(), 1);
        let smaller = truncate_bytes(&t, 30);
        assert_eq!(smaller.matches(TRUNCATION_MARKER).count(), 1);
        let fits = truncate_bytes(&t, t.len());
        assert_eq!(fits, t, "a string that already fits is untouched");
    }
}
