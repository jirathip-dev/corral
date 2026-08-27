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
    /// Fresh-review N1/F8: this harness's own per-daemon limiter (clone
    /// of the one inside `app`), so the busy test can occupy the gate.
    limiter: corrald::api::transcript::TranscriptLimiter,
    _auth_dir: tempfile::TempDir,
    _stores_dir: tempfile::TempDir,
}

impl Harness {
    /// A signed `x-corral-drive` header value from the granted device:
    /// the newest page, default limit (fresh review F3 — page params
    /// live in the SIGNED payload, not the query string).
    fn auth_header(&self, capability: Capability, target: &str) -> String {
        self.auth_header_page(capability, target, None, None)
    }

    /// Fresh review F3: the page parameters are part of the signed
    /// envelope payload — `ts` (default: now), `cursor`, `limit` — so
    /// ONE signature buys exactly ONE page. Paging re-signs per page
    /// with the new cursor.
    fn auth_header_page(
        &self,
        capability: Capability,
        target: &str,
        cursor: Option<&str>,
        limit: Option<u64>,
    ) -> String {
        self.auth_header_signed(capability, target, "req-transcript", cursor, limit, None)
    }

    /// Like [`Harness::auth_header_page`] with an explicit `ts` (stale /
    /// future-window tests, fresh review F3).
    fn auth_header_page_ts(
        &self,
        capability: Capability,
        target: &str,
        cursor: Option<&str>,
        limit: Option<u64>,
        ts: u64,
    ) -> String {
        self.auth_header_signed(
            capability,
            target,
            "req-transcript",
            cursor,
            limit,
            Some(ts),
        )
    }

    /// Raw-payload variant (request_id / malformed-payload tests).
    fn auth_header_payload(
        &self,
        capability: Capability,
        target: &str,
        request_id: &str,
        payload: Value,
    ) -> String {
        let envelope = DriveEnvelope {
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
        serde_json::to_string(&signed).expect("signed header serializes")
    }

    fn auth_header_signed(
        &self,
        capability: Capability,
        target: &str,
        request_id: &str,
        cursor: Option<&str>,
        limit: Option<u64>,
        ts: Option<u64>,
    ) -> String {
        let envelope = DriveEnvelope {
            request_id: request_id.to_string(),
            capability,
            target: target.to_string(),
            payload: json!({
                "ts": ts.unwrap_or_else(corrald::auth::registry::now_secs),
                "cursor": cursor,
                "limit": limit,
            }),
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
        serde_json::to_string(&signed).expect("signed header serializes")
    }

    fn auth_header_from(
        &self,
        signing: &SigningKey,
        pubkey: [u8; 32],
        capability: Capability,
        target: &str,
    ) -> String {
        // Other-device tests: rejected pre-verify (capability) or at
        // verify (grant), so the payload shape is irrelevant — but keep
        // it honest with a ts.
        let envelope = DriveEnvelope {
            request_id: "req-transcript".to_string(),
            capability,
            target: target.to_string(),
            payload: json!({ "ts": corrald::auth::registry::now_secs() }),
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
    harness_with_limiter(corrald::api::transcript::TranscriptLimiter::default())
}

/// Fresh-review F8/N1: a harness with a chosen concurrency cap, so the
/// `busy` 503 path is reachable deterministically (the default cap is
/// per-harness and never contended across tests).
fn harness_with_permits(permits: usize) -> Harness {
    harness_with_limiter(corrald::api::transcript::TranscriptLimiter::new(permits))
}

fn harness_with_limiter(limiter: corrald::api::transcript::TranscriptLimiter) -> Harness {
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
        issues: Arc::new(corrald::api::issues::IssuesCache::default()),
        transcript_roots: roots,
        transcript_limiter: limiter.clone(),
        role_probe_memo: corrald::transcript::RoleProbeMemo::default(),
        fleets: Arc::new(corrald::fleet::cli::CliFleetOpsProvider),
    });
    Harness {
        store,
        auth,
        signing,
        pubkey,
        app,
        worktree,
        limiter,
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
    // R1: worktree rung declared as best-effort provenance ("a1" is not
    // a session id, so the heuristic answered).
    assert_eq!(body["bind"], "worktree");
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

    // #167: blocks are segmented ONCE server-side and ride ADDITIVELY
    // alongside entries (egui still reads entries until #168; the block
    // renderer reads blocks). Order mirrors the newest-first entries.
    let blocks = body["blocks"].as_array().expect("blocks array");
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["kind"], "agent", "assistant entry -> agent block");
    assert!(
        blocks[0]["text"].as_str().unwrap().contains("now"),
        "block text is the cleaned entry text"
    );
    assert_eq!(blocks[1]["kind"], "user", "user entry -> user block");
    assert_eq!(blocks[1]["text"], "please deploy");
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

    // Fresh review F3: the page parameters are signed into the header,
    // so paging RE-SIGNS per page with the new cursor — one signature
    // buys exactly one page. The URL only carries the agent.
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..4 {
        let header =
            h.auth_header_page(Capability::ReadTail, "herdr:a1", cursor.as_deref(), Some(1));
        let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        for e in body["entries"].as_array().expect("entries") {
            seen.push(e["text"].as_str().expect("text").to_string());
        }
        match body["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
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

    // Structurally bad cursor in the SIGNED payload: 400 bad_cursor,
    // refused BEFORE the bind (fresh review F7 — structural validation
    // needs no store; only a fingerprint mismatch costs a bind first).
    let header = h.auth_header_page(Capability::ReadTail, "herdr:a1", Some("garbage"), None);
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
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
    assert_eq!(
        last.capability, "read_tail:transcript",
        "distinguishable from a bounded /drive read_tail entry (R4)"
    );
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

/// F5 (round-1) + fresh-review F5 (efficiency half): the cursor is
/// fingerprinted to the session FILE it was issued for, and the bind is
/// memoized for the life of the page sequence (fingerprint-gated). So a
/// NEW session becoming newest mid-sequence does NOT invalidate the
/// cursor: it keeps paging the file it was issued for — never a silent
/// continuation in a DIFFERENT file — and the next cursor-less request
/// picks the new newest session.
#[tokio::test]
async fn cursor_keeps_paging_its_file_across_a_rebind() {
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

    let header = h.auth_header_page(Capability::ReadTail, "herdr:a1", None, Some(1));
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["session"], "claude:s1.jsonl");
    let cursor = body["next_cursor"].as_str().expect("cursor").to_string();

    // A NEW session appears in the worktree and becomes newest (mtime
    // forced into the future so a fresh bind would now pick s2).
    let p2 = write_claude_session(&h, "s2", &[claude_line("assistant", "fresh session")]);
    let touched = std::process::Command::new("touch")
        .args(["-t", "203712312359.00"])
        .arg(&p2)
        .status()
        .expect("touch runs");
    assert!(touched.success());

    // The s1 cursor keeps paging s1 — it was minted for that file.
    let header = h.auth_header_page(Capability::ReadTail, "herdr:a1", Some(&cursor), Some(1));
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["session"], "claude:s1.jsonl",
        "memoized bind: still s1"
    );
    assert_eq!(
        body["entries"][0]["text"], "two",
        "s1's page 2 — the cursor never continues in s2"
    );

    // A fresh cursor-less request (new page sequence) binds the new
    // newest session.
    let header = h.auth_header_page(Capability::ReadTail, "herdr:a1", None, None);
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["session"], "claude:s2.jsonl");
}

/// F12 + fresh-review F3: values that fail to parse keep the JSON error
/// contract — no axum plaintext 400s — and the SIGNED limit clamps
/// through HTTP the same way the old query limit did.
#[tokio::test]
async fn query_parsing_keeps_the_error_contract_and_limit_clamps() {
    let h = harness();
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;
    write_claude_session(
        &h,
        "s1",
        &[claude_line("user", "one"), claude_line("assistant", "two")],
    );

    // Bad signed limit: typed JSON bad_request, not plaintext.
    let header = h.auth_header_payload(
        Capability::ReadTail,
        "herdr:a1",
        "req-badlimit",
        json!({ "ts": corrald::auth::registry::now_secs(), "limit": "abc" }),
    );
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["kind"], "bad_request", "{body}");

    // Old-style query page parameters are refused outright (they moved
    // into the signed header — a URL knob would unbind the page).
    let header = h.auth_header(Capability::ReadTail, "herdr:a1");
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
    let header = h.auth_header_page(Capability::ReadTail, "herdr:a1", None, Some(0));
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["entries"].as_array().unwrap().len(), 1, "{body}");
    let header = h.auth_header_page(Capability::ReadTail, "herdr:a1", None, Some(999_999_999));
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["entries"].as_array().unwrap().len(), 2, "{body}");
}

/// F17: an unregistered key with a well-formed envelope reaches the
/// verifier and maps to 404 unknown_key (the AC1 mapping), proving the
/// pre-verify capability/target checks don't shadow auth classification
/// — and (fresh review F3) neither does the payload parse, which runs
/// AFTER verify: a `{}` payload yields unknown_key here, not a payload
/// error.
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

    let header = h.auth_header_page(Capability::ReadTail, "herdr:ses_fix", None, Some(1));
    let (status, body) = get(&h.app, "/transcript?agent=herdr:ses_fix", Some(&header)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["store"], "opencode");
    assert_eq!(body["session"], "opencode:ses_fix");
    assert_eq!(body["bind"], "session_id", "the direct rung answered (R1)");
    let entries = body["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["role"], "assistant", "newest first");
    assert!(
        !body.to_string().contains(&secret),
        "redaction end-to-end on the opencode path"
    );

    // Page 2 via the oc cursor: the older message, then exhaustion.
    let cursor = body["next_cursor"].as_str().expect("cursor").to_string();
    let header = h.auth_header_page(
        Capability::ReadTail,
        "herdr:ses_fix",
        Some(&cursor),
        Some(1),
    );
    let (status, body) = get(&h.app, "/transcript?agent=herdr:ses_fix", Some(&header)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entries"][0]["text"], "question");
}

/// Fresh review F4: the audit trail is the endpoint's stated replay
/// mitigation, so an unidentifiable request_id is refused exactly as
/// /drive refuses it — and an oversized one too, since the id is copied
/// verbatim into the hash-chained log.
#[tokio::test]
async fn empty_or_oversized_request_id_is_refused() {
    let h = harness();
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;
    write_claude_session(&h, "s1", &[claude_line("assistant", "hello")]);

    let header = h.auth_header_payload(
        Capability::ReadTail,
        "herdr:a1",
        "",
        json!({ "ts": corrald::auth::registry::now_secs() }),
    );
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["kind"], "bad_request");
    assert!(
        body["message"].as_str().unwrap().contains("request_id"),
        "{body}"
    );

    let header = h.auth_header_payload(
        Capability::ReadTail,
        "herdr:a1",
        &"x".repeat(4096),
        json!({ "ts": corrald::auth::registry::now_secs() }),
    );
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["kind"], "bad_request");
}

/// Fresh review F2: a store_unreadable error body must not carry the
/// absolute host store path (or sqlite3 stderr) to the wire — that
/// diagnostic belongs in the daemon log. This is the first surface that
/// serializes TranscriptError onto HTTP, so pin it here.
#[cfg(unix)]
#[tokio::test]
async fn store_unreadable_body_names_no_host_paths() {
    use std::os::unix::fs::PermissionsExt;
    let h = harness();
    // Session-id rung: agent id carries the session, file exists, binds.
    h.seed_agent("herdr:sessionone", Some(&h.worktree)).await;
    let path = write_claude_session(&h, "sessionone", &[claude_line("assistant", "hello")]);
    // Bound but unreadable: bind stats the file, read_page open fails.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");

    let header = h.auth_header(Capability::ReadTail, "herdr:sessionone");
    let (status, body) = get(&h.app, "/transcript?agent=herdr:sessionone", Some(&header)).await;
    // Restore perms so the tempdir can be cleaned up.
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
    if status == StatusCode::OK {
        // Running as root (perms don't bite): the leak path is not
        // reachable in this environment; nothing to assert.
        return;
    }
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["kind"], "store_unreadable");
    let message = body["message"].as_str().expect("message");
    let stores_root = h._stores_dir.path().to_string_lossy().into_owned();
    assert!(
        !message.contains(&stores_root) && !message.contains("claude-projects"),
        "error body leaked a host path: {message}"
    );
    assert!(
        !message.contains(&path.to_string_lossy().into_owned()),
        "error body leaked the session file path: {message}"
    );
}

/// Fresh review F3: the signed `ts` is freshness-checked with the same
/// 60s `|now - ts|` window as `/step-up` and `/device-token` — a
/// captured header goes stale 60 seconds after signing, in both clock
/// directions, and a payload with no ts at all is refused.
#[tokio::test]
async fn stale_or_missing_ts_is_refused() {
    let h = harness();
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;
    write_claude_session(&h, "s1", &[claude_line("assistant", "hello")]);

    // No ts in the signed payload.
    let header = h.auth_header_payload(Capability::ReadTail, "herdr:a1", "req-nots", json!({}));
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["kind"], "bad_request");
    assert!(
        body["message"].as_str().unwrap().contains("ts"),
        "message should name the missing field: {body}"
    );

    let now = corrald::auth::registry::now_secs();

    // Signed too long ago: stale capture.
    let header = h.auth_header_page_ts(Capability::ReadTail, "herdr:a1", None, None, now - 120);
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["kind"], "bad_request");
    assert!(
        body["message"].as_str().unwrap().contains("stale"),
        "message should say why: {body}"
    );

    // Signed in the future beyond the window (clock skew both ways).
    let header = h.auth_header_page_ts(Capability::ReadTail, "herdr:a1", None, None, now + 120);
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["kind"], "bad_request");

    // Inside the window still serves.
    let header = h.auth_header_page(Capability::ReadTail, "herdr:a1", None, None);
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// Fresh review F3: a captured header is pinned to the exact page it was
/// signed for. cursor/limit live in the SIGNED payload, so there is no
/// URL knob to vary the page — query-string cursor/limit are refused
/// outright, replaying the header re-serves that one page, and a
/// different page requires a NEW signature.
#[tokio::test]
async fn captured_header_serves_only_the_page_it_was_signed_for() {
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

    // Signed for page 1 with limit=1.
    let header = h.auth_header_page(Capability::ReadTail, "herdr:a1", None, Some(1));
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entries"].as_array().unwrap().len(), 1);
    assert_eq!(body["entries"][0]["text"], "three");
    let cursor = body["next_cursor"].as_str().expect("cursor").to_string();

    // The same header cannot be pointed at a different page via the URL.
    let (status, body) = get(
        &h.app,
        &format!("/transcript?agent=herdr:a1&cursor={cursor}"),
        Some(&header),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["kind"], "bad_request");
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1&limit=3", Some(&header)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["kind"], "bad_request");

    // Replaying the captured header re-serves its one page (bounded).
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entries"][0]["text"], "three");

    // A NEW signature over the new cursor buys page 2 — the only way to
    // page on.
    let header2 = h.auth_header_page(Capability::ReadTail, "herdr:a1", Some(&cursor), Some(1));
    let (status, body) = get(&h.app, "/transcript?agent=herdr:a1", Some(&header2)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entries"][0]["text"], "two");
}

/// Fresh review F8/N1: the concurrency cap is per-daemon and injectable —
/// a harness with a 1-permit limiter pins the `busy` 503. The gate is
/// acquired AFTER auth and QUEUES for a short window
/// (`TRANSCRIPT_GATE_WAIT`) before degrading to `busy`, the one error a
/// client should retry on; the busy body still carries no-store (F4).
#[tokio::test]
async fn over_cap_queues_then_returns_busy_503() {
    let h = harness_with_permits(1);
    h.seed_agent("herdr:a1", Some(&h.worktree)).await;
    write_claude_session(&h, "s1", &[claude_line("assistant", "hello")]);

    // Occupy the harness's only permit. The harness limiter is a clone
    // of the one inside `app`, so this shares the gate with the serve
    // path (per-instance — no other test's harness contends).
    let _permit = h
        .limiter
        .acquire()
        .await
        .expect("the lone permit is free at harness start");

    let header = h.auth_header(Capability::ReadTail, "herdr:a1");
    let mut request = Request::get("/transcript?agent=herdr:a1");
    request = request.header(TRANSCRIPT_AUTH_HEADER, header);
    let res = h
        .app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        res.headers()
            .get("cache-control")
            .map(|v| v.to_str().unwrap()),
        Some("no-store"),
        "the busy error must still carry no-store (F4)"
    );
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["kind"], "busy");
}
