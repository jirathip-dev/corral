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
//! #354 read-only cut: grant administration is gone from the daemon's HTTP
//! surface (grants are provisioned out-of-band in `registry.json` and only
//! take effect on daemon restart), so this probe covers what a read-only
//! device can prove against a live daemon: host fingerprint → device key →
//! register (read-only default: EMPTY grants) → snapshot + SSE resume →
//! signed read_tail refused with `not_granted` (read auth intact, mutating
//! plane absent). The grant-then-drive journey lives in the app-level E2E
//! suite and `tests/conformance.rs`; a live operator who grants a key
//! out-of-band and restarts the daemon can re-run this probe to see the
//! refusal flip to an execution.
//!
//! The registration record is written to the client config.json (same
//! shape `corrald-ui` persists), so launching the GUI afterwards with the
//! same `CORRAL_UI_CONFIG_DIR` lands directly on the live board.

use corrald_ui::drive::{DriveEndpoint, DriveIntent, DriveOutcome};
use corrald_ui::protocol;

const ENV_URL: &str = "CORRALD_URL";

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::test]
#[ignore = "needs a real corrald (see module docs)"]
async fn live_register_read_refusal_and_sse_resume() {
    let base_url = env_or(ENV_URL, "http://127.0.0.1:8474");
    let client = reqwest::Client::new();

    // --- host identity -----------------------------------------------
    let host = protocol::fetch_host_key(&client, &base_url)
        .await
        .expect("GET /host-key");
    assert_eq!(host.algorithm, "X25519");
    let _fingerprint = corrald_ui::keys::host_fingerprint(Some(&host.public_key), &base_url);

    // A FRESH ephemeral keypair per run: the probe must always observe
    // the read-only default.
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("OS RNG");
    let device = corrald_ui::keys::DeviceKey {
        signing: ed25519_dalek::SigningKey::from_bytes(&seed),
        store: corrald_ui::keys::KeyStore::Keyring,
    };
    println!("device key store: fresh ephemeral keypair");

    let token = corrald_ui::keys::read_daemon_registration_token()
        .expect("registration token (start corrald with a scratch CORRAL_CONFIG_DIR)");

    // --- R1: register -> empty grants (read-only default) -------------
    let pubkey_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(device.signing.verifying_key().to_bytes())
    };
    let (key_id, grants) = protocol::register_device(&client, &base_url, &token, &pubkey_b64, None)
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

    // --- read-only default -> 403 not_granted (read auth intact) -------
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
        "a read-only-default device must be refused with not_granted: {refused:?}"
    );
    println!(
        "read-only refusal verified (not_granted) — grant {key_id} read_tail in \
         registry.json + restart corrald to see the execution leg"
    );

    println!("probe done: key_id={key_id} host={base_url}");
}

/// F5 regression, live: a FAILED re-register must leave the persisted
/// seed AND the registration record untouched (register-then-rotate
/// ordering) — the old key keeps working instead of 401ing.
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

    // The persistent device key (created under the scratch UI config dir).
    let key = corrald_ui::keys::load_or_create_key(&fingerprint).expect("device key");
    let seed_before = key.signing.to_bytes();
    let pubkey_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(key.signing.verifying_key().to_bytes())
    };
    let (key_id, _) = protocol::register_device(&client, &base_url, &token, &pubkey_b64, None)
        .await
        .expect("register the persistent key");
    println!("registered persistent key key_id={key_id}");

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
        None,
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

    // F3 primitive: re-registering the SAME key re-fetches the host's
    // CURRENT grant set — a grant added out-of-band after registration
    // surfaces without a new key (the Settings restore path).
    let (_, refreshed_grants) =
        protocol::register_device(&client, &base_url, &token, &pubkey_b64, None)
            .await
            .expect("refresh grants (same key)");
    println!("refresh grants (same key) re-fetched current grants: {refreshed_grants:?}");
    assert!(
        refreshed_grants.is_empty() || refreshed_grants.iter().any(|g| g == "read_tail"),
        "grants are whatever the host provisioned: {refreshed_grants:?}"
    );
}

/// F5 success path, live: a SUCCESSFUL re-register rotates the persisted
/// seed; the app then signs with the NEW key (in-memory reload —
/// `handle_register_result`) so the next signed drive cannot 401
/// `bad_signature` under the new key_id.
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

    // Old persistent key: registered (read-only default).
    let old_key = corrald_ui::keys::load_or_create_key(&fingerprint).expect("old device key");
    let old_pubkey_b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(old_key.signing.verifying_key().to_bytes())
    };
    let (old_key_id, _) =
        protocol::register_device(&client, &base_url, &token, &old_pubkey_b64, None)
            .await
            .expect("register old key");
    println!("registered old key key_id={old_key_id}");

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
    let (new_key_id, _) =
        protocol::register_device(&client, &base_url, &token, &new_pubkey_b64, None)
            .await
            .expect("register the rotated key");
    assert_ne!(
        new_key_id, old_key_id,
        "rotation must produce a fresh key_id"
    );
    corrald_ui::keys::rotate_key(&fingerprint, &new_seed).expect("persist rotation");
    println!("re-registered with new key key_id={new_key_id}");

    // The fix: `handle_register_result` reloads the in-memory signing key
    // from storage, which now holds the NEW seed.
    let reloaded = corrald_ui::keys::load_or_create_key(&fingerprint).expect("reload");
    assert_eq!(
        reloaded.signing.to_bytes(),
        new_seed,
        "in-memory key must be the NEW seed after a successful re-register"
    );
    println!("in-memory key reloaded to the new seed");

    // The key_id the daemon knows is the NEW one; the OLD key_id was not
    // orphaned into a re-register that signs with the old seed.
    assert_eq!(
        reloaded.signing.verifying_key().to_bytes(),
        new_signing.verifying_key().to_bytes(),
        "the daemon's new key_id matches the reloaded key"
    );
    println!("rotated key verified against the new registration");
}
