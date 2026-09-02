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

/// Every capability the daemon still serves (#354 read-only cut); the
/// echo-attribution fixtures record provenance events DIRECTLY (the prompt
/// dispatch path itself is gone).
const ALL_CAPABILITIES: [Capability; 2] = [Capability::ReadTail, Capability::ReadDiff];

/// Real W3 auth plane over a temp dir + a registered, fully-granted device,
/// exactly like tests/drive.rs, so every request is genuinely signed.
struct Harness {
    adapter: Arc<TailOnlyAdapter>,
    auth: Arc<AuthPlane>,
    signing: SigningKey,
    pubkey: [u8; 32],
    app: Router,
    /// #330: the store carries the structured exchange ledger; tests record
    /// agent-side events into it exactly like the herdr adapter does.
    store: Store,
    /// The ledger the fixtures record prompt events into (shared with the
    /// router state, like the drive handler used to write it).
    provenance: Arc<corrald::core::provenance::PromptProvenance>,
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
    let env = test_support::envelope("bootstrap", Capability::ReadTail, "bootstrap");
    let token = auth.registry.registration_token();
    let signed = test_support::signed(&auth.registry, &token, &signing, pubkey, &env);
    auth.registry
        .set_grants(&signed.key_id, ALL_CAPABILITIES.to_vec())
        .expect("grants");
    let provenance = Arc::new(corrald::core::provenance::PromptProvenance::new());
    let app = router(AppState {
        store: store.clone(),
        auth: auth.clone(),
        adapter: adapter.clone(),
        replay: Arc::new(ReplayTable::default()),
        issues: Arc::new(corrald::api::issues::IssuesCache::default()),
        provenance: provenance.clone(),
        cors_origins: Vec::new(),
    });
    Harness {
        adapter,
        auth,
        signing,
        pubkey,
        app,
        store,
        provenance,
        _dir: dir,
    }
}

impl Harness {
    /// Record a prompt provenance event DIRECTLY (#354: the dispatch path is
    /// gone) and return a signed `read_tail` request body, so the fixtures
    /// exercise the same attribution pipeline over the kept read plane.
    fn body(
        &self,
        request_id: &str,
        capability: Capability,
        target: &str,
        payload: Value,
    ) -> String {
        assert_eq!(
            capability,
            Capability::ReadTail,
            "post-cut fixtures only send read_tail over the wire"
        );
        // The fixtures carry the prompt text in a `{"kind":"prompt",
        // "text":...}`-shaped payload; #354 removed that wire kind, so the
        // text now goes ONLY into the provenance ledger (PromptEvent::new
        // redacts before hashing) while the signed drive reads the tail.
        let text = payload
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.provenance
            .record(corrald::core::provenance::PromptEvent::new(
                request_id,
                target,
                text,
                corrald::core::util::now_millis(),
            ));
        let envelope = corrald::drive::DriveEnvelope {
            request_id: request_id.to_string(),
            capability,
            target: target.to_string(),
            payload: serde_json::json!({ "kind": "read_tail", "lines": 200 }),
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
            Capability::ReadTail,
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

/// #315 R2 F1: the realistic TUI shape — the transcript echo AND a duplicate
/// composer row of the same text. The recorded event must bind to EXACTLY
/// ONE eligible echo: one `user` block, one occurrence of the text total.
/// A pure `find_by_text` read would stamp both lines with the same
/// request id.
#[tokio::test]
async fn duplicate_echo_renders_the_prompt_exactly_once() {
    let h = harness();
    let mut lines = generic_snapshot_lines();
    // The composer still holds the same prompt: a second eligible echo.
    lines.push(String::new());
    lines.push(format!("› {PROMPT_TEXT}"));
    h.adapter.knows("herdr:abc").tail(lines);

    let (_status, value) = post(
        &h.app,
        h.body(
            "req-prompt",
            Capability::ReadTail,
            "herdr:abc",
            json!({ "kind": "prompt", "text": PROMPT_TEXT }),
        ),
    )
    .await;
    assert_eq!(value["ok"], true, "prompt dispatch: {value}");

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
    let user_blocks: Vec<&Value> = blocks.iter().filter(|b| b["kind"] == "user").collect();
    assert_eq!(
        user_blocks.len(),
        1,
        "echo + duplicate composer echo must yield exactly one user block: {blocks:?}"
    );
    assert_eq!(user_blocks[0]["prompt_request_id"], "req-prompt");
    // Exactly-once: NO other block carries the prompt's provenance, and the
    // duplicate composer echo is demoted (unknown) rather than re-stamped.
    assert!(
        blocks
            .iter()
            .all(|b| b["kind"] != "user" || b["prompt_request_id"] == "req-prompt"),
        "a second user block must not exist for the duplicate echo: {blocks:?}"
    );
    let stamped: Vec<&Value> = blocks
        .iter()
        .filter(|b| !b["prompt_request_id"].is_null())
        .collect();
    assert_eq!(
        stamped.len(),
        1,
        "the recorded event binds to exactly one echo per read: {blocks:?}"
    );
}

/// #315 R2 F2: a prompt recorded long ago plus LATER unprefixed machine
/// output that happens to equal it must stay unknown — no `user` block, no
/// request id. Content identity alone is not attribution; the echo must be
/// a structurally eligible typed-input echo.
#[tokio::test]
async fn unprefixed_model_output_matching_an_old_prompt_is_never_user() {
    let h = harness();
    h.adapter.knows("herdr:abc").tail(vec![
        "Should I proceed with the destructive migration?".into(),
        "yes".into(),
        "".into(),
        "done".into(),
    ]);

    let (status, value) = post(
        &h.app,
        h.body(
            "req-old",
            Capability::ReadTail,
            "herdr:abc",
            json!({ "kind": "read_tail", "lines": 200 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true, "the prompt dispatched (recorded)");

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
        "unprefixed model output equal to an old prompt is never user: {blocks:?}"
    );
    assert!(
        blocks.iter().all(|b| b["prompt_request_id"].is_null()),
        "no block may carry the old request id: {blocks:?}"
    );
}

/// #315 R2 F3: two identical prompts dispatched in order, two eligible
/// echoes in order — each echo binds to its OWN event in ledger order
/// (req-A then req-B), never both to the newest.
#[tokio::test]
async fn repeated_identical_prompts_keep_their_own_request_ids() {
    let h = harness();
    h.adapter.knows("herdr:abc").tail(vec![
        "> continue".into(),
        "".into(),
        "working".into(),
        "".into(),
        "> continue".into(),
    ]);

    for (rid, text) in [("req-A", "continue"), ("req-B", "continue")] {
        let (_status, value) = post(
            &h.app,
            h.body(
                rid,
                Capability::ReadTail,
                "herdr:abc",
                json!({ "kind": "prompt", "text": text }),
            ),
        )
        .await;
        assert_eq!(value["ok"], true, "{rid} dispatch: {value}");
    }

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
    let user_ids: Vec<&str> = blocks
        .iter()
        .filter(|b| b["kind"] == "user")
        .filter_map(|b| b["prompt_request_id"].as_str())
        .collect();
    assert_eq!(
        user_ids,
        vec!["req-A", "req-B"],
        "identical echoes bind one-to-one in ledger order, oldest event first: {blocks:?}"
    );
}

/// #315 R2 F5: a dispatched prompt containing a secret records its
/// REDACTED identity (the read path redacts before hashing, so the record
/// must cover the same redacted bytes), so the echo still renders exactly
/// once as redacted `user` with its request id. The raw secret enters
/// neither blocks nor the ledger.
#[tokio::test]
async fn secret_prompt_keeps_provenance_and_never_leaks_raw_text() {
    let h = harness();
    // The REAL read pipeline redacts BEFORE segmentation (D9, at the
    // adapter boundary): the tail carries the redacted echo only. Serving
    // the raw secret here would bypass that boundary entirely.
    h.adapter.knows("herdr:abc").tail(vec![
        format!("> deploy with token {}", corrald::core::redact::REDACTED),
        "".into(),
        "deploying".into(),
    ]);

    let raw = "deploy with token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef123456";
    let (status, value) = post(
        &h.app,
        h.body(
            "req-secret",
            Capability::ReadTail,
            "herdr:abc",
            json!({ "kind": "prompt", "text": raw }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true, "secret prompt dispatch: {value}");

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
    let result_json = value["result"].to_string();
    assert!(
        !result_json.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef123456"),
        "the raw secret must never appear in the read result: {result_json}"
    );
    let blocks = value["result"]["blocks"].as_array().expect("blocks array");
    let user_blocks: Vec<&Value> = blocks.iter().filter(|b| b["kind"] == "user").collect();
    assert_eq!(
        user_blocks.len(),
        1,
        "the redacted echo still renders exactly once as user: {blocks:?}"
    );
    assert_eq!(user_blocks[0]["prompt_request_id"], "req-secret");
    assert!(
        user_blocks[0]["text"]
            .as_str()
            .unwrap_or("")
            .contains(corrald::core::redact::REDACTED),
        "the rendered user text is the redacted form: {blocks:?}"
    );
}

/// #315 R2 F5 (ledger half): the recorded identity covers the REDACTED
/// dispatch text, never the raw secret — checked directly on the ledger
/// store that the drive handler feeds.
#[test]
fn secret_prompt_record_keeps_only_redacted_identity() {
    let ledger = corrald::core::provenance::PromptProvenance::new();
    ledger.record(corrald::core::provenance::PromptEvent::new(
        "req-s",
        "herdr:a",
        "deploy with token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef123456",
        1,
    ));
    // The redacted echo — the only form a client can ever see — matches.
    let redacted_echo = format!("deploy with token {}", corrald::core::redact::REDACTED);
    let bound = ledger.bind_echoes("herdr:a", &[Some(redacted_echo)], 4);
    assert_eq!(
        bound
            .iter()
            .flatten()
            .map(|e| e.request_id.as_str())
            .collect::<Vec<_>>(),
        vec!["req-s"],
        "the recorded identity is the redacted form the read path hashes"
    );
    // The raw form matches nothing and appears nowhere in the ledger.
    let raw = "deploy with token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef123456";
    assert!(ledger.find_by_text("herdr:a", raw).is_none());
    let debug = format!("{ledger:?}");
    assert!(
        !debug.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef123456"),
        "no raw secret anywhere in the ledger: {debug}"
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
            Capability::ReadTail,
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

/// #315 R3 (AC5, load-bearing): the committed golden fixture is the artifact
/// BOTH clients consume (egui `state.rs` loads it; iOS bundles it via
/// project.yml). This test regenerates the daemon's canonical stream for the
/// SAME generic snapshot + recorded Prompt provenance through the REAL
/// production segmenter and compares the FULL serialized block list, in
/// order, to that fixture — so any grouping, kind, text, order, or
/// prompt-request-id drift fails the daemon suite instead of leaving three
/// hand-authored copies agreeing. (First added in R3: the R2 report named
/// this test but it was absent at 86d1b16.)
#[test]
fn daemon_golden_fixture_matches_canonical_blocks() {
    let prov = corrald::core::provenance::PromptProvenance::new();
    prov.record(corrald::core::provenance::PromptEvent::new(
        "req-prompt",
        "herdr:abc",
        PROMPT_TEXT,
        1,
    ));
    let blocks = corrald::core::blocks::canonical_blocks(
        &generic_snapshot_lines(),
        &prov,
        "herdr:abc",
        None,
    );
    let emitted = serde_json::to_value(&blocks).expect("blocks serialize");
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/canonical_stream_golden.json"))
            .expect("committed golden fixture parses");
    assert_eq!(
        emitted, golden,
        "daemon segmentation drifted from the committed golden fixture \
         (both client contracts consume exactly this artifact)"
    );
}

// ---------------------------------------------------------------------------
// #330: the supported live session — a generic, privacy-safe production-path
// fixture representing a live operator/assistant/tool exchange inside a
// terminal snapshot (composer echo + chrome + tool run + unprovenanced
// prose). The canonical stream joins BOTH ledgers (prompt provenance for the
// operator, the structured exchange ledger for the agent's questions), so a
// real session renders a non-empty Conversation without any prose guessing.
// ---------------------------------------------------------------------------

/// The generic live-session snapshot both clients must segment identically.
/// No harness, provider, or model names anywhere; all content is fictional.
fn live_session_lines() -> Vec<String> {
    vec![
        "──────────────────────────────────────".into(),
        "orch-session ❯ ship the canonical transcript stream".into(),
        "".into(),
        "Canonical stream wired end to end.".into(),
        "".into(),
        "Should I proceed with the destructive migration?".into(),
        "".into(),
        "$ cargo build".into(),
        "Compiling corrald v0.1.0".into(),
        "".into(),
        "status: working · esc to interrupt".into(),
        "fix the flaky test by hand".into(),
    ]
}

/// Record the structured agent-side events a blocked session produces, the
/// same way the herdr adapter's `handle_output_matched` does.
fn record_live_exchange(store: &Store, target: &str) {
    store
        .exchange()
        .record(corrald::core::provenance::ExchangeEvent::new(
            "herdr:abc:sha256:q",
            target,
            corrald::core::provenance::ExchangeRole::Assistant,
            "Should I proceed with the destructive migration?",
            2,
        ));
}

#[tokio::test]
async fn live_session_exchange_produces_canonical_conversation_blocks() {
    let h = harness();
    h.adapter.knows("herdr:abc").tail(live_session_lines());

    // 1. The operator dispatched a prompt through the real drive plane.
    let (status, value) = post(
        &h.app,
        h.body(
            "req-prompt",
            Capability::ReadTail,
            "herdr:abc",
            json!({ "kind": "prompt", "text": "ship the canonical transcript stream" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true, "prompt dispatch: {value}");

    // 2. The agent asked a structured question (output_matched → ledger).
    record_live_exchange(&h.store, "herdr:abc");

    // 3. Read the tail: the canonical stream must carry the full exchange.
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
    let kinds: Vec<&str> = blocks
        .iter()
        .map(|b| b["kind"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        kinds,
        vec![
            "system", "user", "unknown", "agent", "tool", "system", "unknown"
        ],
        "a supported live session renders the full exchange: {blocks:?}"
    );
    // The operator's prompt is exactly-once user with its request id.
    let user_blocks: Vec<&Value> = blocks.iter().filter(|b| b["kind"] == "user").collect();
    assert_eq!(user_blocks.len(), 1);
    assert_eq!(user_blocks[0]["prompt_request_id"], "req-prompt");
    assert_eq!(
        user_blocks[0]["text"],
        "ship the canonical transcript stream"
    );
    // The structured question is attributed by the event, never the prose.
    let agent_blocks: Vec<&Value> = blocks.iter().filter(|b| b["kind"] == "agent").collect();
    assert_eq!(agent_blocks.len(), 1);
    assert_eq!(
        agent_blocks[0]["text"],
        "Should I proceed with the destructive migration?"
    );
}

#[tokio::test]
async fn live_session_without_structured_role_source_stays_unknown() {
    // #330 AC7 RED baseline: remove the structured role source (no exchange
    // event recorded) and the SAME window collapses back to Unknown — the
    // regression test must fail against a reverted fixture.
    let h = harness();
    h.adapter.knows("herdr:abc").tail(live_session_lines());

    let (_status, value) = post(
        &h.app,
        h.body(
            "req-prompt",
            Capability::ReadTail,
            "herdr:abc",
            json!({ "kind": "read_tail", "lines": 200 }),
        ),
    )
    .await;
    assert_eq!(value["ok"], true);

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
        blocks.iter().all(|b| b["kind"] != "agent"),
        "without the structured exchange source the question must stay \
         unattributed: {blocks:?}"
    );
    assert!(
        blocks.iter().any(|b| b["kind"] == "unknown" && {
            b["text"] == "Should I proceed with the destructive migration?"
        }),
        "the unprovenanced question line stays honestly unknown: {blocks:?}"
    );
}

/// #330 AC6: the committed golden fixture for the live session — the SAME
/// artifact iOS and egui bundle, byte-asserted against the real production
/// segmenter so daemon drift fails the daemon suite first.
#[test]
fn daemon_live_session_golden_fixture_matches_canonical_blocks() {
    let prov = corrald::core::provenance::PromptProvenance::new();
    prov.record(corrald::core::provenance::PromptEvent::new(
        "req-prompt",
        "herdr:abc",
        "ship the canonical transcript stream",
        1,
    ));
    let exchange = corrald::core::provenance::ExchangeLedger::new();
    exchange.record(corrald::core::provenance::ExchangeEvent::new(
        "herdr:abc:sha256:q",
        "herdr:abc",
        corrald::core::provenance::ExchangeRole::Assistant,
        "Should I proceed with the destructive migration?",
        2,
    ));
    let blocks = corrald::core::blocks::canonical_blocks_with_exchange(
        &live_session_lines(),
        &prov,
        &exchange,
        "herdr:abc",
        None,
    );
    let emitted = serde_json::to_value(&blocks).expect("blocks serialize");
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/live_session_exchange_golden.json"))
            .expect("committed live-session golden fixture parses");
    assert_eq!(
        emitted, golden,
        "live-session segmentation drifted from the committed golden fixture \
         (both client contracts consume exactly this artifact)"
    );
}
