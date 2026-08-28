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

#[tokio::test]
async fn snapshot_exposes_git_plane_backlog_additively() {
    let (store, app) = app().await;
    store
        .git_plane_backlog()
        .store(true, std::sync::atomic::Ordering::Release);
    let v = get_json(&app, "/snapshot").await;
    assert_eq!(v["git_plane_backlog"], true);
}

#[tokio::test]
async fn native_issues_use_live_repos_only() {
    let live = [Some("  primary-repo  "), Some(" herdr-only "), None];
    let (_store, app) = app_with_repos(&live).await;
    let issues = get_json(&app, "/issues").await;
    assert_eq!(
        object_keys(&issues["repos"]),
        BTreeSet::from(["herdr-only".to_string(), "primary-repo".to_string()])
    );
}

#[tokio::test]
async fn snapshot_never_needs_fleet_ops() {
    let (store, _app) = app_with_repos(&[Some("primary-repo")]).await;
    let state = AppState {
        store,
        ..Default::default()
    };
    let body = get_json(&router(state), "/snapshot").await;
    assert!(body["agents"].is_object());
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

#[tokio::test]
async fn poisoned_path_never_invokes_private_fleet_tool_on_read_plane() {
    let previous = std::env::var_os("PATH");
    unsafe { std::env::set_var("PATH", "/definitely-not-a-real-path") };
    let app = router(AppState::default());
    for path in ["/healthz", "/snapshot", "/events?since=0", "/issues"] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "{path}"
        );
    }
    match previous {
        Some(value) => unsafe { std::env::set_var("PATH", value) },
        None => unsafe { std::env::remove_var("PATH") },
    }
}
