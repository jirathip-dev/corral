//! `POST /drive` tests (#354 read-only cut): the kept read plane —
//! envelope → command mapping for read_tail/read_diff, typed refusals
//! (unknown capability / bad payload / unknown agent / auth), replay
//! idempotency, and audit call sites (grows on writes, never on auth
//! failures or replay hits). Mutating capabilities are refused at the
//! capability boundary — covered end to end by tests/readonly_cut.rs.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use corrald::adapters::{Adapter, DriveCommand, DriveError};
use corrald::api::drive::ReplayTable;
use corrald::api::{AppState, router};
use corrald::auth::AuthPlane;
use corrald::auth::audit::ChainEntry;
use corrald::auth::test_support;
use corrald::core::store::Store;
use corrald::drive::{AuditOutcome, Capability, ReadDiffQuery, ReadDiffResult};
use ed25519_dalek::SigningKey;
use http_body_util::BodyExt;
use serde_json::{Value, json};
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
    stale: Mutex<HashSet<String>>,
    /// Hide one tombstone check so a test can model disappearance after the
    /// handler's initial stale check but before its store lookup.
    deferred_stale: Mutex<HashSet<String>>,
    mode: Mutex<Mode>,
    /// When Some, drive() notifies `started` and blocks on the receiver
    /// before returning (concurrency test support — never in production).
    hold: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    started: Arc<tokio::sync::Notify>,
    /// read_tail lines served back (empty = no output). `read_tail` never
    /// touches `commands` — the seam is distinct from `drive()`.
    tail: Mutex<Option<Vec<String>>>,
    /// The lines argument each read_tail call received.
    tail_requests: Mutex<Vec<u32>>,
    /// #232: read_diff result served back (None = NoWorktree refusal), and
    /// the queries each read_diff call received.
    diff_results: Mutex<Option<Value>>,
    diff_requests: Mutex<Vec<ReadDiffQuery>>,
    diff_refusal: Mutex<Option<DriveError>>,
}

impl Default for RecordingAdapter {
    fn default() -> Self {
        Self {
            dispatches: AtomicUsize::new(0),
            commands: Mutex::new(Vec::new()),
            known: Mutex::new(HashSet::new()),
            stale: Mutex::new(HashSet::new()),
            deferred_stale: Mutex::new(HashSet::new()),
            mode: Mutex::new(Mode::Ok),
            hold: Mutex::new(None),
            started: Arc::new(tokio::sync::Notify::new()),
            tail: Mutex::new(None),
            tail_requests: Mutex::new(Vec::new()),
            diff_results: Mutex::new(None),
            diff_requests: Mutex::new(Vec::new()),
            diff_refusal: Mutex::new(None),
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

    fn stale(&self, agent_id: &str) -> &Self {
        self.stale.lock().unwrap().insert(agent_id.to_string());
        self
    }

    fn tail(&self, lines: Vec<String>) -> &Self {
        *self.tail.lock().unwrap() = Some(lines);
        self
    }

    /// #232: serve a pre-baked read_diff result (used when `known`).
    fn diff_result(&self, result: Value) -> &Self {
        *self.diff_results.lock().unwrap() = Some(result);
        self
    }

    /// #232: force read_diff to flatline with a typed dispatch refusal.
    fn diff_refusal(&self, error: DriveError) -> &Self {
        *self.diff_refusal.lock().unwrap() = Some(error);
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

    fn drive<'a>(
        &'a self,
        agent_id: &'a str,
        command: DriveCommand,
    ) -> futures::future::BoxFuture<'a, Result<(), DriveError>> {
        let agent_id = agent_id.to_string();
        Box::pin(async move {
            self.dispatches.fetch_add(1, Ordering::SeqCst);
            self.commands
                .lock()
                .unwrap()
                .push((agent_id.clone(), command));
            let hold = self.hold.lock().unwrap().take();
            if let Some(rx) = hold {
                self.started.notify_waiters();
                let _ = tokio::task::block_in_place(|| rx.recv());
            }
            match *self.mode.lock().unwrap() {
                Mode::Ok => {
                    if self.known.lock().unwrap().contains(&agent_id) {
                        Ok(())
                    } else {
                        Err(DriveError::UnknownAgent(agent_id))
                    }
                }
                Mode::NotImplemented => Err(DriveError::NotImplemented("test-command")),
                Mode::Transport => Err(DriveError::Transport("boom".to_string())),
            }
        })
    }

    fn knows_agent(&self, agent_id: &str) -> bool {
        self.known.lock().unwrap().contains(agent_id)
    }

    fn is_stale_agent(&self, agent_id: &str) -> bool {
        if self.deferred_stale.lock().unwrap().remove(agent_id) {
            return false;
        }
        self.stale.lock().unwrap().contains(agent_id)
    }

    fn read_tail<'a>(
        &'a self,
        agent_id: &'a str,
        lines: u32,
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, DriveError>> {
        let future = async move {
            self.dispatches.fetch_add(1, Ordering::SeqCst);
            self.tail_requests.lock().unwrap().push(lines);
            match *self.mode.lock().unwrap() {
                Mode::Ok => {
                    if self.known.lock().unwrap().contains(agent_id) {
                        Ok(self.tail.lock().unwrap().clone().unwrap_or_default())
                    } else {
                        Err(DriveError::UnknownAgent(agent_id.to_string()))
                    }
                }
                Mode::NotImplemented => Err(DriveError::NotImplemented("test-command")),
                Mode::Transport => Err(DriveError::Transport("boom".to_string())),
            }
        };
        Box::pin(future)
    }

    fn read_diff<'a>(
        &'a self,
        agent_id: &'a str,
        query: ReadDiffQuery,
    ) -> futures::future::BoxFuture<'a, Result<ReadDiffResult, DriveError>> {
        let future = async move {
            self.dispatches.fetch_add(1, Ordering::SeqCst);
            self.diff_requests.lock().unwrap().push(query);
            if let Some(error) = self.diff_refusal.lock().unwrap().take() {
                return Err(error);
            }
            match *self.mode.lock().unwrap() {
                Mode::Ok => {
                    if self.known.lock().unwrap().contains(agent_id) {
                        Ok(serde_json::from_value(
                            self.diff_results.lock().unwrap().clone().unwrap(),
                        )
                        .expect("test diff result must decode"))
                    } else {
                        Err(DriveError::UnknownAgent(agent_id.to_string()))
                    }
                }
                Mode::NotImplemented => Err(DriveError::NotImplemented("test-command")),
                Mode::Transport => Err(DriveError::Transport("boom".to_string())),
            }
        };
        Box::pin(future)
    }
}

/// Every capability the drive tests exercise, granted to the harness device.
const ALL_CAPABILITIES: [Capability; 2] = [Capability::ReadTail, Capability::ReadDiff];

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
        let env = test_support::envelope("bootstrap-other", Capability::ReadTail, "bootstrap");
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
        self.body_from(
            &self.signing,
            self.pubkey,
            request_id,
            capability,
            target,
            payload,
            rev,
        )
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
        let signed = test_support::signed(&self.auth.registry, &token, signing, pubkey, &envelope);
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
    let env = test_support::envelope("bootstrap", Capability::ReadTail, "bootstrap");
    let token = auth.registry.registration_token();
    let signed = test_support::signed(&auth.registry, &token, &signing, pubkey, &env);
    let key_id = signed.key_id.clone();
    auth.registry
        .set_grants(&key_id, ALL_CAPABILITIES.to_vec())
        .expect("grants");

    let issues = Arc::new(corrald::api::issues::IssuesCache::default());
    let app = router(AppState {
        store: store.clone(),
        auth: auth.clone(),
        adapter: adapter.clone(),
        replay: Arc::new(ReplayTable::default()),
        issues: issues.clone(),
        provenance: Arc::new(corrald::core::provenance::PromptProvenance::new()),
        cors_origins: Vec::new(),
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

fn read_tail_payload(_text: &str) -> Value {
    json!({ "kind": "read_tail", "lines": 200 })
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

// ---------------------------------------------------------------------------
// read_tail result path (P4 W2.1): DriveResponse.result carries the lines
// the adapter returned (redacted, bounded), the audit entry stays
// `executed`, and the seam is the dedicated Adapter::read_tail — never the
// command-returning Adapter::drive path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_tail_result_carries_lines_and_audits_executed() {
    let h = harness();
    h.adapter
        .knows("herdr:abc")
        .tail(vec!["line one".into(), "line two".into()]);

    let (status, value) = post(
        &h.app,
        h.body(
            "req-tail",
            Capability::ReadTail,
            "herdr:abc",
            json!({ "kind": "read_tail", "lines": 200 }),
            None,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true);
    assert_eq!(value["result"]["lines"], json!(["line one", "line two"]));
    // #167: blocks ride ADDITIVELY alongside lines. The two agent lines have
    // no blank separator, so they merge into ONE block (raw pane text has no
    // role hint; the block renderer is the dumb consumer). #315: with no
    // recorded Prompt provenance the merged block is `unknown` — raw pane
    // text is never asserted to be model output.
    assert_eq!(
        value["result"]["blocks"],
        json!([{ "kind": "unknown", "text": "line one\nline two" }]),
        "read_tail must serve segmented blocks alongside lines"
    );
    let rev = h.store.snapshot().await.rev;
    assert_eq!(value["rev"].as_u64(), Some(rev));

    // Routed through Adapter::read_tail, not the command-returning drive
    // path: exactly one dispatch, zero drive() commands, the requested
    // line count passed through.
    assert_eq!(h.adapter.dispatch_count(), 1);
    assert!(
        h.adapter.commands().is_empty(),
        "read_tail must not reach drive()"
    );
    assert_eq!(*h.adapter.tail_requests.lock().unwrap(), vec![200]);

    // Audit: one Executed entry, capability read_tail, fields unchanged.
    let entries = h.audit_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].request_id, "req-tail");
    assert_eq!(entries[0].capability, "read_tail");
    assert_eq!(entries[0].target, "herdr:abc");
    assert!(matches!(&entries[0].outcome, AuditOutcome::Executed));
}

#[tokio::test]
async fn read_tail_with_no_output_is_ok_with_empty_lines() {
    let h = harness();
    h.adapter.knows("herdr:abc").tail(vec![]);

    let (status, value) = post(
        &h.app,
        h.body(
            "req-tail",
            Capability::ReadTail,
            "herdr:abc",
            json!({ "kind": "read_tail" }),
            None,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true);
    assert_eq!(
        value["result"]["lines"],
        json!([]),
        "no output -> clean empty lines"
    );
    assert_eq!(h.adapter.dispatch_count(), 1);
    let entries = h.audit_entries();
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0].outcome, AuditOutcome::Executed));
}

#[tokio::test]
async fn read_tail_blocks_are_cleaned_and_segmented_on_the_wire() {
    let h = harness();
    // Raw pane tail: ANSI color, a CR overdraw, a truncation marker, chrome.
    h.adapter.knows("herdr:abc").tail(vec![
        "\u{1b}[32m$ cargo build\u{1b}[0m".into(),
        "Compiling corrald v0.1.0".into(),
        "Build progress: 10%\rBuild progress: 100%".into(),
        "".into(),
        "... +229 lines (ctrl+t to view transcript)".into(),
        "\u{2022} Ask Codex to do anything".into(),
    ]);

    let (status, value) = post(
        &h.app,
        h.body(
            "req-tail",
            Capability::ReadTail,
            "herdr:abc",
            json!({ "kind": "read_tail", "lines": 200 }),
            None,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    // The existing lines contract is untouched (redacted, bounded lines, ANSI
    // still present — pass 1 is for blocks only, so egui keeps what it had).
    assert_eq!(
        value["result"]["lines"][0],
        "\u{1b}[32m$ cargo build\u{1b}[0m"
    );
    // The additive blocks carry the cleaned, segmented view.
    let blocks = value["result"]["blocks"].as_array().expect("blocks array");
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["kind"], "tool");
    assert_eq!(
        blocks[0]["text"],
        "$ cargo build\nCompiling corrald v0.1.0\nBuild progress: 100%"
    );
    assert_eq!(blocks[1]["kind"], "system");
    assert_eq!(blocks[1]["truncated_before"], 229);
    assert!(
        !blocks[1]["text"].as_str().unwrap().contains("ctrl+t"),
        "marker text is removed from the block"
    );
}

#[tokio::test]
async fn read_tail_transport_failure_is_typed_and_audited_failed() {
    let h = harness();
    h.adapter.knows("herdr:abc").mode(Mode::Transport);

    let (status, value) = post(
        &h.app,
        h.body(
            "req-tail",
            Capability::ReadTail,
            "herdr:abc",
            json!({ "kind": "read_tail" }),
            None,
        ),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "dispatch outcomes ride the DriveResponse"
    );
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], "transport error: boom");
    assert!(value.get("result").is_none());
    let entries = h.audit_entries();
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0].outcome, AuditOutcome::Failed(_)));
}

#[tokio::test]
async fn read_tail_unknown_agent_is_typed_refusal() {
    let h = harness();
    let (status, value) = post(
        &h.app,
        h.body(
            "req-tail",
            Capability::ReadTail,
            "herdr:ghost",
            json!({ "kind": "read_tail" }),
            None,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], "unknown agent: herdr:ghost");
    let entries = h.audit_entries();
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0].outcome, AuditOutcome::Refused(_)));
}

#[tokio::test]
async fn stale_agent_is_typed_conflict_before_dispatch() {
    let h = harness();
    h.adapter.stale("herdr:gone");

    for (request_id, capability, payload) in [
        (
            "req-stale-tail",
            Capability::ReadTail,
            json!({ "kind": "read_tail", "lines": 10 }),
        ),
        (
            "req-stale-diff",
            Capability::ReadDiff,
            json!({ "kind": "read_diff" }),
        ),
    ] {
        let (status, value) = post(
            &h.app,
            h.body(request_id, capability, "herdr:gone", payload, None),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{capability}");
        assert_eq!(value["kind"], "stale_agent", "{capability}");
        assert_eq!(value["request_id"], request_id, "{capability}");
    }
    assert_eq!(h.adapter.dispatch_count(), 0, "stale rows never dispatch");
    assert!(
        h.audit_entries().is_empty(),
        "pre-dispatch stale is not audited"
    );
}

#[tokio::test]
async fn completed_request_replays_after_target_becomes_stale() {
    let h = harness();
    let target = "herdr:replay";
    h.adapter.knows(target);
    let body = h.body(
        "req-replay-after-stale",
        Capability::ReadTail,
        target,
        read_tail_payload("hello"),
        None,
    );

    let (first_status, first) = post(&h.app, body.clone()).await;
    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(first["ok"], true);

    h.adapter.stale(target);
    let (second_status, second) = post(&h.app, body).await;
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(
        second, first,
        "completed request ids replay byte-identically"
    );
    assert_eq!(h.adapter.dispatch_count(), 1, "replay does not redispatch");
    assert_eq!(h.audit_entries().len(), 1, "replay does not re-audit");
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
                "payload": read_tail_payload("x"),
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
        h.body(
            "req",
            Capability::ReadTail,
            "herdr:a",
            json!({ "kind": "read_diff" }),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(value["kind"], "payload");
    assert!(
        value["message"]
            .as_str()
            .unwrap()
            .contains("bad payload for read_tail")
    );
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
        h.body(
            "",
            Capability::ReadTail,
            "herdr:a",
            read_tail_payload("x"),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["kind"], "bad_request");

    let (status, value) = post(
        &h.app,
        h.body(
            "req",
            Capability::ReadTail,
            "",
            read_tail_payload("x"),
            None,
        ),
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
        h.body(
            "req",
            Capability::ReadTail,
            "herdr:ghost",
            read_tail_payload("x"),
            None,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "dispatch outcomes ride the DriveResponse"
    );
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], "unknown agent: herdr:ghost");
    assert_eq!(value["request_id"], "req");
    assert_eq!(h.adapter.dispatch_count(), 1);
    assert_eq!(
        h.audit_entries().len(),
        1,
        "typed refusal at dispatch is audited"
    );
}

#[tokio::test]
async fn not_implemented_and_transport_are_typed() {
    let h = harness();
    h.adapter.mode(Mode::NotImplemented);
    let (status, value) = post(
        &h.app,
        h.body(
            "req",
            Capability::ReadTail,
            "herdr:a",
            read_tail_payload("x"),
            None,
        ),
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
        h.body(
            "req",
            Capability::ReadTail,
            "herdr:a",
            json!({ "kind": "read_tail" }),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], "transport error: boom");
    assert!(matches!(
        &h.audit_entries()[0].outcome,
        AuditOutcome::Failed(_)
    ));
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
        h.body(
            "req",
            Capability::ReadTail,
            "herdr:a",
            read_tail_payload("go"),
            None,
        ),
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
                "capability": "read_tail",
                "target": "herdr:a",
                "payload": read_tail_payload("x"),
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "empty signature on a registered key is a bad signature"
    );
    assert_eq!(value["kind"], "bad_signature");
    assert_eq!(h.adapter.dispatch_count(), 0);
    assert_eq!(h.audit_entries().len(), 0);

    // Bad signature (tampered payload after signing): typed 401.
    let h = harness();
    let body = h.body(
        "req",
        Capability::ReadTail,
        "herdr:a",
        read_tail_payload("go"),
        None,
    );
    let mut tampered: Value = serde_json::from_str(&body).unwrap();
    tampered["envelope"]["payload"]["lines"] = json!(1);
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
                "capability": "read_tail",
                "target": "herdr:a",
                "payload": read_tail_payload("x"),
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
        h.body_from(
            &other_signing,
            other_pubkey,
            "req",
            Capability::ReadTail,
            "herdr:a",
            read_tail_payload("x"),
            None,
        ),
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
        h.body_from(
            &other_signing,
            pubkey_other,
            "req",
            Capability::ReadTail,
            "herdr:a",
            read_tail_payload("x"),
            None,
        ),
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
        h.body_from(
            &other_signing,
            other_pubkey,
            "req",
            Capability::ReadTail,
            "herdr:a",
            read_tail_payload("x"),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(value["kind"], "not_granted");
    assert_eq!(h.adapter.dispatch_count(), 0);
    assert_eq!(
        h.audit_entries().len(),
        0,
        "auth failures are never audited (AC5)"
    );
}

#[tokio::test]
async fn audit_grows_only_on_signed_drive_dispatches() {
    // Auth failure: no audit.
    let h = harness();
    let body = h.body(
        "r1",
        Capability::ReadTail,
        "herdr:a",
        read_tail_payload("x"),
        None,
    );
    let mut tampered: Value = serde_json::from_str(&body).unwrap();
    tampered["envelope"]["payload"]["lines"] = json!(1);
    let (status, _) = post(&h.app, tampered.to_string()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.audit_entries().len(), 0);

    // Payload failure: no audit.
    let h = harness();
    post(
        &h.app,
        h.body(
            "r2",
            Capability::ReadTail,
            "herdr:a",
            json!({ "kind": "read_diff" }),
            None,
        ),
    )
    .await;
    assert_eq!(h.audit_entries().len(), 0);
    assert_eq!(h.adapter.dispatch_count(), 0);

    // Successful write: one Executed entry with the full field set.
    let h = harness();
    h.adapter.knows("herdr:a");
    post(
        &h.app,
        h.body(
            "r3",
            Capability::ReadTail,
            "herdr:a",
            read_tail_payload("go"),
            None,
        ),
    )
    .await;
    let entries = h.audit_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].request_id, "r3");
    assert_eq!(entries[0].capability, "read_tail");
    assert_eq!(entries[0].target, "herdr:a");
    assert_eq!(entries[0].key_id, h.key_id.as_str());
    assert!(matches!(&entries[0].outcome, AuditOutcome::Executed));

    // Dispatch refusal: one Refused entry.
    let h = harness();
    h.adapter.mode(Mode::NotImplemented);
    post(
        &h.app,
        h.body(
            "r4",
            Capability::ReadTail,
            "herdr:a",
            read_tail_payload("x"),
            None,
        ),
    )
    .await;
    let entries = h.audit_entries();
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0].outcome, AuditOutcome::Refused(_)));
}

// ---------------------------------------------------------------------------
// read_diff (#232): the response-bearing diff seam — DriveResponse.result
// carries the paged ReadDiffResult, query bounds are clamped daemon-side,
// the grant gate is the same 403 `not_granted` as read_tail, and dispatched
// reads are audited `executed` / `refused`.
// ---------------------------------------------------------------------------

fn diff_fixture_result() -> Value {
    serde_json::to_value(ReadDiffResult {
        repo: Some("corral".to_string()),
        branch: Some("g232/read-diff".to_string()),
        head: Some("abcdef1".to_string()),
        stats: corrald::drive::DiffStats {
            files: 2,
            adds: 12,
            dels: 3,
        },
        files: vec![
            corrald::drive::DiffFileStat {
                path: "src/a.rs".to_string(),
                adds: 10,
                dels: 1,
            },
            corrald::drive::DiffFileStat {
                path: "src/b.rs".to_string(),
                adds: 2,
                dels: 2,
            },
        ],
        files_truncated: false,
        offset: 0,
        lines: vec![
            "diff --git a/src/a.rs b/src/a.rs".to_string(),
            "+fn main() {}".to_string(),
            "-let x = 1;".to_string(),
        ],
        total: 3,
        has_more: false,
        next_offset: None,
    })
    .expect("result serializes")
}

#[tokio::test]
async fn read_diff_result_carries_page_and_audits_executed() {
    let h = harness();
    h.adapter
        .knows("herdr:abc")
        .diff_result(diff_fixture_result());

    let (status, value) = post(
        &h.app,
        h.body(
            "req-diff",
            Capability::ReadDiff,
            "herdr:abc",
            json!({ "kind": "read_diff", "files": 5, "offset": 0, "lines": 200 }),
            None,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true);
    assert_eq!(value["result"]["stats"]["files"], 2);
    assert_eq!(value["result"]["stats"]["adds"], 12);
    assert_eq!(value["result"]["files"][0]["path"], "src/a.rs");
    assert_eq!(
        value["result"]["lines"][0],
        "diff --git a/src/a.rs b/src/a.rs"
    );
    assert_eq!(value["result"]["head"], "abcdef1");

    // Routed through Adapter::read_diff, never drive().
    assert_eq!(h.adapter.dispatch_count(), 1);
    assert!(h.adapter.commands().is_empty());
    let queries = h.adapter.diff_requests.lock().unwrap();
    assert_eq!(
        *queries,
        vec![ReadDiffQuery {
            files: 5,
            offset: 0,
            lines: 200
        }]
    );

    // Audit mirrors read_tail: one Executed entry.
    let entries = h.audit_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].capability, "read_diff");
    assert_eq!(entries[0].target, "herdr:abc");
    assert!(matches!(&entries[0].outcome, AuditOutcome::Executed));
}

#[tokio::test]
async fn read_diff_bounds_are_clamped_daemon_side() {
    let h = harness();
    h.adapter
        .knows("herdr:abc")
        .diff_result(diff_fixture_result());

    // 0 files / 0 lines / huge offset clamp; defaults fill absent fields.
    let (status, value) = post(
        &h.app,
        h.body(
            "req-clamp",
            Capability::ReadDiff,
            "herdr:abc",
            json!({ "kind": "read_diff", "files": 0, "lines": 0 }),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true);
    let queries = h.adapter.diff_requests.lock().unwrap();
    assert_eq!(
        *queries,
        vec![ReadDiffQuery {
            files: 1,
            offset: 0,
            lines: 1
        }],
        "files/lines clamp to [1, cap]; offset defaults to 0"
    );
}

#[tokio::test]
async fn read_diff_without_grant_is_403_not_granted_and_not_audited() {
    let h = harness();
    h.adapter
        .knows("herdr:abc")
        .diff_result(diff_fixture_result());
    let (other_signing, other_pubkey, _other_key) = h.register_other_device(&[]);

    let (status, value) = post(
        &h.app,
        h.body_from(
            &other_signing,
            other_pubkey,
            "req-diff-nogrant",
            Capability::ReadDiff,
            "herdr:abc",
            json!({ "kind": "read_diff" }),
            None,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(value["kind"], "not_granted");
    // The read is default-empty (no dispatch): not audited, like read_tail.
    assert!(h.audit_entries().is_empty());
    assert_eq!(h.adapter.dispatch_count(), 0);
}

#[tokio::test]
async fn read_diff_dispatch_refusal_is_typed_and_audited_refused() {
    let h = harness();
    h.adapter
        .knows("herdr:abc")
        .diff_refusal(DriveError::NoWorktree("no worktree path".to_string()));

    let (status, value) = post(
        &h.app,
        h.body(
            "req-diff-refused",
            Capability::ReadDiff,
            "herdr:abc",
            json!({ "kind": "read_diff" }),
            None,
        ),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "dispatch outcomes ride the DriveResponse"
    );
    assert_eq!(value["ok"], false);
    assert_eq!(value["error_kind"], "no_worktree");
    assert!(value.get("result").is_none());
    let entries = h.audit_entries();
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0].outcome, AuditOutcome::Refused(_)));
}
