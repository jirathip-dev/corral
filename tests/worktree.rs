//! #113 integration coverage: the real `POST /drive <start_worktree>`
//! dispatch path against a temp git repo.
//!
//! This is deliberately NOT the unit-level `worktree::start` suite: it
//! drives the signed HTTP route into the production `GitCreator` and
//! `HerdrLauncher`, so the missing acceptance criterion is covered without
//! touching GitHub or a real herdr orchestrator.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use corrald::api::issues::IssuesCache;
use corrald::api::{AppState, router};
use corrald::auth::test_support;
use corrald::core::events::GhIssueRef;
use corrald::drive::{Capability, DriveEnvelope, SignedDrive};
use ed25519_dalek::SigningKey;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ISSUE_NUMBER: u64 = 113;
const FLEET_NAME: &str = "corral";
const BRANCH: &str = "issue-113-work";

/// Restore a single process-level environment variable when the test ends,
/// even on panic. The dispatcher reads `HOME` directly (configless #237:
/// no `CORRAL_FLEETS_PATH` fallback exists), so this integration test
/// points it at a temp fixture while the request is in flight.
struct EnvRestore {
    name: &'static str,
    previous: Option<OsString>,
}

impl EnvRestore {
    fn set(name: &'static str, value: &Path) -> Self {
        let previous = std::env::var_os(name);
        // Edition-2024 environment mutation is unsafe by design.
        unsafe { std::env::set_var(name, value) };
        Self { name, previous }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(previous) => unsafe { std::env::set_var(self.name, previous) },
            None => unsafe { std::env::remove_var(self.name) },
        }
    }
}

fn run_git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("spawn git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn temp_git_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().expect("temp git repo");
    run_git(repo.path(), &["init", "-q", "-b", "main"]);
    run_git(repo.path(), &["config", "user.name", "Corral Test"]);
    run_git(
        repo.path(),
        &["config", "user.email", "corral-test@example.invalid"],
    );
    run_git(
        repo.path(),
        &["commit", "-q", "--allow-empty", "-m", "seed"],
    );
    repo
}

fn signed_body(signing: &SigningKey, key_id: &str, request_id: &str, payload: Value) -> String {
    let envelope = DriveEnvelope {
        request_id: request_id.to_string(),
        capability: Capability::StartWorktree,
        target: FLEET_NAME.to_string(),
        payload,
        rev: None,
    };
    let signed = SignedDrive {
        key_id: key_id.to_string(),
        signature: test_support::sign(signing, &envelope),
        envelope,
    };
    serde_json::to_string(&signed).expect("signed drive body serializes")
}

async fn post(app: &Router, body: String) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post("/drive")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request body"),
        )
        .await
        .expect("drive request");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn real_dispatch_creates_exactly_one_issue_worktree_and_defers_handoff() {
    let home_dir = tempfile::tempdir().expect("temp home");
    let checkout = temp_git_repo();
    let issue_payload = json!({
        "kind": "start_worktree",
        "mode": "issue",
        "repo": FLEET_NAME,
        "number": ISSUE_NUMBER,
        "issue_url": "https://github.com/jirathip-dev/corral/issues/113",
    });

    // A throwaway home for the worktree root; configless means NO fleets.json
    // anywhere — the identity comes from the injected provider below.
    let _home_guard = EnvRestore::set("HOME", home_dir.path());

    // Build the daemon state with the issue cache seeded as authoritative,
    // then register a device granted only the worktree capability.
    let mut state = AppState::default();
    let issues = Arc::new(IssuesCache::default());
    issues.update(
        FLEET_NAME,
        vec![GhIssueRef {
            repo: FLEET_NAME.to_string(),
            number: ISSUE_NUMBER,
            state: "OPEN".to_string(),
            title: "Integration coverage".to_string(),
            labels: Vec::new(),
            url: "https://github.com/jirathip-dev/corral/issues/113".to_string(),
            body: None,
            comments: vec![],
            comment_total: None,
        }],
    );
    state.issues = issues;

    // Configless: the fleet identity is injected through the provider (the
    // production daemon shells `herdr-fleet list`; a fixture here is the
    // CLI-validated identity of the temp checkout).
    state.fleets = Arc::new(corrald::fleet::cli::MemoryFleetOpsProvider::new(vec![
        corrald::fleet::cli::FleetIdentity {
            name: FLEET_NAME.to_string(),
            gh_repo: "jirathip-dev/corral".to_string(),
            local: checkout.path().to_path_buf(),
            worktree_dir: "corral".to_string(),
            orch: "orch-corral".to_string(),
            workers: 0,
            paused: false,
        },
    ]));

    let (signing, pubkey) = test_support::keypair();
    let bootstrap = test_support::envelope("bootstrap", Capability::Prompt, "bootstrap");
    let token = state.auth.registry.registration_token();
    let registered =
        test_support::signed(&state.auth.registry, &token, &signing, pubkey, &bootstrap);
    state
        .auth
        .registry
        .set_grants(&registered.key_id, vec![Capability::StartWorktree])
        .expect("grant start_worktree");
    let store = state.store.clone();
    let auth = state.auth.clone();
    let adapter = state.adapter.clone();
    let issues = state.issues.clone();
    let fleets = state.fleets.clone();
    let app = router(state);
    let restarted_app = router(AppState {
        store,
        auth,
        adapter,
        replay: Arc::new(corrald::api::drive::ReplayTable::default()),
        issues,
        fleets,
        cors_origins: Vec::new(),
    });

    // First dispatch: the real git seam creates the branch/worktree and the
    // real herdr launcher reports a deferred handoff (no agent spawned).
    let (status, first) = post(
        &app,
        signed_body(
            &signing,
            &registered.key_id,
            "wt-113",
            issue_payload.clone(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["ok"], true);
    assert_eq!(first["result"]["state"], "started");
    assert_eq!(first["result"]["branch"], BRANCH);
    assert_eq!(first["result"]["handoff"], "deferred");

    let worktree_path = PathBuf::from(first["result"]["path"].as_str().expect("worktree path"));
    assert!(worktree_path.is_dir(), "real worktree exists on disk");
    assert_eq!(
        run_git(&worktree_path, &["rev-parse", "--abbrev-ref", "HEAD"]),
        BRANCH,
        "branch-name metadata carries the issue number"
    );

    // Same request id is a replay: the stored first response is returned and
    // no second worktree is created.
    let (status, replay) = post(
        &app,
        signed_body(
            &signing,
            &registered.key_id,
            "wt-113",
            issue_payload.clone(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["ok"], true);
    assert_eq!(replay["result"]["state"], "started");

    // With the replay table reset (e.g. daemon restart), the identical signed
    // request still dispatches safely: the real `GitCreator::exists` guard
    // returns already_started rather than creating a second worktree.
    let (status, duplicate) = post(
        &restarted_app,
        signed_body(&signing, &registered.key_id, "wt-113", issue_payload),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(duplicate["ok"], true);
    assert_eq!(duplicate["result"]["state"], "already_started");
    assert_eq!(duplicate["result"]["branch"], BRANCH);

    let listing = run_git(checkout.path(), &["worktree", "list", "--porcelain"]);
    assert_eq!(
        listing.matches("branch refs/heads/issue-113-work").count(),
        1,
        "exactly one issue-linked branch/worktree"
    );
    assert!(
        listing.contains(&worktree_path.to_string_lossy().to_string()),
        "git worktree list records the created path"
    );
}

/// #243: a healthy fleet-ops CLI catalog that does NOT contain the
/// requested repo must refuse with the typed `unknown_fleet` error kind —
/// the daemon never synthesizes an identity from display categories, and a
/// catalog miss is a refusal (client-side gating covers the happy path
/// only). The provider is injected with a VALID but non-matching catalog,
/// proving the miss, not a CLI-unavailable condition.
#[tokio::test]
async fn unknown_fleet_catalog_miss_refuses_with_typed_error_kind() {
    let state = AppState {
        fleets: Arc::new(corrald::fleet::cli::MemoryFleetOpsProvider::new(vec![
            corrald::fleet::cli::FleetIdentity {
                name: "plush".to_string(),
                gh_repo: "jirathip-dev/plush-meadow".to_string(),
                local: PathBuf::from("/tmp/plush"),
                worktree_dir: "plush".to_string(),
                orch: "orch-plush".to_string(),
                workers: 0,
                paused: false,
            },
        ])),
        ..Default::default()
    };

    let (signing, pubkey) = test_support::keypair();
    let bootstrap = test_support::envelope("bootstrap", Capability::Prompt, "bootstrap");
    let token = state.auth.registry.registration_token();
    let registered =
        test_support::signed(&state.auth.registry, &token, &signing, pubkey, &bootstrap);
    state
        .auth
        .registry
        .set_grants(&registered.key_id, vec![Capability::StartWorktree])
        .expect("grant start_worktree");
    let app = router(state);

    let issue_payload = json!({
        "kind": "start_worktree",
        "mode": "issue",
        "repo": FLEET_NAME,
        "number": ISSUE_NUMBER,
        "issue_url": "https://github.com/jirathip-dev/corral/issues/113",
    });

    let (status, body) = post(
        &app,
        signed_body(
            &signing,
            &registered.key_id,
            "wt-unknown-fleet",
            issue_payload,
        ),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "dispatch-level refusal carries a 200 body"
    );
    assert_eq!(body["ok"], false);
    assert_eq!(body["error_kind"], "unknown_fleet");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains(FLEET_NAME),
        "error names the missing fleet: {}",
        body["error"]
    );
}
