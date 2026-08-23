//! `POST /drive` tests (P3 W1): envelope → command mapping, typed refusals
//! (unknown capability / bad payload / unknown agent / auth), replay
//! idempotency (sequential + concurrent), read_tail bounds, and audit call
//! sites (grows on writes, never on auth failures or replay hits).

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
use corrald::drive::{AuditOutcome, Capability};
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
    /// attach handle served back on success (used when `known`).
    attach_results: Mutex<Option<Value>>,
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
            attach_results: Mutex::new(None),
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

    fn stale_after_initial_check(&self, agent_id: &str) -> &Self {
        self.stale(agent_id);
        self.deferred_stale
            .lock()
            .unwrap()
            .insert(agent_id.to_string());
        self
    }

    fn tail(&self, lines: Vec<String>) -> &Self {
        *self.tail.lock().unwrap() = Some(lines);
        self
    }

    fn attach_result(&self, handle: Value) -> &Self {
        *self.attach_results.lock().unwrap() = Some(handle);
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

    fn attach<'a>(
        &'a self,
        agent_id: &'a str,
    ) -> futures::future::BoxFuture<'a, Result<Value, DriveError>> {
        let agent_id = agent_id.to_string();
        Box::pin(async move {
            self.dispatches.fetch_add(1, Ordering::SeqCst);
            self.commands
                .lock()
                .unwrap()
                .push((agent_id.clone(), DriveCommand::Attach));
            match *self.mode.lock().unwrap() {
                Mode::Ok => {
                    if self.known.lock().unwrap().contains(&agent_id) {
                        Ok(self
                            .attach_results
                            .lock()
                            .unwrap()
                            .clone()
                            .unwrap_or_else(|| {
                                json!({
                                    "kind": "terminal_ref",
                                    "target": agent_id,
                                    "pane_id": "p1",
                                    "command": format!("herdr agent attach --takeover {agent_id}"),
                                })
                            }))
                    } else {
                        Err(DriveError::UnknownAgent(agent_id))
                    }
                }
                Mode::NotImplemented => Err(DriveError::NotImplemented("test-command")),
                Mode::Transport => Err(DriveError::Transport("boom".to_string())),
            }
        })
    }
}

/// Every capability the drive tests exercise, granted to the harness device.
const ALL_CAPABILITIES: [Capability; 7] = [
    Capability::Prompt,
    Capability::Interrupt,
    Capability::Approve,
    Capability::ReadTail,
    Capability::Kill,
    Capability::Attach,
    Capability::StartWorktree,
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
        issues: Arc::new(corrald::api::issues::IssuesCache::default()),
        transcript_roots: corrald::transcript::bind::TranscriptRoots::hermetic(),
        transcript_limiter: corrald::api::transcript::TranscriptLimiter::default(),
        role_probe_memo: corrald::transcript::RoleProbeMemo::default(),
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
        h.body(
            "req-1",
            Capability::Prompt,
            "herdr:abc",
            prompt_payload("continue"),
            Some(3),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true);
    assert_eq!(value["request_id"], "req-1");
    let rev = h.store.snapshot().await.rev;
    assert_eq!(
        value["rev"].as_u64(),
        Some(rev),
        "response carries the store rev"
    );
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
        let (status, value) = post(&h.app, h.body("req", cap, "herdr:a", Value::Null, None)).await;
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
        h.body(
            "req",
            Capability::Interrupt,
            "herdr:a",
            json!({ "kind": "interrupt" }),
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
            .contains("no payload expected")
    );
    assert_eq!(h.adapter.dispatch_count(), 0);
}

#[tokio::test]
async fn attach_result_carries_handle_audits_executed_and_replays() {
    let h = harness();
    let handle = json!({
        "kind": "terminal_ref",
        "target": "agent-one",
        "pane_id": "w1:p1",
        "command": "herdr agent attach --takeover agent-one",
        "args": ["herdr", "agent", "attach", "--takeover", "agent-one"],
    });
    h.adapter.knows("herdr:abc").attach_result(handle.clone());

    let body = h.body(
        "req-attach",
        Capability::Attach,
        "herdr:abc",
        Value::Null,
        None,
    );
    let (first_status, first) = post(&h.app, body.clone()).await;
    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(first["ok"], true);
    assert_eq!(first["result"], handle);
    assert_eq!(
        h.adapter.commands(),
        vec![("herdr:abc".to_string(), DriveCommand::Attach)]
    );
    let entries = h.audit_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].capability, "attach");
    assert!(matches!(&entries[0].outcome, AuditOutcome::Executed));

    let (second_status, second) = post(&h.app, body).await;
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(
        second, first,
        "attach replay is byte-identical and never re-dispatches"
    );
    assert_eq!(h.adapter.dispatch_count(), 1);
    assert_eq!(h.audit_entries().len(), 1, "replay does not re-audit");
}

#[tokio::test]
async fn attach_unknown_agent_is_typed_refusal_and_audited() {
    let h = harness();
    let (status, value) = post(
        &h.app,
        h.body(
            "req-attach-ghost",
            Capability::Attach,
            "herdr:ghost",
            Value::Null,
            None,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], "unknown agent: herdr:ghost");
    assert_eq!(value["error_kind"], "unknown_agent");
    assert!(value.get("result").is_none());
    assert_eq!(
        h.adapter.commands(),
        vec![("herdr:ghost".to_string(), DriveCommand::Attach)]
    );
    let entries = h.audit_entries();
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0].outcome, AuditOutcome::Refused(_)));
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
        parent_id: None,
        host: None,
        workspace: Default::default(),
        attachment: None,
        display_name: None,
        title: None,
    };
    let store2 = store.clone();
    store2
        .apply(corrald::core::model::Change::upsert(agent))
        .await;
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
    assert_eq!(
        value["kind"], "hash_mismatch",
        "wrong-question race must be typed distinctly"
    );
    assert!(
        h.adapter.commands().is_empty(),
        "stale hash must not dispatch"
    );
    assert_eq!(
        h.audit_entries().len(),
        0,
        "refused approval is not a write (AC5)"
    );
}

#[tokio::test]
async fn approve_with_matching_claim_dispatches_validated_choice_exactly_once() {
    let h = harness();
    h.adapter.knows(W2_AGENT);
    seed_blocked_agent(
        &h.store,
        "Do you want to proceed?",
        vec!["yes".into(), "no".into()],
    )
    .await;

    let body = json!({
        "kind": "approve",
        "approval_id": format!("{W2_AGENT}:{W2_HASH}"),
        "prompt_hash": W2_HASH,
        "choice": "yes"
    });
    let (status, _) = post(
        &h.app,
        h.body("req-ok", Capability::Approve, W2_AGENT, body, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h.adapter.commands(),
        vec![(
            W2_AGENT.to_string(),
            DriveCommand::Approve {
                choice: "yes".to_string()
            }
        )],
        "validated choice must dispatch exactly once"
    );
    assert_eq!(
        h.audit_entries().len(),
        1,
        "executed approval is one write (AC5)"
    );

    // Replay of the same request_id returns the stored response, no double
    // send.
    let body = json!({
        "kind": "approve",
        "approval_id": format!("{W2_AGENT}:{W2_HASH}"),
        "prompt_hash": W2_HASH,
        "choice": "yes"
    });
    let (status, _) = post(
        &h.app,
        h.body("req-ok", Capability::Approve, W2_AGENT, body, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(h.adapter.commands().len(), 1, "replay must not double-send");
}

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
    // no blank separator, so they merge into ONE agent block (raw pane text
    // has no role hint; the block renderer is the dumb consumer).
    assert_eq!(
        value["result"]["blocks"],
        json!([{ "kind": "agent", "text": "line one\nline two" }]),
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
            "req-stale-prompt",
            Capability::Prompt,
            prompt_payload("hello"),
        ),
        (
            "req-stale-tail",
            Capability::ReadTail,
            json!({ "kind": "read_tail", "lines": 10 }),
        ),
        ("req-stale-attach", Capability::Attach, Value::Null),
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
async fn stale_approve_is_conflict_before_current_claim_validation() {
    let h = harness();
    // The live store has already dropped the blocked row, but the adapter
    // still remembers its canonical id as a stale Herdr target. The stale
    // classification must win over the approval lookup's generic 404.
    h.adapter.stale(W2_AGENT);
    let (status, value) = post(
        &h.app,
        h.body(
            "req-stale-approve",
            Capability::Approve,
            W2_AGENT,
            json!({
                "kind": "approve",
                "approval_id": format!("{W2_AGENT}:{W2_HASH}"),
                "prompt_hash": W2_HASH,
                "choice": "yes"
            }),
            None,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["kind"], "stale_agent");
    assert_eq!(value["request_id"], "req-stale-approve");
    assert_eq!(h.adapter.dispatch_count(), 0);
    assert!(h.audit_entries().is_empty());
}

#[tokio::test]
async fn approve_missing_row_after_initial_stale_check_is_reclassified_stale() {
    let h = harness();
    h.adapter.stale_after_initial_check(W2_AGENT);
    seed_blocked_agent(&h.store, "Do you want to proceed?", vec!["yes".into()]).await;
    h.store
        .apply(corrald::core::model::Change::Remove(W2_AGENT.to_string()))
        .await;

    // The first adapter check intentionally says "not stale"; the row is
    // already gone by the time the first approval store.get runs, and the
    // classification check must observe the tombstone instead of returning a
    // generic 404.
    let (status, value) = post(
        &h.app,
        h.body(
            "req-approve-first-read-race",
            Capability::Approve,
            W2_AGENT,
            json!({
                "kind": "approve",
                "approval_id": format!("{W2_AGENT}:{W2_HASH}"),
                "prompt_hash": W2_HASH,
                "choice": "yes"
            }),
            None,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["kind"], "stale_agent");
    assert_eq!(value["request_id"], "req-approve-first-read-race");
    assert_eq!(h.adapter.dispatch_count(), 0);
    assert!(h.audit_entries().is_empty());
}

#[tokio::test]
async fn completed_request_replays_after_target_becomes_stale() {
    let h = harness();
    let target = "herdr:replay";
    h.adapter.knows(target);
    let body = h.body(
        "req-replay-after-stale",
        Capability::Prompt,
        target,
        prompt_payload("hello"),
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
        h.body(
            "req",
            Capability::Prompt,
            "herdr:a",
            json!({ "kind": "read_tail" }),
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
            .contains("bad payload for prompt")
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
        h.body(
            "req",
            Capability::Prompt,
            "herdr:ghost",
            prompt_payload("x"),
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
            Capability::Prompt,
            "herdr:a",
            prompt_payload("x"),
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
        h.body("req", Capability::Interrupt, "herdr:a", Value::Null, None),
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

#[tokio::test]
async fn awaited_prompt_and_approve_refusals_are_cached_as_failures() {
    let h = harness();
    h.adapter.knows(W2_AGENT).mode(Mode::Transport);
    seed_blocked_agent(&h.store, "Continue?", vec!["y".into()]).await;

    let (status, prompt_failure) = post(
        &h.app,
        h.body(
            "req-prompt-failure",
            Capability::Prompt,
            W2_AGENT,
            prompt_payload("hello"),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(prompt_failure["ok"], false);
    assert_eq!(prompt_failure["error"], "transport error: boom");
    assert_eq!(h.audit_entries().len(), 1, "RPC refusal is audited once");

    let (status, replayed) = post(
        &h.app,
        h.body(
            "req-prompt-failure",
            Capability::Prompt,
            W2_AGENT,
            prompt_payload("hello"),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed, prompt_failure, "failure replay is byte-identical");
    assert_eq!(
        h.adapter.dispatch_count(),
        1,
        "prompt retry does not redispatch"
    );

    let approve_payload = json!({
        "kind": "approve",
        "approval_id": format!("{W2_AGENT}:{W2_HASH}"),
        "prompt_hash": W2_HASH,
        "choice": "y"
    });
    let (status, approve_failure) = post(
        &h.app,
        h.body(
            "req-approve-failure",
            Capability::Approve,
            W2_AGENT,
            approve_payload,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(approve_failure["ok"], false);
    assert_eq!(approve_failure["error"], "transport error: boom");
    assert_eq!(
        h.audit_entries().len(),
        2,
        "approve RPC refusal is audited once"
    );
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
            Capability::Prompt,
            "herdr:a",
            prompt_payload("go"),
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
                "capability": "prompt",
                "target": "herdr:a",
                "payload": prompt_payload("x"),
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
        Capability::Prompt,
        "herdr:a",
        prompt_payload("go"),
        None,
    );
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
        h.body_from(
            &other_signing,
            other_pubkey,
            "req",
            Capability::Prompt,
            "herdr:a",
            prompt_payload("x"),
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
            Capability::Prompt,
            "herdr:a",
            prompt_payload("x"),
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
            Capability::Prompt,
            "herdr:a",
            prompt_payload("x"),
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
async fn step_up_gate_blocks_destructive_payloads_and_recovers_with_token() {
    // F2 (W3 review): the step-up gate must be spliced into the REAL drive
    // handler — destructive payload without a token → 403 step_up_required,
    // audit 0; with a minted token → executes, audit 1.
    let h = harness();
    h.adapter.knows("herdr:a");

    let (status, value) = post(
        &h.app,
        h.body(
            "req-destr",
            Capability::Prompt,
            "herdr:a",
            prompt_payload("rm -rf /tmp/x"),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(value["kind"], "step_up_required");
    assert_eq!(h.adapter.dispatch_count(), 0, "no dispatch without step-up");
    assert_eq!(
        h.audit_entries().len(),
        0,
        "step-up failures are not audited (AC5)"
    );

    // Mint a token for the harness device and retry with the header.
    let token = h
        .auth
        .step_up
        .mint(&h.key_id, std::time::Duration::from_secs(300));
    let res = h
        .app
        .clone()
        .oneshot(
            Request::post("/drive")
                .header("content-type", "application/json")
                .header("X-Step-Up-Token", token.clone())
                .body(Body::from(h.body(
                    "req-destr3",
                    Capability::Prompt,
                    "herdr:a",
                    prompt_payload("rm -rf /tmp/x"),
                    None,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "minted token unlocks the destructive payload"
    );
    assert_eq!(h.adapter.dispatch_count(), 1, "exactly one dispatch");
    assert_eq!(
        h.audit_entries().len(),
        1,
        "executed write is audited exactly once"
    );

    // Token replay is refused (single-use).
    let res = h
        .app
        .clone()
        .oneshot(
            Request::post("/drive")
                .header("content-type", "application/json")
                .header("X-Step-Up-Token", token)
                .body(Body::from(h.body(
                    "req-destr4",
                    Capability::Prompt,
                    "herdr:a",
                    prompt_payload("rm -rf /tmp/x"),
                    None,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "replayed step-up token refused"
    );
}

#[tokio::test]
async fn audit_grows_only_on_writes() {
    // Auth failure: no audit.
    let h = harness();
    let body = h.body(
        "r1",
        Capability::Prompt,
        "herdr:a",
        prompt_payload("x"),
        None,
    );
    let mut tampered: Value = serde_json::from_str(&body).unwrap();
    tampered["envelope"]["payload"]["text"] = json!("tampered");
    let (status, _) = post(&h.app, tampered.to_string()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.audit_entries().len(), 0);

    // Payload failure: no audit.
    let h = harness();
    post(
        &h.app,
        h.body(
            "r2",
            Capability::Prompt,
            "herdr:a",
            json!({ "kind": "read_tail" }),
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
            Capability::Prompt,
            "herdr:a",
            prompt_payload("go"),
            None,
        ),
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
        h.body(
            "r4",
            Capability::Prompt,
            "herdr:a",
            prompt_payload("x"),
            None,
        ),
    )
    .await;
    let entries = h.audit_entries();
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0].outcome, AuditOutcome::Refused(_)));
}

/// #113: `start_worktree` is capability-gated like every write — a device
/// WITHOUT the grant is refused 403 `not_granted` before ANY worktree is
/// touched (read-only default).
#[tokio::test]
async fn start_worktree_needs_the_capability_grant() {
    let h = harness();
    let (other_signing, other_pubkey, _other_key) = h.register_other_device(&[]);
    let payload = json!({
        "kind": "start_worktree",
        "mode": "issue",
        "repo": "corral",
        "number": 113,
        "issue_url": "https://github.com/jirathip-dev/corral/issues/113",
    });
    let (status, value) = post(
        &h.app,
        h.body_from(
            &other_signing,
            other_pubkey,
            "wt-1",
            Capability::StartWorktree,
            "corral",
            payload,
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
        "auth failures are never audited"
    );
}

/// #113: the `start_worktree` payload maps into a typed `WorktreeRequest`
/// (issue-linked vs issue-free) so a malformed request is refused before any
/// filesystem work. This is the pre-dispatch parse seam.
#[tokio::test]
async fn start_worktree_payload_maps_to_issue_or_free_request() {
    // A well-formed issue-linked payload is accepted by the parse seam.
    let issue_payload = json!({
        "kind": "start_worktree",
        "mode": "issue",
        "repo": "corral",
        "number": 113,
        "issue_url": "https://github.com/jirathip-dev/corral/issues/113",
    });
    let parsed = corrald::drive::DrivePayload::parse(Capability::StartWorktree, &issue_payload)
        .expect("issue payload parses");
    assert_eq!(
        parsed,
        corrald::drive::DrivePayload::StartWorktree {
            mode: "issue".into(),
            repo: "corral".into(),
            number: Some(113),
            issue_url: Some("https://github.com/jirathip-dev/corral/issues/113".into()),
            name: None,
        }
    );

    // A free payload is accepted; an empty free name is a typed refusal.
    let free_payload =
        json!({ "kind": "start_worktree", "mode": "free", "repo": "corral", "name": "explore" });
    assert!(matches!(
        corrald::drive::DrivePayload::parse(Capability::StartWorktree, &free_payload),
        Ok(corrald::drive::DrivePayload::StartWorktree { mode, name: Some(_), .. }) if mode == "free"
    ));
    // A free payload with an empty name still parses at the serde layer; the
    // non-empty-name validation is the pre-dispatch `command_for` seam (see
    // the drive.rs unit test).
    let bad_free =
        json!({ "kind": "start_worktree", "mode": "free", "repo": "corral", "name": "" });
    assert!(matches!(
        corrald::drive::DrivePayload::parse(Capability::StartWorktree, &bad_free),
        Ok(corrald::drive::DrivePayload::StartWorktree { name: Some(name), .. }) if name.is_empty()
    ));

    // An unknown mode is refused at the serde layer (no matching variant).
    let bad_mode = json!({ "kind": "start_worktree", "mode": "chaos", "repo": "corral" });
    assert!(
        corrald::drive::DrivePayload::parse(Capability::StartWorktree, &bad_mode).is_err(),
        "unknown mode is a typed refusal"
    );
}

/// #113 review 2: the signed envelope `target` is the repo the audit will
/// record. A granted client must not sign target=A + payload.repo=B, because
/// that would create a worktree on B while the audit says A. The handler must
/// refuse with a typed payload error BEFORE any dispatch.
#[tokio::test]
async fn start_worktree_refuses_target_payload_repo_mismatch() {
    let h = harness();
    let payload = json!({
        "kind": "start_worktree",
        "mode": "issue",
        "repo": "plush",
        "number": 5,
        "issue_url": "https://github.com/jirathip-dev/plush-meadow/issues/5",
    });
    // Sign for target "corral" but request repo "plush": mismatch.
    let (status, value) = post(
        &h.app,
        h.body(
            "wt-mismatch",
            Capability::StartWorktree,
            "corral",
            payload,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(value["kind"], "payload");
    assert_eq!(
        h.adapter.dispatch_count(),
        0,
        "nothing dispatches on a mismatch"
    );
    assert!(
        h.audit_entries().is_empty(),
        "payload refusals are not audited"
    );
}
