//! #63 conformance: `GET /transcript` — grant gate, page shape,
//! end-to-end redaction, ambiguity, cursor errors. Same in-process
//! oneshot style as tests/drive.rs; all stores are fixtures under temp
//! dirs (never this machine's live session stores).

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::SigningKey;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use corrald::api::drive::ReplayTable;
use corrald::api::transcript::TRANSCRIPT_AUTH_HEADER;
use corrald::api::{AppState, router};
use corrald::auth::{AuthPlane, test_support};
use corrald::core::model::{Agent, AgentState, Change};
use corrald::core::store::Store;
use corrald::drive::{Capability, DriveEnvelope};
use corrald::transcript::bind::{TranscriptRoots, encode_claude_project_dir};

struct Harness {
    store: Store,
    auth: Arc<AuthPlane>,
    signing: SigningKey,
    pubkey: [u8; 32],
    app: Router,
    /// The worktree path agents in this harness claim to run in.
    worktree: String,
    _auth_dir: tempfile::TempDir,
    _stores_dir: tempfile::TempDir,
}

impl Harness {
    /// A signed `x-corral-drive` header value from the granted device.
    fn auth_header(&self, capability: Capability, target: &str) -> String {
        self.auth_header_from(&self.signing, self.pubkey, capability, target)
    }

    fn auth_header_from(
        &self,
        signing: &SigningKey,
        pubkey: [u8; 32],
        capability: Capability,
        target: &str,
    ) -> String {
        let envelope = DriveEnvelope {
            request_id: "req-transcript".to_string(),
            capability,
            target: target.to_string(),
            payload: json!({}),
            rev: None,
        };
        let token = self.auth.registry.registration_token();
        let signed = test_support::signed(&self.auth.registry, &token, signing, pubkey, &envelope);
        serde_json::to_string(&signed).expect("signed header serializes")
    }

    fn register_other_device(&self, grants: &[Capability]) -> (SigningKey, [u8; 32]) {
        let (signing, pubkey) = test_support::keypair();
        let env = test_support::envelope("bootstrap-other", Capability::Prompt, "bootstrap");
        let token = self.auth.registry.registration_token();
        let signed = test_support::signed(&self.auth.registry, &token, &signing, pubkey, &env);
        self.auth
            .registry
            .set_grants(&signed.key_id, grants.to_vec())
            .expect("set grants");
        (signing, pubkey)
    }

    async fn seed_agent(&self, id: &str, worktree: Option<&str>) {
        let mut agent = Agent {
            agent_id: id.to_string(),
            source: "herdr".to_string(),
            tool: "claude".to_string(),
            state: AgentState::Working,
            reason: None,
            seq: 1,
            ts: 0,
            capabilities: vec![],
            waiting_on: None,
            cost: None,
            parent_id: None,
            host: None,
            workspace: Default::default(),
            attachment: None,
            display_name: None,
            title: None,
        };
        agent.workspace.worktree_path = worktree.map(str::to_string);
        self.store.apply(Change::upsert(agent)).await;
    }
}

fn harness() -> Harness {
    let store = Store::new();
    let coalescer = store.clone();
    std::mem::drop(tokio::spawn(async move { coalescer.run_coalescer().await }));

    let auth_dir = tempfile::tempdir().expect("auth tempdir");
    let auth =
        Arc::new(AuthPlane::load_or_create(auth_dir.path().to_path_buf()).expect("auth plane"));
    let (signing, pubkey) = test_support::keypair();
    let env = test_support::envelope("bootstrap", Capability::Prompt, "bootstrap");
    let token = auth.registry.registration_token();
    let signed = test_support::signed(&auth.registry, &token, &signing, pubkey, &env);
    auth.registry
        .set_grants(&signed.key_id, vec![Capability::ReadTail])
        .expect("grants");

    let stores_dir = tempfile::tempdir().expect("stores tempdir");
    let worktree = stores_dir
        .path()
        .join("wt/repo")
        .to_string_lossy()
        .into_owned();
    let roots = TranscriptRoots {
        opencode_db: stores_dir.path().join("opencode.db"),
        claude_dir: stores_dir.path().join("claude-projects"),
        codex_dir: stores_dir.path().join("codex-sessions"),
    };

    let app = router(AppState {
        store: store.clone(),
        auth: auth.clone(),
        replay: Arc::new(ReplayTable::default()),
        transcript_roots: roots,
        ..Default::default()
    });
    Harness {
        store,
        auth,
        signing,
        pubkey,
        app,
        worktree,
        _auth_dir: auth_dir,
        _stores_dir: stores_dir,
    }
}

/// Write a claude-shaped session jsonl into the harness's claude root for
/// its worktree; returns the file path.
fn write_claude_session(h: &Harness, name: &str, lines: &[String]) -> std::path::PathBuf {
    let project = h
        ._stores_dir
        .path()
        .join("claude-projects")
        .join(encode_claude_project_dir(&h.worktree));
    std::fs::create_dir_all(&project).expect("mkdir project");
    let path = project.join(format!("{name}.jsonl"));
    std::fs::write(&path, lines.join("\n") + "\n").expect("write session");
    path
}

fn claude_line(role: &str, text: &str) -> String {
    json!({
        "type": role,
        "message": { "role": role, "content": [{ "type": "text", "text": text }] }
    })
    .to_string()
}

async fn get(app: &Router, uri: &str, auth_header: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::get(uri);
    if let Some(value) = auth_header {
        request = request.header(TRANSCRIPT_AUTH_HEADER, value);
    }
    let res = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn refused_without_header_and_without_grant() {
    let h = harness();
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;
    write_claude_session(&h, "s1", &[claude_line("assistant", "hello")]);

    // No header at all: missing_signature, nothing leaks.
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["kind"], "missing_signature");

    // A registered device WITHOUT the read_tail grant: 403 not_granted —
    // the same trust decision as the drive plane's read_tail.
    let (other_signing, other_pubkey) = h.register_other_device(&[Capability::Prompt]);
    let header = h.auth_header_from(
        &other_signing,
        other_pubkey,
        Capability::ReadTail,
        "herdr:a1",
    );
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["kind"], "not_granted");
}

#[tokio::test]
async fn envelope_must_carry_read_tail_for_the_queried_agent() {
    let h = harness();
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;

    // Wrong capability in the envelope (even though the device holds
    // read_tail): typed refusal, not a silent accept.
    let header = h.auth_header(Capability::Prompt, "herdr:a1");
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["kind"], "bad_request");

    // Target mismatch: a signature minted for agent A cannot page agent B.
    let header = h.auth_header(Capability::ReadTail, "herdr:other");
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["kind"], "bad_request");
}

#[tokio::test]
async fn page_shape_is_stable_and_redacted_end_to_end() {
    let h = harness();
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;
    let secret = format!("sk-ant-api03-{}", "AbCd1234".repeat(12));
    write_claude_session(
        &h,
        "s1",
        &[
            claude_line("user", "please deploy"),
            claude_line("assistant", &format!("using key {secret} now")),
        ],
    );

    let header = h.auth_header(Capability::ReadTail, "herdr:a1");
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Stable page shape (golden keys, newest first).
    assert_eq!(body["agent"], "herdr:a1");
    assert_eq!(body["store"], "claude");
    assert_eq!(body["skipped"], 0);
    assert!(body["next_cursor"].is_null(), "tiny store fits one page");
    let entries = body["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["role"], "assistant", "newest first");
    assert_eq!(entries[1]["role"], "user");
    assert_eq!(entries[1]["text"], "please deploy");
    for entry in entries {
        assert!(entry.get("ts").is_some(), "ts key always present");
    }

    // Redaction end-to-end THROUGH the HTTP path: the seeded secret never
    // appears anywhere in the response body.
    let raw = body.to_string();
    assert!(!raw.contains(&secret), "secret leaked through /transcript");
    assert!(
        entries[0]["text"].as_str().unwrap().contains("now"),
        "non-secret text survives"
    );
}

#[tokio::test]
async fn paging_via_opaque_cursor_walks_without_gaps_or_duplicates() {
    let h = harness();
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;
    write_claude_session(
        &h,
        "s1",
        &[
            claude_line("user", "one"),
            claude_line("assistant", "two"),
            claude_line("user", "three"),
        ],
    );

    let header = h.auth_header(Capability::ReadTail, "herdr:a1");
    let mut seen = Vec::new();
    let mut uri = "/transcript?agent=herdr:a1&limit=1".to_string();
    for _ in 0..4 {
        let (status, body) = get(&h.app, &uri, Some(&header)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        for e in body["entries"].as_array().expect("entries") {
            seen.push(e["text"].as_str().expect("text").to_string());
        }
        match body["next_cursor"].as_str() {
            Some(cursor) => uri = format!("/transcript?agent=herdr:a1&limit=1&cursor={cursor}"),
            None => break,
        }
    }
    assert_eq!(seen, vec!["three", "two", "one"], "newest-first, complete");
}

#[tokio::test]
async fn ambiguous_binding_returns_candidate_list_never_a_guess() {
    let h = harness();
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;
    let p1 = write_claude_session(&h, "s1", &[claude_line("assistant", "alpha")]);
    let p2 = write_claude_session(&h, "s2", &[claude_line("assistant", "beta")]);
    // Force an exact recency tie (mtime granularity would otherwise make
    // this flaky-pass): same explicit timestamp on both.
    let touch = std::process::Command::new("touch")
        .args(["-t", "202601011200.00"])
        .arg(&p1)
        .arg(&p2)
        .status()
        .expect("touch runs");
    assert!(touch.success());

    let header = h.auth_header(Capability::ReadTail, "herdr:a1");
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["kind"], "ambiguous_session");
    let candidates = body["candidates"].as_array().expect("candidate list");
    assert_eq!(candidates.len(), 2);
    for candidate in candidates {
        assert!(candidate["label"].as_str().unwrap().starts_with("claude:"));
        assert!(candidate.get("recency_ms").is_some());
    }
}

#[tokio::test]
async fn typed_misses_bad_cursor_unknown_agent_no_session() {
    let h = harness();
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;
    h.seed_agent("herdr:homeless", None).await;
    write_claude_session(&h, "s1", &[claude_line("assistant", "hello")]);

    // Bad cursor: 400 bad_cursor, refused before any store IO.
    let header = h.auth_header(Capability::ReadTail, "herdr:a1");
    let (status, body) = get(
        &h.app,
        "/transcript?agent=herdr:a1&cursor=garbage",
        Some(&header),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["kind"], "bad_cursor");

    // Unknown agent: 404, distinct kind.
    let header = h.auth_header(Capability::ReadTail, "herdr:nope");
    let (status, body) = get(&h.app, "/transcript?agent=herdr:nope", Some(&header)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["kind"], "unknown_agent");

    // Known agent, no worktree: 404 no_session.
    let header = h.auth_header(Capability::ReadTail, "herdr:homeless");
    let (status, body) = get(&h.app, "/transcript?agent=herdr:homeless", Some(&header)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["kind"], "no_session");

    // Known agent, worktree with no session store: 404 no_session too.
    h.seed_agent("herdr:elsewhere", Some("/nowhere/at/all"))
        .await;
    let header = h.auth_header(Capability::ReadTail, "herdr:elsewhere");
    let (status, body) = get(&h.app, "/transcript?agent=herdr:elsewhere", Some(&header)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["kind"], "no_session");
}
