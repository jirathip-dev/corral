//! #485 integration tests: host-approved read-tail enrollment and live
//! revocation over the REAL router and the REAL local owner unix socket.
//!
//! Frozen contract: `.proposal-485-owner-resolved.md` (sha256 bfa5e392…).
//! All state is disposable (temp dirs, temp socket paths, in-process
//! router); this suite never touches an installed daemon or live grants.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use corrald::adapters::{Adapter, DriveCommand, DriveError};
use corrald::api::{AppState, router};
use corrald::auth::owner::{self, PeerIdentity};
use corrald::auth::test_support::{keypair, sign};
use corrald::auth::{AuthPlane, DeviceRegistry};
use corrald::core::store::Store;
use corrald::drive::{Capability, DriveEnvelope};
use http_body_util::BodyExt;
use tower::ServiceExt;

// ---------------------------------------------------------------- helpers

fn json_body(v: serde_json::Value) -> Body {
    Body::from(serde_json::to_vec(&v).unwrap())
}

async fn read_json(res: axum::response::Response) -> serde_json::Value {
    let body = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
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
/// methods; headers_mut is the typed path) — same helper as tests/auth.rs.
fn with_header(mut req: Request<Body>, name: &'static str, value: &str) -> Request<Body> {
    req.headers_mut().insert(
        axum::http::HeaderName::from_static(name),
        axum::http::HeaderValue::from_str(value).unwrap(),
    );
    req
}

/// Accepts every drive dispatch so these tests exercise AUTH + enrollment,
/// not the adapter (mirrors tests/auth.rs).
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
        Box::pin(async { Ok(Vec::new()) })
    }

    fn knows_agent(&self, _agent_id: &str) -> bool {
        true
    }
}

/// Injected peer identity (frozen C7(g): the denial path is driven through
/// an injectable credential check).
struct FakePeerIdentity(u32);

impl PeerIdentity for FakePeerIdentity {
    fn uid(&self, _stream: &tokio::net::UnixStream) -> std::io::Result<u32> {
        Ok(self.0)
    }
}

struct Harness {
    auth: Arc<AuthPlane>,
    dir: tempfile::TempDir,
    app: axum::Router,
    sock: PathBuf,
    owner_task: tokio::task::JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.owner_task.abort();
    }
}

async fn harness_with(ttl: Duration, identity: Arc<dyn PeerIdentity>) -> Harness {
    let store = Store::new();
    let coalescer = store.clone();
    std::mem::drop(tokio::spawn(async move { coalescer.run_coalescer().await }));
    let dir = tempfile::tempdir().unwrap();
    let auth =
        Arc::new(AuthPlane::load_or_create_with_enroll_ttl(dir.path().to_path_buf(), ttl).unwrap());
    let app = router(AppState {
        store,
        auth: auth.clone(),
        adapter: Arc::new(AcceptAllAdapter),
        replay: Arc::new(corrald::api::drive::ReplayTable::default()),
        issues: Arc::new(corrald::api::issues::IssuesCache::default()),
        provenance: Arc::new(corrald::core::provenance::PromptProvenance::new()),
        cors_origins: Vec::new(),
    });
    let sock = owner::socket_path(dir.path());
    let listener = owner::bind(&sock).await.expect("owner socket binds");
    let owner_task = tokio::spawn(owner::serve_with_identity(listener, auth.clone(), identity));
    Harness {
        auth,
        dir,
        app,
        sock,
        owner_task,
    }
}

async fn harness() -> Harness {
    harness_with(Duration::from_secs(300), Arc::new(owner::RealPeerIdentity)).await
}

async fn owner_call_raw(sock: &Path, bytes: &[u8]) -> serde_json::Value {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::UnixStream::connect(sock)
        .await
        .expect("connect owner socket");
    stream.write_all(bytes).await.unwrap();
    stream.write_all(b"\n").await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).trim().to_string();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("bad owner response {e}: {text}"))
}

async fn owner_call(sock: &Path, req: serde_json::Value) -> serde_json::Value {
    owner_call_raw(sock, &serde_json::to_vec(&req).unwrap()).await
}

async fn mint(h: &Harness) -> serde_json::Value {
    let resp = owner_call(
        &h.sock,
        serde_json::json!({ "op": "mint", "endpoint": "https://host.example.ts.net" }),
    )
    .await;
    assert!(resp["enrollment_id"].as_str().unwrap().starts_with("enr_"));
    resp
}

async fn redeem(
    h: &Harness,
    code: &str,
    pubkey: &[u8; 32],
    name: Option<&str>,
) -> axum::response::Response {
    let mut body = serde_json::json!({
        "v": 1,
        "code": code,
        "public_key": corrald::auth::test_support::public_b64(pubkey),
    });
    if let Some(n) = name {
        body["name"] = serde_json::json!(n);
    }
    h.app
        .clone()
        .oneshot(post("/enroll/redeem", body))
        .await
        .unwrap()
}

fn signed_drive(
    key_id: &str,
    signing: &ed25519_dalek::SigningKey,
    capability: Capability,
) -> serde_json::Value {
    let payload = match capability {
        Capability::ReadTail => serde_json::json!({ "kind": "read_tail", "lines": 50 }),
        Capability::ReadDiff => serde_json::json!({ "kind": "read_diff", "files": [] }),
    };
    let envelope = DriveEnvelope {
        request_id: "enroll-req-1".to_string(),
        capability,
        target: "herdr:agent-a".to_string(),
        payload,
        rev: None,
    };
    serde_json::json!({
        "key_id": key_id,
        "signature": sign(signing, &envelope),
        "envelope": envelope,
    })
}

async fn drive(h: &Harness, body: serde_json::Value) -> axum::response::Response {
    h.app.clone().oneshot(post("/drive", body)).await.unwrap()
}

// ---------------------------------------------------------------- T1: full flow

#[tokio::test]
async fn full_flow_owner_only_approval_grants_read_tail_exactly() {
    let h = harness().await;
    let (signing, pubkey) = keypair();
    let reg_token = h.auth.registry.registration_token();

    // Generic registration stays default-deny (empty grants).
    let res = h
        .app
        .clone()
        .oneshot(post(
            "/register",
            serde_json::json!({
                "token": reg_token,
                "public_key": corrald::auth::test_support::public_b64(&pubkey),
                "name": "iPhone",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let key_id = read_json(res).await["key_id"].as_str().unwrap().to_string();
    assert_eq!(h.auth.registry.get(&key_id).unwrap().grants.len(), 0);

    // Read-only by default: the signed drive is refused (not granted).
    let res = drive(&h, signed_drive(&key_id, &signing, Capability::ReadTail)).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(read_json(res).await["kind"], "not_granted");

    // Owner mints on the LOCAL socket; the payload is the frozen v1 set.
    let minted = mint(&h).await;
    let payload = minted["payload"].as_str().unwrap();
    assert!(
        payload.starts_with("{\"v\":1,\"host_key\":\""),
        "frozen field order: {payload}"
    );
    assert!(payload.contains(&format!(
        "\"host_key\":\"{}\"",
        h.auth.host.public_key_b64()
    )));
    assert!(payload.contains("\"endpoint\":\"https://host.example.ts.net\""));
    assert!(payload.contains("\"scope\":\"read_tail\""));
    assert_eq!(
        payload.matches('"').count() % 2,
        0,
        "sanity: balanced quotes"
    );
    let parsed: serde_json::Value = serde_json::from_str(payload).unwrap();
    assert_eq!(parsed.as_object().unwrap().len(), 6, "no extra fields");
    assert_eq!(parsed["v"], 1);
    assert_eq!(parsed["expires_ts"], minted["expires_ts"]);
    assert_eq!(minted["scope"], serde_json::json!(["read_tail"]));

    // A FRESH device (never registered — enrollment is for new devices)
    // redeems: pending only, no record, nothing granted.
    let (signing_b, pubkey_b) = keypair();
    let key_id_b = corrald::auth::registry::key_id_for(&pubkey_b);
    let code = parsed["code"].as_str().unwrap();
    let res = redeem(&h, code, &pubkey_b, Some("iPhone")).await;
    assert_eq!(res.status(), StatusCode::OK);
    let pending = read_json(res).await;
    assert_eq!(pending["state"], "pending");
    assert_eq!(pending["key_id"], key_id_b);
    assert!(
        h.auth.registry.find_by_public_key(&pubkey_b).is_none(),
        "redeem creates no registry record"
    );

    // Owner sees the pending request with the full key_id.
    let listed = owner_call(&h.sock, serde_json::json!({ "op": "pending" })).await;
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["key_id"], key_id_b);
    assert_eq!(sessions[0]["state"], "redeemed");

    // Owner approves: record committed with read_tail exactly.
    let approved = owner_call(
        &h.sock,
        serde_json::json!({ "op": "approve", "enrollment_id": minted["enrollment_id"] }),
    )
    .await;
    assert_eq!(approved["ok"], true);
    assert_eq!(approved["key_id"], key_id_b);
    assert_eq!(approved["grants"], serde_json::json!(["read_tail"]));
    let rec = h.auth.registry.get(&key_id_b).unwrap();
    assert_eq!(rec.grants, vec![Capability::ReadTail]);

    // Signed read_tail drive now succeeds; read_diff is NOT granted.
    let res = drive(
        &h,
        signed_drive(&key_id_b, &signing_b, Capability::ReadTail),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(read_json(res).await["ok"], true);
    let res = drive(
        &h,
        signed_drive(&key_id_b, &signing_b, Capability::ReadDiff),
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(read_json(res).await["kind"], "not_granted");

    // Status converges to the approved shape with current state.
    let res = h
        .app
        .clone()
        .oneshot(post(
            "/enroll/status",
            serde_json::json!({ "v": 1, "code": code }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let status = read_json(res).await;
    assert_eq!(status["state"], "approved");
    assert_eq!(status["key_id"], key_id_b);
    assert_eq!(status["grants"], serde_json::json!(["read_tail"]));
    assert_eq!(status["revoked"], false);

    // A now-LIVE key re-redeeming is refused: only REVOKED keys re-enter
    // the restore path (frozen C1: live keys keep `already_registered`).
    let second = mint(&h).await;
    let code2 = serde_json::from_str::<serde_json::Value>(second["payload"].as_str().unwrap())
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string();
    let res = redeem(&h, &code2, &pubkey_b, None).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(read_json(res).await["code"], "already_registered");
}

// ---------------------------------------------------------------- T2: replay/conflict

#[tokio::test]
async fn replay_and_conflict_semantics_are_typed() {
    let h = harness().await;
    let (_, pk) = keypair();
    let minted = mint(&h).await;
    let code = serde_json::from_str::<serde_json::Value>(minted["payload"].as_str().unwrap())
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string();

    let res = redeem(&h, &code, &pk, None).await;
    assert_eq!(res.status(), StatusCode::OK);

    // Replay: the same code cannot be redeemed twice (single-use witness).
    let res = redeem(&h, &code, &pk, None).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(read_json(res).await["code"], "enroll_redeemed");

    // Approve twice: 409 carries the approved record (C3 self-heal).
    let approve_req = serde_json::json!({
        "op": "approve",
        "enrollment_id": minted["enrollment_id"],
    });
    let first = owner_call(&h.sock, approve_req.clone()).await;
    assert_eq!(first["ok"], true);
    let second = owner_call(&h.sock, approve_req).await;
    assert_eq!(second["code"], "enroll_already_approved");
    assert_eq!(second["key_id"], first["key_id"]);
    assert_eq!(second["grants"], serde_json::json!(["read_tail"]));
    assert!(second["expiry_ts"].as_u64().unwrap() > 0);

    // Approve before redeem / unknown session are typed.
    let fresh = mint(&h).await;
    let before = owner_call(
        &h.sock,
        serde_json::json!({ "op": "approve", "enrollment_id": fresh["enrollment_id"] }),
    )
    .await;
    assert_eq!(before["code"], "enroll_not_redeemed");
    let unknown = owner_call(
        &h.sock,
        serde_json::json!({ "op": "approve", "enrollment_id": "enr_ffffffffffffffffffffffffffffffff" }),
    )
    .await;
    assert_eq!(unknown["code"], "enroll_unknown_session");

    // Unknown code on the device routes.
    let res = redeem(
        &h,
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        &pk,
        None,
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(read_json(res).await["code"], "enroll_unknown_code");
}

// ---------------------------------------------------------------- T3: status/expiry

#[tokio::test]
async fn status_never_conflicts_and_expiry_precedes_state() {
    let h = harness_with(Duration::from_secs(1), Arc::new(owner::RealPeerIdentity)).await;
    let (_, pk) = keypair();
    let minted = mint(&h).await;
    let code = serde_json::from_str::<serde_json::Value>(minted["payload"].as_str().unwrap())
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string();

    // Before redeem: pending (read-only; never a conflict).
    let res = h
        .app
        .clone()
        .oneshot(post(
            "/enroll/status",
            serde_json::json!({ "v": 1, "code": code }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(read_json(res).await["state"], "pending");

    // Consumed + unapproved: STILL 200 pending — never 409 (frozen C2).
    let res = redeem(&h, &code, &pk, None).await;
    assert_eq!(res.status(), StatusCode::OK);
    let res = h
        .app
        .clone()
        .oneshot(post(
            "/enroll/status",
            serde_json::json!({ "v": 1, "code": code }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(read_json(res).await["state"], "pending");

    // Past the deadline: 410 (expiry wins over the consumed state).
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let res = h
        .app
        .clone()
        .oneshot(post(
            "/enroll/status",
            serde_json::json!({ "v": 1, "code": code }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::GONE);
    assert_eq!(read_json(res).await["code"], "enroll_expired");
    let res = redeem(&h, &code, &pk, None).await;
    assert_eq!(res.status(), StatusCode::GONE);
}

// ---------------------------------------------------------------- T4: wrong host/key race

#[tokio::test]
async fn wrong_host_404_and_two_key_race_binds_one_winner() {
    let a = harness().await;
    let b = harness().await;
    let minted = mint(&a).await;
    let code = serde_json::from_str::<serde_json::Value>(minted["payload"].as_str().unwrap())
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, pk_a) = keypair();

    // A code minted by host A is unknown to host B.
    let res = redeem(&b, &code, &pk_a, None).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(read_json(res).await["code"], "enroll_unknown_code");

    // Racing redemption with two DIFFERENT keys: exactly one 200.
    let (_, pk_b) = keypair();
    let (ra, rb) = tokio::join!(
        redeem(&a, &code, &pk_a, None),
        redeem(&a, &code, &pk_b, None)
    );
    let (sa, sb) = (ra.status(), rb.status());
    assert!(
        (sa == StatusCode::OK && sb == StatusCode::CONFLICT)
            || (sb == StatusCode::OK && sa == StatusCode::CONFLICT),
        "exactly one winner: {sa} / {sb}"
    );
    let winner_pk = if sa == StatusCode::OK { pk_a } else { pk_b };
    let loser_pk = if sa == StatusCode::OK { pk_b } else { pk_a };

    let approved = owner_call(
        &a.sock,
        serde_json::json!({ "op": "approve", "enrollment_id": minted["enrollment_id"] }),
    )
    .await;
    assert_eq!(approved["ok"], true);
    let winner_key = approved["key_id"].as_str().unwrap();
    assert_eq!(winner_key, corrald::auth::registry::key_id_for(&winner_pk));
    // The loser is nowhere in the registry — never bound by approve.
    assert!(a.auth.registry.find_by_public_key(&loser_pk).is_none());
}

// ---------------------------------------------------------------- T5: revoked restore

#[tokio::test]
async fn revoked_key_restores_only_via_fresh_explicit_approval() {
    let h = harness().await;
    let (signing, pk) = keypair();
    let reg_token = h.auth.registry.registration_token();
    // A device that was registered, granted out-of-band, and then revoked
    // with a soon-to-expire registration lifetime.
    let rec = h
        .auth
        .registry
        .register(&reg_token, pk, Duration::from_secs(60))
        .unwrap();
    h.auth
        .registry
        .set_grants(&rec.key_id, vec![Capability::ReadTail])
        .unwrap();
    h.auth.registry.revoke_committed(&rec.key_id).unwrap();
    let old_expiry = h.auth.registry.get(&rec.key_id).unwrap().expiry_ts;

    // Fresh consented enrollment: owner mints, device redeems the REVOKED
    // key -> pending restore request, bound to the known key id.
    let minted = mint(&h).await;
    let code = serde_json::from_str::<serde_json::Value>(minted["payload"].as_str().unwrap())
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string();
    let res = redeem(&h, &code, &pk, Some("iPhone")).await;
    assert_eq!(res.status(), StatusCode::OK);
    let v = read_json(res).await;
    assert_eq!(v["key_id"], rec.key_id);

    // NEVER automatic: still revoked after redeem, drive still refused.
    let still = h.auth.registry.get(&rec.key_id).unwrap();
    assert!(still.revoked, "redeem must not restore");
    let res = drive(
        &h,
        signed_drive(&rec.key_id, &signing, Capability::ReadTail),
    )
    .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // Explicit owner approval restores: read_tail exactly, revoked
    // cleared, revoked_ts cleared, expiry re-stamped to the 90-day TTL.
    let approved = owner_call(
        &h.sock,
        serde_json::json!({ "op": "approve", "enrollment_id": minted["enrollment_id"] }),
    )
    .await;
    assert_eq!(approved["ok"], true);
    let restored = h.auth.registry.get(&rec.key_id).unwrap();
    assert!(!restored.revoked);
    assert_eq!(restored.grants, vec![Capability::ReadTail]);
    assert_eq!(restored.revoked_ts, None);
    assert!(
        restored.expiry_ts > old_expiry,
        "expiry re-stamped (old {old_expiry}, new {})",
        restored.expiry_ts
    );
    assert!(
        restored.expiry_ts >= corrald::auth::registry::now_secs() + 90 * 24 * 3600 - 5,
        "restore grants the 90-day registration TTL"
    );

    // The restored record survives a fresh load from disk.
    let reloaded = DeviceRegistry::load_or_create(h.dir.path()).unwrap();
    let from_disk = reloaded.get(&rec.key_id).unwrap();
    assert!(!from_disk.revoked);
    assert_eq!(from_disk.grants, vec![Capability::ReadTail]);

    // And the device can drive now (read_tail only).
    let res = drive(
        &h,
        signed_drive(&rec.key_id, &signing, Capability::ReadTail),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
}

// ---------------------------------------------------------------- T6: revoke live+persist

#[tokio::test]
async fn revoke_is_immediate_idempotent_and_restart_surviving() {
    let h = harness().await;
    let (signing, pk) = keypair();
    let minted = mint(&h).await;
    let code = serde_json::from_str::<serde_json::Value>(minted["payload"].as_str().unwrap())
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string();
    let res = redeem(&h, &code, &pk, None).await;
    assert_eq!(res.status(), StatusCode::OK);
    let approved = owner_call(
        &h.sock,
        serde_json::json!({ "op": "approve", "enrollment_id": minted["enrollment_id"] }),
    )
    .await;
    let key_id = approved["key_id"].as_str().unwrap().to_string();
    let res = drive(&h, signed_drive(&key_id, &signing, Capability::ReadTail)).await;
    assert_eq!(res.status(), StatusCode::OK);

    // Owner revoke over the local socket: immediate (no restart) and
    // idempotent; unknown keys are typed.
    let revoked = owner_call(
        &h.sock,
        serde_json::json!({ "op": "revoke", "key_id": key_id }),
    )
    .await;
    assert_eq!(revoked["ok"], true);
    assert_eq!(revoked["revoked"], true);
    let res = drive(&h, signed_drive(&key_id, &signing, Capability::ReadTail)).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(read_json(res).await["kind"], "revoked");
    let again = owner_call(
        &h.sock,
        serde_json::json!({ "op": "revoke", "key_id": key_id }),
    )
    .await;
    assert_eq!(again["ok"], true);
    let missing = owner_call(
        &h.sock,
        serde_json::json!({ "op": "revoke", "key_id": "dev_ffffffffffffffffffffffffffffffff" }),
    )
    .await;
    assert_eq!(missing["code"], "unknown_key");

    // Survives a restart (fresh registry load from disk).
    let reloaded = DeviceRegistry::load_or_create(h.dir.path()).unwrap();
    assert!(reloaded.get(&key_id).unwrap().revoked);
}

// ---------------------------------------------------------------- T7: persist failure

#[tokio::test]
async fn persist_failure_is_disk_first_and_retryable() {
    let h = harness().await;
    let (signing, pk) = keypair();
    let minted = mint(&h).await;
    let code = serde_json::from_str::<serde_json::Value>(minted["payload"].as_str().unwrap())
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string();
    let res = redeem(&h, &code, &pk, None).await;
    assert_eq!(res.status(), StatusCode::OK);

    // Inject the F8-style failure: registry.json becomes a directory (a
    // fresh registry may not have written the file yet).
    let reg_path = h.dir.path().join("registry.json");
    if reg_path.exists() {
        std::fs::remove_file(&reg_path).unwrap();
    }
    std::fs::create_dir(&reg_path).unwrap();

    let failed = owner_call(
        &h.sock,
        serde_json::json!({ "op": "approve", "enrollment_id": minted["enrollment_id"] }),
    )
    .await;
    assert_eq!(failed["code"], "registry_persist_failed");
    // Disk-first: NOTHING installed in memory; no record, no grant.
    assert!(h.auth.registry.find_by_public_key(&pk).is_none());
    assert_eq!(h.auth.registry.device_count(), 0);

    // Restore the disk and retry: the session is still retryable.
    std::fs::remove_dir(&reg_path).unwrap();
    let retry = owner_call(
        &h.sock,
        serde_json::json!({ "op": "approve", "enrollment_id": minted["enrollment_id"] }),
    )
    .await;
    assert_eq!(retry["ok"], true, "retry after restore: {retry}");
    let key_id = retry["key_id"].as_str().unwrap().to_string();
    let res = drive(&h, signed_drive(&key_id, &signing, Capability::ReadTail)).await;
    assert_eq!(res.status(), StatusCode::OK);

    // Revoke under the same injection: 500-equivalent typed error, device
    // still live-authorized (loud failure, no silent half-state).
    if reg_path.exists() {
        std::fs::remove_file(&reg_path).unwrap();
    }
    std::fs::create_dir(&reg_path).unwrap();
    let failed = owner_call(
        &h.sock,
        serde_json::json!({ "op": "revoke", "key_id": key_id }),
    )
    .await;
    assert_eq!(failed["code"], "registry_persist_failed");
    assert!(
        !h.auth.registry.get(&key_id).unwrap().revoked,
        "no live change"
    );
    let res = drive(&h, signed_drive(&key_id, &signing, Capability::ReadTail)).await;
    assert_eq!(res.status(), StatusCode::OK, "device still authorized");
    std::fs::remove_dir(&reg_path).unwrap();
}

// ---------------------------------------------------------------- T8: HTTP route absence

#[tokio::test]
async fn http_listener_exposes_no_owner_mutation_routes() {
    let h = harness().await;
    // Every owner operation is route-absent on the network listener —
    // with and without an admin Bearer (no admin path exists either).
    let owner_posts: [(&str, serde_json::Value); 4] = [
        ("/enroll/mint", serde_json::json!({})),
        ("/enroll/pending", serde_json::json!({})),
        (
            "/enroll/approve",
            serde_json::json!({ "enrollment_id": "enr_x" }),
        ),
        ("/revoke", serde_json::json!({ "key_id": "dev_x" })),
    ];
    for (uri, body) in owner_posts {
        let res = h
            .app
            .clone()
            .oneshot(post(uri, body.clone()))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri} must be absent");
        let res = h
            .app
            .clone()
            .oneshot(with_header(
                post(uri, body),
                "authorization",
                "Bearer wrong",
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri} (bearer)");
    }
    // The device routes DO exist (method discipline sanity check).
    let res = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/enroll/redeem")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
}

// ---------------------------------------------------------------- T9: local channel auth

#[tokio::test]
async fn owner_socket_requires_owner_peer_credentials_and_0600() {
    let h = harness().await;
    // Socket file 0600 inside the 0700 dir.
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&h.sock).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "owner socket must be 0600");
    let dir_mode = std::fs::metadata(h.dir.path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "config dir must be 0700");

    // Injected non-owner peer: refused before any request parse, and the
    // request (a mint!) had no effect.
    let foreign = harness_with(
        Duration::from_secs(300),
        Arc::new(FakePeerIdentity(999_999_999)),
    )
    .await;
    // The STRICT read in `owner_call` is deliberate: a denied peer must
    // receive the full typed refusal on every OS. A connection reset (Linux
    // close-with-unread-data) or a lost response FAILS this test instead of
    // being tolerated — hosted rust run 34675447060 caught exactly that;
    // the daemon now drains the peer's pending bytes before closing
    // (src/auth/owner.rs `drain_pending`).
    let refused = owner_call(
        &foreign.sock,
        serde_json::json!({ "op": "mint", "endpoint": "https://host.example.ts.net" }),
    )
    .await;
    assert_eq!(refused["code"], "owner_credential_mismatch");
    assert!(refused.get("enrollment_id").is_none());

    // Malformed requests on the real channel are typed, and the fixed
    // shapes refuse a `grants` parameter anywhere (P3).
    let bad = owner_call_raw(&h.sock, b"not json").await;
    assert_eq!(bad["code"], "malformed_request");
    let unknown = owner_call(&h.sock, serde_json::json!({ "op": "set_grants" })).await;
    assert_eq!(unknown["code"], "malformed_request");
    let grants_param = owner_call(
        &h.sock,
        serde_json::json!({ "op": "revoke", "key_id": "dev_x", "grants": ["read_tail"] }),
    )
    .await;
    assert_eq!(grants_param["code"], "malformed_request");
    let no_endpoint = owner_call(&h.sock, serde_json::json!({ "op": "mint" })).await;
    assert_eq!(no_endpoint["code"], "malformed_request");
    let bad_endpoint = owner_call(
        &h.sock,
        serde_json::json!({ "op": "mint", "endpoint": "http://insecure.ts.net" }),
    )
    .await;
    assert_eq!(bad_endpoint["code"], "malformed_request");
}

// ---------------------------------------------------------------- T10: pending cap

#[tokio::test]
async fn pending_cap_rejects_runaway_minting() {
    let h = harness().await;
    for _ in 0..8 {
        let m = owner_call(
            &h.sock,
            serde_json::json!({ "op": "mint", "endpoint": "https://host.example.ts.net" }),
        )
        .await;
        assert!(m["enrollment_id"].as_str().is_some(), "mint ok: {m}");
    }
    let over = owner_call(
        &h.sock,
        serde_json::json!({ "op": "mint", "endpoint": "https://host.example.ts.net" }),
    )
    .await;
    assert_eq!(over["code"], "enroll_pending_cap");
}
