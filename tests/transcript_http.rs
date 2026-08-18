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
        adapter: Arc::new(corrald::api::drive::NoopAdapter),
        replay: Arc::new(ReplayTable::default()),
        transcript_roots: roots,
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
    // F15: the bound session is named, so a client can pin the bind.
    assert_eq!(body["session"], "claude:s1.jsonl");
    // F9: no store failed, and the field says so explicitly.
    assert_eq!(body["stores_unavailable"], json!([]));
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

/// F3: every SERVED page appends one audit entry (same shape as drive's
/// read_tail); auth failures append nothing (AC5).
#[tokio::test]
async fn served_pages_are_audited_and_auth_failures_are_not() {
    let h = harness();
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;
    write_claude_session(&h, "s1", &[claude_line("assistant", "hello")]);

    let before = h.auth.audit.chain().0.len();

    // Auth failure: no header → nothing audited.
    let (status, _) = get(&h.app, "/transcript?agent=herdr:a1", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        h.auth.audit.chain().0.len(),
        before,
        "AC5: failures unaudited"
    );

    // Served page: exactly one entry, read_tail capability, agent target.
    let header = h.auth_header(Capability::ReadTail, "herdr:a1");
    let (status, _) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::OK);
    let chain = h.auth.audit.chain().0;
    assert_eq!(chain.len(), before + 1, "one audit entry per served page");
    let last = chain.last().expect("entry");
    assert_eq!(last.capability, "read_tail");
    assert_eq!(last.target, "herdr:a1");
}

/// F4: the access-controlled GET must never be cacheable — no-store and
/// Vary on the credential header ride every response, success and error.
#[tokio::test]
async fn responses_carry_no_store_and_vary() {
    let h = harness();
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;
    write_claude_session(&h, "s1", &[claude_line("assistant", "hello")]);

    let header = h.auth_header(Capability::ReadTail, "herdr:a1");
    for (uri, auth) in [
        ("/transcript?agent=herdr:a1", Some(header.as_str())),
        ("/transcript?agent=herdr:a1", None), // error path
    ] {
        let mut request = Request::get(uri);
        if let Some(value) = auth {
            request = request.header(TRANSCRIPT_AUTH_HEADER, value);
        }
        let res = h
            .app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            res.headers()
                .get("cache-control")
                .map(|v| v.to_str().unwrap()),
            Some("no-store"),
            "{uri} auth={}",
            auth.is_some()
        );
        assert_eq!(
            res.headers().get("vary").map(|v| v.to_str().unwrap()),
            Some("x-corral-drive"),
            "{uri} auth={}",
            auth.is_some()
        );
    }
}

/// F5: a cursor issued against one session must not silently continue in
/// another after a rebind — through HTTP, the stale cursor is a typed
/// 400 bad_cursor once a newer session becomes the bind target.
#[tokio::test]
async fn stale_cursor_after_rebind_is_refused_not_continued() {
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
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1&limit=1", Some(&header)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let cursor = body["next_cursor"].as_str().expect("cursor").to_string();

    // A NEW session appears in the worktree and becomes newest (mtime
    // forced into the future so the rebind is deterministic).
    let p2 = write_claude_session(&h, "s2", &[claude_line("assistant", "fresh session")]);
    let touched = std::process::Command::new("touch")
        .args(["-t", "203712312359.00"])
        .arg(&p2)
        .status()
        .expect("touch runs");
    assert!(touched.success());

    let (status, body) = get(
        &h.app,
        &format!("/transcript?agent=herdr:a1&limit=1&cursor={cursor}"),
        Some(&header),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["kind"], "bad_cursor",
        "stale cursor must not read s2 at s1's offset"
    );
}

/// F12: query values that fail to parse keep the JSON error contract —
/// no axum plaintext 400s. Also pins the limit clamp through HTTP.
#[tokio::test]
async fn query_parsing_keeps_the_error_contract_and_limit_clamps() {
    let h = harness();
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;
    write_claude_session(
        &h,
        "s1",
        &[claude_line("user", "one"), claude_line("assistant", "two")],
    );
    let header = h.auth_header(Capability::ReadTail, "herdr:a1");

    // Bad limit: typed JSON bad_request, not plaintext.
    let (status, body) = get(
        &h.app,
        "/transcript?agent=herdr:a1&limit=abc",
        Some(&header),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["kind"], "bad_request", "{body}");

    // Missing agent entirely: same contract.
    let (status, body) = get(&h.app, "/transcript", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["kind"], "bad_request", "{body}");

    // limit=0 clamps to one entry; a huge limit clamps to the page cap
    // and returns everything present.
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1&limit=0", Some(&header)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["entries"].as_array().unwrap().len(), 1, "{body}");
    let (status, body) = get(
        &h.app,
        "/transcript?agent=herdr:a1&limit=999999999",
        Some(&header),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["entries"].as_array().unwrap().len(), 2, "{body}");
}

/// F17: an unregistered key with a well-formed envelope reaches the
/// verifier and maps to 404 unknown_key (the AC1 mapping), proving the
/// pre-verify capability/target checks don't shadow auth classification.
#[tokio::test]
async fn unregistered_key_is_unknown_key_not_bad_request() {
    let h = harness();
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;

    let (signing, _pubkey) = test_support::keypair();
    let envelope = DriveEnvelope {
        request_id: "req-x".to_string(),
        capability: Capability::ReadTail,
        target: "herdr:a1".to_string(),
        payload: json!({}),
        rev: None,
    };
    let signed = corrald::drive::SignedDrive {
        key_id: "dev_never_registered".to_string(),
        signature: test_support::sign(&signing, &envelope),
        envelope,
    };
    let header = serde_json::to_string(&signed).expect("serializes");
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["kind"], "unknown_key");
}

/// F17: the opencode path end-to-end through HTTP — direct session-id
/// binding (agent id carries the opencode session id), sqlite3-CLI read,
/// oc-cursor paging, redaction. Skips cleanly without sqlite3.
#[tokio::test]
async fn opencode_end_to_end_via_direct_session_id() {
    if std::process::Command::new("sqlite3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("sqlite3 not on PATH; skipping");
        return;
    }
    let h = harness();
    let secret = format!("ghp_{}", "Ab129Zz4".repeat(5));
    let seed = format!(
        r#"
CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT);
CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, role TEXT, data TEXT);
CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, type TEXT, data TEXT);
INSERT INTO session VALUES ('ses_fix', '{wt}');
INSERT INTO message VALUES ('m1', 'ses_fix', 100, 'user', '{{"role":"user"}}');
INSERT INTO message VALUES ('m2', 'ses_fix', 200, 'assistant', '{{"role":"assistant"}}');
INSERT INTO part VALUES ('p1', 'm1', 'text', '{{"type":"text","text":"question"}}');
INSERT INTO part VALUES ('p2', 'm2', 'text', '{{"type":"text","text":"answer with {secret}"}}');
"#,
        wt = h.worktree,
    );
    let status = std::process::Command::new("sqlite3")
        .arg(h._stores_dir.path().join("opencode.db"))
        .arg(&seed)
        .status()
        .expect("sqlite3 runs");
    assert!(status.success());

    // The agent id CARRIES the opencode session id — the direct rung
    // binds it with no worktree heuristic in play (F1).
    h.seed_agent("herdr:ses_fix", Some(&h.worktree)).await;
    h.store
        .apply(Change::upsert({
            let mut a = h.store.get("herdr:ses_fix").await.expect("seeded");
            a.tool = "opencode".to_string();
            a.seq = 2;
            a
        }))
        .await;

    let header = h.auth_header(Capability::ReadTail, "herdr:ses_fix");
    let (status, body) = get(
        &h.app,
        "/transcript?agent=herdr:ses_fix&limit=1",
        Some(&header),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["store"], "opencode");
    assert_eq!(body["session"], "opencode:ses_fix");
    let entries = body["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["role"], "assistant", "newest first");
    assert!(
        !body.to_string().contains(&secret),
        "redaction end-to-end on the opencode path"
    );

    // Page 2 via the oc cursor: the older message, then exhaustion.
    let cursor = body["next_cursor"].as_str().expect("cursor").to_string();
    let (status, body) = get(
        &h.app,
        &format!("/transcript?agent=herdr:ses_fix&limit=1&cursor={cursor}"),
        Some(&header),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entries"][0]["text"], "question");
}
