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
use corrald::api::drive::{ReplayTable, StubAudit, StubAuthorizer};
use corrald::api::{router, AppState};
use corrald::core::store::Store;
use corrald::drive::{
    AuditOutcome, AuthError, AuthorizedDrive, DriveAuthorizer, READ_TAIL_MAX_LINES, SignedDrive,
};
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
            Mode::NotImplemented => Err(DriveError::NotImplemented("approve")),
            Mode::Transport => Err(DriveError::Transport("boom".to_string())),
        }
    }

    fn knows_agent(&self, agent_id: &str) -> bool {
        self.known.lock().unwrap().contains(agent_id)
    }
}

/// Authorizer stub: valid passes, configured failures return the typed error.
#[derive(Debug)]
struct TestAuthorizer {
    error: Option<AuthError>,
}

impl TestAuthorizer {
    fn accept() -> Arc<Self> {
        Arc::new(Self { error: None })
    }

    fn reject(error: AuthError) -> Arc<Self> {
        Arc::new(Self { error: Some(error) })
    }
}

impl DriveAuthorizer for TestAuthorizer {
    fn verify(&self, signed: &SignedDrive) -> Result<AuthorizedDrive, AuthError> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        Ok(AuthorizedDrive {
            key_id: signed.key_id.clone(),
            envelope: signed.envelope.clone(),
        })
    }
}

struct Harness {
    store: Store,
    adapter: Arc<RecordingAdapter>,
    audit: Arc<StubAudit>,
    app: Router,
}

fn harness(authorizer: Arc<dyn DriveAuthorizer>) -> Harness {
    let store = Store::new();
    let coalescer = store.clone();
    std::mem::drop(tokio::spawn(async move { coalescer.run_coalescer().await }));
    let adapter = Arc::new(RecordingAdapter::default());
    let audit = Arc::new(StubAudit::default());
    let app = router(AppState {
        store: store.clone(),
        adapter: adapter.clone(),
        authorizer,
        audit: audit.clone(),
        replay: Arc::new(ReplayTable::default()),
    });
    Harness {
        store,
        adapter,
        audit,
        app,
    }
}

fn signed_body(
    request_id: &str,
    capability: &str,
    target: &str,
    payload: Value,
    rev: Option<u64>,
) -> String {
    json!({
        "key_id": "test-key",
        "signature": "dGVzdC1zaWc",
        "envelope": {
            "request_id": request_id,
            "capability": capability,
            "target": target,
            "payload": payload,
            "rev": rev,
        }
    })
    .to_string()
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
    let h = harness(TestAuthorizer::accept());
    h.adapter.knows("herdr:abc");

    let (status, value) = post(
        &h.app,
        signed_body("req-1", "prompt", "herdr:abc", prompt_payload("continue"), Some(3)),
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
    for (capability, command) in [
        ("interrupt", DriveCommand::Interrupt),
        ("kill", DriveCommand::Kill),
        ("attach", DriveCommand::Attach),
    ] {
        let h = harness(TestAuthorizer::accept());
        h.adapter.knows("herdr:a");
        let (status, value) = post(
            &h.app,
            signed_body("req", capability, "herdr:a", Value::Null, None),
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
    let h = harness(TestAuthorizer::accept());
    let (status, value) = post(
        &h.app,
        signed_body("req", "interrupt", "herdr:a", json!({ "kind": "interrupt" }), None),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(value["kind"], "payload");
    assert!(value["message"].as_str().unwrap().contains("no payload expected"));
    assert_eq!(h.adapter.dispatch_count(), 0);
}

#[tokio::test]
async fn approve_maps_to_approve_command() {
    let h = harness(TestAuthorizer::accept());
    h.adapter.knows("herdr:a");
    let (status, _) = post(
        &h.app,
        signed_body(
            "req",
            "approve",
            "herdr:a",
            json!({ "kind": "approve", "approval_id": "ap-1", "prompt_hash": "sha256:x", "choice": "y" }),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h.adapter.commands(),
        vec![("herdr:a".to_string(), DriveCommand::Approve)]
    );
}

// ---------------------------------------------------------------------------
// Typed refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_capability_is_typed_refusal() {
    let h = harness(TestAuthorizer::accept());
    let (status, value) = post(
        &h.app,
        signed_body("req", "sudo", "herdr:a", prompt_payload("x"), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["kind"], "unknown_capability");
    assert_eq!(value["message"], "unknown capability: sudo");
    assert_eq!(value["request_id"], "req");
    assert_eq!(h.adapter.dispatch_count(), 0);
    assert_eq!(h.audit.entries().len(), 0);
}

#[tokio::test]
async fn payload_kind_mismatch_is_typed_refusal() {
    let h = harness(TestAuthorizer::accept());
    let (status, value) = post(
        &h.app,
        signed_body("req", "prompt", "herdr:a", json!({ "kind": "read_tail" }), None),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(value["kind"], "payload");
    assert!(value["message"].as_str().unwrap().contains("bad payload for prompt"));
    assert_eq!(h.adapter.dispatch_count(), 0);
    assert_eq!(h.audit.entries().len(), 0);
}

#[tokio::test]
async fn malformed_body_is_bad_request() {
    let h = harness(TestAuthorizer::accept());
    let (status, value) = post(&h.app, "{ not json".to_string()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["kind"], "bad_request");
    assert_eq!(h.adapter.dispatch_count(), 0);
}

#[tokio::test]
async fn empty_request_id_or_target_is_bad_request() {
    let h = harness(TestAuthorizer::accept());
    let (status, value) = post(
        &h.app,
        signed_body("", "prompt", "herdr:a", prompt_payload("x"), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["kind"], "bad_request");

    let (status, value) = post(
        &h.app,
        signed_body("req", "prompt", "", prompt_payload("x"), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["kind"], "bad_request");
    assert_eq!(h.adapter.dispatch_count(), 0);
}

#[tokio::test]
async fn unknown_agent_is_typed_refusal_at_dispatch() {
    let h = harness(TestAuthorizer::accept());
    let (status, value) = post(
        &h.app,
        signed_body("req", "prompt", "herdr:ghost", prompt_payload("x"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "dispatch outcomes ride the DriveResponse");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], "unknown agent: herdr:ghost");
    assert_eq!(value["request_id"], "req");
    assert_eq!(h.adapter.dispatch_count(), 1);
    assert_eq!(h.audit.entries().len(), 1, "typed refusal at dispatch is audited");
}

#[tokio::test]
async fn not_implemented_and_transport_are_typed() {
    let h = harness(TestAuthorizer::accept());
    h.adapter.mode(Mode::NotImplemented);
    let (status, value) = post(
        &h.app,
        signed_body(
            "req",
            "approve",
            "herdr:a",
            json!({ "kind": "approve", "approval_id": "a", "prompt_hash": "h", "choice": "y" }),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], "command not implemented: approve");
    assert!(matches!(
        &h.audit.entries()[0].outcome,
        AuditOutcome::Refused(_)
    ));

    let h = harness(TestAuthorizer::accept());
    h.adapter.mode(Mode::Transport);
    let (status, value) = post(
        &h.app,
        signed_body("req", "interrupt", "herdr:a", Value::Null, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], "transport error: boom");
    assert!(matches!(&h.audit.entries()[0].outcome, AuditOutcome::Failed(_)));
}

// ---------------------------------------------------------------------------
// Auth seam
// ---------------------------------------------------------------------------

#[tokio::test]
async fn valid_signature_passes_and_invalid_is_typed_auth_error() {
    let h = harness(TestAuthorizer::accept());
    h.adapter.knows("herdr:a");
    let (status, value) = post(
        &h.app,
        signed_body("req", "prompt", "herdr:a", prompt_payload("go"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true);

    for error in [
        AuthError::MissingSignature,
        AuthError::BadSignature,
        AuthError::UnknownKey,
        AuthError::Expired,
        AuthError::Revoked,
        AuthError::NotGranted(corrald::drive::Capability::Prompt),
    ] {
        let h = harness(TestAuthorizer::reject(error.clone()));
        let (status, value) = post(
            &h.app,
            signed_body("req", "prompt", "herdr:a", prompt_payload("go"), None),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{error:?}");
        assert_eq!(value["kind"], "auth", "{error:?}");
        assert_eq!(value["message"], error.to_string(), "{error:?}");
        assert_eq!(value["request_id"], "req", "{error:?}");
        assert_eq!(h.adapter.dispatch_count(), 0, "{error:?}");
        assert_eq!(h.audit.entries().len(), 0, "{error:?}");
    }
}

#[tokio::test]
async fn stub_authorizer_fails_closed_on_missing_signature() {
    let h = harness(Arc::new(StubAuthorizer));
    h.adapter.knows("herdr:a");
    let (status, _) = post(
        &h.app,
        signed_body("req", "prompt", "herdr:a", prompt_payload("x"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "non-empty stub signature passes");

    let (status, value) = post(
        &h.app,
        json!({
            "key_id": "",
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
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(value["kind"], "auth");
    assert_eq!(value["message"], "missing signature");
    assert_eq!(
        h.adapter.dispatch_count(),
        1,
        "the missing-signature attempt must not dispatch (only the earlier valid one did)"
    );
}

// ---------------------------------------------------------------------------
// Replay idempotency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn replay_returns_first_response_without_redispatch() {
    let h = harness(TestAuthorizer::accept());
    h.adapter.knows("herdr:a");
    let body = signed_body("req-dup", "prompt", "herdr:a", prompt_payload("go"), None);

    let (status_1, value_1) = post(&h.app, body.clone()).await;
    let (status_2, value_2) = post(&h.app, body).await;

    assert_eq!(status_1, StatusCode::OK);
    assert_eq!(status_2, StatusCode::OK);
    assert_eq!(value_1, value_2, "replay returns the first response verbatim");
    assert_eq!(value_1["request_id"], "req-dup");
    assert_eq!(h.adapter.dispatch_count(), 1, "no double send");
    assert_eq!(h.audit.entries().len(), 1, "replay hit is not a new write");
}

#[tokio::test]
async fn replay_of_a_refusal_is_also_idempotent() {
    let h = harness(TestAuthorizer::accept());
    h.adapter.mode(Mode::NotImplemented);
    let body = signed_body(
        "req-ref",
        "approve",
        "herdr:a",
        json!({ "kind": "approve", "approval_id": "a", "prompt_hash": "h", "choice": "y" }),
        None,
    );

    let (status_1, value_1) = post(&h.app, body.clone()).await;
    let (_, value_2) = post(&h.app, body).await;

    assert_eq!(status_1, StatusCode::OK);
    assert_eq!(value_1["ok"], false);
    assert_eq!(value_1, value_2);
    assert_eq!(h.adapter.dispatch_count(), 1);
    assert_eq!(h.audit.entries().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_duplicate_dispatches_exactly_once() {
    let h = harness(TestAuthorizer::accept());
    h.adapter.knows("herdr:a");
    let (hold_tx, hold_rx) = std::sync::mpsc::channel();
    *h.adapter.hold.lock().unwrap() = Some(hold_rx);

    let body = signed_body("req-conc", "prompt", "herdr:a", prompt_payload("go"), None);
    let first = {
        let app = h.app.clone();
        let body = body.clone();
        tokio::spawn(async move { post(&app, body).await })
    };
    h.adapter.started.notified().await;

    // The duplicate arrives while the first is mid-dispatch.
    let (status_b, value_b) = post(&h.app, body.clone()).await;
    assert_eq!(status_b, StatusCode::CONFLICT);
    assert_eq!(value_b["kind"], "in_flight");

    hold_tx.send(()).unwrap();
    let (status_a, value_a) = first.await.unwrap();
    assert_eq!(status_a, StatusCode::OK);
    assert_eq!(value_a["ok"], true);
    assert_eq!(h.adapter.dispatch_count(), 1, "concurrent duplicates never double-send");

    // After completion the retry gets the stored response.
    let (status_c, value_c) = post(&h.app, body).await;
    assert_eq!(status_c, StatusCode::OK);
    assert_eq!(value_c, value_a);
    assert_eq!(h.adapter.dispatch_count(), 1);
    assert_eq!(h.audit.entries().len(), 1);
}

// ---------------------------------------------------------------------------
// read_tail bounds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_tail_lines_are_bounded() {
    let h = harness(TestAuthorizer::accept());
    h.adapter.knows("herdr:a");
    post(
        &h.app,
        signed_body("req", "read_tail", "herdr:a", json!({ "kind": "read_tail", "lines": 999_999 }), None),
    )
    .await;
    assert_eq!(
        h.adapter.commands()[0].1,
        DriveCommand::ReadTail {
            lines: Some(READ_TAIL_MAX_LINES)
        },
        "oversized lines clamp to READ_TAIL_MAX_LINES"
    );

    let h = harness(TestAuthorizer::accept());
    h.adapter.knows("herdr:a");
    post(
        &h.app,
        signed_body("req", "read_tail", "herdr:a", json!({ "kind": "read_tail" }), None),
    )
    .await;
    assert_eq!(
        h.adapter.commands()[0].1,
        DriveCommand::ReadTail {
            lines: Some(READ_TAIL_MAX_LINES)
        },
        "omitted lines default to READ_TAIL_MAX_LINES"
    );

    let h = harness(TestAuthorizer::accept());
    h.adapter.knows("herdr:a");
    post(
        &h.app,
        signed_body("req", "read_tail", "herdr:a", json!({ "kind": "read_tail", "lines": 0 }), None),
    )
    .await;
    assert_eq!(
        h.adapter.commands()[0].1,
        DriveCommand::ReadTail { lines: Some(1) },
        "zero lines clamp up to 1"
    );
}

// ---------------------------------------------------------------------------
// Audit call sites (AC5: grows only on writes)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn audit_grows_only_on_writes() {
    // Auth failure: no audit.
    let h = harness(TestAuthorizer::reject(AuthError::BadSignature));
    post(&h.app, signed_body("r1", "prompt", "herdr:a", prompt_payload("x"), None)).await;
    assert_eq!(h.audit.entries().len(), 0);

    // Payload failure: no audit.
    let h = harness(TestAuthorizer::accept());
    post(
        &h.app,
        signed_body("r2", "prompt", "herdr:a", json!({ "kind": "read_tail" }), None),
    )
    .await;
    assert_eq!(h.audit.entries().len(), 0);
    assert_eq!(h.adapter.dispatch_count(), 0);

    // Successful write: one Executed entry with the full field set.
    let h = harness(TestAuthorizer::accept());
    h.adapter.knows("herdr:a");
    post(
        &h.app,
        signed_body("r3", "prompt", "herdr:a", prompt_payload("go"), None),
    )
    .await;
    let entries = h.audit.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].request_id, "r3");
    assert_eq!(entries[0].capability, "prompt");
    assert_eq!(entries[0].target, "herdr:a");
    assert_eq!(entries[0].key_id, "test-key");
    assert!(matches!(&entries[0].outcome, AuditOutcome::Executed));

    // Dispatch refusal: one Refused entry.
    let h = harness(TestAuthorizer::accept());
    h.adapter.mode(Mode::NotImplemented);
    post(
        &h.app,
        signed_body(
            "r4",
            "approve",
            "herdr:a",
            json!({ "kind": "approve", "approval_id": "a", "prompt_hash": "h", "choice": "y" }),
            None,
        ),
    )
    .await;
    let entries = h.audit.entries();
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0].outcome, AuditOutcome::Refused(_)));
}
