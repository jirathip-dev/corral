//! The W1 conformance suite (P4-conformance.md, scenarios R1-R10) against a
//! REAL corrald (spawned binary, scratch config dir, fake herdr socket).
//!
//! All live-daemon tests are `#[ignore]`d — they need a running daemon
//! binary and are the W1 acceptance bar, run by W2/W3 reviewers as the
//! shared gate:
//!
//! ```text
//! cargo build                      # workspace root; builds the real corrald
//! cargo test -p corrald-client -- --ignored
//! ```
//!
//! Pure wire-format unit tests (canonical bytes pinned to the daemon's own
//! locked literal from tests/auth.rs) run in the normal gate.

mod common;

use std::time::Duration;

use common::{
    AGENT_ID, AGENT_PANE, FakeAgent, LiveDaemon, audit_len, raw_drive, spawn_live_daemon,
    spawn_live_daemon_at, wait_for_agent, wait_for_dispatch_count, wait_for_head,
    wait_for_waiting_on,
};
use corrald_client::approval::{approval_id_for, prompt_hash_of};
use corrald_client::client::envelope;
use corrald_client::drive::{DrivePayload, SignedDrive, canonical_envelope_bytes};
use corrald_client::errors::{ApiError, DriveErrorKind};
use corrald_client::keypair::DeviceKeypair;
use corrald_client::model::AgentState;
use corrald_client::stepup::{StepUpRequest, canonical_step_up_bytes};
use corrald_client::{
    CorralClient, DriveClient, SseEvent,
    model::{SCHEMA_VERSION, apply_delta},
};
use futures::StreamExt as _;
use serde_json::json;

use corrald::adapters::gh_plane::TRACKED_REPOS;

const TIME_BUDGET: Duration = Duration::from_secs(20);
/// R11 waits on the REAL gh API + the git plane: one poll round-trip plus
/// probe/debounce margins, plus the gh plane's 60s foreground cadence as
/// slack. Generous on purpose — a real regression fails on the assertion,
/// not on a tight clock.
const LIVE_GH_BUDGET: Duration = Duration::from_secs(60);

async fn client_of(daemon: &LiveDaemon) -> CorralClient {
    CorralClient::new(&daemon.base).expect("client")
}

/// Grant a capability to a device via the admin surface.
async fn grant(
    daemon: &LiveDaemon,
    client: &CorralClient,
    key_id: &str,
    caps: &[corrald_client::Capability],
) {
    client
        .grants_set(&daemon.admin_token, key_id, caps)
        .await
        .expect("admin grant");
}

// ---------------------------------------------------------------------------
// Wire-format pins (no daemon; part of the normal gate)
// ---------------------------------------------------------------------------

/// The daemon's own test (`tests/auth.rs::canonical_bytes_are_the_locked_signing_format`)
/// pins this literal as equal to its `serde_json::to_vec`. The client must
/// reproduce it byte-for-byte — this is the canonical-byte cross-check.
#[test]
fn canonical_envelope_bytes_match_the_daemon_locked_literal() {
    let envelope = corrald_client::DriveEnvelope {
        request_id: "req-1".to_string(),
        capability: corrald_client::Capability::Prompt,
        target: "herdr:agent-a".to_string(),
        payload: json!({ "kind": "prompt", "text": "hi" }),
        rev: None,
    };
    let literal = br#"{"request_id":"req-1","capability":"prompt","target":"herdr:agent-a","payload":{"kind":"prompt","text":"hi"}}"#;
    assert_eq!(canonical_envelope_bytes(&envelope), literal);
}

/// The rev field must serialize in the same fixed position the daemon uses
/// (`{"request_id":...,"capability":...,"target":...,"payload":...,"rev":N}`).
#[test]
fn canonical_bytes_include_rev_in_fixed_position() {
    let envelope = corrald_client::DriveEnvelope {
        request_id: "r".to_string(),
        capability: corrald_client::Capability::Prompt,
        target: "t".to_string(),
        payload: json!({ "kind": "prompt", "text": "go" }),
        rev: Some(7),
    };
    let literal = br#"{"request_id":"r","capability":"prompt","target":"t","payload":{"kind":"prompt","text":"go"},"rev":7}"#;
    assert_eq!(canonical_envelope_bytes(&envelope), literal);
    // None omits the field entirely.
    let none = corrald_client::DriveEnvelope {
        rev: None,
        ..envelope
    };
    assert!(
        !canonical_envelope_bytes(&none)
            .windows(4)
            .any(|w| w == b"\"rev\"")
    );
}

/// Payload object keys serialize sorted (serde_json Map = BTreeMap), so the
/// signature is independent of construction order.
#[test]
fn payload_bytes_are_key_order_independent() {
    let env1 = corrald_client::DriveEnvelope {
        request_id: "r".into(),
        capability: corrald_client::Capability::Prompt,
        target: "t".into(),
        payload: json!({ "kind": "prompt", "text": "hi" }),
        rev: None,
    };
    // Same object, reverse insertion order.
    let mut reversed = serde_json::Map::new();
    reversed.insert("text".into(), json!("hi"));
    reversed.insert("kind".into(), json!("prompt"));
    let env2 = corrald_client::DriveEnvelope {
        payload: serde_json::Value::Object(reversed),
        ..env1.clone()
    };
    assert_eq!(
        canonical_envelope_bytes(&env1),
        canonical_envelope_bytes(&env2)
    );
}

/// Step-up request bytes: fixed struct field order, `ts` last.
#[test]
fn step_up_canonical_bytes_are_fixed_order() {
    let request = StepUpRequest {
        key_id: "dev_x".to_string(),
        purpose: "destructive".to_string(),
        nonce: "n-1".to_string(),
        ts: 1234,
    };
    let literal = br#"{"key_id":"dev_x","purpose":"destructive","nonce":"n-1","ts":1234}"#;
    assert_eq!(canonical_step_up_bytes(&request), literal);
}

/// G21 wire pin: `head_sha` + `head_subject` survive snapshot decode with
/// their values intact — the conformance decode path a v4 client uses for
/// /snapshot and SSE frames (schema-strict, additive).
#[test]
fn workspace_head_fields_round_trip_through_json() {
    use corrald_client::model::Workspace;

    let ws = Workspace {
        repo: Some("herdr-board".to_string()),
        branch: Some("g21/head-fields".to_string()),
        worktree_path: Some("/wt/a".to_string()),
        pr_number: Some(42),
        ci_status: Some(corrald_client::model::CiStatus::Success),
        dirty: false,
        ahead: 1,
        behind: 0,
        pr_match_source: None,
        issues: Vec::new(),
        head_sha: Some("a1b3f9c48b8e9cfbe7f42ee64f4e8cd8f5f6b9a2".to_string()),
        head_subject: Some("corral: add head fields".to_string()),
    };
    let wire = serde_json::to_string(&ws).expect("serialize");
    let back: Workspace = serde_json::from_str(&wire).expect("decode");
    assert_eq!(
        back, ws,
        "head fields round-trip through the snapshot wire format"
    );
    assert!(
        wire.contains("\"head_sha\""),
        "head_sha serializes on the wire"
    );
    assert!(
        wire.contains("\"head_subject\""),
        "head_subject serializes on the wire"
    );

    // Unborn/empty checkout: null on the wire decodes to None.
    let ws: Workspace = serde_json::from_str(
        r#"{"repo":null,"branch":null,"worktree_path":"/wt/u","pr_number":null,"head_sha":null,"head_subject":null}"#,
    )
    .expect("null head fields decode");
    assert_eq!(ws.head_sha, None);
    assert_eq!(ws.head_subject, None);
}

// ---------------------------------------------------------------------------
// R1-R10 against a real corrald (the W1 acceptance bar)
// ---------------------------------------------------------------------------

/// R1 — Register: POST /register with the registration token + a fresh
/// device keypair → key_id with EMPTY grants (read-only default).
#[tokio::test]
#[ignore = "requires a live corrald; run the suite with --ignored"]
async fn r1_register() {
    let daemon = spawn_live_daemon().await;
    let client = client_of(&daemon).await;
    let keypair = DeviceKeypair::generate();

    // Wrong token -> 401, typed.
    let err = client
        .register("nope", &keypair.public_key_b64())
        .await
        .expect_err("bad token must be refused");
    match err {
        ApiError::Plain { status, error } => {
            assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
            assert!(error.contains("token"), "typed error: {error}");
        }
        other => panic!("expected plain 401, got {other:?}"),
    }

    // Right token -> key_id + empty grants (read-only default).
    let dev = client
        .register(&daemon.registration_token, &keypair.public_key_b64())
        .await
        .expect("register");
    assert!(dev.key_id.starts_with("dev_"));
    assert!(dev.grants.is_empty(), "read-only default: {dev:?}");
    assert_eq!(dev.algorithm.as_deref(), Some("Ed25519"));
    // The client derives the same key_id the daemon's registry assigns.
    assert_eq!(dev.key_id, keypair.key_id());
    println!("R1 pass: key_id={} grants=[]", dev.key_id);
}

/// R2 — Read path: /snapshot returns schema 4, monotonic rev, agents with
/// head facts; /events resumes from Last-Event-ID (snapshot | deltas) and
/// delivers live deltas.
#[tokio::test]
#[ignore = "requires a live corrald; run the suite with --ignored"]
async fn r2_read_path_and_sse_resume() {
    let daemon = spawn_live_daemon().await;
    let client = client_of(&daemon).await;
    // G21: wait until the git plane's head facts merged, then pin them —
    // the scratch repo's HEAD sha + first-line subject must round-trip
    // through the real pipeline (plane -> integrator -> store -> snapshot
    // -> client decode) unchanged.
    let snap = wait_for_head(&client, AGENT_ID, TIME_BUDGET).await;
    assert_eq!(snap.schema_version, SCHEMA_VERSION);
    assert!(snap.agents.contains_key(AGENT_ID));
    let agent = &snap.agents[AGENT_ID];
    assert_eq!(agent.source, "herdr");
    assert_eq!(agent.state, AgentState::Idle);
    assert_eq!(
        agent.attachment.as_ref().map(|a| a.kind.as_str()),
        Some("herdr-pane")
    );
    assert_eq!(
        agent.workspace.head_sha.as_deref(),
        Some(daemon.repo_head_sha.as_str()),
        "snapshot carries the real HEAD sha (G21 acceptance 1)"
    );
    // F4: the FULL subject text is pinned — the harness commits the subject
    // with leading whitespace ("  conformance initial commit  "), so this
    // exact literal proves the probe's first-line extraction + trim.
    assert_eq!(
        agent.workspace.head_subject.as_deref(),
        Some("conformance initial commit"),
        "snapshot carries the trimmed first-line subject (G21 acceptance 1, F4)"
    );
    let rev0 = snap.rev;

    // (a) Current cursor -> Live: NO initial frame. The daemon emits nothing
    // until the next change.
    let mut live = client.events(Some(rev0));
    let nothing = tokio::time::timeout(Duration::from_millis(700), live.next()).await;
    assert!(
        nothing.is_err(),
        "current cursor must go straight to live (no frame)"
    );

    // (b) Live delta: push a status change, expect a delta with rev0+1.
    daemon.herdr.set_status(AGENT_PANE, "working").await;
    let event = tokio::time::timeout(TIME_BUDGET, live.next())
        .await
        .expect("live delta")
        .expect("no error")
        .expect("event");
    match &event {
        SseEvent::Delta(delta) => {
            assert_eq!(delta.rev, rev0 + 1, "deltas bump the monotonic rev");
            assert!(
                delta.upd.iter().any(|a| a.agent_id == AGENT_ID),
                "delta carries the updated record"
            );
            assert!(delta.del.is_empty());
        }
        other => panic!("expected a delta, got {other:?}"),
    }

    // (c) Covered stale cursor -> delta replay from Last-Event-ID: rev0.
    let mut replay = client.events(Some(rev0));
    let event = tokio::time::timeout(TIME_BUDGET, replay.next())
        .await
        .expect("replay frame")
        .expect("no error")
        .expect("event");
    match &event {
        SseEvent::Delta(delta) => assert!(delta.rev > rev0, "replay resumes from the cursor"),
        other => panic!("expected a delta replay, got {other:?}"),
    }

    // (d) No cursor -> full snapshot frame.
    let mut fresh = client.events(None);
    let event = tokio::time::timeout(TIME_BUDGET, fresh.next())
        .await
        .expect("snapshot frame")
        .expect("no error")
        .expect("event");
    match &event {
        SseEvent::Snapshot(snap) => {
            assert_eq!(snap.schema_version, SCHEMA_VERSION);
            let a = snap
                .agents
                .get(AGENT_ID)
                .expect("agent in SSE snapshot frame");
            assert_eq!(
                a.workspace.head_sha.as_deref(),
                Some(daemon.repo_head_sha.as_str()),
                "head fields survive the SSE snapshot frame (G21)"
            );
            assert_eq!(
                a.workspace.head_subject.as_deref(),
                Some("conformance initial commit"),
                "full pinned subject survives the SSE frame (F4)"
            );
        }
        other => panic!("expected a snapshot frame, got {other:?}"),
    }

    // (e) Monotonic rev: after pushes, the next snapshot rev exceeds rev0.
    let snap_after = client.snapshot().await.expect("snapshot");
    assert!(snap_after.rev > rev0, "rev must be monotonic");
    println!("R2 pass: rev0={rev0} -> rev={}", snap_after.rev);
}

/// R3 — Signed drive executes: grant `prompt`, sign an envelope over the
/// canonical bytes, POST /drive → 200 ok:true, response rev ≥ request rev.
#[tokio::test]
#[ignore = "requires a live corrald; run the suite with --ignored"]
async fn r3_signed_drive_executes() {
    let daemon = spawn_live_daemon().await;
    let client = client_of(&daemon).await;
    let _ = wait_for_agent(&client, AGENT_ID, TIME_BUDGET).await;

    let keypair = DeviceKeypair::generate();
    let dev = client
        .register(&daemon.registration_token, &keypair.public_key_b64())
        .await
        .expect("register");
    grant(
        &daemon,
        &client,
        &dev.key_id,
        &[corrald_client::Capability::Prompt],
    )
    .await;

    let snap = client.snapshot().await.expect("snapshot");
    let env = envelope(
        "r3-1",
        corrald_client::Capability::Prompt,
        AGENT_ID,
        DrivePayload::Prompt {
            text: "run the test suite".into(),
        },
        Some(snap.rev),
    );
    let drive = DriveClient::new(client.clone(), keypair);
    let response = drive.drive(&env, None).await.expect("drive");
    assert!(response.ok, "executed: {response:?}");
    assert_eq!(response.request_id, "r3-1");
    assert!(response.error.is_none());
    assert!(response.rev >= snap.rev, "response rev >= request rev");

    // Exactly one dispatch reached the (fake) herdr.
    wait_for_dispatch_count(&daemon.herdr, |n| n >= 1, TIME_BUDGET).await;
    assert_eq!(daemon.herdr.count_prompts_with("run the test suite"), 1);
    println!("R3 pass: rev={} (request was {})", response.rev, snap.rev);
}

/// R4 — Tampered refused: same envelope, payload mutated after signing →
/// 401 bad_signature; zero dispatch.
#[tokio::test]
#[ignore = "requires a live corrald; run the suite with --ignored"]
async fn r4_tampered_envelope_refused() {
    let daemon = spawn_live_daemon().await;
    let client = client_of(&daemon).await;
    let _ = wait_for_agent(&client, AGENT_ID, TIME_BUDGET).await;

    let keypair = DeviceKeypair::generate();
    let dev = client
        .register(&daemon.registration_token, &keypair.public_key_b64())
        .await
        .expect("register");
    grant(
        &daemon,
        &client,
        &dev.key_id,
        &[corrald_client::Capability::Prompt],
    )
    .await;

    // Sign the original, then mutate the payload AFTER signing.
    let mut env = envelope(
        "r4-1",
        corrald_client::Capability::Prompt,
        AGENT_ID,
        DrivePayload::Prompt {
            text: "continue".into(),
        },
        None,
    );
    let signature = keypair.sign_envelope(&env);
    env.payload = DrivePayload::Prompt {
        text: "continue!".into(),
    }
    .to_json();
    let signed = SignedDrive {
        key_id: dev.key_id.clone(),
        signature,
        envelope: env,
    };

    let err = client
        .drive(&signed, None)
        .await
        .expect_err("tampered must be refused");
    match &err {
        ApiError::Drive(refusal) => {
            assert_eq!(refusal.status, reqwest::StatusCode::UNAUTHORIZED);
            assert_eq!(refusal.kind, Some(DriveErrorKind::BadSignature));
        }
        other => panic!("expected 401 bad_signature, got {other:?}"),
    }

    // Zero dispatch.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        daemon.herdr.commands().len(),
        0,
        "no dispatch for a bad signature"
    );
    println!("R4 pass: 401 bad_signature, zero dispatch");
}

/// R5 — Read-only denied: fresh device, no grants → 403 not_granted; zero
/// audit growth.
#[tokio::test]
#[ignore = "requires a live corrald; run the suite with --ignored"]
async fn r5_read_only_device_denied() {
    let daemon = spawn_live_daemon().await;
    let client = client_of(&daemon).await;
    let _ = wait_for_agent(&client, AGENT_ID, TIME_BUDGET).await;

    let before = audit_len(&client, &daemon.admin_token).await;
    let keypair = DeviceKeypair::generate();
    let dev = client
        .register(&daemon.registration_token, &keypair.public_key_b64())
        .await
        .expect("register");
    assert!(dev.grants.is_empty());

    let env = envelope(
        "r5-1",
        corrald_client::Capability::Prompt,
        AGENT_ID,
        DrivePayload::Prompt {
            text: "do the thing".into(),
        },
        None,
    );
    let drive = DriveClient::new(client.clone(), keypair);
    let err = drive
        .drive(&env, None)
        .await
        .expect_err("no grants -> refused");
    match &err {
        ApiError::Drive(refusal) => {
            assert_eq!(refusal.status, reqwest::StatusCode::FORBIDDEN);
            assert_eq!(refusal.kind, Some(DriveErrorKind::NotGranted));
        }
        other => panic!("expected 403 not_granted, got {other:?}"),
    }
    let after = audit_len(&client, &daemon.admin_token).await;
    assert_eq!(after, before, "refused auth is never audited");
    assert_eq!(daemon.herdr.commands().len(), 0);
    println!("R5 pass: 403 not_granted, audit {before} -> {after}");
}

/// R6 — Replay idempotent: same request_id twice → byte-identical
/// responses, exactly one dispatch.
#[tokio::test]
#[ignore = "requires a live corrald; run the suite with --ignored"]
async fn r6_replay_is_idempotent() {
    let daemon = spawn_live_daemon().await;
    let client = client_of(&daemon).await;
    let _ = wait_for_agent(&client, AGENT_ID, TIME_BUDGET).await;

    let keypair = DeviceKeypair::generate();
    let dev = client
        .register(&daemon.registration_token, &keypair.public_key_b64())
        .await
        .expect("register");
    grant(
        &daemon,
        &client,
        &dev.key_id,
        &[corrald_client::Capability::Prompt],
    )
    .await;

    let env = envelope(
        "r6-1",
        corrald_client::Capability::Prompt,
        AGENT_ID,
        DrivePayload::Prompt {
            text: "replay me".into(),
        },
        None,
    );
    let signed = SignedDrive {
        key_id: dev.key_id.clone(),
        signature: keypair.sign_envelope(&env),
        envelope: env,
    };

    // Wire-level: two POSTs with the same signed envelope.
    let (status1, body1) = raw_drive(&daemon.base, &signed, None).await;
    let (status2, body2) = raw_drive(&daemon.base, &signed, None).await;
    assert_eq!(status1, reqwest::StatusCode::OK);
    assert_eq!(status2, reqwest::StatusCode::OK);
    assert_eq!(
        body1, body2,
        "replay must return the first response byte-identical"
    );
    let first: serde_json::Value = serde_json::from_slice(&body1).unwrap();
    assert_eq!(first["ok"], true);
    assert_eq!(first["request_id"], "r6-1");

    // Client-side: DriveClient dedupes from its replay table, no third send.
    let drive = DriveClient::new(client.clone(), keypair);
    let out1 = drive
        .drive(&signed.envelope.clone(), None)
        .await
        .expect("drive");
    let out2 = drive
        .drive(&signed.envelope.clone(), None)
        .await
        .expect("drive");
    assert_eq!(out1, out2);
    assert_eq!(out1.request_id, "r6-1");

    // Exactly one dispatch across all of the above.
    wait_for_dispatch_count(&daemon.herdr, |n| n >= 1, TIME_BUDGET).await;
    assert_eq!(
        daemon.herdr.count_prompts_with("replay me"),
        1,
        "exactly one dispatch"
    );
    println!("R6 pass: byte-identical replay, 1 dispatch");
}

/// R7 — Stale hash refused: approve with the current approval_id but a
/// WRONG prompt_hash → 409 hash_mismatch, zero dispatch, zero audit.
#[tokio::test]
#[ignore = "requires a live corrald; run the suite with --ignored"]
async fn r7_stale_hash_refused() {
    let daemon = spawn_live_daemon().await;
    let client = client_of(&daemon).await;
    let _ = wait_for_agent(&client, AGENT_ID, TIME_BUDGET).await;

    let keypair = DeviceKeypair::generate();
    let dev = client
        .register(&daemon.registration_token, &keypair.public_key_b64())
        .await
        .expect("register");
    grant(
        &daemon,
        &client,
        &dev.key_id,
        &[corrald_client::Capability::Approve],
    )
    .await;

    daemon
        .herdr
        .wait_for_approval("Approve this change? [y/n]", "[y/n]\n1. yes\n2. no\n")
        .await;
    let (_snap, waiting_on) = wait_for_waiting_on(&client, AGENT_ID, TIME_BUDGET).await;
    let approval_id = approval_id_for(AGENT_ID, &waiting_on.prompt_hash);

    // The client's own hash of the snapshot prompt string must equal the
    // daemon's stored hash — the byte-for-byte untrimmed contract.
    assert_eq!(
        prompt_hash_of(&waiting_on.prompt),
        waiting_on.prompt_hash,
        "client must hash the EXACT snapshot prompt string"
    );

    let before = audit_len(&client, &daemon.admin_token).await;
    let wrong_hash = prompt_hash_of("some other question?");
    assert_ne!(wrong_hash, waiting_on.prompt_hash);
    let env = envelope(
        "r7-1",
        corrald_client::Capability::Approve,
        AGENT_ID,
        DrivePayload::Approve {
            approval_id: approval_id.clone(),
            prompt_hash: wrong_hash,
            choice: "y".into(),
        },
        None,
    );
    let drive = DriveClient::new(client.clone(), keypair);
    let err = drive
        .drive(&env, None)
        .await
        .expect_err("wrong hash must be refused");
    match &err {
        ApiError::Drive(refusal) => {
            assert_eq!(refusal.status, reqwest::StatusCode::CONFLICT);
            assert_eq!(refusal.kind, Some(DriveErrorKind::HashMismatch));
        }
        other => panic!("expected 409 hash_mismatch, got {other:?}"),
    }

    let after = audit_len(&client, &daemon.admin_token).await;
    assert_eq!(after, before, "hash mismatch is never audited");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(daemon.herdr.count_approves_with("y"), 0, "zero dispatch");
    println!("R7 pass: 409 hash_mismatch, zero dispatch, audit {before} -> {after}");
}

/// R8 — Matching approve executes: correct approval_id + prompt_hash +
/// choice ∈ choices → 200, dispatch exactly once, audit +1.
#[tokio::test]
#[ignore = "requires a live corrald; run the suite with --ignored"]
async fn r8_matching_approve_executes() {
    let daemon = spawn_live_daemon().await;
    let client = client_of(&daemon).await;
    let _ = wait_for_agent(&client, AGENT_ID, TIME_BUDGET).await;

    let keypair = DeviceKeypair::generate();
    let dev = client
        .register(&daemon.registration_token, &keypair.public_key_b64())
        .await
        .expect("register");
    grant(
        &daemon,
        &client,
        &dev.key_id,
        &[corrald_client::Capability::Approve],
    )
    .await;

    daemon
        .herdr
        .wait_for_approval("Approve this change? [y/n]", "[y/n]\n1. yes\n2. no\n")
        .await;
    let (_snap, waiting_on) = wait_for_waiting_on(&client, AGENT_ID, TIME_BUDGET).await;
    assert_eq!(waiting_on.choices, vec!["y".to_string(), "n".to_string()]);
    let approval_id = approval_id_for(AGENT_ID, &waiting_on.prompt_hash);
    assert_eq!(
        waiting_on.approval_id, approval_id,
        "daemon attaches the same claim id"
    );

    let before = audit_len(&client, &daemon.admin_token).await;
    let env = envelope(
        "r8-1",
        corrald_client::Capability::Approve,
        AGENT_ID,
        DrivePayload::Approve {
            approval_id,
            prompt_hash: waiting_on.prompt_hash.clone(),
            choice: "y".into(),
        },
        None,
    );
    let drive = DriveClient::new(client.clone(), keypair);
    let response = drive.drive(&env, None).await.expect("approve executes");
    assert!(response.ok, "approve must execute: {response:?}");
    assert_eq!(response.request_id, "r8-1");

    // Exactly one dispatch — the validated choice, to the pane.
    wait_for_dispatch_count(&daemon.herdr, |n| n >= 1, TIME_BUDGET).await;
    assert_eq!(
        daemon.herdr.count_approves_with("y"),
        1,
        "exactly one approve dispatch"
    );

    let after = audit_len(&client, &daemon.admin_token).await;
    assert_eq!(after, before + 1, "the executed approve is audited");
    println!("R8 pass: 200 ok, 1 dispatch, audit {before} -> {after}");
}

/// R9 — Step-up: `rm -rf ...` payload without token → 403 step_up_required
/// (audit 0); mint via /step-up, retry with header → 200, audit +1; token
/// replay → 401 step_up_failed.
#[tokio::test]
#[ignore = "requires a live corrald; run the suite with --ignored"]
async fn r9_step_up_flow() {
    let daemon = spawn_live_daemon().await;
    let client = client_of(&daemon).await;
    let _ = wait_for_agent(&client, AGENT_ID, TIME_BUDGET).await;

    let keypair = DeviceKeypair::generate();
    let dev = client
        .register(&daemon.registration_token, &keypair.public_key_b64())
        .await
        .expect("register");
    grant(
        &daemon,
        &client,
        &dev.key_id,
        &[corrald_client::Capability::Prompt],
    )
    .await;

    let env = envelope(
        "r9-1",
        corrald_client::Capability::Prompt,
        AGENT_ID,
        DrivePayload::Prompt {
            text: "rm -rf /tmp/scratch".into(),
        },
        None,
    );
    let signed = SignedDrive {
        key_id: dev.key_id.clone(),
        signature: keypair.sign_envelope(&env),
        envelope: env,
    };

    let before = audit_len(&client, &daemon.admin_token).await;

    // (a) No token -> 403 step_up_required, not audited, no dispatch.
    let drive = DriveClient::new(client.clone(), keypair.clone());
    let err = drive
        .drive(&signed.envelope.clone(), None)
        .await
        .expect_err("must require step-up");
    match &err {
        ApiError::Drive(refusal) => {
            assert_eq!(refusal.status, reqwest::StatusCode::FORBIDDEN);
            assert_eq!(refusal.kind, Some(DriveErrorKind::StepUpRequired));
        }
        other => panic!("expected 403 step_up_required, got {other:?}"),
    }
    assert_eq!(
        audit_len(&client, &daemon.admin_token).await,
        before,
        "step-up refusal not audited"
    );

    // (b) Mint via /step-up (signed proof of possession) -> retry with header -> 200.
    let request = StepUpRequest::new(&dev.key_id, "r9-nonce");
    let signature = keypair.sign_bytes(&canonical_step_up_bytes(&request));
    let token = client
        .step_up(&request, &signature)
        .await
        .expect("mint step-up token");
    assert_eq!(token.key_id, dev.key_id);
    assert_eq!(token.ttl_secs, 300, "5-minute TTL");
    assert!(!token.token.is_empty());

    let response = drive
        .drive(&signed.envelope.clone(), Some(&token.token))
        .await
        .expect("drive with step-up token");
    assert!(response.ok, "step-up drive executes: {response:?}");
    assert_eq!(
        audit_len(&client, &daemon.admin_token).await,
        before + 1,
        "execution audited"
    );
    wait_for_dispatch_count(&daemon.herdr, |n| n >= 1, TIME_BUDGET).await;
    assert_eq!(daemon.herdr.count_prompts_with("rm -rf /tmp/scratch"), 1);

    // (c) Token replay -> 401 step_up_failed (single-use), not audited.
    let (status, body) = raw_drive(&daemon.base, &signed, Some(&token.token)).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["kind"], "step_up_failed");
    assert_eq!(
        audit_len(&client, &daemon.admin_token).await,
        before + 1,
        "replay not audited"
    );
    println!(
        "R9 pass: 403 -> mint -> 200 -> 401, audit {before} -> {}",
        before + 1
    );
}

/// R10 — Audit grows only on writes: GETs, auth failures, and step-up
/// failures never grow the log; each executed / refused-at-dispatch drive
/// does.
#[tokio::test]
#[ignore = "requires a live corrald; run the suite with --ignored"]
async fn r10_audit_grows_only_on_writes() {
    let daemon = spawn_live_daemon().await;
    let client = client_of(&daemon).await;
    let _ = wait_for_agent(&client, AGENT_ID, TIME_BUDGET).await;

    let keypair = DeviceKeypair::generate();
    let dev = client
        .register(&daemon.registration_token, &keypair.public_key_b64())
        .await
        .expect("register");
    grant(
        &daemon,
        &client,
        &dev.key_id,
        &[corrald_client::Capability::Prompt],
    )
    .await;

    let baseline = audit_len(&client, &daemon.admin_token).await;

    // (a) Reads never grow the log.
    client.snapshot().await.expect("snapshot");
    client.host_key().await.expect("host-key");
    let _ = client.events(None).next().await;
    client.audit(&daemon.admin_token).await.expect("audit");
    assert_eq!(
        audit_len(&client, &daemon.admin_token).await,
        baseline,
        "reads are never audited"
    );

    // (b) Auth failures never grow the log.
    // Bad signature (tampered after signing).
    let mut env = envelope(
        "r10-tamper",
        corrald_client::Capability::Prompt,
        AGENT_ID,
        DrivePayload::Prompt {
            text: "tampered".into(),
        },
        None,
    );
    let signature = keypair.sign_envelope(&env);
    env.payload = DrivePayload::Prompt {
        text: "tampered!".into(),
    }
    .to_json();
    let _ = client
        .drive(
            &SignedDrive {
                key_id: dev.key_id.clone(),
                signature,
                envelope: env,
            },
            None,
        )
        .await;
    // Read-only device (never granted).
    let read_only = DeviceKeypair::generate();
    let _ = client
        .register(&daemon.registration_token, &read_only.public_key_b64())
        .await
        .expect("register read-only device");
    let env_ro = envelope(
        "r10-ro",
        corrald_client::Capability::Prompt,
        AGENT_ID,
        DrivePayload::Prompt {
            text: "nope".into(),
        },
        None,
    );
    let _ = DriveClient::new(client.clone(), read_only)
        .drive(&env_ro, None)
        .await;
    // Step-up required (destructive, no token).
    let env_destructive = envelope(
        "r10-dest",
        corrald_client::Capability::Prompt,
        AGENT_ID,
        DrivePayload::Prompt {
            text: "rm -rf /tmp/x".into(),
        },
        None,
    );
    let signed_destructive = SignedDrive {
        key_id: dev.key_id.clone(),
        signature: keypair.sign_envelope(&env_destructive),
        envelope: env_destructive,
    };
    let _ = client.drive(&signed_destructive, None).await;
    assert_eq!(
        audit_len(&client, &daemon.admin_token).await,
        baseline,
        "auth failures are never audited"
    );

    // (c) Refused-at-dispatch (unknown agent) rides HTTP 200 ok:false and
    // IS audited.
    let env_unknown = envelope(
        "r10-unknown",
        corrald_client::Capability::Prompt,
        "herdr:no-such-agent",
        DrivePayload::Prompt {
            text: "who?".into(),
        },
        None,
    );
    let drive = DriveClient::new(client.clone(), keypair.clone());
    let response = drive
        .drive(&env_unknown, None)
        .await
        .expect("dispatch-level refusal rides 200");
    assert!(!response.ok, "unknown agent refused at dispatch");
    assert_eq!(
        audit_len(&client, &daemon.admin_token).await,
        baseline + 1,
        "dispatch refusal audited"
    );

    // (d) Executed write -> audited.
    let env_ok = envelope(
        "r10-ok",
        corrald_client::Capability::Prompt,
        AGENT_ID,
        DrivePayload::Prompt {
            text: "audit me".into(),
        },
        None,
    );
    let response = drive.drive(&env_ok, None).await.expect("executed drive");
    assert!(response.ok);
    wait_for_dispatch_count(&daemon.herdr, |n| n >= 1, TIME_BUDGET).await;
    let audit = client.audit(&daemon.admin_token).await.expect("audit");
    assert!(audit.valid, "hash chain must verify");
    assert_eq!(audit.len(), baseline + 2);
    let entries = audit.entries;
    assert!(
        entries[entries.len() - 2]["hash"].is_string(),
        "chained entries"
    );
    assert_eq!(
        entries[entries.len() - 1]["prev"],
        entries[entries.len() - 2]["hash"]
    );
    println!("R10 pass: baseline={baseline}, reads/auth 0 growth, writes +2, chain valid");
}

/// End-to-end chain on ONE daemon: register -> read -> sign -> drive ->
/// step-up -> approve in a single process (the brief's acceptance chain).
/// The per-scenario tests above spawn their own daemons for isolation.
#[tokio::test]
#[ignore = "requires a live corrald; run the suite with --ignored"]
async fn register_read_sign_drive_step_up_approve() {
    let daemon = spawn_live_daemon().await;
    let client = client_of(&daemon).await;
    let snap = wait_for_agent(&client, AGENT_ID, TIME_BUDGET).await;
    assert_eq!(snap.schema_version, SCHEMA_VERSION);

    // register
    let keypair = DeviceKeypair::generate();
    let dev = client
        .register(&daemon.registration_token, &keypair.public_key_b64())
        .await
        .expect("register");
    assert!(dev.grants.is_empty());
    grant(
        &daemon,
        &client,
        &dev.key_id,
        &[
            corrald_client::Capability::Prompt,
            corrald_client::Capability::Approve,
        ],
    )
    .await;

    // sign + drive
    let drive = DriveClient::new(client.clone(), keypair);
    let env = envelope(
        "e2e-1",
        corrald_client::Capability::Prompt,
        AGENT_ID,
        DrivePayload::Prompt { text: "go".into() },
        Some(snap.rev),
    );
    let response = drive.drive(&env, None).await.expect("signed drive");
    assert!(response.ok);
    assert!(response.rev >= snap.rev);
    wait_for_dispatch_count(&daemon.herdr, |n| n >= 1, TIME_BUDGET).await;

    // step-up + approve: block the agent on a menu, approve it.
    daemon
        .herdr
        .wait_for_approval("Approve this change? [y/n]", "[y/n]\n1. yes\n2. no\n")
        .await;
    let (_snap, waiting_on) = wait_for_waiting_on(&client, AGENT_ID, TIME_BUDGET).await;
    let approve = envelope(
        "e2e-2",
        corrald_client::Capability::Approve,
        AGENT_ID,
        DrivePayload::Approve {
            approval_id: approval_id_for(AGENT_ID, &waiting_on.prompt_hash),
            prompt_hash: waiting_on.prompt_hash,
            choice: "y".into(),
        },
        None,
    );
    let response = drive.drive(&approve, None).await.expect("approve");
    assert!(response.ok);
    wait_for_dispatch_count(&daemon.herdr, |n| n >= 2, TIME_BUDGET).await;
    assert_eq!(daemon.herdr.count_approves_with("y"), 1);

    let audit = client.audit(&daemon.admin_token).await.expect("audit");
    assert!(audit.valid);
    println!(
        "e2e pass: register -> read -> sign -> drive -> approve, audit={}",
        audit.len()
    );
}

// ---------------------------------------------------------------------------
// SSE resume edge cases
// ---------------------------------------------------------------------------

/// SSE resume edge cases against the real daemon:
/// no cursor → snapshot; current cursor → live; stale-but-covered cursor →
/// delta replay; FUTURE cursor (dead epoch, e.g. daemon restart) →
/// snapshot; mid-stream lag → resnapshot is the daemon's side of the
/// contract (a `snapshot` event resets the epoch — asserted here by the
/// client accepting it).
#[tokio::test]
#[ignore = "requires a live corrald; run the suite with --ignored"]
async fn sse_resume_edge_cases() {
    let daemon = spawn_live_daemon().await;
    let client = client_of(&daemon).await;
    // G21 harness: the config dir is a real repo, so the git plane merges
    // facts at boot — wait until they are flushed so the baseline rev
    // captures them (otherwise the boot merge could land as a live delta
    // mid-test and reorder the pushes below).
    let _ = wait_for_head(&client, AGENT_ID, TIME_BUDGET).await;

    // (1) No cursor -> full snapshot.
    let mut stream = client.events(None);
    let first = tokio::time::timeout(TIME_BUDGET, stream.next())
        .await
        .expect("frame")
        .expect("no error")
        .expect("event");
    assert!(
        first.is_snapshot(),
        "no cursor must yield a snapshot: {first:?}"
    );
    let snap = first.as_snapshot().expect("snapshot");
    let current = snap.rev;
    drop(stream);

    // (2) Current cursor -> straight to live (no frame until a change).
    let mut live = client.events(Some(current));
    assert!(
        tokio::time::timeout(Duration::from_millis(700), live.next())
            .await
            .is_err(),
        "current cursor must emit nothing until the next change"
    );

    // (3) Future cursor (dead epoch) -> snapshot, never silence.
    let mut future = client.events(Some(current + 10_000));
    let event = tokio::time::timeout(TIME_BUDGET, future.next())
        .await
        .expect("frame")
        .expect("no error")
        .expect("event");
    assert!(
        event.is_snapshot(),
        "a future cursor (dead epoch) must resnapshot, not go live: {event:?}"
    );

    // (4) Live delta + apply_delta keeps a local model coherent.
    daemon.herdr.set_status(AGENT_PANE, "blocked").await;
    let event = tokio::time::timeout(TIME_BUDGET, live.next())
        .await
        .expect("live delta")
        .expect("no error")
        .expect("event");
    let SseEvent::Delta(delta) = event else {
        panic!("expected delta after a push");
    };
    let mut agents = snap.agents.clone();
    apply_delta(&mut agents, &delta);
    assert_eq!(
        agents[AGENT_ID].state,
        AgentState::Blocked,
        "apply_delta must fold the delta into the snapshot"
    );
    assert!(delta.rev > current, "monotonic live delta");

    // (5) Covered stale cursor: Last-Event-ID one behind current replays
    // the newest delta.
    let mut replay = client.events(Some(current));
    let event = tokio::time::timeout(TIME_BUDGET, replay.next())
        .await
        .expect("frame")
        .expect("no error")
        .expect("event");
    match &event {
        SseEvent::Delta(replayed) => assert_eq!(replayed.rev, delta.rev, "replay covers the gap"),
        other => panic!("expected delta replay from a covered cursor, got {other:?}"),
    }
    println!("sse edge cases pass: snapshot / live / future-resnapshot / replay");
}

// ---------------------------------------------------------------------------
// R11: live gh facts — branch-fallback PR binding (#22) + authoritative
// issues (#23) against the REAL GitHub API
// ---------------------------------------------------------------------------

/// One open PR on a tracked repo, as the live API sees it — the exact
/// surfaces the daemon's gh plane polls.
struct LivePrCandidate {
    tracked: &'static corrald::adapters::gh_plane::TrackedRepo,
    pr_number: u64,
    head_branch: String,
    /// PR head sha (`headRefOid`) — the "pushed" state the clone starts at.
    head_sha: String,
    /// closingIssuesReferences: (number, title, live state).
    closing: Vec<(u64, String, String)>,
    /// The repo's recent issues top-10 (number, state) — the SAME-poll
    /// fetch the daemon's `issues` leg performs, used to predict the
    /// daemon's state enrichment.
    recent_issues: Vec<(u64, String)>,
}

fn gh_auth_available() -> bool {
    if std::env::var("GITHUB_TOKEN").is_ok_and(|t| !t.trim().is_empty()) {
        return true;
    }
    std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Find an open PR on a tracked repo whose head branch is fetchable over
/// https (same owner/repo — fork PRs are skipped) via ONE aliased GraphQL
/// query shaped like the gh plane's. Prefers a PR WITH closing refs so the
/// populated-issues leg can run; falls back to any open PR (which asserts
/// the authoritative-empty leg). `None` -> "no suitable live repo exists".
async fn find_live_pr_candidate() -> Option<LivePrCandidate> {
    let mut aliases = String::new();
    for (i, tracked) in TRACKED_REPOS.iter().enumerate() {
        // Alias prefix `q` is required: GraphQL aliases cannot start with a
        // digit (the gh plane uses the same q0..q7 convention).
        aliases.push_str(&format!(
            r#"q{i}: repository(owner: "{}", name: "{}") {{
  pullRequests(first: 20, states: OPEN, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{
    nodes {{
      number
      headRefName
      headRefOid
      closingIssuesReferences(first: 10) {{ nodes {{ number title state }} }}
    }}
  }}
  issues(first: 10, orderBy: {{field: UPDATED_AT, direction: DESC}}, states: [OPEN, CLOSED]) {{
    nodes {{ number state }}
  }}
}}"#,
            tracked.owner, tracked.repo
        ));
    }
    let query = format!("query {{ {aliases} }}");
    let output = tokio::process::Command::new("gh")
        .args(["api", "graphql", "-f"])
        .arg(format!("query={query}"))
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let data = value.get("data")?;
    let mut fallback: Option<LivePrCandidate> = None;
    for (i, tracked) in TRACKED_REPOS.iter().enumerate() {
        let alias = data.get(format!("q{i}"))?;
        let recent_issues: Vec<(u64, String)> = alias["issues"]["nodes"]
            .as_array()
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|n| Some((n["number"].as_u64()?, n["state"].as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let nodes = alias["pullRequests"]["nodes"].as_array()?;
        for pr in nodes {
            let (Some(number), Some(head_branch), Some(head_sha)) = (
                pr["number"].as_u64(),
                pr["headRefName"].as_str().map(str::to_string),
                pr["headRefOid"].as_str().map(str::to_string),
            ) else {
                continue;
            };
            if head_branch.is_empty() || head_sha.is_empty() {
                continue;
            }
            let closing: Vec<(u64, String, String)> = pr["closingIssuesReferences"]["nodes"]
                .as_array()
                .map(|nodes| {
                    nodes
                        .iter()
                        .filter_map(|n| {
                            Some((
                                n["number"].as_u64()?,
                                n["title"].as_str()?.to_string(),
                                n["state"].as_str()?.to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let candidate = LivePrCandidate {
                tracked,
                pr_number: number,
                head_branch,
                head_sha,
                closing,
                recent_issues: recent_issues.clone(),
            };
            if !candidate.closing.is_empty() {
                return Some(candidate); // prefer the populated-issues leg
            }
            if fallback.is_none() {
                fallback = Some(candidate);
            }
        }
    }
    fallback
}

async fn git_ok(clone: &std::path::Path, args: &[&str]) -> std::process::Output {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C").arg(clone);
    cmd.args(args);
    cmd.output().await.expect("git spawn")
}

/// Poll /snapshot until the agent binds `pr_number` via `source`
/// (`"head_sha"` | `"branch"` | `None` for unbound).
async fn wait_for_pr_binding(
    client: &CorralClient,
    agent_id: &str,
    pr_number: u64,
    source: Option<&str>,
    timeout: Duration,
) -> corrald_client::Snapshot {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let snap = client.snapshot().await.expect("snapshot");
        if let Some(agent) = snap.agents.get(agent_id) {
            let bound = agent.workspace.pr_number == Some(pr_number)
                && agent.workspace.pr_match_source.as_deref() == source;
            if bound {
                return snap;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "agent {agent_id} never bound PR {pr_number} via {source:?} — last snapshot: {:?}",
            snap.agents.get(agent_id).map(|a| a.workspace.clone())
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// R11 — against the REAL GitHub API: an agent whose worktree holds a
/// committed-but-unpushed head on an open PR's branch binds its PR by the
/// (repo, branch) fallback (issue #22), binds by head-SHA first in the
/// pushed state (no regression), and its `workspace.issues` mirror ONLY the
/// bound PR's authoritative `closingIssuesReferences` (issue #23).
///
/// Fully skippable — "if a suitable live repo exists": needs a gh token AND
/// an open PR on a tracked repo whose head branch is fetchable over https.
/// The scenario is staged in the daemon's scratch root (a local clone,
/// never pushed; the tempdir owns it), so no remote state is touched.
#[tokio::test]
#[ignore = "live: requires gh auth + a suitable open PR on a tracked repo; run with --ignored"]
async fn r11_gh_pr_binds_by_branch_fallback_and_populates_issues() {
    if !gh_auth_available() {
        println!("R11 skip: no GITHUB_TOKEN and no `gh auth token` — gh plane would stay down");
        return;
    }
    let Some(candidate) = find_live_pr_candidate().await else {
        println!(
            "R11 skip: no suitable live repo (no open PR with a fetchable head branch on a tracked repo)"
        );
        return;
    };
    let tracked = candidate.tracked;
    println!(
        "R11 candidate: {}#{} branch={} head={:.8} closing_refs={}",
        tracked.name,
        candidate.pr_number,
        candidate.head_branch,
        candidate.head_sha,
        candidate.closing.len()
    );

    // (1) Stage the worktree: a local clone of the PR's head branch inside
    // a scratch dir that becomes the daemon's repo/worktrees root. The
    // clone sits at the PR head (pushed state) before the daemon boots.
    let scratch = tempfile::tempdir().expect("scratch dir");
    let clone = scratch
        .path()
        .join(tracked.name)
        .join(format!("corral-r11-{}", candidate.pr_number));
    let url = format!("https://github.com/{}/{}.git", tracked.owner, tracked.repo);
    let cloned = tokio::process::Command::new("git")
        .args([
            "clone",
            "-q",
            "--single-branch",
            "--branch",
            &candidate.head_branch,
        ])
        .arg(&url)
        .arg(&clone)
        .output()
        .await
        .ok();
    match cloned {
        Some(output) if output.status.success() => {}
        _ => {
            println!(
                "R11 skip: cannot clone {url} branch {} — not a suitable live repo",
                candidate.head_branch
            );
            return;
        }
    }
    let actual_head = git_ok(&clone, &["rev-parse", "HEAD"]).await;
    let actual_head = String::from_utf8_lossy(&actual_head.stdout)
        .trim()
        .to_string();
    if actual_head != candidate.head_sha {
        println!(
            "R11 skip: PR head moved mid-test ({actual_head:.8} != {}); branch churn — not suitable now",
            &candidate.head_sha[..candidate.head_sha.len().min(8)]
        );
        return;
    }

    // (2) The fake herdr agent's cwd IS the clone; the daemon's git+gh
    // planes fold real facts onto it. The first SSE subscriber makes the gh
    // plane poll immediately (SWR).
    let cwd = std::fs::canonicalize(&clone)
        .unwrap_or_else(|_| clone.clone())
        .to_string_lossy()
        .into_owned();
    let agent = FakeAgent {
        cwd,
        title: format!("r11: PR #{}", candidate.pr_number),
        ..Default::default()
    };
    let daemon = spawn_live_daemon_at(scratch.path(), vec![agent]).await;
    let client = client_of(&daemon).await;
    // The first-ever SSE subscriber makes the gh plane poll immediately
    // (SWR). The stream connects lazily on the first poll, so it must be
    // DRAINED in a background task — a held-but-never-read stream would
    // never register a subscriber and the gh plane would stay down.
    let mut live = client.events(None);
    let _subscriber = tokio::spawn(async move { while live.next().await.is_some() {} });

    // (3) Pushed state: the clone HEAD equals the PR's head sha, so the
    // PRIMARY head-SHA match binds — the fallback never fires (acceptance
    // #22-3: no regression).
    let snap = wait_for_pr_binding(
        &client,
        AGENT_ID,
        candidate.pr_number,
        Some("head_sha"),
        LIVE_GH_BUDGET,
    )
    .await;
    let agent = &snap.agents[AGENT_ID];
    assert_eq!(
        agent.workspace.repo.as_deref(),
        Some(tracked.name),
        "repo derived from the worktree path"
    );
    assert_eq!(
        agent.workspace.branch.as_deref(),
        Some(candidate.head_branch.as_str())
    );
    println!(
        "R11 step 3 pass: pushed head binds PR #{} via head_sha (ci={:?})",
        candidate.pr_number, agent.workspace.ci_status
    );

    // (4) Committed-but-unpushed (issue #22): a LOCAL commit on the PR
    // branch — never pushed. The head-SHA match now misses and the (repo,
    // branch) fallback must bind the SAME PR (acceptance #22-1: no blank
    // badge for committed-but-unpushed work).
    let committed = git_ok(
        &clone,
        &[
            "-c",
            "user.name=corral r11 conformance",
            "-c",
            "user.email=conformance@corral.local",
            "commit",
            "--allow-empty",
            "-m",
            "corral r11: committed-but-unpushed head probe",
        ],
    )
    .await;
    assert!(
        committed.status.success(),
        "scratch commit failed: {}",
        String::from_utf8_lossy(&committed.stderr)
    );
    wait_for_pr_binding(
        &client,
        AGENT_ID,
        candidate.pr_number,
        Some("branch"),
        LIVE_GH_BUDGET,
    )
    .await;
    println!(
        "R11 step 4 pass: committed-but-unpushed head re-binds PR #{} via (repo, branch)",
        candidate.pr_number
    );

    // (5) Push-state round trip: reset the clone to the PR head — the
    // primary head-SHA match must win again once the head matches.
    let reset = git_ok(&clone, &["reset", "--hard", &candidate.head_sha]).await;
    assert!(
        reset.status.success(),
        "reset failed: {}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let snap = wait_for_pr_binding(
        &client,
        AGENT_ID,
        candidate.pr_number,
        Some("head_sha"),
        LIVE_GH_BUDGET,
    )
    .await;
    println!(
        "R11 step 5 pass: reset head re-binds PR #{} via head_sha",
        candidate.pr_number
    );

    // (6) Issues (issue #23): ONLY the bound PR's authoritative closing
    // refs — never the repo's recent issues, never a guess.
    let agent = &snap.agents[AGENT_ID];
    if candidate.closing.is_empty() {
        assert!(
            agent.workspace.issues.is_empty(),
            "PR with no closing refs must yield an empty issues array, got {:?}",
            agent.workspace.issues
        );
        println!(
            "R11 step 6 pass: PR #{} has no closing refs -> issues == [] (authoritative empty)",
            candidate.pr_number
        );
    } else {
        assert_eq!(
            agent.workspace.issues.len(),
            candidate.closing.len(),
            "issues must mirror the PR's closingIssuesReferences exactly"
        );
        for (actual, (number, title, live_state)) in
            agent.workspace.issues.iter().zip(candidate.closing.iter())
        {
            assert_eq!(actual.repo, tracked.name);
            assert_eq!(
                actual.number, *number,
                "linkage comes only from closing refs"
            );
            assert_eq!(actual.title, *title, "titles come only from closing refs");
            // State: the daemon enriches from the SAME poll's repo-level
            // issues fetch (UNKNOWN when the issue is not among the recent
            // top-10); a live-API race between our query and the daemon's
            // poll can flip the enrichment, so the live state is tolerated.
            let expected = candidate
                .recent_issues
                .iter()
                .find(|(n, _)| *n == *number)
                .map(|(_, s)| s.clone())
                .unwrap_or_else(|| "UNKNOWN".to_string());
            assert!(
                actual.state == expected
                    || actual.state == "UNKNOWN"
                    || actual.state == *live_state,
                "issue #{number}: daemon state {:?} not in {{expected {expected:?}, live {live_state:?}, UNKNOWN}}",
                actual.state
            );
        }
        println!(
            "R11 step 6 pass: issues[] mirrors closingIssuesReferences ({} refs)",
            candidate.closing.len()
        );
    }
    println!(
        "R11 pass: real-repo branch fallback + head-SHA primary + authoritative issues on {}",
        tracked.name
    );
}
