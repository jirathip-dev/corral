//! HTTP read path tests: /snapshot JSON shape, /healthz, SSE resume with
//! Last-Event-ID (fresh cursor -> deltas; stale cursor -> full snapshot).

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use corrald::api::{AppState, router};
use corrald::core::model::{Agent, AgentState, Change, Resume};
use corrald::core::store::Store;
use futures::stream::StreamExt;
use http_body_util::BodyExt;
use tower::ServiceExt;

/// Serializes tests that mutate `CORRAL_FLEETS_PATH`: env mutation is
/// process-wide, while the daemon resolves it synchronously from the request
/// handler. Kept as a tokio mutex so the async tests can hold the guard
/// across the in-flight request.
static REGISTRY_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct EnvRestore {
    name: &'static str,
    previous: Option<OsString>,
}

impl EnvRestore {
    fn set(name: &'static str, value: &Path) -> Self {
        let previous = std::env::var_os(name);
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

fn write_registry(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("temp registry dir");
    let path = dir.path().join("fleets.json");
    std::fs::write(&path, body).expect("write registry fixture");
    (dir, path)
}

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

fn registry_body(fleets: &[(&str, &str)]) -> String {
    let fleets = fleets
        .iter()
        .map(|(name, gh_repo)| {
            serde_json::json!({
                "name": name,
                "gh_repo": gh_repo,
                "local": format!("/tmp/{name}"),
                "worktree_dir": format!("wt-{name}"),
                "orch": "orch",
                "workers": [],
                "models": {"orch": "o", "impl": "i", "review": "r"}
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "fleets": fleets }).to_string()
}

fn object_keys(value: &serde_json::Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("repo category map")
        .keys()
        .cloned()
        .collect()
}

fn array_values(value: &serde_json::Value) -> BTreeSet<String> {
    value
        .as_array()
        .expect("repo category list")
        .iter()
        .map(|repo| repo.as_str().expect("repo category string").to_string())
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
async fn fleet_registry_projects_status_path_and_all_fleet_fields() {
    let (_dir, path) = write_registry(
        r#"{
            "fleets": [
                {
                    "name": "corral",
                    "gh_repo": "jirathip-dev/corral",
                    "local": "~/Projects/corral",
                    "worktree_dir": "corral",
                    "orch": "orch-corral",
                    "workers": ["w1", "w2"],
                    "paused": true,
                    "models": {
                        "orch": "codex/deepseek-v4-flash-vision-exp",
                        "impl": "codex/deepseek-v4-flash-vision-exp",
                        "review": "codex/deepseek-v4-flash-vision-exp",
                        "impl_alt": "opencode-go/deepseek-v4-flash",
                        "impl_alt2": "codex/deepseek-v4-flash",
                        "reasoning_effort": {
                            "orch": "medium",
                            "impl": "max",
                            "review": "xhigh",
                            "future_effort": "high"
                        }
                    }
                },
                {
                    "name": "board",
                    "gh_repo": "jirathip-dev/herdr-board",
                    "local": "/opt/board",
                    "worktree_dir": "board",
                    "orch": "orch-board",
                    "workers": [],
                    "models": {"orch": "fable", "impl": "sonnet", "review": "opus"}
                }
            ]
        }"#,
    );
    let _env_guard = REGISTRY_ENV_LOCK.lock().await;
    let _registry_guard = EnvRestore::set("CORRAL_FLEETS_PATH", &path);
    let (_store, app) = app().await;

    let response = app
        .oneshot(Request::get("/fleet-registry").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(body["status"], "ok");
    assert!(body["error"].is_null());
    assert_eq!(body["path"], path.to_string_lossy().as_ref());
    assert_eq!(body["fleets"].as_array().unwrap().len(), 2);

    let corral = &body["fleets"][0];
    assert_eq!(corral["name"], "corral");
    assert_eq!(corral["gh_repo"], "jirathip-dev/corral");
    assert_eq!(corral["local"], "~/Projects/corral");
    assert_eq!(corral["worktree_dir"], "corral");
    assert_eq!(corral["orch"], "orch-corral");
    assert_eq!(corral["workers"], serde_json::json!(["w1", "w2"]));
    assert_eq!(corral["paused"], true);
    assert_eq!(
        corral["models"]["impl"],
        "codex/deepseek-v4-flash-vision-exp"
    );
    assert_eq!(
        corral["models"]["impl_alt"],
        "opencode-go/deepseek-v4-flash"
    );
    assert_eq!(corral["models"]["impl_alt2"], "codex/deepseek-v4-flash");
    assert_eq!(corral["models"]["reasoning_effort"]["orch"], "medium");
    assert_eq!(corral["models"]["reasoning_effort"]["impl"], "max");
    assert_eq!(corral["models"]["reasoning_effort"]["review"], "xhigh");
    assert_eq!(
        corral["models"]["reasoning_effort"]["future_effort"],
        "high"
    );

    let board = &body["fleets"][1];
    assert_eq!(board["name"], "board");
    assert_eq!(board["paused"], false);
    assert!(board["workers"].as_array().unwrap().is_empty());
    assert_eq!(board["models"]["reasoning_effort"], serde_json::Value::Null);
}

#[tokio::test]
async fn fleet_registry_malformed_file_returns_http_200_error_shape() {
    let (_dir, path) = write_registry(r#"{ "fleets": [ oops"#);
    let _env_guard = REGISTRY_ENV_LOCK.lock().await;
    let _registry_guard = EnvRestore::set("CORRAL_FLEETS_PATH", &path);
    let (_store, app) = app().await;

    let response = app
        .oneshot(Request::get("/fleet-registry").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("still JSON");
    assert_eq!(body["status"], "error");
    assert!(body["error"].as_str().is_some_and(|e| !e.is_empty()));
    assert!(body["fleets"].as_array().unwrap().is_empty());
    assert_eq!(body["path"], path.to_string_lossy().as_ref());
}

/// #216: all registry states must retain the live category source. The
/// fixtures deliberately use a fleet name that differs from its canonical
/// `gh_repo` basename: fleet names remain the start-worktree issue keys, but
/// the separate registry category list is canonical and the `/issues` map
/// contains both compatible keys and live-only placeholders.
#[tokio::test]
async fn union_read_model_handles_absent_unloadable_partial_and_full_registry() {
    let live = [Some("primary-repo"), Some("herdr-only"), None];
    let expected_live = BTreeSet::from(["herdr-only".to_string(), "primary-repo".to_string()]);

    // Absent registry: status remains an explicit error, while both read
    // surfaces still expose the live categories and the orphan contributes
    // no guessed category.
    {
        let dir = tempfile::tempdir().expect("absent registry dir");
        let path = dir.path().join("missing-fleets.json");
        let _env_guard = REGISTRY_ENV_LOCK.lock().await;
        let _registry_guard = EnvRestore::set("CORRAL_FLEETS_PATH", &path);
        let (store, app) = app_with_repos(&live).await;
        let issues = get_json(&app, "/issues").await;
        let registry = get_json(&app, "/fleet-registry").await;

        assert_eq!(
            issues["repos"],
            serde_json::json!({
                "herdr-only": [],
                "primary-repo": []
            })
        );
        assert_eq!(registry["status"], "error");
        assert!(registry["fleets"].as_array().unwrap().is_empty());
        assert_eq!(array_values(&registry["repos"]), expected_live);
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

    // Unloadable registry: malformed content has the same live fallback but
    // must remain visibly distinct from a successful empty registry.
    {
        let (_dir, path) = write_registry(r#"{ "fleets": [ oops"#);
        let _env_guard = REGISTRY_ENV_LOCK.lock().await;
        let _registry_guard = EnvRestore::set("CORRAL_FLEETS_PATH", &path);
        let (_store, app) = app_with_repos(&live).await;
        let issues = get_json(&app, "/issues").await;
        let registry = get_json(&app, "/fleet-registry").await;

        assert_eq!(object_keys(&issues["repos"]), expected_live);
        assert_eq!(registry["status"], "error");
        assert!(
            registry["error"]
                .as_str()
                .is_some_and(|error| { error.contains("cannot parse fleet registry") })
        );
        assert_eq!(array_values(&registry["repos"]), expected_live);
    }

    // Partial registry: canonical registry identity and a live-only Herdr
    // identity are both present; no registry entry is fabricated for the
    // live-only repo.
    {
        let (_dir, path) = write_registry(&registry_body(&[(
            "fleet-canonical",
            "owner/canonical-repo",
        )]));
        let _env_guard = REGISTRY_ENV_LOCK.lock().await;
        let _registry_guard = EnvRestore::set("CORRAL_FLEETS_PATH", &path);
        let (_store, app) = app_with_repos(&live).await;
        let issues = get_json(&app, "/issues").await;
        let registry = get_json(&app, "/fleet-registry").await;

        assert_eq!(
            object_keys(&issues["repos"]),
            BTreeSet::from([
                "canonical-repo".to_string(),
                "fleet-canonical".to_string(),
                "herdr-only".to_string(),
                "primary-repo".to_string(),
            ])
        );
        assert_eq!(
            array_values(&registry["repos"]),
            BTreeSet::from([
                "canonical-repo".to_string(),
                "herdr-only".to_string(),
                "primary-repo".to_string(),
            ])
        );
        assert_eq!(registry["fleets"].as_array().unwrap().len(), 1);
    }

    // Full registry: the union includes every canonical registry basename and
    // every live workspace repo, with the shared identity deduplicated.
    {
        let (_dir, path) = write_registry(&registry_body(&[
            ("primary-fleet", "owner/primary-repo"),
            ("registry-fleet", "owner/registry-only"),
        ]));
        let _env_guard = REGISTRY_ENV_LOCK.lock().await;
        let _registry_guard = EnvRestore::set("CORRAL_FLEETS_PATH", &path);
        let (_store, app) = app_with_repos(&live).await;
        let issues = get_json(&app, "/issues").await;
        let registry = get_json(&app, "/fleet-registry").await;

        assert_eq!(
            object_keys(&issues["repos"]),
            BTreeSet::from([
                "herdr-only".to_string(),
                "primary-fleet".to_string(),
                "primary-repo".to_string(),
                "registry-fleet".to_string(),
                "registry-only".to_string(),
            ])
        );
        assert_eq!(
            array_values(&registry["repos"]),
            BTreeSet::from([
                "herdr-only".to_string(),
                "primary-repo".to_string(),
                "registry-only".to_string(),
            ])
        );
        assert_eq!(registry["fleets"].as_array().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn fleet_registry_and_issues_read_the_same_registry_source() {
    let (_dir, path) = write_registry(
        r#"{
            "fleets": [
                {
                    "name": "corral",
                    "gh_repo": "jirathip-dev/corral",
                    "local": "~/Projects/corral",
                    "worktree_dir": "corral",
                    "orch": "orch-corral",
                    "workers": [],
                    "models": {"orch": "a", "impl": "b", "review": "c"}
                },
                {
                    "name": "board",
                    "gh_repo": "jirathip-dev/herdr-board",
                    "local": "/opt/board",
                    "worktree_dir": "board",
                    "orch": "orch-board",
                    "workers": [],
                    "models": {"orch": "a", "impl": "b", "review": "c"}
                }
            ]
        }"#,
    );
    let _env_guard = REGISTRY_ENV_LOCK.lock().await;
    let _registry_guard = EnvRestore::set("CORRAL_FLEETS_PATH", &path);
    let (_store, app) = app().await;

    let issues_response = app
        .clone()
        .oneshot(Request::get("/issues").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(issues_response.status(), StatusCode::OK);
    let issues_bytes = issues_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let issues: serde_json::Value = serde_json::from_slice(&issues_bytes).unwrap();
    assert_eq!(issues["repos"]["corral"], serde_json::json!([]));
    assert_eq!(issues["repos"]["board"], serde_json::json!([]));

    let registry_response = app
        .oneshot(Request::get("/fleet-registry").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(registry_response.status(), StatusCode::OK);
    let bytes = registry_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let registry: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(registry["fleets"][0]["name"], "corral");
    assert_eq!(registry["fleets"][0]["gh_repo"], "jirathip-dev/corral");
    assert_eq!(registry["fleets"][1]["name"], "board");
    assert_eq!(registry["fleets"][1]["gh_repo"], "jirathip-dev/herdr-board");
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
