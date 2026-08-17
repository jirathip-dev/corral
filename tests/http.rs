//! HTTP read path tests: /snapshot JSON shape, /healthz, SSE resume with
//! Last-Event-ID (fresh cursor -> deltas; stale cursor -> full snapshot).

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use corrald::api::{router, AppState};
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
        cost: None,
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
async fn snapshot_returns_json_with_rev_and_agents() {
    let (store, app) = app().await;
    store.apply(Change::upsert(agent("a", AgentState::Blocked))).await;

    let res = app
        .oneshot(Request::get("/snapshot").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("application/json"));

    let body = res.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // v4 (P4 G21): Workspace gained `head_sha` + `head_subject` — versioned
    // strictly.
    assert_eq!(v["schema_version"], 4);
    assert_eq!(v["rev"], 1);
    assert_eq!(v["agents"]["a"]["state"], "blocked");
}

#[tokio::test]
async fn sse_with_fresh_cursor_resumes_incrementally() {
    let (store, app) = app().await;
    store.apply(Change::upsert(agent("a", AgentState::Working))).await;
    store.flush().await; // rev 1
    store.apply(Change::upsert(agent("b", AgentState::Idle))).await;
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
    assert!(res.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));

    let mut stream = res.into_body().into_data_stream();
    let first = read_frame(&mut stream, Duration::from_secs(2)).await;
    assert!(first.contains("event: delta"), "fresh cursor must not resnapshot: {first}");
    assert!(first.contains("id: 2"));
    assert!(first.contains("\"agent_id\":\"b\""), "resumed delta carries only newer agents: {first}");

    // A new change flows live as the next delta.
    store.apply(Change::upsert(agent("c", AgentState::Done))).await;
    let next = read_frame(&mut stream, Duration::from_secs(2)).await;
    assert!(next.contains("event: delta"));
    assert!(next.contains("\"agent_id\":\"c\""), "live delta carries the new agent: {next}");
    assert!(!next.contains("\"agent_id\":\"a\""), "live delta is incremental, not full state");
}

#[tokio::test]
async fn sse_with_stale_cursor_returns_full_snapshot() {
    let (store, app) = app().await;
    store.apply(Change::upsert(agent("a", AgentState::Working))).await;
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
    assert!(first.contains("event: snapshot"), "stale cursor must resnapshot: {first}");
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
    store.apply(Change::upsert(agent("live", AgentState::Done))).await;
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
    store.apply(Change::upsert(agent("a", AgentState::Working))).await;
    store.flush().await; // rev 1

    let req = Request::builder()
        .uri("/events")
        .header("Last-Event-ID", "1")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let mut stream = res.into_body().into_data_stream();

    store.apply(Change::upsert(agent("b", AgentState::Done))).await;
    let first = read_frame(&mut stream, Duration::from_secs(3)).await;
    assert!(first.contains("event: delta"));
    assert!(!first.contains("\"upd\":[]"), "no fabricated empty delta");
    assert!(first.contains("\"agent_id\":\"b\""));
}

#[tokio::test]
async fn resume_semantics_are_exposed_by_store() {
    let store = Store::new();
    store.apply(Change::upsert(agent("a", AgentState::Working))).await;
    store.flush().await;
    assert!(matches!(store.resume_from(None).await, Resume::Snapshot(_)));
}
