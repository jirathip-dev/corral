//! D16 push notifier end-to-end (issue #26): the real HTTP router, a real
//! registry, and a MOCK provider (never real APNs) — transition → payload
//! → provider call, with the acceptance timing assertion: a blocked
//! transition must reach the provider within ~10s.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use corrald::api::{AppState, router};
use corrald::auth::test_support;
use corrald::core::model::{Agent, AgentState, Change, WaitingOn, WaitingOnKind, Workspace};
use corrald::push::Notifier;
use corrald::push::config::Config;
use corrald::push::payload::{DeviceTokenRequest, canonical_device_token_bytes};
use corrald::push::provider::MockProvider;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

fn test_config() -> Config {
    Config {
        team_id: "t".to_string(),
        key_id: "k".to_string(),
        auth_key_path: String::new(),
        endpoint: corrald::push::config::Endpoint::Sandbox,
        topic: "com.corral.fleetnotifier".to_string(),
    }
}

/// A valid-format APNs token (64 lowercase hex chars — N13).
const DEVICE_TOKEN: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

fn blocked_agent(id: &str, hash: &str, prompt: &str) -> Agent {
    Agent {
        agent_id: id.to_string(),
        source: "herdr".to_string(),
        tool: "claude".to_string(),
        state: AgentState::Blocked,
        reason: None,
        seq: 1,
        ts: 1,
        capabilities: vec!["approve".to_string()],
        waiting_on: Some(WaitingOn {
            kind: WaitingOnKind::Menu,
            prompt: prompt.to_string(),
            prompt_hash: hash.to_string(),
            approval_id: format!("{id}:{hash}"),
            choices: vec!["y".to_string(), "n".to_string()],
        }),
        cost: None,
        parent_id: None,
        host: None,
        workspace: Workspace {
            repo: Some("jirathip-k/corral".to_string()),
            branch: Some("g26/apns-push".to_string()),
            ..Default::default()
        },
        attachment: None,
        display_name: Some("builder".to_string()),
        title: None,
    }
}

/// Register a device via the real `/register` route, then enroll its push
/// token via the real `/device-token` route (signed request, the same
/// proof of possession the iOS app performs).
async fn enroll_device(app: &Router, state: &AppState) -> (String, ed25519_dalek::SigningKey) {
    let (signing, pubkey) = test_support::keypair();
    let token = state.auth.registry.registration_token();
    let body = json!({
        "token": token,
        "public_key": test_support::public_b64(&pubkey),
    });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let key_id = {
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice::<Value>(&bytes).unwrap()["key_id"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let request = DeviceTokenRequest {
        key_id: key_id.clone(),
        device_token: DEVICE_TOKEN.to_string(),
        ts: corrald::auth::registry::now_secs(),
    };
    let signature = test_support::sign_bytes(&signing, &canonical_device_token_bytes(&request));
    let body = json!({
        "key_id": key_id,
        "signature": signature,
        "request": request,
    });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/device-token")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    (key_id, signing)
}

/// Acceptance #1 + #4: a blocked transition reaches the (mock) provider
/// with the D16 blocked payload well within ~10s, and the follow-up done
/// transition pushes the plain completion.
#[tokio::test]
async fn blocked_transition_reaches_provider_within_10s_then_done_pushes() {
    let state = AppState::default();
    let app = router(state.clone());
    let _ = enroll_device(&app, &state).await;

    // The notifier is armed manually with the mock provider (the real
    // daemon arms it from CORRAL_APNS_* env inside router()).
    let (provider, mut received) = MockProvider::new();
    let notifier = Arc::new(Notifier::new(
        state.store.clone(),
        state.auth.registry.clone(),
        provider,
        test_config(),
    ));
    notifier.clone().start();
    notifier.ready().await;

    // Real coalescer: the same 250ms-foreground cadence main.rs runs.
    let coalescer = state.store.clone();
    tokio::spawn(async move { coalescer.run_coalescer().await });

    let started = std::time::Instant::now();
    state
        .store
        .apply(Change::upsert(blocked_agent(
            "herdr:ses-e2e",
            "sha256:e2e",
            "ship it? [y/n]",
        )))
        .await;

    let (token, payload) = tokio::time::timeout(Duration::from_secs(10), received.recv())
        .await
        .expect("ACCEPTANCE #1: blocked -> push within ~10s of the transition")
        .expect("provider channel closed");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "push arrived in {:?} (well inside the 10s budget)",
        started.elapsed()
    );
    assert_eq!(token, DEVICE_TOKEN);
    assert_eq!(payload["type"], "blocked");
    assert_eq!(payload["agent_id"], "herdr:ses-e2e");
    assert_eq!(payload["prompt_hash"], "sha256:e2e");
    assert_eq!(payload["approval_id"], "herdr:ses-e2e:sha256:e2e");
    assert_eq!(payload["choices"], json!(["y", "n"]));
    assert_eq!(payload["aps"]["alert"]["body"], "ship it? [y/n]");
    assert_eq!(payload["aps"]["category"], "AGENT_BLOCKED");

    // ACCEPTANCE #4: done -> plain completion push (no reply surface).
    let mut done = blocked_agent("herdr:ses-e2e", "sha256:e2e", "ship it? [y/n]");
    done.state = AgentState::Done;
    done.waiting_on = None;
    state.store.apply(Change::upsert(done)).await;
    let (_token, done_payload) = tokio::time::timeout(Duration::from_secs(10), received.recv())
        .await
        .expect("ACCEPTANCE #4: done -> completion push within 10s")
        .expect("provider channel closed");
    assert_eq!(done_payload["type"], "done");
    assert_eq!(done_payload["agent_id"], "herdr:ses-e2e");
    assert!(
        done_payload["aps"].get("category").is_none(),
        "done pushes carry no action category"
    );
}

/// The token registry is the notifier's delivery filter: a device that
/// enrolled a token gets pushes; an un-enrolled (or revoked) device gets
/// none — proven through the real routes.
#[tokio::test]
async fn only_enrolled_live_devices_receive_pushes() {
    let state = AppState::default();
    let app = router(state.clone());
    let (key_id, _signing) = enroll_device(&app, &state).await;

    let (provider, mut received) = MockProvider::new();
    let notifier = Arc::new(Notifier::new(
        state.store.clone(),
        state.auth.registry.clone(),
        provider,
        test_config(),
    ));
    notifier.clone().start();
    notifier.ready().await;
    let coalescer = state.store.clone();
    tokio::spawn(async move { coalescer.run_coalescer().await });

    // Revoke the enrolled device: no push may leave the machine.
    state.auth.registry.set_revoked(&key_id, true).unwrap();
    state
        .store
        .apply(Change::upsert(blocked_agent(
            "herdr:revoked",
            "sha256:r",
            "go?",
        )))
        .await;
    assert!(
        tokio::time::timeout(Duration::from_millis(600), received.recv())
            .await
            .is_err(),
        "revoked device must not receive a push"
    );
}
