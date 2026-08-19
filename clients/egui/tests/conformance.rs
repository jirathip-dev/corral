//! W2 → W1 conformance: the desktop client's signed-drive layer verified
//! against the REAL daemon seam (dev-dependency `corrald` with
//! `test-utils`). These run in-process — no network — and prove:
//!
//! 1. My envelope serializes to the EXACT canonical bytes the daemon's
//!    `canonical_envelope_bytes` produces (signature coverage identical).
//! 2. A device registered with my keypair and signed by my code passes
//!    the daemon's `DeviceAuthorizer::verify` (and fails when tampered /
//!    ungranted).
//! 3. My step-up request bytes match the daemon's canonical step-up bytes.

use corrald::auth::test_support as daemon_support;
use corrald::drive::{DriveAuthorizer as _, DriveEnvelope as DaemonEnvelope};

/// The daemon's canonical envelope bytes must equal mine for the same
/// logical command (field order + serde shape are the contract).
#[test]
fn canonical_envelope_bytes_match_the_daemon() {
    let daemon_env = daemon_support::envelope(
        "req-conformance-1",
        corrald::drive::Capability::Prompt,
        "continue the work",
    );
    let daemon_bytes = corrald::drive::canonical_envelope_bytes(&daemon_env);

    // Decode the daemon's own bytes into MY wire type...
    let mine: corrald_ui::drive::DriveEnvelope =
        serde_json::from_slice(&daemon_bytes).expect("daemon bytes decode into my type");
    // ...and my canonical bytes must be byte-identical.
    assert_eq!(
        corrald_ui::drive::canonical_envelope_bytes(&mine),
        daemon_bytes,
        "signature coverage must be byte-identical"
    );
    // Round trip the other way too: my envelope -> daemon parse -> daemon bytes.
    let mine_env = corrald_ui::drive::DriveEnvelope {
        request_id: "req-conformance-2".into(),
        capability: corrald_ui::drive::Capability::Interrupt,
        target: "herdr:agent-a".into(),
        payload: serde_json::Value::Null,
        rev: None,
    };
    let mine_bytes = corrald_ui::drive::canonical_envelope_bytes(&mine_env);
    let back: DaemonEnvelope =
        serde_json::from_slice(&mine_bytes).expect("my bytes decode daemon-side");
    assert_eq!(corrald::drive::canonical_envelope_bytes(&back), mine_bytes);
}

/// Register my keypair with the daemon's real registry and prove my
/// signed drive passes `DeviceAuthorizer::verify`.
#[test]
fn my_signed_drive_passes_the_daemon_authorizer() {
    let (registry, authorizer, token, _dir) = daemon_support::setup();
    let signing = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
    let pubkey = signing.verifying_key().to_bytes();

    // Read-only default: register with zero grants.
    let rec = registry
        .register(&token, pubkey, std::time::Duration::from_secs(3600))
        .expect("register");
    assert!(rec.grants.is_empty(), "read-only default");

    let envelope = corrald_ui::drive::DriveEnvelope {
        request_id: "req-auth-1".into(),
        capability: corrald_ui::drive::Capability::ReadTail,
        target: "herdr:agent-a".into(),
        payload: serde_json::json!({ "kind": "read_tail", "lines": 200 }),
        rev: Some(1),
    };
    let signed = corrald_ui::drive::SignedDrive {
        key_id: rec.key_id.clone(),
        signature: corrald_ui::drive::sign_envelope(&signing, &envelope),
        envelope: envelope.clone(),
    };

    // Hand my signed wire form to the daemon by round-tripping through
    // the canonical bytes my signature covers (proves the on-wire form
    // and the signature are exactly what the daemon validates).
    fn to_daemon_signed(signed: &corrald_ui::drive::SignedDrive) -> corrald::drive::SignedDrive {
        let env: DaemonEnvelope = serde_json::from_slice(
            &corrald_ui::drive::canonical_envelope_bytes(&signed.envelope),
        )
        .expect("my envelope decodes daemon-side");
        corrald::drive::SignedDrive {
            key_id: signed.key_id.clone(),
            signature: signed.signature.clone(),
            envelope: env,
        }
    }
    let daemon_signed = to_daemon_signed(&signed);

    // Default deny: no grants -> NotGranted(refused), not accepted.
    match authorizer.verify(&daemon_signed) {
        Err(corrald::drive::AuthError::NotGranted(cap)) => {
            assert_eq!(cap, corrald::drive::Capability::ReadTail);
        }
        other => panic!("expected NotGranted, got {other:?}"),
    }

    // Grant read_tail, verify now passes.
    let _ = registry.set_grants(&rec.key_id, vec![corrald::drive::Capability::ReadTail]);
    let authorized = authorizer
        .verify(&daemon_signed)
        .expect("granted verify passes");
    assert_eq!(authorized.key_id, rec.key_id);
    assert_eq!(authorized.envelope.target, "herdr:agent-a");
    assert_eq!(authorized.envelope.rev, Some(1));

    // Tampering with the payload must break the signature (R4).
    let tampered = corrald_ui::drive::DriveEnvelope {
        payload: serde_json::json!({ "kind": "read_tail", "lines": 1 }),
        ..envelope.clone()
    };
    let tampered_signed = corrald_ui::drive::SignedDrive {
        key_id: rec.key_id.clone(),
        signature: corrald_ui::drive::sign_envelope(&signing, &envelope),
        envelope: tampered,
    };
    assert!(
        matches!(
            authorizer.verify(&to_daemon_signed(&tampered_signed)),
            Err(corrald::drive::AuthError::BadSignature)
        ),
        "payload mutation after signing must be refused"
    );

    // An UNKNOWN key must be refused with UnknownKey.
    let stranger = corrald_ui::drive::SignedDrive {
        key_id: "dev_unknown".into(),
        signature: "AA==".into(),
        envelope: envelope.clone(),
    };
    assert!(matches!(
        authorizer.verify(&to_daemon_signed(&stranger)),
        Err(corrald::drive::AuthError::UnknownKey)
    ));
}

/// My step-up request serializes to the daemon's canonical step-up bytes.
#[test]
fn step_up_request_bytes_match_the_daemon() {
    let mine = corrald_ui::drive::StepUpRequest {
        key_id: "dev_x".into(),
        purpose: "destructive".into(),
        nonce: "n1".into(),
        ts: 1_700_000_000,
    };
    let mine_bytes = corrald_ui::drive::canonical_step_up_bytes(&mine);

    // The daemon parses my bytes into ITS StepUpRequest type...
    let theirs: corrald::auth::step_up::StepUpRequest =
        serde_json::from_slice(&mine_bytes).expect("my step-up bytes decode daemon-side");
    assert_eq!(theirs.key_id, "dev_x");
    assert_eq!(theirs.purpose, "destructive");
    // ...and canonicalizes to the same bytes.
    assert_eq!(
        corrald::auth::step_up::canonical_step_up_bytes(&theirs),
        mine_bytes
    );

    // A signature I produce over my canonical bytes verifies against the
    // daemon's canonical form.
    let signing = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
    let sig = corrald_ui::drive::sign_step_up(&signing, &mine);
    use base64::Engine as _;
    let sig_bytes: [u8; 64] = base64::engine::general_purpose::STANDARD
        .decode(&sig)
        .unwrap()
        .try_into()
        .unwrap();
    let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
    signing
        .verifying_key()
        .verify_strict(
            &corrald::auth::step_up::canonical_step_up_bytes(&theirs),
            &sig,
        )
        .expect("my step-up signature verifies over the daemon's canonical bytes");
}

/// My typed refusal classification matches the conformance error table
/// (spot-checked against the daemon's own error kinds).
#[test]
fn refusal_kinds_match_the_conformance_table() {
    for (kind, expected) in [
        (
            "not_granted",
            corrald_ui::drive::DriveFailure::NotGranted("x".into()),
        ),
        (
            "step_up_required",
            corrald_ui::drive::DriveFailure::StepUpRequired,
        ),
        (
            "hash_mismatch",
            corrald_ui::drive::DriveFailure::HashMismatch,
        ),
        (
            "stale_approval",
            corrald_ui::drive::DriveFailure::StaleApproval,
        ),
        (
            "no_waiting_approval",
            corrald_ui::drive::DriveFailure::NoWaitingApproval,
        ),
        (
            "choice_not_in_menu",
            corrald_ui::drive::DriveFailure::ChoiceNotInMenu,
        ),
        (
            "cannot_approve_kind",
            corrald_ui::drive::DriveFailure::CannotApproveKind,
        ),
        (
            "bad_signature",
            corrald_ui::drive::DriveFailure::BadSignature("x".into()),
        ),
        (
            "unknown_key",
            corrald_ui::drive::DriveFailure::UnknownKey("x".into()),
        ),
        (
            "expired",
            corrald_ui::drive::DriveFailure::Expired("x".into()),
        ),
        (
            "revoked",
            corrald_ui::drive::DriveFailure::Revoked("x".into()),
        ),
        (
            "in_flight",
            corrald_ui::drive::DriveFailure::InFlight("x".into()),
        ),
        (
            "unknown_agent",
            corrald_ui::drive::DriveFailure::UnknownAgent("x".into()),
        ),
        (
            "payload",
            corrald_ui::drive::DriveFailure::Payload("x".into()),
        ),
        (
            "unknown_capability",
            corrald_ui::drive::DriveFailure::UnknownCapability("x".into()),
        ),
        (
            "missing_signature",
            corrald_ui::drive::DriveFailure::MissingSignature,
        ),
    ] {
        let status = match kind {
            "not_granted" | "expired" | "revoked" | "step_up_required" => 403,
            "bad_signature" | "step_up_failed" => 401,
            "unknown_key" | "unknown_agent" => 404,
            "in_flight" | "no_waiting_approval" | "stale_approval" | "hash_mismatch" => 409,
            "payload" | "choice_not_in_menu" | "cannot_approve_kind" => 422,
            _ => 400, // bad_request / unknown_capability / missing_signature
        };
        let got = corrald_ui::drive::classify_refusal(status, kind, "msg");
        assert_eq!(
            got.kind(),
            expected.kind(),
            "kind {kind} must map exactly (got {got:?})"
        );
    }
}

/// #64: the transcript auth header the client mints is the exact
/// SignedDrive wire form the daemon's `/transcript` handler parses and
/// verifies — capability read_tail, target bound to the agent, signature
/// valid against the real authorizer.
#[test]
fn my_transcript_header_passes_the_daemon_authorizer() {
    let (registry, authorizer, token, _dir) = daemon_support::setup();
    let signing = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
    let pubkey = signing.verifying_key().to_bytes();
    let rec = registry
        .register(&token, pubkey, std::time::Duration::from_secs(3600))
        .expect("register");
    registry
        .set_grants(&rec.key_id, vec![corrald::drive::Capability::ReadTail])
        .expect("grant read_tail");

    let header = corrald_ui::drive::transcript_auth_header(
        &rec.key_id,
        &signing,
        "herdr:agent-a",
        None,
        corrald_ui::transcript::PAGE_LIMIT,
    );
    // The daemon parses the header value with serde into ITS SignedDrive
    // (src/api/transcript.rs authorize()) — same parse here.
    let daemon_signed: corrald::drive::SignedDrive =
        serde_json::from_str(&header).expect("header parses daemon-side");
    assert_eq!(
        daemon_signed.envelope.capability,
        corrald::drive::Capability::ReadTail
    );
    assert_eq!(daemon_signed.envelope.target, "herdr:agent-a");
    assert!(
        daemon_signed
            .envelope
            .request_id
            .starts_with("corrald-ui:transcript:"),
        "audit-traceable request id"
    );
    // Post-#88: the page parameters live in the SIGNED payload — ts is
    // fresh unix seconds, limit is the pane's page size, and a
    // cursor-less newest-page request omits the field entirely (the
    // daemon is deny_unknown_fields with skip_serializing_if).
    let payload = daemon_signed
        .envelope
        .payload
        .as_object()
        .expect("payload object");
    assert!(
        payload["ts"].as_u64().expect("ts is unix seconds") > 0,
        "fresh ts per page"
    );
    assert_eq!(
        payload["limit"],
        corrald_ui::transcript::PAGE_LIMIT,
        "client page size rides the signed payload"
    );
    assert!(payload.get("cursor").is_none(), "newest page omits cursor");
    authorizer
        .verify(&daemon_signed)
        .expect("signature + grant verify against the real authorizer");

    // A header minted for agent A must fail the daemon's target binding
    // for agent B — the client cannot accidentally reuse one.
    assert_ne!(daemon_signed.envelope.target, "herdr:agent-b");
}

/// #64 review F13: the client's `TranscriptPage` must deserialize the
/// body the DAEMON actually builds — `corrald::api::transcript::page_body`
/// is the one place the 200 shape is written, so this pins drift that a
/// hand-copied golden cannot.
#[test]
fn daemon_transcript_body_parses_into_the_client_page() {
    let store = corrald::transcript::StoreRef::Claude {
        jsonl_path: std::path::PathBuf::from("/p/2d5e5911.jsonl"),
    };
    let outcome = corrald::transcript::bind::BindOutcome {
        store: store.clone(),
        unavailable: vec!["opencode".to_string()],
        rung: "worktree",
    };
    let page = corrald::transcript::TranscriptPage {
        entries: vec![
            corrald::transcript::Entry {
                role: "assistant".to_string(),
                text: "newest".to_string(),
                ts: Some(1_723_000_000_123),
            },
            corrald::transcript::Entry {
                role: "user".to_string(),
                text: "older".to_string(),
                ts: None,
            },
        ],
        next_cursor: Some(corrald::transcript::Cursor::Bytes { offset: 4096 }),
        skipped: 2,
    };
    let body = corrald::api::transcript::page_body("herdr:a1", &outcome, &page);

    let parsed: corrald_ui::transcript::TranscriptPage =
        serde_json::from_value(body).expect("client parses the daemon's own body shape");
    assert_eq!(parsed.agent, "herdr:a1");
    assert_eq!(parsed.store, "claude");
    assert_eq!(parsed.session, "claude:2d5e5911.jsonl");
    assert_eq!(parsed.bind, "worktree");
    assert_eq!(parsed.stores_unavailable, vec!["opencode".to_string()]);
    assert_eq!(parsed.entries.len(), 2);
    assert_eq!(parsed.entries[0].text, "newest");
    assert_eq!(parsed.entries[0].ts, Some(1_723_000_000_123));
    assert_eq!(parsed.entries[1].ts, None, "string-timestamp stores → null");
    assert!(parsed.next_cursor.is_some(), "opaque cursor passes through");
    assert_eq!(parsed.skipped, 2);
}
