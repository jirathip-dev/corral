//! `POST /drive` tests (P3 W1): envelope → command mapping, typed refusals
//! (unknown capability / bad payload / unknown agent / auth), replay
//! idempotency (sequential + concurrent), read_tail bounds, and audit call
//! sites (grows on writes, never on auth failures or replay hits).

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use corrald::adapters::{Adapter, DriveCommand, DriveError};
use corrald::api::drive::ReplayTable;
use corrald::api::{router, AppState};
use corrald::auth::audit::ChainEntry;
use corrald::auth::test_support;
use corrald::auth::AuthPlane;
use corrald::core::store::Store;
use corrald::drive::{AuditOutcome, Capability};
use ed25519_dalek::SigningKey;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Test double plumbing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Ok,
    NotImplemented,
    Transport,
}

/// Records every dispatch; configurable outcome for refusal tests.
#[derive(Debug)]
struct RecordingAdapter {
    dispatches: AtomicUsize,
    commands: Mutex<Vec<(String, DriveCommand)>>,
    known: Mutex<HashSet<String>>,
    mode: Mutex<Mode>,
    /// When Some, drive() notifies `started` and blocks on the receiver
    /// before returning (concurrency test support — never in production).
    hold: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    started: Arc<tokio::sync::Notify>,
}

impl Default for RecordingAdapter {
    fn default() -> Self {
        Self {
            dispatches: AtomicUsize::new(0),
            commands: Mutex::new(Vec::new()),
            known: Mutex::new(HashSet::new()),
            mode: Mutex::new(Mode::Ok),
            hold: Mutex::new(None),
            started: Arc::new(tokio::sync::Notify::new()),
        }
    }
}

impl RecordingAdapter {
    fn knows(&self, agent_id: &str) -> &Self {
        self.known.lock().unwrap().insert(agent_id.to_string());
        self
    }

    fn mode(&self, mode: Mode) -> &Self {
        *self.mode.lock().unwrap() = mode;
        self
    }

    fn dispatch_count(&self) -> usize {
        self.dispatches.load(Ordering::SeqCst)
    }

    fn commands(&self) -> Vec<(String, DriveCommand)> {
        self.commands.lock().unwrap().clone()
    }
}

impl Adapter for RecordingAdapter {
    fn source(&self) -> &'static str {
        "test"
    }

    fn start(self: Arc<Self>, _store: Store) {}

    fn drive(&self, agent_id: &str, command: DriveCommand) -> Result<(), DriveError> {
        self.dispatches.fetch_add(1, Ordering::SeqCst);
        self.commands
            .lock()
            .unwrap()
            .push((agent_id.to_string(), command.clone()));
        let hold = self.hold.lock().unwrap().take();
        if let Some(rx) = hold {
            self.started.notify_waiters();
            let _ = rx.recv();
        }
        match *self.mode.lock().unwrap() {
            Mode::Ok => {
                if self.known.lock().unwrap().contains(agent_id) {
                    Ok(())
                } else {
                    Err(DriveError::UnknownAgent(agent_id.to_string()))
                }
            }
            Mode::NotImplemented => Err(DriveError::NotImplemented("test-command")),
            Mode::Transport => Err(DriveError::Transport("boom".to_string())),
        }
    }

    fn knows_agent(&self, agent_id: &str) -> bool {
        self.known.lock().unwrap().contains(agent_id)
    }
}

/// Every capability the drive tests exercise, granted to the harness device.
const ALL_CAPABILITIES: [Capability; 6] = [
    Capability::Prompt,
    Capability::Interrupt,
    Capability::Approve,
    Capability::ReadTail,
    Capability::Kill,
    Capability::Attach,
];

/// Real W3 auth plane over a temp dir + a registered, fully-granted device.
/// Every `body()` request is genuinely signed by that device — no stubs.
struct Harness {
    store: Store,
    adapter: Arc<RecordingAdapter>,
    auth: Arc<AuthPlane>,
    signing: SigningKey,
    pubkey: [u8; 32],
    key_id: String,
    app: Router,
    _dir: tempfile::TempDir,
}

impl Harness {
    /// Register a SECOND device (distinct keypair); returns the signing key,
    /// its pubkey and key_id.
    fn register_other_device(&self, grants: &[Capability]) -> (SigningKey, [u8; 32], String) {
        let (signing, pubkey) = test_support::keypair();
        let env = test_support::envelope("bootstrap-other", Capability::Prompt, "bootstrap");
        let token = self.auth.registry.registration_token();
        let signed = test_support::signed(&self.auth.registry, &token, &signing, pubkey, &env);
        self.auth
            .registry
            .set_grants(&signed.key_id, grants.to_vec())
            .expect("set grants");
        (signing, pubkey, signed.key_id)
    }

    /// A fully-signed drive request body from the harness device.
    fn body(
        &self,
        request_id: &str,
        capability: Capability,
        target: &str,
        payload: Value,
        rev: Option<u64>,
    ) -> String {
        self.body_from(&self.signing, self.pubkey, request_id, capability, target, payload, rev)
    }

    #[allow(clippy::too_many_arguments)]
    fn body_from(
        &self,
        signing: &SigningKey,
        pubkey: [u8; 32],
        request_id: &str,
        capability: Capability,
        target: &str,
        payload: Value,
        rev: Option<u64>,
    ) -> String {
        let envelope = corrald::drive::DriveEnvelope {
            request_id: request_id.to_string(),
            capability,
            target: target.to_string(),
            payload,
            rev,
        };
        let token = self.auth.registry.registration_token();
        let signed =
            test_support::signed(&self.auth.registry, &token, signing, pubkey, &envelope);
        serde_json::to_string(&signed).expect("signed body serializes")
    }

    /// Audit chain entries (W3's hash-chained log).
    fn audit_entries(&self) -> Vec<ChainEntry> {
        self.auth.audit.chain().0
    }
}

fn harness() -> Harness {
    let store = Store::new();
    let coalescer = store.clone();
    std::mem::drop(tokio::spawn(async move { coalescer.run_coalescer().await }));
    let adapter = Arc::new(RecordingAdapter::default());
    let dir = tempfile::tempdir().expect("tempdir");
    let auth = Arc::new(AuthPlane::load_or_create(dir.path().to_path_buf()).expect("auth plane"));

    // Register the harness device once (idempotent) and grant everything.
    let (signing, pubkey) = test_support::keypair();
    let env = test_support::envelope("bootstrap", Capability::Prompt, "bootstrap");
    let token = auth.registry.registration_token();
    let signed = test_support::signed(&auth.registry, &token, &signing, pubkey, &env);
    let key_id = signed.key_id.clone();
    auth.registry
        .set_grants(&key_id, ALL_CAPABILITIES.to_vec())
        .expect("grants");

    let app = router(AppState {
        store: store.clone(),
        auth: auth.clone(),
        adapter: adapter.clone(),
        replay: Arc::new(ReplayTable::default()),
    });
    Harness {
        store,
        adapter,
        auth,
        signing,
        pubkey,
        key_id,
        app,
        _dir: dir,
    }
}

fn prompt_payload(text: &str) -> Value {
    json!({ "kind": "prompt", "text": text })
}

async fn post(app: &Router, body: String) -> (StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(
            Request::post("/drive")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

// ---------------------------------------------------------------------------
// Envelope -> command mapping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prompt_dispatches_with_typed_command_and_current_rev() {
    let h = harness();
    h.adapter.knows("herdr:abc");

    let (status, value) = post(
        &h.app,
        h.body("req-1", Capability::Prompt, "herdr:abc", prompt_payload("continue"), Some(3)),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true);
    assert_eq!(value["request_id"], "req-1");
    let rev = h.store.snapshot().await.rev;
    assert_eq!(value["rev"].as_u64(), Some(rev), "response carries the store rev");
    assert_eq!(
        h.adapter.commands(),
        vec![(
            "herdr:abc".to_string(),
            DriveCommand::Prompt {
                text: "continue".to_string()
            }
        )]
    );
    assert_eq!(h.adapter.dispatch_count(), 1);
}

#[tokio::test]
async fn command_only_capabilities_need_no_payload() {
    for (capability, command, cap) in [
        ("interrupt", DriveCommand::Interrupt, Capability::Interrupt),
        ("kill", DriveCommand::Kill, Capability::Kill),
        ("attach", DriveCommand::Attach, Capability::Attach),
    ] {
        let h = harness();
        h.adapter.knows("herdr:a");
        let (status, value) = post(
            &h.app,
            h.body("req", cap, "herdr:a", Value::Null, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{capability}");
        assert_eq!(value["ok"], true, "{capability}");
        let commands = h.adapter.commands();
        assert_eq!(commands.len(), 1, "{capability}");
        assert_eq!(commands[0].1, command, "{capability}");
    }
}

#[tokio::test]
async fn command_only_capability_with_payload_is_refused() {
    let h = harness();
    let (status, value) = post(
        &h.app,
        h.body("req", Capability::Interrupt, "herdr:a", json!({ "kind": "interrupt" }), None),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(value["kind"], "payload");
    assert!(value["message"].as_str().unwrap().contains("no payload expected"));
    assert_eq!(h.adapter.dispatch_count(), 0);
}

#[tokio::test]
async fn approve_maps_to_approve_command() {
    let h = harness();
    h.adapter.knows("herdr:a");
    let (status, _) = post(
        &h.app,
        h.body(
            "req",
            Capability::Approve,
            "herdr:a",
            json!({ "kind": "approve", "approval_id": "ap-1", "prompt_hash": "sha256:x", "choice": "y" }),
            None,
        ),
    )
    .await;
    // W2 claim check: the store has no record for the target (and thus no
    // waiting approval) → typed 404, never a dispatch.
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(h.adapter.commands().is_empty());
}

// ---------------------------------------------------------------------------
// Claim-based approvals (W2 wiring): the handler must validate the claim
// against the store BEFORE dispatching, refuse stale hashes with a typed
// error (no replay slot, no dispatch, no audit), and dispatch the validated
// choice exactly once on a matching claim.
// ---------------------------------------------------------------------------

const W2_AGENT: &str = "herdr:ses-live";
const W2_HASH: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

async fn seed_blocked_agent(store: &Store, prompt: &str, choices: Vec<String>) {
    let approval_id = format!("{W2_AGENT}:{W2_HASH}");
    let agent = corrald::core::model::Agent {
        agent_id: W2_AGENT.to_string(),
        source: "herdr".to_string(),
        tool: "opencode".to_string(),
        state: corrald::core::model::AgentState::Blocked,
        reason: Some("waiting_for_input".to_string()),
        seq: 1,
        ts: 1,
        capabilities: Vec::new(),
        waiting_on: Some(corrald::core::model::WaitingOn {
            kind: corrald::core::model::WaitingOnKind::AnswerQuestion,
            prompt: prompt.to_string(),
            prompt_hash: W2_HASH.to_string(),
            approval_id,
            choices,
        }),
        cost: None,
        parent_id: None,
        host: None,
        workspace: Default::default(),
        attachment: None,
        display_name: None,
        title: None,
    };
    let store2 = store.clone();
    store2.apply(corrald::core::model::Change::upsert(agent)).await;
}

#[tokio::test]
async fn approve_with_stale_hash_is_refused_without_dispatch_or_audit() {
    let h = harness();
    h.adapter.knows(W2_AGENT);
    seed_blocked_agent(&h.store, "Do you want to proceed?", vec![]).await;

    // Approval id matches, but the prompt_hash is stale (wrong question).
    let (status, value) = post(
        &h.app,
        h.body(
            "req-stale",
            Capability::Approve,
            W2_AGENT,
            json!({
                "kind": "approve",
                "approval_id": format!("{W2_AGENT}:{W2_HASH}"),
                "prompt_hash": "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "choice": "yes"
            }),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["kind"], "hash_mismatch", "wrong-question race must be typed distinctly");
    assert!(h.adapter.commands().is_empty(), "stale hash must not dispatch");
    assert_eq!(h.audit_entries().len(), 0, "refused approval is not a write (AC5)");
}

#[tokio::test]
async fn approve_with_matching_claim_dispatches_validated_choice_exactly_once() {
    let h = harness();
    h.adapter.knows(W2_AGENT);
    seed_blocked_agent(&h.store, "Do you want to proceed?", vec!["yes".into(), "no".into()]).await;

    let body = json!({
        "kind": "approve",
        "approval_id": format!("{W2_AGENT}:{W2_HASH}"),
        "prompt_hash": W2_HASH,
        "choice": "yes"
    });
    let (status, _) = post(&h.app, h.body("req-ok", Capability::Approve, W2_AGENT, body, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h.adapter.commands(),
        vec![(W2_AGENT.to_string(), DriveCommand::Approve { choice: "yes".to_string() })],
        "validated choice must dispatch exactly once"
    );
    assert_eq!(h.audit_entries().len(), 1, "executed approval is one write (AC5)");

    // Replay of the same request_id returns the stored response, no double
    // send.
    let body = json!({
        "kind": "approve",
        "approval_id": format!("{W2_AGENT}:{W2_HASH}"),
        "prompt_hash": W2_HASH,
        "choice": "yes"
    });
    let (status, _) = post(&h.app, h.body("req-ok", Capability::Approve, W2_AGENT, body, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h.adapter.commands().len(),
        1,
        "replay must not double-send"
    );
}

// ---------------------------------------------------------------------------
// Typed refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_capability_is_typed_refusal() {
    let h = harness();
    let (status, value) = post(
        &h.app,
        json!({
            "key_id": h.key_id,
            "signature": "stub",
            "envelope": {
                "request_id": "req",
                "capability": "sudo",
                "target": "herdr:a",
                "payload": prompt_payload("x"),
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["kind"], "unknown_capability");
    assert_eq!(value["message"], "unknown capability: sudo");
    assert_eq!(value["request_id"], "req");
    assert_eq!(h.adapter.dispatch_count(), 0);
    assert_eq!(h.audit_entries().len(), 0);
}

#[tokio::test]
async fn payload_kind_mismatch_is_typed_refusal() {
    let h = harness();
    let (status, value) = post(
        &h.app,
        h.body("req", Capability::Prompt, "herdr:a", json!({ "kind": "read_tail" }), None),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(value["kind"], "payload");
    assert!(value["message"].as_str().unwrap().contains("bad payload for prompt"));
    assert_eq!(h.adapter.dispatch_count(), 0);
    assert_eq!(h.audit_entries().len(), 0);
}

#[tokio::test]
async fn malformed_body_is_bad_request() {
    let h = harness();
    let (status, value) = post(&h.app, "{ not json".to_string()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["kind"], "bad_request");
    assert_eq!(h.adapter.dispatch_count(), 0);
}

#[tokio::test]
async fn empty_request_id_or_target_is_bad_request() {
    let h = harness();
    let (status, value) = post(
        &h.app,
        h.body("", Capability::Prompt, "herdr:a", prompt_payload("x"), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["kind"], "bad_request");

    let (status, value) = post(
        &h.app,
        h.body("req", Capability::Prompt, "", prompt_payload("x"), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["kind"], "bad_request");
    assert_eq!(h.adapter.dispatch_count(), 0);
}

#[tokio::test]
async fn unknown_agent_is_typed_refusal_at_dispatch() {
    let h = harness();
    let (status, value) = post(
        &h.app,
        h.body("req", Capability::Prompt, "herdr:ghost", prompt_payload("x"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "dispatch outcomes ride the DriveResponse");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], "unknown agent: herdr:ghost");
    assert_eq!(value["request_id"], "req");
    assert_eq!(h.adapter.dispatch_count(), 1);
    assert_eq!(h.audit_entries().len(), 1, "typed refusal at dispatch is audited");
}

#[tokio::test]
async fn not_implemented_and_transport_are_typed() {
    let h = harness();
    h.adapter.mode(Mode::NotImplemented);
    let (status, value) = post(
        &h.app,
        h.body("req", Capability::Prompt, "herdr:a", prompt_payload("x"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], "command not implemented: test-command");
    assert!(matches!(
        &h.audit_entries()[0].outcome,
        AuditOutcome::Refused(_)
    ));

    let h = harness();
    h.adapter.mode(Mode::Transport);
    let (status, value) = post(
        &h.app,
        h.body("req", Capability::Interrupt, "herdr:a", Value::Null, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], "transport error: boom");
    assert!(matches!(&h.audit_entries()[0].outcome, AuditOutcome::Failed(_)));
}

// ---------------------------------------------------------------------------
// Auth seam
// ---------------------------------------------------------------------------

#[tokio::test]
async fn valid_signature_passes_and_invalid_is_typed_auth_error() {
    let h = harness();
    h.adapter.knows("herdr:a");
    let (status, value) = post(
        &h.app,
        h.body("req", Capability::Prompt, "herdr:a", prompt_payload("go"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true);

    // Missing signature: typed 400, no dispatch, no audit.
    let h = harness();
    let (status, value) = post(
        &h.app,
        json!({
            "key_id": h.key_id,
            "signature": "",
            "envelope": {
                "request_id": "req",
                "capability": "prompt",
                "target": "herdr:a",
                "payload": prompt_payload("x"),
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "empty signature on a registered key is a bad signature");
    assert_eq!(value["kind"], "bad_signature");
    assert_eq!(h.adapter.dispatch_count(), 0);
    assert_eq!(h.audit_entries().len(), 0);

    // Bad signature (tampered payload after signing): typed 401.
    let h = harness();
    let body = h.body("req", Capability::Prompt, "herdr:a", prompt_payload("go"), None);
    let mut tampered: Value = serde_json::from_str(&body).unwrap();
    tampered["envelope"]["payload"]["text"] = json!("go AND rm -rf /");
    let (status, value) = post(&h.app, tampered.to_string()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["kind"], "bad_signature");
    assert_eq!(h.adapter.dispatch_count(), 0);
    assert_eq!(h.audit_entries().len(), 0);

    // Unknown key: typed 404 (verify order: key validity before signature).
    let h = harness();
    let (status, value) = post(
        &h.app,
        json!({
            "key_id": "dev_00000000000000000000000000000000",
            "signature": "AAAA",
            "envelope": {
                "request_id": "req",
                "capability": "prompt",
                "target": "herdr:a",
                "payload": prompt_payload("x"),
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(value["kind"], "unknown_key");
    assert_eq!(h.adapter.dispatch_count(), 0);

    // Revoked key: typed 403 (second device, then revoked).
    let h = harness();
    let (other_signing, other_pubkey, other_key) = h.register_other_device(&ALL_CAPABILITIES);
    h.auth
        .registry
        .set_revoked(&other_key, true)
        .expect("revoke");
    let (status, value) = post(
        &h.app,
        h.body_from(&other_signing, other_pubkey, "req", Capability::Prompt, "herdr:a", prompt_payload("x"), None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(value["kind"], "revoked");
    assert_eq!(h.adapter.dispatch_count(), 0);

    // Expired key: typed 403. Register a second device with a 1s TTL and
    // wait it out (verifier checks expiry before signature).
    let h = harness();
    let (other_signing, pubkey_other) = test_support::keypair();
    let token = h.auth.registry.registration_token();
    let rec = h
        .auth
        .registry
        .register(&token, pubkey_other, std::time::Duration::from_secs(1))
        .expect("register with short ttl");
    h.auth
        .registry
        .set_grants(&rec.key_id, ALL_CAPABILITIES.to_vec())
        .expect("grants");
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let (status, value) = post(
        &h.app,
        h.body_from(&other_signing, pubkey_other, "req", Capability::Prompt, "herdr:a", prompt_payload("x"), None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(value["kind"], "expired");
    assert_eq!(h.adapter.dispatch_count(), 0);

    // Read-only default: a device with NO grants cannot drive (AC3).
    let h = harness();
    let (other_signing, other_pubkey, _other_key) = h.register_other_device(&[]);
    let (status, value) = post(
        &h.app,
        h.body_from(&other_signing, other_pubkey, "req", Capability::Prompt, "herdr:a", prompt_payload("x"), None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(value["kind"], "not_granted");
    assert_eq!(h.adapter.dispatch_count(), 0);
    assert_eq!(h.audit_entries().len(), 0, "auth failures are never audited (AC5)");
}

#[tokio::test]
async fn step_up_gate_blocks_destructive_payloads_and_recovers_with_token() {
    // F2 (W3 review): the step-up gate must be spliced into the REAL drive
    // handler — destructive payload without a token → 403 step_up_required,
    // audit 0; with a minted token → executes, audit 1.
    let h = harness();
    h.adapter.knows("herdr:a");

    let (status, value) = post(
        &h.app,
        h.body("req-destr", Capability::Prompt, "herdr:a", prompt_payload("rm -rf /tmp/x"), None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(value["kind"], "step_up_required");
    assert_eq!(h.adapter.dispatch_count(), 0, "no dispatch without step-up");
    assert_eq!(h.audit_entries().len(), 0, "step-up failures are not audited (AC5)");

    // Mint a token for the harness device and retry with the header.
    let token = h.auth.step_up.mint(&h.key_id, std::time::Duration::from_secs(300));
    let res = h
        .app
        .clone()
        .oneshot(
            Request::post("/drive")
                .header("content-type", "application/json")
                .header("X-Step-Up-Token", token.clone())
                .body(Body::from(h.body("req-destr3", Capability::Prompt, "herdr:a", prompt_payload("rm -rf /tmp/x"), None)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "minted token unlocks the destructive payload");
    assert_eq!(h.adapter.dispatch_count(), 1, "exactly one dispatch");
    assert_eq!(h.audit_entries().len(), 1, "executed write is audited exactly once");

    // Token replay is refused (single-use).
    let res = h
        .app
        .clone()
        .oneshot(
            Request::post("/drive")
                .header("content-type", "application/json")
                .header("X-Step-Up-Token", token)
                .body(Body::from(h.body("req-destr4", Capability::Prompt, "herdr:a", prompt_payload("rm -rf /tmp/x"), None)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "replayed step-up token refused");
}

#[tokio::test]
async fn audit_grows_only_on_writes() {
    // Auth failure: no audit.
    let h = harness();
    let body = h.body("r1", Capability::Prompt, "herdr:a", prompt_payload("x"), None);
    let mut tampered: Value = serde_json::from_str(&body).unwrap();
    tampered["envelope"]["payload"]["text"] = json!("tampered");
    let (status, _) = post(&h.app, tampered.to_string()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.audit_entries().len(), 0);

    // Payload failure: no audit.
    let h = harness();
    post(
        &h.app,
        h.body("r2", Capability::Prompt, "herdr:a", json!({ "kind": "read_tail" }), None),
    )
    .await;
    assert_eq!(h.audit_entries().len(), 0);
    assert_eq!(h.adapter.dispatch_count(), 0);

    // Successful write: one Executed entry with the full field set.
    let h = harness();
    h.adapter.knows("herdr:a");
    post(
        &h.app,
        h.body("r3", Capability::Prompt, "herdr:a", prompt_payload("go"), None),
    )
    .await;
    let entries = h.audit_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].request_id, "r3");
    assert_eq!(entries[0].capability, "prompt");
    assert_eq!(entries[0].target, "herdr:a");
    assert_eq!(entries[0].key_id, h.key_id.as_str());
    assert!(matches!(&entries[0].outcome, AuditOutcome::Executed));

    // Dispatch refusal: one Refused entry.
    let h = harness();
    h.adapter.mode(Mode::NotImplemented);
    post(
        &h.app,
        h.body("r4", Capability::Prompt, "herdr:a", prompt_payload("x"), None),
    )
    .await;
    let entries = h.audit_entries();
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0].outcome, AuditOutcome::Refused(_)));
}
