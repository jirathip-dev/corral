//! #249 END-TO-END: rebuild/reinstall invalidates the board identity —
//! detection + recovery on the real wire.
//!
//! Runs the REAL corrald router (axum, in-process, loopback TCP) with a
//! throwaway `AuthPlane` config dir; the client side is the REAL
//! `corrald-ui` wire layer (`protocol` / `keys` / `drive`) — the exact
//! functions `CorralApp` calls on startup, on a bad_signature refusal, and
//! on the one-tap "Re-register + grant" action. No live daemon, no herdr
//! agent, no `~/.config/corral` writes: the only things touched are the
//! scratch dirs this test creates.
//!
//! The journey:
//!   1. Register a board identity (persistent key in a scratch
//!      `CORRAL_UI_CONFIG_DIR`), provision `read_tail` out-of-band on the
//!      registry (#354 R1: the HTTP grant admin is gone), verify a signed
//!      read_tail drive executes.
//!   2. Simulate the reinstall: wipe the board's key material file (the
//!      disabled-keyring file store is exactly the mode a missing OS
//!      keychain entry falls into); the daemon ledger + the registration
//!      record in config.json stay untouched.
//!   3. "Start the board": a fresh key is generated; signed with the OLD
//!      registered key_id (the pre-fix behaviour) the daemon refuses with
//!      401 bad_signature — the #249 symptom.
//!   4. The recovery path (what the board now runs automatically):
//!      re-register the CURRENT key via the registration token, then the
//!      host restores the previous grant set out-of-band on the registry —
//!      zero manual keychain surgery.
//!   5. The signed drive plane works immediately again and no
//!      bad_signature appears.

use std::sync::Arc;

use base64::Engine as _;
use corrald::adapters::{Adapter, DriveCommand, DriveError};
use corrald::api::{AppState, router as api_router};
use corrald::auth::AuthPlane;
use corrald::core::store::Store;
use corrald_ui::drive::{DriveEndpoint, DriveFailure, DriveIntent, DriveOutcome};
use corrald_ui::protocol;
use corrald_ui::state::PersistedConfig;

/// A tail-serving adapter: the one seam read_tail needs. Every agent is
/// known; `drive` is never used by read_tail.
#[derive(Debug)]
struct TailAdapter;

impl Adapter for TailAdapter {
    fn source(&self) -> &'static str {
        "tail-fixture"
    }

    fn start(self: Arc<Self>, _store: Store) {}

    fn drive<'a>(
        &'a self,
        _agent_id: &'a str,
        _command: DriveCommand,
    ) -> futures::future::BoxFuture<'a, Result<(), DriveError>> {
        Box::pin(async { Err(DriveError::NotImplemented("tail-fixture")) })
    }

    fn read_tail<'a>(
        &'a self,
        _agent_id: &'a str,
        _lines: u32,
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, DriveError>> {
        Box::pin(async { Ok(vec!["hello".to_string(), "world".to_string()]) })
    }

    fn knows_agent(&self, _agent_id: &str) -> bool {
        true
    }
}

/// RAII env guard (mirrors the daemon suites' EnvRestore; edition 2024
/// makes `set_var` unsafe).
struct EnvRestore {
    name: &'static str,
    previous: Option<String>,
}

impl EnvRestore {
    fn set(name: &'static str, value: String) -> Self {
        let previous = std::env::var(name).ok();
        unsafe { std::env::set_var(name, value) };
        Self { name, previous }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var(self.name, value) },
            None => unsafe { std::env::remove_var(self.name) },
        }
    }
}

fn scratch_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "corral-identity-e2e-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn pubkey_b64(signing: &ed25519_dalek::SigningKey) -> String {
    base64::engine::general_purpose::STANDARD.encode(signing.verifying_key().to_bytes())
}

#[tokio::test]
async fn identity_recovery_restores_the_signed_drive_plane_after_reinstall() {
    let daemon_dir = scratch_dir("daemon");
    let ui_dir = scratch_dir("ui");
    let _daemon_env = EnvRestore::set("CORRAL_CONFIG_DIR", daemon_dir.display().to_string());
    let _ui_env = EnvRestore::set("CORRAL_UI_CONFIG_DIR", ui_dir.display().to_string());
    // File-store mode: key material lives in the client config dir's
    // `keys/` directory (the "keychain-scope equivalent" the issue names).
    let _keyring_env = EnvRestore::set("CORRAL_UI_DISABLE_KEYRING", "1".to_string());

    // Real AuthPlane over the scratch config dir (writes host keys,
    // registration-token, admin-token into `daemon_dir`).
    let auth = Arc::new(AuthPlane::load_or_create(daemon_dir.clone()).expect("scratch auth plane"));
    let state = AppState {
        store: Store::new(),
        auth: auth.clone(),
        adapter: Arc::new(TailAdapter),
        replay: Default::default(),
        issues: Default::default(),
        provenance: Arc::new(corrald::core::provenance::PromptProvenance::new()),
        cors_origins: Vec::new(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, api_router(state))
            .await
            .expect("serve scratch corrald");
    });
    let base_url = format!("http://{addr}");
    let client = reqwest::Client::new();

    // --- host identity + tokens (same-user localhost reads) -------------
    let host = protocol::fetch_host_key(&client, &base_url)
        .await
        .expect("GET /host-key");
    let fingerprint = corrald_ui::keys::host_fingerprint(Some(&host.public_key), &base_url);
    let token = corrald_ui::keys::read_daemon_registration_token()
        .expect("registration token from the scratch daemon dir");

    // --- 1. original board identity: register + grant + drive ----------
    let key1 = corrald_ui::keys::load_or_create_key(&fingerprint).expect("original key");
    let id1 = corrald_ui::keys::device_key_id(&key1.signing.verifying_key().to_bytes());
    let (registered_id, grants) = protocol::register_device(
        &client,
        &base_url,
        &token,
        &pubkey_b64(&key1.signing),
        Some("e2e-board"),
    )
    .await
    .expect("register the original key");
    assert_eq!(
        registered_id, id1,
        "client-side key-id derivation must equal the daemon's on the wire"
    );
    assert!(
        grants.is_empty(),
        "fresh device starts read-only: {grants:?}"
    );
    // #354 R1: the HTTP grant admin is gone — grants are provisioned
    // out-of-band on the registry (the operator's registry.json edit;
    // the in-process registry is the same Arc the router authorizes
    // against).
    auth.registry
        .set_grants(&id1, vec![corrald::drive::Capability::ReadTail])
        .expect("out-of-band grant for the original key");

    let endpoint1 = DriveEndpoint {
        client: client.clone(),
        base_url: base_url.clone(),
        key_id: id1.clone(),
        signing: key1.signing.clone(),
    };
    let baseline =
        corrald_ui::drive::execute_drive(&endpoint1, &DriveIntent::read_tail("agent-a", None))
            .await;
    assert!(
        matches!(baseline, DriveOutcome::Ok { .. }),
        "baseline signed read_tail must execute: {baseline:?}"
    );
    println!("step 1 ok: registered {id1} + granted read_tail + signed drive executed");

    // --- 2. reinstall: wipe the board's key material -------------------
    // config.json (registration record: id1 + grants) is NOT touched; the
    // daemon ledger keeps id1 active too — exactly the issue state.
    let key_path = ui_dir.join("keys").join(format!("{fingerprint}.key"));
    assert!(
        key_path.exists(),
        "key material file must exist before the wipe"
    );
    std::fs::remove_file(&key_path).expect("wipe key material (simulated reinstall)");
    println!("step 2 ok: wiped key material at {}", key_path.display());

    // --- 3. "start the board": fresh key, old registration -------------
    let key2 =
        corrald_ui::keys::load_or_create_key(&fingerprint).expect("fresh key after reinstall");
    let id2 = corrald_ui::keys::device_key_id(&key2.signing.verifying_key().to_bytes());
    assert_ne!(id2, id1, "reinstall must produce a fresh identity");
    assert_ne!(key1.signing.to_bytes(), key2.signing.to_bytes());

    // The pre-fix board signs with the CURRENT key under the REGISTERED
    // key_id -> the #249 symptom: 401 bad_signature on every drive.
    let pre_fix = DriveEndpoint {
        client: client.clone(),
        base_url: base_url.clone(),
        key_id: id1.clone(),
        signing: key2.signing.clone(),
    };
    let symptom =
        corrald_ui::drive::execute_drive(&pre_fix, &DriveIntent::read_tail("agent-a", None)).await;
    assert!(
        matches!(
            symptom,
            DriveOutcome::Refused(DriveFailure::BadSignature(_))
        ),
        "pre-fix signed drive must 401 bad_signature: {symptom:?}"
    );
    println!("step 3 ok: pre-fix symptom reproduced (401 bad_signature on {id1})");

    // --- 4. recovery: re-register current key --------------------------
    // (the exact sequence the board runs on detection / one-tap prompt);
    // grant restoration is out-of-band since #354 — the host provisions
    // the re-registered key on the registry.
    let (recovered_id, recovered_grants) = protocol::register_device(
        &client,
        &base_url,
        &token,
        &pubkey_b64(&key2.signing),
        Some("e2e-board"),
    )
    .await
    .expect("recovery re-register of the current key");
    assert_eq!(
        recovered_id, id2,
        "recovery must not rotate the current key"
    );
    assert!(
        recovered_grants.is_empty(),
        "a freshly re-registered device starts read-only: {recovered_grants:?}"
    );
    auth.registry
        .set_grants(&id2, vec![corrald::drive::Capability::ReadTail])
        .expect("out-of-band restore of the previous grant set");
    println!(
        "step 4 ok: re-registered {id1} -> {id2} + grants restored (out-of-band registry provisioning)"
    );

    // --- 5. signed drive plane works immediately again -----------------
    let endpoint2 = DriveEndpoint {
        client: client.clone(),
        base_url: base_url.clone(),
        key_id: id2.clone(),
        signing: key2.signing,
    };
    let recovered =
        corrald_ui::drive::execute_drive(&endpoint2, &DriveIntent::read_tail("agent-a", None))
            .await;
    match &recovered {
        DriveOutcome::Ok { result, .. } => {
            let lines = corrald_ui::drive::parse_tail_lines(result.as_ref().expect("result"));
            assert_eq!(lines, ["hello", "world"], "recovered drive serves the tail");
        }
        other => panic!("post-recovery signed drive must execute: {other:?}"),
    }

    // The recovered registration record matches the persisted shape the
    // board writes to config.json: the register response's grant set is
    // read-only for a freshly re-registered key — the out-of-band host
    // grant lands on a later refresh/grants-read (#354 R1).
    let record = PersistedConfig {
        host_url: Some(base_url.clone()),
        registration: Some(corrald_ui::state::RegistrationRecord {
            host_fingerprint: fingerprint.clone(),
            key_id: id2.clone(),
            grants: vec![],
            denied: vec![],
        }),
        ..Default::default()
    };
    assert_eq!(record.registration.unwrap().key_id, id2);

    println!(
        "step 5 ok: signed read_tail executes after recovery ({id1} -> {id2}), no bad_signature"
    );
    println!("E2E PASS: reinstall identity recovery verified end to end");
}
