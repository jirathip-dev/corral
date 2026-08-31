//! #315 canonical transcript provenance: a successfully dispatched signed
//! Prompt records an authoritative user event for its target; the matching
//! terminal echo is deduplicated; clients consume the daemon's semantic
//! blocks without re-deriving roles from raw pane lines.
//!
//! These tests use one GENERIC terminal snapshot (no harness/model markers)
//! plus recorded Prompt provenance to prove:
//! - the echoed prompt renders exactly once as `user` on the wire;
//! - terminal-only chrome/status never becomes `user` output;
//! - unprovenanced direct input stays `unknown`, never falsely attributed.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use corrald::adapters::{Adapter, DriveCommand, DriveError};
use corrald::api::drive::ReplayTable;
use corrald::api::{AppState, router};
use corrald::auth::AuthPlane;
use corrald::auth::test_support;
use corrald::core::store::Store;
use corrald::drive::Capability;
use ed25519_dalek::SigningKey;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

/// Ok-mode adapter with a programmable tail; mirrors tests/drive.rs's
/// RecordingAdapter trimmed to the seams these tests touch.
#[derive(Debug, Default)]
struct TailOnlyAdapter {
    known: Mutex<HashSet<String>>,
    tail: Mutex<Vec<String>>,
}

impl TailOnlyAdapter {
    fn knows(&self, agent_id: &str) -> &Self {
        self.known.lock().unwrap().insert(agent_id.to_string());
        self
    }

    fn tail(&self, lines: Vec<String>) -> &Self {
        *self.tail.lock().unwrap() = lines;
        self
    }
}

impl Adapter for TailOnlyAdapter {
    fn source(&self) -> &'static str {
        "test"
    }

    fn start(self: Arc<Self>, _store: Store) {}

    fn drive<'a>(
        &'a self,
        _agent_id: &'a str,
        _command: DriveCommand,
    ) -> futures::future::BoxFuture<'a, Result<(), DriveError>> {
        Box::pin(async { Ok(()) })
    }

    fn knows_agent(&self, agent_id: &str) -> bool {
        self.known.lock().unwrap().contains(agent_id)
    }

    fn read_tail<'a>(
        &'a self,
        agent_id: &'a str,
        _lines: u32,
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, DriveError>> {
        let known = self.known_agent(agent_id);
        let tail = self.tail.lock().unwrap().clone();
        Box::pin(async move {
            if known {
                Ok(tail)
            } else {
                Err(DriveError::UnknownAgent(agent_id.to_string()))
            }
        })
    }
}

impl TailOnlyAdapter {
    fn known_agent(&self, agent_id: &str) -> bool {
        self.known.lock().unwrap().contains(agent_id)
    }
}

/// Every capability; the prompt round trip needs prompt + read_tail.
const ALL_CAPABILITIES: [Capability; 9] = [
    Capability::Prompt,
    Capability::Interrupt,
    Capability::Approve,
    Capability::ReadTail,
    Capability::Kill,
    Capability::Attach,
    Capability::StartWorktree,
    Capability::ReadDiff,
    Capability::ReadIssues,
];

/// Real W3 auth plane over a temp dir + a registered, fully-granted device,
/// exactly like tests/drive.rs, so every request is genuinely signed.
struct Harness {
    adapter: Arc<TailOnlyAdapter>,
    auth: Arc<AuthPlane>,
    signing: SigningKey,
    pubkey: [u8; 32],
    app: Router,
    _dir: tempfile::TempDir,
}

fn harness() -> Harness {
    let store = Store::new();
    let coalescer = store.clone();
    std::mem::drop(tokio::spawn(async move { coalescer.run_coalescer().await }));
    let adapter = Arc::new(TailOnlyAdapter::default());
    let dir = tempfile::tempdir().expect("tempdir");
    let auth = Arc::new(AuthPlane::load_or_create(dir.path().to_path_buf()).expect("auth plane"));
    let (signing, pubkey) = test_support::keypair();
    let env = test_support::envelope("bootstrap", Capability::Prompt, "bootstrap");
    let token = auth.registry.registration_token();
    let signed = test_support::signed(&auth.registry, &token, &signing, pubkey, &env);
    auth.registry
        .set_grants(&signed.key_id, ALL_CAPABILITIES.to_vec())
        .expect("grants");
    let app = router(AppState {
        store,
        auth: auth.clone(),
        adapter: adapter.clone(),
        replay: Arc::new(ReplayTable::default()),
        issues: Arc::new(corrald::api::issues::IssuesCache::default()),
        provenance: Arc::new(corrald::core::provenance::PromptProvenance::new()),
        cors_origins: Vec::new(),
    });
    Harness {
        adapter,
        auth,
        signing,
        pubkey,
        app,
        _dir: dir,
    }
}

impl Harness {
    fn body(
        &self,
        request_id: &str,
        capability: Capability,
        target: &str,
        payload: Value,
    ) -> String {
        let envelope = corrald::drive::DriveEnvelope {
            request_id: request_id.to_string(),
            capability,
            target: target.to_string(),
            payload,
            rev: None,
        };
        let token = self.auth.registry.registration_token();
        let signed = test_support::signed(
            &self.auth.registry,
            &token,
            &self.signing,
            self.pubkey,
            &envelope,
        );
        serde_json::to_string(&signed).expect("signed body serializes")
    }
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

/// The generic snapshot both clients must segment identically: a tool run,
/// the echoed user prompt, assistant prose, status/metadata chrome, and a
/// line a human typed straight into the terminal (no Corral provenance).
fn generic_snapshot_lines() -> Vec<String> {
    vec![
        "$ cargo build".into(),
        "Compiling corrald v0.1.0".into(),
        "".into(),
        "> ship the canonical transcript stream".into(),
        "".into(),
        "Canonical stream wired end to end.".into(),
        "".into(),
        "model context 42% · tokens in flight".into(),
        "status: working · esc to interrupt".into(),
        "fix the flaky test by hand".into(),
    ]
}

const ECHO_LINE: &str = "> ship the canonical transcript stream";
const PROMPT_TEXT: &str = "ship the canonical transcript stream";

#[tokio::test]
async fn prompt_round_trip_renders_exactly_once_as_user() {
    let h = harness();
    h.adapter.knows("herdr:abc").tail(generic_snapshot_lines());

    // 1. Dispatch a signed Prompt through the real plane (recorded).
    let (status, value) = post(
        &h.app,
        h.body(
            "req-prompt",
            Capability::Prompt,
            "herdr:abc",
            json!({ "kind": "prompt", "text": PROMPT_TEXT }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "prompt dispatch: {value}");
    assert_eq!(value["ok"], true, "prompt must dispatch cleanly");

    // 2. Read the tail: the echo must be exactly one `user` block.
    let (status, value) = post(
        &h.app,
        h.body(
            "req-tail",
            Capability::ReadTail,
            "herdr:abc",
            json!({ "kind": "read_tail", "lines": 200 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true);

    let blocks = value["result"]["blocks"].as_array().expect("blocks array");
    let user_blocks: Vec<&Value> = blocks.iter().filter(|b| b["kind"] == "user").collect();
    assert_eq!(
        user_blocks.len(),
        1,
        "the Corral prompt must render exactly once as user: {blocks:?}"
    );
    assert_eq!(user_blocks[0]["text"], PROMPT_TEXT);
    // Provenance rides the block so clients can audit the attribution.
    assert_eq!(
        user_blocks[0]["prompt_request_id"], "req-prompt",
        "the user block must carry its recorded provenance"
    );

    // 3. The echo line must not ALSO appear anywhere else (no duplicate).
    let echo_count = blocks
        .iter()
        .filter(|b| b["text"].as_str().map(|t| t.contains(PROMPT_TEXT)) == Some(true))
        .count();
    assert_eq!(
        echo_count, 1,
        "the terminal echo is deduplicated against the recorded prompt"
    );

    // 4. Chrome/status never becomes user output (AC3).
    assert!(
        blocks
            .iter()
            .all(|b| b["kind"] != "user" || b["text"] == PROMPT_TEXT),
        "no other user block may exist: {blocks:?}"
    );
}

#[tokio::test]
async fn unprovenanced_direct_input_stays_unknown() {
    let h = harness();
    // NO prompt dispatched: every line in this snapshot lacks provenance.
    h.adapter.knows("herdr:abc").tail(vec![
        "fix the flaky test by hand".into(),
        "".into(),
        "status: working · esc to interrupt".into(),
    ]);

    let (_status, value) = post(
        &h.app,
        h.body(
            "req-tail",
            Capability::ReadTail,
            "herdr:abc",
            json!({ "kind": "read_tail", "lines": 200 }),
        ),
    )
    .await;

    let blocks = value["result"]["blocks"].as_array().expect("blocks array");
    assert!(
        blocks.iter().all(|b| b["kind"] != "user"),
        "direct terminal input without provenance is never rendered as user: {blocks:?}"
    );
    assert!(
        blocks.iter().any(|b| b["kind"] == "unknown"),
        "unprovenanced input is preserved as unknown: {blocks:?}"
    );
}

#[tokio::test]
async fn prompt_provenance_is_scoped_per_target() {
    let h = harness();
    h.adapter.knows("herdr:abc").knows("herdr:other");
    h.adapter
        .knows("herdr:other")
        .tail(vec![ECHO_LINE.to_string()]);

    let (status, value) = post(
        &h.app,
        h.body(
            "req-p",
            Capability::Prompt,
            "herdr:abc",
            json!({ "kind": "prompt", "text": PROMPT_TEXT }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true);

    // The same text echoed on a DIFFERENT target must not be deduplicated
    // into a user block: provenance is scoped per (request, target).
    let (_status, value) = post(
        &h.app,
        h.body(
            "req-t",
            Capability::ReadTail,
            "herdr:other",
            json!({ "kind": "read_tail", "lines": 200 }),
        ),
    )
    .await;
    let blocks = value["result"]["blocks"].as_array().expect("blocks array");
    assert!(
        blocks.iter().all(|b| b["kind"] != "user"),
        "another target's echo has no recorded provenance here: {blocks:?}"
    );
}

#[tokio::test]
async fn read_tail_result_still_serves_lines_additively() {
    let h = harness();
    h.adapter.knows("herdr:abc").tail(vec!["plain".into()]);

    let (_status, value) = post(
        &h.app,
        h.body(
            "req-t",
            Capability::ReadTail,
            "herdr:abc",
            json!({ "kind": "read_tail", "lines": 50 }),
        ),
    )
    .await;

    // Backward compatibility: the legacy `lines` surface is untouched.
    assert_eq!(value["result"]["lines"], json!(["plain"]));
    assert!(
        value["result"]["blocks"].is_array(),
        "blocks remain additive alongside lines"
    );
}
