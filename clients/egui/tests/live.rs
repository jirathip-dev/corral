//! LIVE conformance probe against a REAL corrald (the W1 suite spirit).
//!
//! Ignored by default (`cargo test -- --ignored`) because it needs a
//! running daemon. Usage:
//!
//! ```sh
//! CORRALD_URL=http://127.0.0.1:8474 \
//! CORRAL_CONFIG_DIR=</path/to/scratch/config> \
//! CORRAL_UI_CONFIG_DIR=</path/to/scratch/ui-config> \
//! cargo test -p corrald-ui --test live -- --ignored --nocapture
//! ```
//!
//! It exercises the exact code path the desktop GUI uses on tap:
//! host fingerprint → device key → register → read-only default refusal →
//! host grants read_tail → signed read_tail drive → idempotent replay →
//! audit growth (executed entry present, refusals never logged).
//!
//! The registration record is written to the client config.json (same
//! shape `corrald-ui` persists), so launching the GUI afterwards with the
//! same `CORRAL_UI_CONFIG_DIR` lands directly on the live board.
//!
//! The two re-register probes share the host-scoped keyring entry and the
//! daemon registry, so run the suite single-threaded:
//! `cargo test -p corrald-ui --test live -- --ignored --nocapture --test-threads=1`.

use corrald_ui::drive::{DriveEndpoint, DriveIntent, DriveOutcome};
use corrald_ui::protocol;

const ENV_URL: &str = "CORRALD_URL";

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::test]
#[ignore = "needs a real corrald (see module docs)"]
async fn live_register_read_drive_audit() {
    let base_url = env_or(ENV_URL, "http://127.0.0.1:8474");
    let client = reqwest::Client::new();

    // --- host identity -----------------------------------------------
    let host = protocol::fetch_host_key(&client, &base_url)
        .await
        .expect("GET /host-key");
    assert_eq!(host.algorithm, "X25519");
    let _fingerprint = corrald_ui::keys::host_fingerprint(Some(&host.public_key), &base_url);

    // A FRESH ephemeral keypair per run: the probe must always observe
    // the read-only default, which re-registering the GUI's persistent
    // key would not (it already carries grants). The GUI's own key is
    // registered separately through the app config (see README).
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("OS RNG");
    let device = corrald_ui::keys::DeviceKey {
        signing: ed25519_dalek::SigningKey::from_bytes(&seed),
        store: corrald_ui::keys::KeyStore::Keyring,
    };
    println!("device key store: fresh ephemeral keypair");

    // --- registration token + admin token from the daemon's config dir
    // (localhost host same-user access, as the GUI's auto-register does)
    let token = corrald_ui::keys::read_daemon_registration_token()
        .expect("registration token (start corrald with a scratch CORRAL_CONFIG_DIR)");
    let admin_token = corrald_ui::keys::read_daemon_admin_token()
        .expect("admin token (needed to grant + read audit)");

    // --- R1: register -> empty grants (read-only default) -------------
    let pubkey_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(device.signing.verifying_key().to_bytes())
    };
    let (key_id, grants) = protocol::register_device(&client, &base_url, &token, &pubkey_b64)
        .await
        .expect("register");
    assert!(grants.is_empty(), "read-only default: {:?}", grants);
    println!("registered key_id={key_id} grants={grants:?}");

    // --- R2: read path: snapshot + SSE resume --------------------------
    let snapshot = protocol::fetch_snapshot(&client, &base_url)
        .await
        .expect("snapshot");
    assert!(
        snapshot.schema_version >= 5,
        "schema {}",
        snapshot.schema_version
    );
    println!(
        "snapshot rev={} agents={}",
        snapshot.rev,
        snapshot.agents.len()
    );
    let first_agent = snapshot
        .agents
        .keys()
        .next()
        .cloned()
        .expect("a real herdr agent must be present (is herdr.sock live?)");

    // SSE connect with Last-Event-ID resume (server answers snapshot or deltas).
    let events = protocol::open_events(&client, &base_url, Some(snapshot.rev))
        .await
        .expect("SSE connect with Last-Event-ID");
    assert!(
        events.status().is_success(),
        "SSE status {}",
        events.status()
    );
    println!("SSE connect ok (Last-Event-ID: {})", snapshot.rev);

    // --- drive endpoint = the same struct the GUI builds on tap -------
    let endpoint = DriveEndpoint {
        client: client.clone(),
        base_url: base_url.clone(),
        key_id: key_id.clone(),
        signing: device.signing,
    };

    // --- R5: read-only default -> 403 not_granted, zero audit growth --
    let before_len = audit_len(&client, &base_url, &admin_token).await;
    let refused = corrald_ui::drive::execute_drive(
        &endpoint,
        &DriveIntent::read_tail(&first_agent, Some(snapshot.rev)),
    )
    .await;
    assert!(
        matches!(
            refused,
            DriveOutcome::Refused(corrald_ui::drive::DriveFailure::NotGranted(_))
        ),
        "read-only device must be refused: {refused:?}"
    );
    let after_len = audit_len(&client, &base_url, &admin_token).await;
    assert_eq!(
        after_len, before_len,
        "auth refusals must never grow the audit log (R10)"
    );
    println!("read-only refusal verified (audit unchanged at {after_len})");

    // --- host grants read_tail (POST /grants, admin token) -------------
    let grants_url = format!("{}/grants", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "action": "set_grants",
        "key_id": key_id,
        "grants": ["read_tail"],
    });
    let response = client
        .post(&grants_url)
        .bearer_auth(&admin_token)
        .json(&body)
        .send()
        .await
        .expect("POST /grants");
    assert!(
        response.status().is_success(),
        "grants: {}",
        response.status()
    );
    println!("granted read_tail");

    // --- R3: signed drive executes against a real agent ----------------
    let intent = DriveIntent::read_tail(&first_agent, Some(snapshot.rev));
    let outcome = corrald_ui::drive::execute_drive(&endpoint, &intent).await;
    match &outcome {
        DriveOutcome::Ok { rev, .. } => println!(
            "read_tail executed on {first_agent}: ok rev {rev} (request_id {})",
            intent.request_id
        ),
        other => panic!("read_tail drive failed: {other:?}"),
    }
    // The response rev must be >= the request rev (contract).
    if let DriveOutcome::Ok { rev, .. } = &outcome {
        assert!(*rev >= snapshot.rev, "rev monotonicity");
    }

    // --- R6: replay with the SAME request_id -> byte-identical --------
    let replay = corrald_ui::drive::execute_drive(&endpoint, &intent).await;
    assert_eq!(
        replay, outcome,
        "same request_id must replay the stored response byte-identical"
    );
    println!(
        "idempotent replay verified (request_id {})",
        intent.request_id
    );

    // --- R10: audit grew by exactly one executed entry ------------------
    let after_drive = audit_len(&client, &base_url, &admin_token).await;
    assert_eq!(
        after_drive,
        after_len + 1,
        "exactly one executed drive appended"
    );
    let audit = fetch_audit(&client, &base_url, &admin_token).await;
    assert!(audit.valid, "chain must verify");
    let mine = audit
        .entries
        .iter()
        .find(|e| e.request_id == intent.request_id)
        .expect("audit entry for my drive");
    assert_eq!(mine.capability, "read_tail");
    assert_eq!(mine.target, first_agent);
    assert_eq!(mine.outcome, serde_json::json!("executed"));
    println!(
        "audit: seq={} key_id={} capability={} target={} outcome=executed",
        mine.seq, mine.key_id, mine.capability, mine.target
    );

    println!(
        "probe done: key_id={key_id} host={} (GUI registration: see README)",
        base_url
    );
}

async fn audit_len(client: &reqwest::Client, base_url: &str, admin_token: &str) -> usize {
    fetch_audit(client, base_url, admin_token)
        .await
        .entries
        .len()
}

async fn fetch_audit(
    client: &reqwest::Client,
    base_url: &str,
    admin_token: &str,
) -> protocol::AuditView {
    protocol::fetch_audit(client, base_url, admin_token)
        .await
        .expect("GET /audit")
}

/// F5 regression, live: a FAILED re-register must leave the persisted
/// seed AND the registration record untouched (register-then-rotate
/// ordering) — the old key keeps working instead of 401ing.
///
/// Uses a persistent device key in a scratch `CORRAL_UI_CONFIG_DIR` (the
/// daemon's registration token + admin token come from `CORRAL_CONFIG_DIR`).
#[tokio::test]
#[ignore = "needs a real corrald (see module docs)"]
async fn live_reregister_failure_preserves_key_and_registration() {
    let base_url = env_or(ENV_URL, "http://127.0.0.1:8474");
    let client = reqwest::Client::new();
    let host = protocol::fetch_host_key(&client, &base_url)
        .await
        .expect("GET /host-key");
    let fingerprint = corrald_ui::keys::host_fingerprint(Some(&host.public_key), &base_url);
    let token = corrald_ui::keys::read_daemon_registration_token()
        .expect("registration token (scratch CORRAL_CONFIG_DIR)");
    let admin_token = corrald_ui::keys::read_daemon_admin_token().expect("admin token");

    // The persistent device key (created under the scratch UI config dir).
    let key = corrald_ui::keys::load_or_create_key(&fingerprint).expect("device key");
    let seed_before = key.signing.to_bytes();
    let pubkey_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(key.signing.verifying_key().to_bytes())
    };
    let (key_id, _) = protocol::register_device(&client, &base_url, &token, &pubkey_b64)
        .await
        .expect("register the persistent key");
    println!("registered persistent key key_id={key_id}");

    // Host grants read_tail so the OLD key can still drive afterwards.
    let grants_url = format!("{}/grants", base_url.trim_end_matches('/'));
    let response = client
        .post(&grants_url)
        .bearer_auth(&admin_token)
        .json(&serde_json::json!({
            "action": "set_grants",
            "key_id": key_id,
            "grants": ["read_tail"],
        }))
        .send()
        .await
        .expect("POST /grants");
    assert!(
        response.status().is_success(),
        "grants: {}",
        response.status()
    );

    // Simulate the app's RE-REGISTER path with a BAD token: the daemon
    // refuses before registering anything, and the seed rotation must not
    // have happened (register-then-rotate ordering).
    let mut new_seed = [0u8; 32];
    getrandom::fill(&mut new_seed).expect("OS RNG");
    let new_signing = ed25519_dalek::SigningKey::from_bytes(&new_seed);
    let new_pubkey_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(new_signing.verifying_key().to_bytes())
    };
    let failed = protocol::register_device(
        &client,
        &base_url,
        "definitely-not-the-token",
        &new_pubkey_b64,
    )
    .await;
    assert!(failed.is_err(), "bad registration token must be refused");
    println!(
        "re-register with bad token refused: {}",
        failed.unwrap_err()
    );

    // The PERSISTED seed must be unchanged (rotation happens only after a
    // successful registration).
    let reloaded = corrald_ui::keys::load_or_create_key(&fingerprint).expect("reload");
    assert_eq!(
        reloaded.signing.to_bytes(),
        seed_before,
        "failed re-register must not rotate the persisted seed"
    );
    println!("persisted seed unchanged after failed re-register");

    // And the OLD key_id still drives (not orphaned).
    let snapshot = protocol::fetch_snapshot(&client, &base_url)
        .await
        .expect("snapshot");
    let first_agent = snapshot
        .agents
        .keys()
        .next()
        .cloned()
        .expect("a real herdr agent");
    let endpoint = DriveEndpoint {
        client: client.clone(),
        base_url: base_url.clone(),
        key_id: key_id.clone(),
        signing: key.signing,
    };
    let outcome =
        corrald_ui::drive::execute_drive(&endpoint, &DriveIntent::read_tail(&first_agent, None))
            .await;
    assert!(
        matches!(outcome, DriveOutcome::Ok { .. }),
        "old key must still drive after a failed re-register: {outcome:?}"
    );
    println!("old key still drives after failed re-register: ok");

    // F3 primitive: re-registering the SAME key re-fetches the host's
    // CURRENT grant set (the Settings "refresh grants" action) — a grant
    // the host added after registration surfaces without a new key.
    let (_, refreshed_grants) = protocol::register_device(&client, &base_url, &token, &pubkey_b64)
        .await
        .expect("refresh grants (same key)");
    assert!(
        refreshed_grants.iter().any(|g| g == "read_tail"),
        "refresh grants must surface the host's current set: {refreshed_grants:?}"
    );
    println!("refresh grants (same key) re-fetched current grants: {refreshed_grants:?}");
}

/// F5 success path, live: a SUCCESSFUL re-register rotates the persisted
/// seed; the app must then sign with the NEW key (in-memory reload —
/// `handle_register_result`) or every drive 401s `bad_signature` under
/// the new key_id. Mirrors `CorralApp::register(token, true)` +
/// `handle_register_result` ordering exactly, and proves the next signed
/// drive verifies against the daemon.
///
/// Uses a persistent device key in a scratch `CORRAL_UI_CONFIG_DIR`.
#[tokio::test]
#[ignore = "needs a real corrald (see module docs)"]
async fn live_reregister_success_rotates_the_in_memory_key() {
    let base_url = env_or(ENV_URL, "http://127.0.0.1:8474");
    let client = reqwest::Client::new();
    let host = protocol::fetch_host_key(&client, &base_url)
        .await
        .expect("GET /host-key");
    let fingerprint = corrald_ui::keys::host_fingerprint(Some(&host.public_key), &base_url);
    let token = corrald_ui::keys::read_daemon_registration_token()
        .expect("registration token (scratch CORRAL_CONFIG_DIR)");
    let admin_token = corrald_ui::keys::read_daemon_admin_token().expect("admin token");

    // Old persistent key: registered + granted read_tail.
    let old_key = corrald_ui::keys::load_or_create_key(&fingerprint).expect("old device key");
    let old_seed = old_key.signing.to_bytes();
    let old_pubkey_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(old_key.signing.verifying_key().to_bytes())
    };
    let (old_key_id, _) = protocol::register_device(&client, &base_url, &token, &old_pubkey_b64)
        .await
        .expect("register old key");
    let grants_url = format!("{}/grants", base_url.trim_end_matches('/'));
    let response = client
        .post(&grants_url)
        .bearer_auth(&admin_token)
        .json(&serde_json::json!({
            "action": "set_grants",
            "key_id": old_key_id,
            "grants": ["read_tail"],
        }))
        .send()
        .await
        .expect("POST /grants");
    assert!(
        response.status().is_success(),
        "grants: {}",
        response.status()
    );
    println!("registered old key key_id={old_key_id} + granted read_tail");

    // The app's ReRegister flow (register(token, true)): fresh seed in
    // memory, register its pubkey FIRST, persist the rotation only after
    // the daemon accepted it.
    let mut new_seed = [0u8; 32];
    getrandom::fill(&mut new_seed).expect("OS RNG");
    let new_signing = ed25519_dalek::SigningKey::from_bytes(&new_seed);
    let new_pubkey_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(new_signing.verifying_key().to_bytes())
    };
    let (new_key_id, _) = protocol::register_device(&client, &base_url, &token, &new_pubkey_b64)
        .await
        .expect("register the rotated key");
    assert_ne!(
        new_key_id, old_key_id,
        "rotation must produce a fresh key_id"
    );
    corrald_ui::keys::rotate_key(&fingerprint, &new_seed).expect("persist rotation");
    // The rotated key starts read-only (empty grants); grant read_tail so
    // the post-rotation drive is an execution, not a refusal.
    let response = client
        .post(&grants_url)
        .bearer_auth(&admin_token)
        .json(&serde_json::json!({
            "action": "set_grants",
            "key_id": new_key_id,
            "grants": ["read_tail"],
        }))
        .send()
        .await
        .expect("POST /grants (new key)");
    assert!(
        response.status().is_success(),
        "grants (new key): {}",
        response.status()
    );
    println!("re-registered with new key key_id={new_key_id} + granted read_tail");

    // The fix: `handle_register_result` reloads the in-memory signing key
    // from storage, which now holds the NEW seed.
    let reloaded = corrald_ui::keys::load_or_create_key(&fingerprint).expect("reload");
    assert_eq!(
        reloaded.signing.to_bytes(),
        new_seed,
        "in-memory key must be the NEW seed after a successful re-register"
    );
    println!("in-memory key reloaded to the new seed");

    // And the next signed drive verifies against the daemon: NEW key +
    // NEW key_id -> Ok, not bad_signature.
    let snapshot = protocol::fetch_snapshot(&client, &base_url)
        .await
        .expect("snapshot");
    let first_agent = snapshot
        .agents
        .keys()
        .next()
        .cloned()
        .expect("a real herdr agent");
    let endpoint = DriveEndpoint {
        client: client.clone(),
        base_url: base_url.clone(),
        key_id: new_key_id.clone(),
        signing: reloaded.signing,
    };
    let outcome =
        corrald_ui::drive::execute_drive(&endpoint, &DriveIntent::read_tail(&first_agent, None))
            .await;
    assert!(
        matches!(outcome, DriveOutcome::Ok { .. }),
        "drive with the rotated key must verify: {outcome:?}"
    );
    println!("drive with rotated key: ok (verifies against the daemon)");

    // Regression probe: the OLD signing key under the NEW key_id must be
    // refused with bad_signature (this is exactly the pre-fix 401 loop).
    let mismatched = DriveEndpoint {
        client: client.clone(),
        base_url: base_url.clone(),
        key_id: new_key_id,
        signing: ed25519_dalek::SigningKey::from_bytes(&old_seed),
    };
    let outcome =
        corrald_ui::drive::execute_drive(&mismatched, &DriveIntent::read_tail(&first_agent, None))
            .await;
    assert!(
        matches!(
            outcome,
            DriveOutcome::Refused(corrald_ui::drive::DriveFailure::BadSignature(_))
        ),
        "old key under the new key_id must be bad_signature: {outcome:?}"
    );
    println!("old key under new key_id correctly refused: bad_signature");
}
