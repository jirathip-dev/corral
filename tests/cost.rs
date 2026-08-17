//! G34: `GET /cost` — shape, missing-store resilience, and a D-083
//! end-to-end check that chat content injected into a fixture transcript
//! never reaches the response body. Runs in its own process (a separate
//! `tests/*.rs` binary), so the `CORRAL_*_DIR`/`CORRAL_OPENCODE_DB` env
//! overrides below cannot race with the lib's own unit tests.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use corrald::api::{router, AppState};
use corrald::core::store::Store;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

async fn app() -> axum::Router {
    let store = Store::new();
    let coalescer = store.clone();
    std::mem::drop(tokio::spawn(async move { coalescer.run_coalescer().await }));
    router(AppState { store, ..Default::default() })
}

/// Both tests below set the same process-wide `CORRAL_*` env vars that the
/// `/cost` handler reads; serialize them so they can't observe each
/// other's overrides mid-request.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn get_cost(app: axum::Router) -> Value {
    let response = app
        .oneshot(Request::builder().uri("/cost").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).expect("valid JSON")
}

#[tokio::test]
async fn shape_has_all_three_providers_and_all_three_windows() {
    let _guard = ENV_LOCK.lock().await;
    unsafe {
        std::env::set_var("CORRAL_OPENCODE_DB", "/nonexistent/g34-shape/opencode.db");
        std::env::set_var("CORRAL_CLAUDE_DIR", "/nonexistent/g34-shape/claude");
        std::env::set_var("CORRAL_CODEX_DIR", "/nonexistent/g34-shape/codex");
    }
    let body = get_cost(app().await).await;

    let providers = body["providers"].as_array().expect("providers array");
    assert_eq!(providers.len(), 3);
    let names: Vec<&str> = providers.iter().map(|p| p["provider"].as_str().unwrap()).collect();
    assert!(names.contains(&"opencode"));
    assert!(names.contains(&"claude"));
    assert!(names.contains(&"codex"));

    for provider in providers {
        assert_eq!(provider["store_found"], false, "provider {provider} should report no store");
        let windows = provider["windows"].as_array().expect("windows array");
        assert_eq!(windows.len(), 3);
        let window_names: Vec<&str> = windows.iter().map(|w| w["window"].as_str().unwrap()).collect();
        assert!(window_names.contains(&"five_hour"));
        assert!(window_names.contains(&"weekly"));
        assert!(window_names.contains(&"monthly"));
        for w in windows {
            assert_eq!(w["usd"], 0.0);
            assert_eq!(w["status"], "ok", "an absent store must never itself trigger an alert");
            assert!(w["cap_is_placeholder"].as_bool().unwrap(), "no caps configured in this test");
        }
    }
}

#[tokio::test]
async fn d083_injected_message_content_never_reaches_the_response_body() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().expect("tmpdir");
    let claude_dir = tmp.path().join("claude-projects");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let secret = "sk-ant-totally-real-secret-do-not-leak-this-6f19c2";
    let line = serde_json::json!({
        "type": "assistant",
        "cwd": "/repo",
        "timestamp": "2026-08-17T05:48:59.202Z",
        "message": {
            "model": "claude-opus-5",
            "usage": {"input_tokens": 100, "output_tokens": 50},
            "content": [{"type": "text", "text": secret}],
        },
    });
    std::fs::write(claude_dir.join("s.jsonl"), format!("{line}\n")).unwrap();

    unsafe {
        std::env::set_var("CORRAL_OPENCODE_DB", "/nonexistent/g34-d083/opencode.db");
        std::env::set_var("CORRAL_CLAUDE_DIR", claude_dir.to_str().unwrap());
        std::env::set_var("CORRAL_CODEX_DIR", "/nonexistent/g34-d083/codex");
    }
    let response = app()
        .await
        .oneshot(Request::builder().uri("/cost").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let raw = response.into_body().collect().await.unwrap().to_bytes();
    let raw_str = String::from_utf8_lossy(&raw);

    assert!(!raw_str.contains(secret), "chat content must never egress through /cost");
    // The window did register spend, proving the reader actually parsed
    // the fixture rather than silently no-opping past it.
    let body: Value = serde_json::from_str(&raw_str).unwrap();
    let claude = body["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["provider"] == "claude")
        .unwrap();
    assert!(claude["store_found"].as_bool().unwrap());
    let monthly = claude["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["window"] == "monthly")
        .unwrap();
    assert!(monthly["usd"].as_f64().unwrap() > 0.0, "the fixture line must be priced");
}
