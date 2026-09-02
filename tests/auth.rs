//! W3 auth + audit plane integration tests (P3, D10/D13):
//!
//! - sign/verify round trips over the contract's canonical envelope bytes;
//! - tamper/unknown/expired/revoked rejection;
//! - read-only default deny (AC3);
//! - hash-chained audit integrity + growth-on-writes-only (AC5);
//! - full HTTP surface: /host-key, /register, /audit, and the /drive auth
//!   seam (AC1), including a spawned-daemon live test.
//!
//! #354: step-up, the mutating capability surface, and the /grants admin
//! surface are removed (route-absent probes included).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use corrald::adapters::{Adapter, DriveCommand, DriveError};
use corrald::api::{AppState, router};
use corrald::auth::test_support::{envelope, keypair, setup, sign, signed};
use corrald::auth::{AuthPlane, DeviceRegistry, HostIdentity, RegisterError};
use corrald::core::store::Store;
use corrald::drive::{
    AuthError, Capability, DriveAuthorizer, DriveEnvelope, SignedDrive, canonical_envelope_bytes,
};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn json_body(v: serde_json::Value) -> Body {
    Body::from(serde_json::to_vec(&v).unwrap())
}

async fn read_json(res: axum::response::Response) -> serde_json::Value {
    let body = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn post(uri: &str, v: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(json_body(v))
        .unwrap()
}

/// Add a header to an already-built request (http::Request has no builder
/// methods; headers_mut is the typed path).
fn with_header(mut req: Request<Body>, name: &'static str, value: &str) -> Request<Body> {
    req.headers_mut().insert(
        axum::http::HeaderName::from_static(name),
        axum::http::HeaderValue::from_str(value).unwrap(),
    );
    req
}

/// POST with the admin bearer token.
fn post_admin(uri: &str, v: serde_json::Value, admin: &str) -> Request<Body> {
    with_header(post(uri, v), "authorization", &bearer(admin))
}

fn get_admin(uri: &str, admin: &str) -> Request<Body> {
    with_header(get(uri), "authorization", &bearer(admin))
}

// ---------------------------------------------------------------- unit: authorizer

#[test]
fn sign_verify_round_trip() {
    let (registry, authorizer, token, _dir) = setup();
    let (signing, pubkey) = keypair();
    let env = envelope("req-1", Capability::ReadTail, "continue");
    registry
        .register(&token, pubkey, Duration::from_secs(3600))
        .unwrap();
    registry
        .set_grants(&registry.records()[0].key_id, vec![Capability::ReadTail])
        .unwrap();
    let sd = signed(&registry, &token, &signing, pubkey, &env);
    let out = authorizer.verify(&sd).unwrap();
    assert_eq!(out.key_id, sd.key_id);
    assert_eq!(out.envelope, env);
}

#[test]
fn canonical_bytes_are_the_locked_signing_format() {
    // Locks the exact bytes a client must reproduce (fixed field order,
    // payload map sorted, rev omitted when None). The drive contract test
    // asserts this equals serde_json::to_vec; here we pin the literal.
    let env = envelope("req-1", Capability::ReadTail, "hi");
    let bytes = canonical_envelope_bytes(&env);
    let literal = br#"{"request_id":"req-1","capability":"read_tail","target":"herdr:agent-a","payload":{"kind":"read_tail","lines":50}}"#;
    assert_eq!(&bytes[..], &literal[..]);
}

#[test]
fn tampered_envelope_is_bad_signature() {
    let (registry, authorizer, token, _dir) = setup();
    let (signing, pubkey) = keypair();
    let env = envelope("req-1", Capability::ReadTail, "continue");
    registry
        .register(&token, pubkey, Duration::from_secs(3600))
        .unwrap();
    registry
        .set_grants(&registry.records()[0].key_id, vec![Capability::ReadTail])
        .unwrap();
    let sd = signed(&registry, &token, &signing, pubkey, &env);

    // Flip the payload text: bytes differ, signature no longer covers them.
    let mut tampered = sd.envelope.clone();
    tampered.payload = serde_json::json!({ "kind": "read_tail", "lines": 1 });
    let sd = SignedDrive {
        envelope: tampered,
        ..sd
    };
    assert_eq!(authorizer.verify(&sd), Err(AuthError::BadSignature));
}

#[test]
fn unknown_key_rejected() {
    let (_, authorizer, _, _dir) = setup();
    let (signing, _) = keypair();
    let env = envelope("req-1", Capability::ReadTail, "hi");
    let sd = SignedDrive {
        key_id: "dev_unknown000000000000000000000000000000".to_string(),
        signature: sign(&signing, &env),
        envelope: env,
    };
    assert_eq!(authorizer.verify(&sd), Err(AuthError::UnknownKey));
}

#[test]
fn revoked_key_rejected() {
    let (registry, authorizer, token, _dir) = setup();
    let (signing, pubkey) = keypair();
    let env = envelope("req-1", Capability::ReadTail, "hi");
    let rec = registry
        .register(&token, pubkey, Duration::from_secs(3600))
        .unwrap();
    registry
        .set_grants(&rec.key_id, vec![Capability::ReadTail])
        .unwrap();
    let sd = signed(&registry, &token, &signing, pubkey, &env);
    assert_eq!(authorizer.verify(&sd).unwrap().key_id, rec.key_id);
    registry.set_revoked(&rec.key_id, true).unwrap();
    assert_eq!(authorizer.verify(&sd), Err(AuthError::Revoked));
}

#[test]
fn expired_key_rejected() {
    let (registry, authorizer, token, _dir) = setup();
    let (signing, pubkey) = keypair();
    let env = envelope("req-1", Capability::ReadTail, "hi");
    // TTL zero: already expired at registration time.
    let rec = registry.register(&token, pubkey, Duration::ZERO).unwrap();
    registry
        .set_grants(&rec.key_id, vec![Capability::ReadTail])
        .unwrap();
    let sd = signed(&registry, &token, &signing, pubkey, &env);
    assert_eq!(authorizer.verify(&sd), Err(AuthError::Expired));
}

#[test]
fn read_only_device_cannot_drive_ac3() {
    let (registry, authorizer, token, _dir) = setup();
    let (signing, pubkey) = keypair();
    let env = envelope("req-1", Capability::ReadTail, "hi");
    let sd = signed(&registry, &token, &signing, pubkey, &env);

    // Default: no grants at all -> every drive capability refused.
    assert_eq!(
        authorizer.verify(&sd),
        Err(AuthError::NotGranted(Capability::ReadTail))
    );

    // A key granted only read_tail still cannot dispatch read_diff:
    // default deny is per capability, not per device.
    let key_id = sd.key_id.clone();
    registry
        .set_grants(&key_id, vec![Capability::ReadTail])
        .unwrap();
    let diff_env = DriveEnvelope {
        request_id: "req-diff".into(),
        capability: Capability::ReadDiff,
        target: "herdr:agent-a".into(),
        payload: serde_json::json!({ "kind": "read_diff" }),
        rev: None,
    };
    let diff_sd = SignedDrive {
        key_id: key_id.clone(),
        signature: sign(&signing, &diff_env),
        envelope: diff_env,
    };
    assert_eq!(
        authorizer.verify(&diff_sd),
        Err(AuthError::NotGranted(Capability::ReadDiff))
    );
    let read_env = DriveEnvelope {
        request_id: "req-2".into(),
        capability: Capability::ReadTail,
        target: "herdr:agent-a".into(),
        payload: serde_json::json!({ "kind": "read_tail", "lines": 10 }),
        rev: None,
    };
    let read_sd = SignedDrive {
        key_id: key_id.clone(),
        signature: sign(&signing, &read_env),
        envelope: read_env,
    };
    assert!(
        authorizer.verify(&read_sd).is_ok(),
        "read_tail is a granted read"
    );

    // Malformed signature (not base64) is a typed error, not a panic.
    let bad = SignedDrive {
        signature: "!!!".to_string(),
        ..sd
    };
    assert_eq!(authorizer.verify(&bad), Err(AuthError::BadSignature));
}

// ---------------------------------------------------------------- unit: registry persistence

#[test]
fn registry_persists_across_reload_with_0600_perms() {
    let dir = tempfile::tempdir().unwrap();
    // Load the full auth plane so every credential file exists for the
    // permission sweep (audit.log is created at AuthPlane load).
    let plane = AuthPlane::load_or_create(dir.path().to_path_buf()).unwrap();
    let reg = plane.registry.clone();
    let token = reg.registration_token();
    let (_, pubkey) = keypair();
    let rec = reg
        .register(&token, pubkey, Duration::from_secs(3600))
        .unwrap();
    reg.set_grants(&rec.key_id, vec![Capability::ReadTail])
        .unwrap();

    let reloaded = DeviceRegistry::load_or_create(dir.path()).unwrap();
    let got = reloaded.get(&rec.key_id).unwrap();
    assert_eq!(got.grants, vec![Capability::ReadTail]);
    assert_eq!(got.public_key, pubkey);
    // Same registration token across restarts.
    assert_eq!(reloaded.registration_token(), token);

    use std::os::unix::fs::PermissionsExt;
    for f in [
        "registry.json",
        "host-key",
        "registration-token",
        "admin-token",
        "audit.log",
    ] {
        let meta = std::fs::metadata(dir.path().join(f)).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600, "{f} must be 0600");
    }
    let dir_mode = std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700, "config dir must be 0700");
}

#[test]
fn host_identity_published_as_x25519_not_hostname() {
    let dir = tempfile::tempdir().unwrap();
    let host = HostIdentity::load_or_create(dir.path()).unwrap();
    assert_eq!(host.algorithm(), "X25519");
    assert_eq!(host.public_key().len(), 32);
    let other = HostIdentity::load_or_create(dir.path()).unwrap();
    assert_eq!(host.public_key(), other.public_key());
}

// ---------------------------------------------------------------- HTTP surface

/// Bounded readiness wait for a spawned daemon (no polling loops beyond
/// readiness).
async fn wait_for_daemon(client: &reqwest::Client, base: &str) -> bool {
    for _ in 0..100 {
        if client.get(format!("{base}/healthz")).send().await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn http_app() -> (Arc<AuthPlane>, tempfile::TempDir, axum::Router) {
    let store = Store::new();
    let coalescer = store.clone();
    std::mem::drop(tokio::spawn(async move { coalescer.run_coalescer().await }));
    let dir = tempfile::tempdir().unwrap();
    let auth = Arc::new(AuthPlane::load_or_create(dir.path().to_path_buf()).unwrap());
    let adapter = Arc::new(AcceptAllAdapter);
    let app = router(AppState {
        store,
        auth: auth.clone(),
        adapter,
        replay: Arc::new(corrald::api::drive::ReplayTable::default()),
        issues: Arc::new(corrald::api::issues::IssuesCache::default()),
        provenance: Arc::new(corrald::core::provenance::PromptProvenance::new()),
        cors_origins: Vec::new(),
    });
    (auth, dir, app)
}

/// Accepts every drive dispatch so these tests exercise the AUTH plane, not
/// the adapter; the drive-adapter behavior itself is covered by tests/drive.rs.
#[derive(Debug, Clone, Copy, Default)]
struct AcceptAllAdapter;

impl Adapter for AcceptAllAdapter {
    fn source(&self) -> &'static str {
        "accept-all"
    }

    fn start(self: Arc<Self>, _store: Store) {}

    fn drive<'a>(
        &'a self,
        _agent_id: &'a str,
        _command: DriveCommand,
    ) -> futures::future::BoxFuture<'a, Result<(), DriveError>> {
        Box::pin(async { Ok(()) })
    }

    fn read_tail<'a>(
        &'a self,
        _agent_id: &'a str,
        _lines: u32,
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, DriveError>> {
        // The read seam (read_tail_since_with_rev) funnels here; an empty
        // tail keeps these tests focused on the AUTH plane.
        Box::pin(async { Ok(Vec::new()) })
    }

    fn knows_agent(&self, _agent_id: &str) -> bool {
        true
    }
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

#[tokio::test]
async fn host_key_endpoint_returns_algorithm_and_key() {
    let (auth, _dir, app) = http_app().await;
    let res = app
        .clone()
        .oneshot(Request::get("/host-key").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = read_json(res).await;
    assert_eq!(v["algorithm"], "X25519");
    assert_eq!(v["public_key"], auth.host.public_key_b64());
    assert!(
        v["public_key"].as_str().unwrap().len() == 44,
        "base64 of 32 bytes"
    );
}

#[tokio::test]
async fn register_endpoint_gates_on_token_and_returns_grants() {
    let (auth, _dir, app) = http_app().await;
    let (_, pubkey) = keypair();
    let pubkey_b64 = corrald::auth::test_support::public_b64(&pubkey);

    // Wrong token -> 401, typed.
    let res = app
        .clone()
        .oneshot(post(
            "/register",
            serde_json::json!({
                "token": "nope",
                "public_key": pubkey_b64,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(read_json(res).await["error"], "bad registration token");

    // Right token -> key_id + empty grants (read-only default).
    let token = auth.registry.registration_token();
    let res = app
        .clone()
        .oneshot(post(
            "/register",
            serde_json::json!({
                "token": token,
                "public_key": pubkey_b64,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = read_json(res).await;
    assert!(v["key_id"].as_str().unwrap().starts_with("dev_"));
    assert_eq!(v["grants"].as_array().unwrap().len(), 0);
    assert_eq!(v["algorithm"], "Ed25519");

    // Malformed key -> 400.
    let res = app
        .clone()
        .oneshot(post(
            "/register",
            serde_json::json!({
                "token": token,
                "public_key": "aGVsbG8=", // 5 bytes
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn register_endpoint_accepts_and_truncates_display_name() {
    let (auth, _dir, app) = http_app().await;
    let token = auth.registry.registration_token();
    let (_, pubkey) = keypair();
    let pubkey_b64 = corrald::auth::test_support::public_b64(&pubkey);

    // Register with a display name -> accepted, stored trimmed.
    let res = app
        .clone()
        .oneshot(post(
            "/register",
            serde_json::json!({
                "token": token,
                "public_key": pubkey_b64,
                "name": "  iPhone 15 Pro  ",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = read_json(res).await;
    let key_id = v["key_id"].as_str().unwrap().to_string();
    assert_eq!(v["grants"].as_array().unwrap().len(), 0);

    // The registry stores the trimmed label (#209 display labels are
    // cosmetic; the /grants admin projection that used to serve it was
    // removed in #354).
    let rec = auth.registry.get(&key_id).expect("registered device");
    assert_eq!(rec.name.as_deref(), Some("iPhone 15 Pro"));

    // Malformed names fail loudly (F10): non-string, empty, control chars.
    for bad in [
        serde_json::json!({ "name": 7 }),
        serde_json::json!({ "token": token, "public_key": pubkey_b64, "name": "   " }),
        serde_json::json!({ "token": token, "public_key": pubkey_b64, "name": "a\nb" }),
    ] {
        let res = app.clone().oneshot(post("/register", bad)).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    // An over-long name is truncated, never blocks enrollment.
    let (_, long_pubkey) = keypair();
    let res = app
        .clone()
        .oneshot(post(
            "/register",
            serde_json::json!({
                "token": token,
                "public_key": corrald::auth::test_support::public_b64(&long_pubkey),
                "name": "x".repeat(100),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let long_key = read_json(res).await["key_id"].as_str().unwrap().to_string();
    let rec = auth.registry.get(&long_key).expect("registered device");
    let name = rec.name.as_deref().unwrap();
    assert_eq!(name.len(), corrald::auth::registry::MAX_DEVICE_NAME_CHARS);
    assert_eq!(name, "x".repeat(corrald::auth::registry::MAX_DEVICE_NAME_CHARS));
}

#[tokio::test]
async fn audit_requires_admin_token_and_grants_surface_is_route_absent() {
    let (auth, _dir, app) = http_app().await;
    let admin = corrald::auth::admin_token_for_test(&auth);

    let res = app
        .clone()
        .oneshot(Request::get("/audit").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = app
        .clone()
        .oneshot(with_header(
            get("/audit"),
            "authorization",
            &bearer("wrong"),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = app
        .clone()
        .oneshot(with_header(get("/audit"), "authorization", &bearer(&admin)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = read_json(res).await;
    assert_eq!(v["entries"].as_array().unwrap().len(), 0);
    assert_eq!(v["valid"], true);

    // The /grants admin surface is gone (#354): even a valid admin token
    // gets 404 for the mutation and the projection.
    let res = app
        .clone()
        .oneshot(post(
            "/grants",
            serde_json::json!({
                "action": "set_grants", "key_id": "dev_x", "grants": ["read_tail"],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let res = app
        .clone()
        .oneshot(post_admin(
            "/grants",
            serde_json::json!({
                "action": "set_grants", "key_id": "dev_x", "grants": ["read_tail"],
            }),
            &admin,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let res = app
        .clone()
        .oneshot(get_admin("/grants", &admin))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn drive_seam_full_flow_ac1_and_ac3_over_http() {
    let (auth, _dir, app) = http_app().await;
    let admin = corrald::auth::admin_token_for_test(&auth);
    let reg_token = auth.registry.registration_token();

    // Register a scratch device (read-only default).
    let (signing, pubkey) = keypair();
    let res = app
        .clone()
        .oneshot(post(
            "/register",
            serde_json::json!({
                "token": reg_token,
                "public_key": corrald::auth::test_support::public_b64(&pubkey),
            }),
        ))
        .await
        .unwrap();
    let key_id = read_json(res).await["key_id"].as_str().unwrap().to_string();

    let env = serde_json::json!({
        "request_id": "live-1",
        "capability": "read_tail",
        "target": "herdr:agent-a",
        "payload": { "kind": "read_tail", "lines": 50 },
    });
    let envelope: DriveEnvelope = serde_json::from_value(env).unwrap();
    let sd = SignedDrive {
        key_id: key_id.clone(),
        signature: sign(&signing, &envelope),
        envelope,
    };

    // AC3: read-only device cannot drive -> NotGranted, and NOT audited.
    let res = app
        .clone()
        .oneshot(post("/drive", serde_json::to_value(&sd).unwrap()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let v = read_json(res).await;
    assert_eq!(v["kind"], "not_granted");

    // Promote via the registry API (the /grants HTTP surface was removed
    // in #354; provisioning is out-of-band) -> drive executes.
    auth.registry
        .set_grants(&key_id, vec![Capability::ReadTail])
        .unwrap();

    let res = app
        .clone()
        .oneshot(post("/drive", serde_json::to_value(&sd).unwrap()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = read_json(res).await;
    assert_eq!(v["ok"], true);
    assert_eq!(v["request_id"], "live-1");

    // Tampered envelope -> BadSignature (AC1), 401, NOT audited.
    let mut tampered = sd.clone();
    tampered.envelope.payload = serde_json::json!({ "kind": "read_tail", "lines": 1 });
    let res = app
        .clone()
        .oneshot(post("/drive", serde_json::to_value(&tampered).unwrap()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(read_json(res).await["kind"], "bad_signature");

    // AC5: exactly one executed write in the log.
    let res = app
        .clone()
        .oneshot(with_header(get("/audit"), "authorization", &bearer(&admin)))
        .await
        .unwrap();
    let v = read_json(res).await;
    assert_eq!(v["valid"], true);
    assert_eq!(v["entries"].as_array().unwrap().len(), 1);
    assert_eq!(v["entries"][0]["outcome"], "executed");
    assert_eq!(v["entries"][0]["key_id"], key_id);
    assert_eq!(v["entries"][0]["request_id"], "live-1");
    assert!(v["head"].as_str().unwrap().len() == 64);
}

#[tokio::test]
async fn revoke_takes_effect_immediately_on_next_drive() {
    let (auth, _dir, app) = http_app().await;
    let reg_token = auth.registry.registration_token();

    let (signing, pubkey) = keypair();
    let res = app
        .clone()
        .oneshot(post(
            "/register",
            serde_json::json!({
                "token": reg_token,
                "public_key": corrald::auth::test_support::public_b64(&pubkey),
            }),
        ))
        .await
        .unwrap();
    let key_id = read_json(res).await["key_id"].as_str().unwrap().to_string();
    auth.registry
        .set_grants(&key_id, vec![Capability::ReadTail])
        .unwrap();

    let env = serde_json::json!({
        "request_id": "r-1", "capability": "read_tail",
        "target": "herdr:agent-a",
        "payload": { "kind": "read_tail", "lines": 50 },
    });
    let envelope: DriveEnvelope = serde_json::from_value(env).unwrap();
    let sd = SignedDrive {
        key_id: key_id.clone(),
        signature: sign(&signing, &envelope),
        envelope,
    };
    assert_eq!(
        app.clone()
            .oneshot(post("/drive", serde_json::to_value(&sd).unwrap()))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    // Revoke (registry API, out-of-band since #354) -> next drive refused,
    // immediately.
    auth.registry.set_revoked(&key_id, true).unwrap();
    let res = app
        .clone()
        .oneshot(post("/drive", serde_json::to_value(&sd).unwrap()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(read_json(res).await["kind"], "revoked");
}

/// F11: GET /host-key must not disclose the config-dir path.
#[tokio::test]
async fn f11_host_key_has_no_path_disclosure() {
    let (_auth, _dir, app) = http_app().await;
    let res = app.clone().oneshot(get("/host-key")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = read_json(res).await;
    assert_eq!(v["algorithm"], "X25519");
    assert!(
        v.get("key_file").is_none() && v.get("path").is_none(),
        "no filesystem path may be disclosed: {v}"
    );
}

#[tokio::test]
async fn debug_output_never_leaks_secrets() {
    let (auth, _dir, _app) = http_app().await;
    let dbg = format!("{auth:?}");
    assert!(
        !dbg.contains(&corrald::auth::admin_token_for_test(&auth)),
        "admin token leaked"
    );
    assert!(
        !dbg.contains(&auth.registry.registration_token()),
        "registration token leaked"
    );
    let _ = &dbg;

    // Registry Debug: counts + ids only.
    let (registry, _, token, _d) = setup();
    let (_, pubkey) = keypair();
    let rec = registry
        .register(&token, pubkey, Duration::from_secs(3600))
        .unwrap();
    let dbg = format!("{registry:?}");
    assert!(dbg.contains(&rec.key_id));
    assert!(!dbg.contains(&token));
    assert_eq!(
        register_with_bad_token(&registry, &pubkey),
        Err(RegisterError::BadToken)
    );
}

fn register_with_bad_token(
    registry: &DeviceRegistry,
    pubkey: &[u8; 32],
) -> Result<(), RegisterError> {
    registry
        .register("bogus", *pubkey, Duration::from_secs(1))
        .map(|_| ())
}

// ---------------------------------------------------------------- live daemon test

/// Spawns the real daemon binary (release or test profile via
/// `CARGO_BIN_EXE_corrald`) with a scratch config dir and drives the full
/// AC1/AC3/AC5 loop over real HTTP: register -> read-only refusal ->
/// promote -> sign -> execute -> tamper -> refused -> audit chain grows
/// only on writes.
#[tokio::test]
async fn live_daemon_self_test() {
    use std::process::Command;

    let bin = std::env::var("CARGO_BIN_EXE_corrald").expect("corrald binary");
    let dir = tempfile::tempdir().unwrap();
    let port = 18400u16 + (std::process::id() as u16) % 1000;

    let spawn_daemon = || -> std::process::Child {
        Command::new(&bin)
            .env("CORRAL_CONFIG_DIR", dir.path())
            .env("CORRAL_REPO_ROOT", dir.path())
            .env("CORRAL_WORKTREES_ROOT", dir.path())
            .arg("--port")
            .arg(port.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn daemon")
    };

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let mut child = spawn_daemon();
    assert!(
        wait_for_daemon(&client, &base).await,
        "daemon did not come up"
    );

    // Load host-side credentials from the scratch config dir.
    let auth = AuthPlane::load_or_create(dir.path().to_path_buf()).unwrap();
    let reg_token = auth.registry.registration_token();
    let admin = corrald::auth::admin_token_for_test(&auth);

    // 1. Host identity (AC: not a hostname).
    let v: serde_json::Value = client
        .get(format!("{base}/host-key"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["algorithm"], "X25519");
    assert_eq!(v["public_key"], auth.host.public_key_b64());

    // 2. Register a scratch device.
    let (signing, pubkey) = keypair();
    let v: serde_json::Value = client
        .post(format!("{base}/register"))
        .json(&serde_json::json!({ "token": reg_token, "public_key": corrald::auth::test_support::public_b64(&pubkey) }))
        .send().await.unwrap().json().await.unwrap();
    let key_id = v["key_id"].as_str().unwrap().to_string();

    let envelope: DriveEnvelope = serde_json::from_value(serde_json::json!({
        "request_id": "live-daemon-1",
        "capability": "read_tail",
        "target": "herdr:agent-a",
        "payload": { "kind": "read_tail", "lines": 50 },
    }))
    .unwrap();
    let sd = SignedDrive {
        key_id: key_id.clone(),
        signature: sign(&signing, &envelope),
        envelope,
    };

    // 3. AC3: read-only default -> drive refused with NotGranted.
    let res = client
        .post(format!("{base}/drive"))
        .json(&sd)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);
    assert_eq!(
        res.json::<serde_json::Value>().await.unwrap()["kind"],
        "not_granted"
    );

    // 4. Provision read_tail out-of-band (the /grants HTTP surface was
    // removed in #354): registry writes apply on daemon restart, so stop
    // the daemon, grant on the reloaded registry file, and respawn.
    child.kill().expect("kill daemon");
    let _ = child.wait();
    let auth = AuthPlane::load_or_create(dir.path().to_path_buf()).unwrap();
    auth.registry
        .set_grants(&key_id, vec![Capability::ReadTail])
        .unwrap();
    child = spawn_daemon();
    assert!(
        wait_for_daemon(&client, &base).await,
        "daemon did not come up after restart"
    );

    // The signed drive is accepted by auth (AC1). The real herdr adapter
    // refuses the synthetic target as unknown — that typed dispatch
    // refusal is still a write attempt: audited, ok:false, 200.
    let res = client
        .post(format!("{base}/drive"))
        .json(&sd)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let ok_v = res.json::<serde_json::Value>().await.unwrap();
    assert_eq!(
        ok_v["ok"], false,
        "unknown-agent refusal rides the DriveResponse: {ok_v}"
    );
    assert_eq!(ok_v["error"], "unknown agent: herdr:agent-a");

    // 5. Tampered envelope -> refused (AC1), and NOT audited.
    let mut tampered = sd.clone();
    tampered.envelope.payload = serde_json::json!({ "kind": "read_tail", "lines": 1 });
    let res = client
        .post(format!("{base}/drive"))
        .json(&tampered)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);

    // 6. AC5: audit grew by exactly one (the executed write), chain valid.
    let v: serde_json::Value = client
        .get(format!("{base}/audit"))
        .header("Authorization", format!("Bearer {admin}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["valid"], true);
    let n1 = v["entries"].as_array().unwrap().len();
    assert_eq!(n1, 1, "only the execution is logged: {v}");

    // Reads (GET /audit + GET /snapshot) do not grow the log.
    client
        .get(format!("{base}/audit"))
        .header("Authorization", format!("Bearer {admin}"))
        .send()
        .await
        .unwrap();
    client.get(format!("{base}/snapshot")).send().await.unwrap();
    let v: serde_json::Value = client
        .get(format!("{base}/audit"))
        .header("Authorization", format!("Bearer {admin}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        v["entries"].as_array().unwrap().len(),
        n1,
        "GETs must not grow the audit log"
    );

    // One more write -> exactly one more entry, still chained.
    let envelope2: DriveEnvelope = serde_json::from_value(serde_json::json!({
        "request_id": "live-daemon-2",
        "capability": "read_tail",
        "target": "herdr:agent-a",
        "payload": { "kind": "read_tail", "lines": 50 },
    }))
    .unwrap();
    let sd2 = SignedDrive {
        key_id: key_id.clone(),
        signature: sign(&signing, &envelope2),
        envelope: envelope2,
    };
    client
        .post(format!("{base}/drive"))
        .json(&sd2)
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = client
        .get(format!("{base}/audit"))
        .header("Authorization", format!("Bearer {admin}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["entries"].as_array().unwrap().len(), n1 + 1);
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(entries[1]["prev"], entries[0]["hash"], "hash-chained");

    // Unauthenticated /audit stays locked.
    let res = client.get(format!("{base}/audit")).send().await.unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);

    child.kill().expect("kill daemon");
    let _ = child.wait();
}
