//! #354 L1 focused RED/GREEN probes: the daemon's read-only cut.
//!
//! Two halves, kept deliberately narrow so the whole file is the gate:
//!
//! 1. **RIP probe (RED on the pre-cut daemon):** every mutating drive
//!    capability that was UI-reachable before the cut — `prompt`, `approve`
//!    (the answer path), `interrupt`, `kill`, `attach`, `start_worktree`,
//!    `read_issues` — must be refused AT THE CAPABILITY BOUNDARY
//!    (`unknown_capability`) even when the signing device is fully granted
//!    and the target agent is known. The refusal happens before the
//!    authorizer, before any adapter dispatch, and before the audit log:
//!    `dispatch_count == 0` and zero audit entries. On the pre-cut daemon
//!    these drives dispatched (or reached a later typed refusal), so this
//!    test is RED before the cut and GREEN after it.
//!
//! 2. **GREEN probes (the kept read surface):** signed `read_tail` still
//!    dispatches and returns the bounded tail, a fresh device registration
//!    still lands with empty (read-only) grants, and the SSE/snapshot read
//!    plane still serves. These pin the parts the cut MUST NOT touch.

use std::sync::atomic::{AtomicUsize, Ordering};
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
use corrald::drive::{Capability, DriveEnvelope, SignedDrive};
use ed25519_dalek::SigningKey;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Minimal recording adapter (drive dispatch counter + canned read_tail)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct ProbeAdapter {
    dispatches: AtomicUsize,
    read_tail_calls: AtomicUsize,
    tail: Mutex<Vec<String>>,
}

impl ProbeAdapter {
    fn dispatch_count(&self) -> usize {
        self.dispatches.load(Ordering::SeqCst)
    }

    fn read_tail_count(&self) -> usize {
        self.read_tail_calls.load(Ordering::SeqCst)
    }
}

impl Adapter for ProbeAdapter {
    fn source(&self) -> &'static str {
        "probe"
    }

    fn start(self: Arc<Self>, _store: Store) {}

    fn drive<'a>(
        &'a self,
        _agent_id: &'a str,
        _command: DriveCommand,
    ) -> futures::future::BoxFuture<'a, Result<(), DriveError>> {
        Box::pin(async move {
            self.dispatches.fetch_add(1, Ordering::SeqCst);
            Err(DriveError::NotImplemented("probe"))
        })
    }

    fn knows_agent(&self, _agent_id: &str) -> bool {
        true
    }

    fn read_tail<'a>(
        &'a self,
        _agent_id: &'a str,
        _lines: u32,
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, DriveError>> {
        self.read_tail_calls.fetch_add(1, Ordering::SeqCst);
        let tail = self.tail.lock().unwrap().clone();
        Box::pin(async move { Ok(tail) })
    }
}

// ---------------------------------------------------------------------------
// Harness: real auth plane over a scratch dir, fully granted signing device
// ---------------------------------------------------------------------------

struct Harness {
    adapter: Arc<ProbeAdapter>,
    auth: Arc<AuthPlane>,
    signing: SigningKey,
    app: Router,
    _dir: tempfile::TempDir,
}

fn harness() -> Harness {
    let store = Store::new();
    let coalescer = store.clone();
    std::mem::drop(tokio::spawn(async move { coalescer.run_coalescer().await }));
    let adapter = Arc::new(ProbeAdapter::default());
    let dir = tempfile::tempdir().expect("tempdir");
    let auth = Arc::new(AuthPlane::load_or_create(dir.path().to_path_buf()).expect("auth plane"));
    let (signing, pubkey) = test_support::keypair();
    let token = auth.registry.registration_token();
    // Register the harness device once; grant every legacy capability so the
    // pre-cut daemon reaches dispatch instead of stopping at authorization.
    let rec = auth
        .registry
        .register(&token, pubkey, std::time::Duration::from_secs(3600))
        .expect("register");
    auth.registry
        .set_grants(
            &rec.key_id,
            vec![
                Capability::Prompt,
                Capability::Interrupt,
                Capability::Approve,
                Capability::ReadTail,
                Capability::Kill,
                Capability::Attach,
                Capability::StartWorktree,
                Capability::ReadDiff,
                Capability::ReadIssues,
            ],
        )
        .expect("read grants");
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
        app,
        _dir: dir,
    }
}

impl Harness {
    /// A genuinely signed drive body for `capability` (signature over the
    /// canonical envelope bytes with the harness device's key).
    fn signed_body(&self, request_id: &str, capability: &str, payload: Value) -> String {
        let envelope = DriveEnvelope {
            request_id: request_id.to_string(),
            capability: capability.parse().expect("probe capability parses"),
            target: "herdr:agent-a".to_string(),
            payload,
            rev: None,
        };
        let signed = SignedDrive {
            key_id: self.auth.registry.records()[0].key_id.clone(),
            signature: test_support::sign(&self.signing, &envelope),
            envelope,
        };
        serde_json::to_string(&signed).expect("signed body serializes")
    }

    fn audit_len(&self) -> usize {
        self.auth.audit.chain().0.len()
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

async fn get(app: &Router, uri: &str) -> StatusCode {
    app.clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

// ---------------------------------------------------------------------------
// RIP probe: every mutating drive capability is refused at the boundary
// ---------------------------------------------------------------------------

/// Wire capability string -> payload shaped for that capability. The payload
/// only matters on the PRE-CUT daemon (the refusal lands before payload
/// parsing once the cut lands); each is the canonical shape for that drive.
fn mutating_drives() -> Vec<(&'static str, Value)> {
    vec![
        ("prompt", json!({ "kind": "prompt", "text": "continue" })),
        (
            "approve",
            json!({
                "kind": "approve",
                "approval_id": "herdr:agent-a:sha256:x",
                "prompt_hash": "sha256:x",
                "choice": "yes"
            }),
        ),
        ("interrupt", json!({})),
        ("kill", json!({})),
        ("attach", json!({})),
        (
            "start_worktree",
            json!({
                "kind": "start_worktree",
                "mode": "free",
                "repo": "no-such-fleet/repo",
                "name": "probe"
            }),
        ),
        ("read_issues", json!({ "kind": "read_issues" })),
    ]
}

#[tokio::test]
async fn mutating_drives_are_refused_without_dispatch_or_audit() {
    let h = harness();

    for (i, (capability, payload)) in mutating_drives().into_iter().enumerate() {
        let (status, value) = post(
            &h.app,
            h.signed_body(&format!("req-{capability}"), capability, payload),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{capability}: mutating drive must be refused at the capability boundary (got {status}, {value})"
        );
        assert_eq!(
            value["kind"], "unknown_capability",
            "{capability}: refusal kind must be unknown_capability (got {value})"
        );
        assert_eq!(
            value["request_id"], format!("req-{capability}"),
            "{capability}: refusal must carry the request id"
        );
        // The refusal happens before ANY dispatch and before the audit log,
        // even though this device is granted and the agent is known.
        assert_eq!(
            h.adapter.dispatch_count(),
            0,
            "{capability}: no adapter dispatch may occur for a mutating drive ({i})"
        );
        assert_eq!(
            h.audit_len(),
            0,
            "{capability}: a boundary-refused drive must not be audited"
        );
    }
}

// ---------------------------------------------------------------------------
// GREEN probes: the kept read-only surface stays intact
// ---------------------------------------------------------------------------

#[tokio::test]
async fn signed_read_tail_still_dispatches_after_the_cut() {
    let h = harness();
    h.adapter
        .tail
        .lock()
        .unwrap()
        .clone_from(&vec!["hello".to_string(), "world".to_string()]);

    let (status, value) = post(
        &h.app,
        h.signed_body(
            "req-read-tail",
            "read_tail",
            json!({ "kind": "read_tail", "lines": 200 }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "read_tail stays signed-dispatchable");
    assert_eq!(value["ok"], true, "read_tail response ok: {value}");
    assert_eq!(
        value["result"]["lines"],
        json!(["hello", "world"]),
        "read_tail must serve the bounded tail"
    );
    assert_eq!(
        h.adapter.read_tail_count(),
        1,
        "read_tail dispatches exactly once"
    );
}

#[tokio::test]
async fn fresh_registration_is_still_read_only_default() {
    let h = harness();
    let (signing, pubkey) = test_support::keypair();
    let token = h.auth.registry.registration_token();
    let rec = h
        .auth
        .registry
        .register(&token, pubkey, std::time::Duration::from_secs(3600))
        .expect("second device registers");
    assert!(
        rec.grants.is_empty(),
        "fresh device stays read-only (empty grants)"
    );
    // The fresh device can be granted read_tail and its signed drive verifies.
    h.auth
        .registry
        .set_grants(&rec.key_id, vec![Capability::ReadTail])
        .expect("grant read_tail");
    let envelope = DriveEnvelope {
        request_id: "req-fresh-read".to_string(),
        capability: Capability::ReadTail,
        target: "herdr:agent-a".to_string(),
        payload: json!({ "kind": "read_tail", "lines": 50 }),
        rev: None,
    };
    let signed = SignedDrive {
        key_id: rec.key_id,
        signature: test_support::sign(&signing, &envelope),
        envelope,
    };
    let (status, value) = post(
        &h.app,
        serde_json::to_string(&signed).expect("signed body serializes"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "granted fresh device reads: {value}");
    assert_eq!(value["ok"], true);
}

#[tokio::test]
async fn sse_and_snapshot_read_plane_still_serves() {
    let h = harness();
    assert_eq!(get(&h.app, "/snapshot").await, StatusCode::OK);
    assert_eq!(get(&h.app, "/events").await, StatusCode::OK);
    assert_eq!(get(&h.app, "/healthz").await, StatusCode::OK);
}
