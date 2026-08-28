//! HTTP read path tests: /snapshot JSON shape, /healthz, SSE resume with
//! Last-Event-ID (fresh cursor -> deltas; stale cursor -> full snapshot).

use std::collections::BTreeSet;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode, header};
use corrald::api::{AppState, router};
use corrald::core::model::{Agent, AgentState, Change, Resume};
use corrald::core::store::Store;
use futures::stream::StreamExt;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn agent(id: &str, state: AgentState) -> Agent {
    Agent {
        agent_id: id.to_string(),
        source: "herdr".to_string(),
        tool: "opencode".to_string(),
        state,
        reason: None,
        seq: 1,
        ts: 0,
        capabilities: vec!["prompt".to_string()],
        waiting_on: None,
        parent_id: None,
        host: None,
        workspace: Default::default(),
        attachment: None,
        display_name: None,
        title: None,
    }
}

async fn app() -> (Store, axum::Router) {
    let store = Store::new();
    let coalescer = store.clone();
    std::mem::drop(tokio::spawn(async move { coalescer.run_coalescer().await }));
    let app = router(AppState {
        store: store.clone(),
        ..Default::default()
    });
    (store, app)
}

async fn app_with_repos(repos: &[Option<&str>]) -> (Store, axum::Router) {
    let (store, app) = app().await;
    for (index, repo) in repos.iter().enumerate() {
        let mut agent = agent(&format!("agent-{index}"), AgentState::Working);
        agent.workspace.repo = repo.map(str::to_string);
        store.apply(Change::upsert(agent)).await;
    }
    (store, app)
}

fn object_keys(value: &serde_json::Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("repo category map")
        .keys()
        .cloned()
        .collect()
}

async fn get_json(app: &axum::Router, path: &str) -> serde_json::Value {
    let response = app
        .clone()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("JSON response")
}

/// Read one data frame from an SSE body stream.
async fn read_frame<S>(stream: &mut S, timeout: Duration) -> String
where
    S: futures::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin,
{
    let frame = tokio::time::timeout(timeout, stream.next())
        .await
        .expect("SSE frame timeout")
        .expect("SSE stream ended early")
        .expect("SSE frame error");
    String::from_utf8_lossy(&frame).to_string()
}

#[tokio::test]
async fn healthz_ok() {
    let (_store, app) = app().await;
    let res = app
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn removed_cost_route_is_not_a_json_fallback() {
    let (_store, app) = app().await;
    let res = app
        .oneshot(Request::get("/cost").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert!(
        res.headers().get(header::CONTENT_TYPE).is_none(),
        "the removed route must not fall through to a JSON handler"
    );
    assert_eq!(
        res.into_body().collect().await.unwrap().to_bytes().as_ref(),
        b""
    );
}

#[tokio::test]
async fn snapshot_returns_json_with_rev_and_agents() {
    let (store, app) = app().await;
    store
        .apply(Change::upsert(agent("a", AgentState::Blocked)))
        .await;

    let res = app
        .oneshot(Request::get("/snapshot").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );

    let body = res.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // v4 (P4 G21): Workspace gained `head_sha` + `head_subject` — versioned
    // strictly.
    assert_eq!(v["schema_version"], 5);
    assert_eq!(v["rev"], 1);
    assert_eq!(v["agents"]["a"]["state"], "blocked");
}

use std::sync::Arc;

fn fleet_identity(name: &str, gh_repo: &str) -> corrald::fleet::cli::FleetIdentity {
    corrald::fleet::cli::FleetIdentity {
        name: name.to_string(),
        gh_repo: gh_repo.to_string(),
        local: std::path::PathBuf::from(format!("/tmp/{name}")),
        worktree_dir: format!("wt-{name}"),
        orch: format!("orch-{name}"),
        workers: 0,
        paused: false,
    }
}

/// #237 configless app: live agents with these repos AND an injected
/// fleet-ops CLI validated identity catalog (production shells herdr-fleet).
async fn app_with_repos_and_fleets(
    repos: &[Option<&str>],
    identities: Vec<corrald::fleet::cli::FleetIdentity>,
) -> (Store, axum::Router) {
    let (store, _app) = app_with_repos(repos).await;
    let mut state = AppState {
        fleets: Arc::new(corrald::fleet::cli::MemoryFleetOpsProvider::new(identities)),
        ..Default::default()
    };
    let store2 = store.clone();
    let coalescer = store2.clone();
    std::mem::drop(tokio::spawn(async move { coalescer.run_coalescer().await }));
    state.store = store2;
    let app = router(state);
    (store, app)
}

/// #237: the daemon starts and the board renders with NO fleets.json
/// anywhere — /issues returns the live workspace.repo union (and, with an
/// unavailable fleet-ops CLI, nothing else).
#[tokio::test]
async fn configless_startup_issues_use_live_repos_only() {
    let live = [Some("  primary-repo  "), Some(" herdr-only "), None];
    let (store, app) = app_with_repos_and_fleets(&live, Vec::new()).await;
    let issues = get_json(&app, "/issues").await;

    assert_eq!(
        object_keys(&issues["repos"]),
        BTreeSet::from(["herdr-only".to_string(), "primary-repo".to_string()]),
        "categories are the trimmed live workspace.repo union; no registry keys, \
         no basenames, no fleets.json anywhere"
    );
    assert!(
        store
            .get("agent-2")
            .await
            .expect("orphan agent")
            .workspace
            .repo
            .is_none(),
        "a missing repo identity stays in the orphan bucket"
    );
}

/// #237: category source is the live snapshot ONLY. A CLI-validated fleet
/// identity contributes its NAME as the action key, never a registry-derived
/// gh_repo basename category.
#[tokio::test]
async fn category_source_never_derives_from_gh_repo_basenames() {
    let identities = vec![
        fleet_identity("fleet-canonical", "owner/canonical-repo"),
        fleet_identity("fleet-primary", "owner/primary-repo"),
        fleet_identity("fleet-orphan", "owner/orphan-repo"),
    ];
    let live = [Some("  primary-repo  "), Some(" herdr-only "), None];
    let (store, app) = app_with_repos_and_fleets(&live, identities).await;
    let issues = get_json(&app, "/issues").await;

    // Live repo categories are the trimmed live values; fleet identity keys
    // are the CLI-validated fleet NAMES. NO registry-derived basenames
    // ("canonical-repo", "orphan-repo") may appear as categories.
    let keys = object_keys(&issues["repos"]);
    assert!(keys.contains("fleet-canonical"));
    assert!(keys.contains("fleet-primary"));
    assert!(keys.contains("fleet-orphan"));
    assert!(keys.contains("herdr-only"));
    assert!(keys.contains("primary-repo"));
    assert!(
        !keys.contains("canonical-repo") && !keys.contains("orphan-repo"),
        "gh_repo basenames are never categories: {keys:?}"
    );
    let _ = store;
}

/// #237: GET /fleets serves the fleet-ops CLI validated identity catalog and
/// nothing else — no local/worktree_dir/models/reasoning_effort/path fields.
#[tokio::test]
async fn fleets_endpoint_is_the_validated_identity_catalog() {
    let identities = vec![
        fleet_identity("corral", "jirathip-dev/corral"),
        fleet_identity("board", "jirathip-dev/herdr-board"),
    ];
    let (_store, app) = app_with_repos_and_fleets(&[], identities).await;
    let body = get_json(&app, "/fleets").await;

    assert_eq!(body["status"], "ok");
    assert!(body["error"].is_null());
    assert_eq!(body["fleets"].as_array().unwrap().len(), 2);
    assert_eq!(body["fleets"][0]["name"], "corral");
    assert_eq!(body["fleets"][0]["gh_repo"], "jirathip-dev/corral");
    assert_eq!(body["fleets"][0]["orch"], "orch-corral");
    assert!(
        body["fleets"][0].get("local").is_none()
            && body["fleets"][0].get("worktree_dir").is_none()
            && body["fleets"][0].get("models").is_none()
            && body.get("path").is_none(),
        "no registry projection fields remain on the identity catalog"
    );
}

/// #210: GET /snapshot carries the read-only per-fleet health aggregation
/// computed from the injected fleet identities + the live agent set. The
/// strip never carries spend/balance state — the health rows are exactly
/// and only the three health signals (orch alive, worker count, heartbeat).
#[tokio::test]
async fn snapshot_carries_fleet_health_aggregation() {
    let identities = vec![fleet_identity("corral", "jirathip-dev/corral")];
    let (store, app) = app_with_repos_and_fleets(&[], identities).await;
    let mut orch = agent("orch", AgentState::Working);
    orch.display_name = Some("orch-corral".into());
    orch.workspace.repo = Some("primary-repo".into());
    let mut worker_a = agent("worker-a", AgentState::Working);
    worker_a.workspace.repo = Some("primary-repo".into());
    let mut worker_b = agent("worker-b", AgentState::Idle);
    worker_b.workspace.repo = Some("primary-repo".into());
    let mut foreign = agent("foreign", AgentState::Working);
    foreign.workspace.repo = Some("other-repo".into());
    for agent in [orch, worker_a, worker_b, foreign] {
        store.apply(Change::upsert(agent)).await;
    }

    let body = get_json(&app, "/snapshot").await;
    let health = body["fleet_health"]
        .as_array()
        .expect("snapshot carries fleet_health");
    assert_eq!(health.len(), 1, "one row per fleet identity");
    let row = &health[0];
    assert_eq!(row["name"], "corral");
    assert_eq!(row["gh_repo"], "jirathip-dev/corral");
    assert_eq!(row["orch"], "orch-corral");
    assert_eq!(row["orch_alive"], true);
    assert_eq!(row["orch_state"], "working");
    assert_eq!(row["workers"], 2, "repo-group workers only, orch excluded");
    assert_eq!(row["last_heartbeat"], serde_json::Value::Null);
    assert!(
        row["degraded"] == serde_json::json!(true),
        "no adapter presence signal -> a refusal to guess a fresh heartbeat walks `heartbeat_stale`"
    );
    assert!(
        row["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w == "heartbeat_stale")
    );
    // HEALTH ONLY guard: nothing but the health fields is ever carried.
    let keys = body["fleet_health"][0]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<BTreeSet<String>>();
    assert_eq!(
        keys,
        BTreeSet::from([
            "name".into(),
            "gh_repo".into(),
            "paused".into(),
            "orch".into(),
            "orch_alive".into(),
            "orch_state".into(),
            "workers".into(),
            "last_heartbeat".into(),
            "degraded".into(),
            "warnings".into(),
        ]),
        "no spend/balance field can ever appear on the fleet-health strip"
    );
}

#[tokio::test]
async fn snapshot_with_unavailable_fleet_ops_carries_no_health_rows() {
    struct Down;
    impl corrald::fleet::cli::FleetOpsProvider for Down {
        fn list(
            &self,
        ) -> Result<Vec<corrald::fleet::cli::FleetIdentity>, corrald::fleet::cli::FleetOpsError>
        {
            Err(corrald::fleet::cli::FleetOpsError::Unavailable {
                detail: "no herdr-fleet".to_string(),
            })
        }
    }
    let (store, _app) = app_with_repos(&[]).await;
    let mut state = AppState {
        fleets: Arc::new(Down),
        ..Default::default()
    };
    let store2 = store.clone();
    let coalescer = store2.clone();
    std::mem::drop(tokio::spawn(async move { coalescer.run_coalescer().await }));
    state.store = store2;
    let app = router(state);
    let body = get_json(&app, "/snapshot").await;
    let rows = body.get("fleet_health").and_then(|v| v.as_array());
    assert!(
        rows.is_none_or(|rows| rows.is_empty()),
        "an unavailable identity path yields an empty/absent strip, never a guessed roster: {body}"
    );
}

/// #237: an explicitly unavailable fleet-ops CLI is an explicit status, and
/// the board still renders live categories (configless-safe daemon).
#[tokio::test]
async fn unavailable_provider_reports_error_but_keeps_live_categories() {
    struct Down;
    impl corrald::fleet::cli::FleetOpsProvider for Down {
        fn list(
            &self,
        ) -> Result<Vec<corrald::fleet::cli::FleetIdentity>, corrald::fleet::cli::FleetOpsError>
        {
            Err(corrald::fleet::cli::FleetOpsError::Unavailable {
                detail: "no herdr-fleet".to_string(),
            })
        }
    }
    let live = [Some("  primary-repo  "), None];
    let (store, _app) = app_with_repos(&live).await;
    let mut state = AppState {
        fleets: Arc::new(Down),
        ..Default::default()
    };
    let store2 = store.clone();
    let coalescer = store2.clone();
    std::mem::drop(tokio::spawn(async move { coalescer.run_coalescer().await }));
    state.store = store2;
    let app = router(state);
    let body = get_json(&app, "/fleets").await;
    assert_eq!(body["status"], "error");
    assert!(body["fleets"].as_array().unwrap().is_empty());
    let issues = get_json(&app, "/issues").await;
    assert_eq!(
        object_keys(&issues["repos"]),
        BTreeSet::from(["primary-repo".to_string()]),
        "live categories still render when the identity path is down"
    );
}

#[test]
fn no_fleets_json_reference_anywhere_in_src() {
    // The no-write/no-read guarantee (#237): configless corral never owns
    // fleets.json, so no non-comment source line may reference it at all
    // (prose that explains the guarantee is permitted and ignored).
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();
    let mut stack = vec![root.join("src"), root.join("crates"), root.join("clients")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().map(|n| n == "target").unwrap_or(false)
                    || path.file_name().map(|n| n == ".git").unwrap_or(false)
                {
                    continue;
                }
                stack.push(path);
            } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let in_block_comment = text.matches("/*").count() - text.matches("*/").count();
                for (index, line) in text.lines().enumerate() {
                    if !line.contains("fleets.json") {
                        continue;
                    }
                    let trimmed = line.trim_start();
                    let comment = trimmed.starts_with("//");
                    let mut seen_block = false;
                    if in_block_comment > 0 {
                        let prior =
                            &text[..text.lines().take(index).map(|l| l.len() + 1).sum::<usize>()];
                        let opens_diff = prior.matches("/*").count() - prior.matches("*/").count();
                        seen_block = opens_diff > 0;
                    }
                    if !comment && !seen_block {
                        offenders.push(format!("{}:{}: {}", path.display(), index + 1, trimmed));
                    }
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "fleets.json must not be referenced in executable src code (configless #237): {offenders:?}"
    );
}

#[tokio::test]
async fn sse_with_fresh_cursor_resumes_incrementally() {
    let (store, app) = app().await;
    store
        .apply(Change::upsert(agent("a", AgentState::Working)))
        .await;
    store.flush().await; // rev 1
    store
        .apply(Change::upsert(agent("b", AgentState::Idle)))
        .await;
    store.flush().await; // rev 2

    // Cursor 1 is within the retained history -> incremental delta (rev 2),
    // NOT a full snapshot.
    let req = Request::builder()
        .uri("/events")
        .header("Last-Event-ID", "1")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );

    let mut stream = res.into_body().into_data_stream();
    let first = read_frame(&mut stream, Duration::from_secs(2)).await;
    assert!(
        first.contains("event: delta"),
        "fresh cursor must not resnapshot: {first}"
    );
    assert!(first.contains("id: 2"));
    assert!(
        first.contains("\"agent_id\":\"b\""),
        "resumed delta carries only newer agents: {first}"
    );

    // A new change flows live as the next delta.
    store
        .apply(Change::upsert(agent("c", AgentState::Done)))
        .await;
    let next = read_frame(&mut stream, Duration::from_secs(2)).await;
    assert!(next.contains("event: delta"));
    assert!(
        next.contains("\"agent_id\":\"c\""),
        "live delta carries the new agent: {next}"
    );
    assert!(
        !next.contains("\"agent_id\":\"a\""),
        "live delta is incremental, not full state"
    );
}

#[tokio::test]
async fn sse_with_stale_cursor_returns_full_snapshot() {
    let (store, app) = app().await;
    store
        .apply(Change::upsert(agent("a", AgentState::Working)))
        .await;
    store.flush().await;

    // Client cursor is behind the retained history (0 < oldest rev 1)
    // -> full snapshot frame.
    let req = Request::builder()
        .uri("/events")
        .header("Last-Event-ID", "0")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let mut stream = res.into_body().into_data_stream();
    let first = read_frame(&mut stream, Duration::from_secs(2)).await;
    assert!(
        first.contains("event: snapshot"),
        "stale cursor must resnapshot: {first}"
    );
    assert!(first.contains("\"agents\""));

    // No cursor at all -> snapshot.
    let res = app
        .oneshot(Request::get("/events").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let mut stream = res.into_body().into_data_stream();
    let first = read_frame(&mut stream, Duration::from_secs(2)).await;
    assert!(first.contains("event: snapshot"));
}

#[tokio::test]
async fn sse_no_cursor_snapshot_then_live_delta() {
    let (store, app) = app().await;
    let req = Request::get("/events").body(Body::empty()).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let mut stream = res.into_body().into_data_stream();
    // First frame: snapshot at rev 0 (empty store).
    let first = read_frame(&mut stream, Duration::from_secs(2)).await;
    assert!(first.contains("event: snapshot"));
    assert!(first.contains("\"rev\":0"));

    // Live update flows as the next frame on the same connection.
    store
        .apply(Change::upsert(agent("live", AgentState::Done)))
        .await;
    let next = read_frame(&mut stream, Duration::from_secs(2)).await;
    assert!(next.contains("event: delta"));
    assert!(next.contains("\"agent_id\":\"live\""));
}

#[tokio::test]
async fn sse_live_cursor_emits_no_fabricated_delta() {
    // m9: a client whose cursor equals current rev must not receive a
    // synthetic empty delta (it would look like a state change); the first
    // frame on the wire must be a real delta.
    let (store, app) = app().await;
    store
        .apply(Change::upsert(agent("a", AgentState::Working)))
        .await;
    store.flush().await; // rev 1

    let req = Request::builder()
        .uri("/events")
        .header("Last-Event-ID", "1")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let mut stream = res.into_body().into_data_stream();

    store
        .apply(Change::upsert(agent("b", AgentState::Done)))
        .await;
    let first = read_frame(&mut stream, Duration::from_secs(3)).await;
    assert!(first.contains("event: delta"));
    assert!(!first.contains("\"upd\":[]"), "no fabricated empty delta");
    assert!(first.contains("\"agent_id\":\"b\""));
}

#[tokio::test]
async fn resume_semantics_are_exposed_by_store() {
    let store = Store::new();
    store
        .apply(Change::upsert(agent("a", AgentState::Working)))
        .await;
    store.flush().await;
    assert!(matches!(store.resume_from(None).await, Resume::Snapshot(_)));
}

/// #113: `GET /issues` serves the read-only repo-level issue view without
/// touching GitHub — the cache is the only source, and it is populated via
/// the integrator (here injected directly for the hermetic test).
#[tokio::test]
async fn issues_endpoint_serves_last_known_repo_issues() {
    let state = AppState::default();
    state.issues.update(
        "herdr-board",
        vec![corrald::core::events::GhIssueRef {
            repo: "herdr-board".to_string(),
            number: 4,
            state: "OPEN".to_string(),
            title: "P2 planes".to_string(),
            labels: vec![corrald::core::events::GhIssueLabel {
                name: "p2".to_string(),
                color: "5319E7".to_string(),
            }],
            url: "https://github.com/herdr-board/herdr-board/issues/4".to_string(),
            body: None,
            comments: vec![],
            comment_total: None,
        }],
    );
    let app = router(state);
    let res = app
        .oneshot(Request::get("/issues").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let repos = &json["repos"];
    assert_eq!(repos["herdr-board"][0]["number"], 4);
    assert_eq!(repos["herdr-board"][0]["state"], "OPEN");
    assert_eq!(repos["herdr-board"][0]["title"], "P2 planes");
    assert_eq!(
        repos["herdr-board"][0]["url"],
        "https://github.com/herdr-board/herdr-board/issues/4"
    );
    assert_eq!(repos["herdr-board"][0]["labels"][0]["name"], "p2");
}

#[tokio::test]
async fn issues_shared_gh_repo_keeps_one_cached_issue_and_both_fleet_keys() {
    let identities = vec![
        fleet_identity("alpha", "owner/foo"),
        fleet_identity("beta", "owner/foo"),
    ];
    let state = AppState {
        fleets: Arc::new(corrald::fleet::cli::MemoryFleetOpsProvider::new(identities)),
        ..Default::default()
    };
    state.issues.update(
        "alpha",
        vec![corrald::core::events::GhIssueRef {
            repo: "foo".to_string(),
            number: 42,
            state: "OPEN".to_string(),
            title: "shared issue".to_string(),
            labels: vec![],
            url: "https://github.com/example/foo/issues/42".to_string(),
            body: None,
            comments: vec![],
            comment_total: None,
        }],
    );
    let app = router(state);
    let json = get_json(&app, "/issues").await;
    assert_eq!(json["repos"]["alpha"].as_array().unwrap().len(), 1);
    assert!(json["repos"]["beta"].as_array().unwrap().is_empty());
    // The live category union has no repo keys here; only validated fleet
    // identity keys exist (no registry-derived basenames like "foo").
    assert!(
        json["repos"].get("foo").is_none(),
        "no guessed category from gh_repo"
    );
}

// --- #215: narrow CORS layer on the credential-free read plane ---

const CORS_ORIGIN: &str = "https://demo.corral.pages.github.io";

#[tokio::test]
async fn cors_read_plane_reflects_only_allowlisted_origin() {
    let store = Store::new();
    let app = router(AppState {
        store,
        cors_origins: vec![CORS_ORIGIN.to_string()],
        ..Default::default()
    });

    // Matching origin -> reflected on a read route.
    let res = app
        .clone()
        .oneshot(
            Request::get("/healthz")
                .header(header::ORIGIN, CORS_ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static(CORS_ORIGIN))
    );
    assert!(
        res.headers()
            .get_all(header::VARY)
            .iter()
            .any(|v| v == "Origin"),
        "Vary: Origin so caches key on the echoed origin"
    );

    // Non-allowlisted origin -> no CORS headers at all.
    let res = app
        .clone()
        .oneshot(
            Request::get("/healthz")
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        res.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none(),
        "unlisted origin never gets CORS"
    );

    // No Origin header (same-origin/curl) -> unchanged response, no CORS.
    let res = app
        .clone()
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(
        res.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}

#[tokio::test]
async fn cors_preflight_answers_read_routes_only_for_allowlisted_origin() {
    let store = Store::new();
    let app = router(AppState {
        store,
        cors_origins: vec![CORS_ORIGIN.to_string()],
        ..Default::default()
    });

    // OPTIONS /snapshot + matching origin -> 204 with the read method set.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/snapshot")
                .header(header::ORIGIN, CORS_ORIGIN)
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        res.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static(CORS_ORIGIN))
    );
    assert_eq!(
        res.headers().get(header::ACCESS_CONTROL_ALLOW_METHODS),
        Some(&HeaderValue::from_static("GET, OPTIONS"))
    );

    // OPTIONS from an unlisted origin -> blocked (no ACAO).
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/snapshot")
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        res.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}

#[tokio::test]
async fn cors_never_emitted_on_the_write_plane() {
    let store = Store::new();
    let app = router(AppState {
        store,
        cors_origins: vec![CORS_ORIGIN.to_string()],
        ..Default::default()
    });

    // POST /drive from the allowlisted origin: the response carries NO
    // Access-Control-Allow-Origin, so a browser cannot read it (and the
    // preflight for the JSON POST fails).
    let res = app
        .clone()
        .oneshot(
            Request::post("/drive")
                .header(header::ORIGIN, CORS_ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        res.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none(),
        "the write plane never emits CORS headers"
    );

    // The write plane has no OPTIONS route at all: no CORS anywhere.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/drive")
                .header(header::ORIGIN, CORS_ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        res.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none(),
        "write plane is never CORS-enabled"
    );

    // Auth routes (host-key) are write-adjacent and also stay CORS-free.
    let res = app
        .clone()
        .oneshot(
            Request::get("/host-key")
                .header(header::ORIGIN, CORS_ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        res.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}

#[tokio::test]
async fn cors_default_state_never_emits_cors_headers() {
    let store = Store::new();
    let app = router(AppState {
        store,
        ..Default::default()
    });
    let res = app
        .oneshot(
            Request::get("/healthz")
                .header(header::ORIGIN, CORS_ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        res.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none(),
        "no allowlist configured -> the daemon behaves exactly as before"
    );
}
