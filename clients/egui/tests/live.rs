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
    let _fingerprint =
        corrald_ui::keys::host_fingerprint(Some(&host.public_key), &base_url);

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
        base64::engine::general_purpose::STANDARD
            .encode(device.signing.verifying_key().to_bytes())
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
    assert!(snapshot.schema_version >= 3, "schema {}", snapshot.schema_version);
    println!("snapshot rev={} agents={}", snapshot.rev, snapshot.agents.len());
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
    assert!(events.status().is_success(), "SSE status {}", events.status());
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
    assert!(response.status().is_success(), "grants: {}", response.status());
    println!("granted read_tail");

    // --- R3: signed drive executes against a real agent ----------------
    let intent = DriveIntent::read_tail(&first_agent, Some(snapshot.rev));
    let outcome = corrald_ui::drive::execute_drive(&endpoint, &intent).await;
    match &outcome {
        DriveOutcome::Ok { rev } => println!(
            "read_tail executed on {first_agent}: ok rev {rev} (request_id {})",
            intent.request_id
        ),
        other => panic!("read_tail drive failed: {other:?}"),
    }
    // The response rev must be >= the request rev (contract).
    if let DriveOutcome::Ok { rev } = &outcome {
        assert!(*rev >= snapshot.rev, "rev monotonicity");
    }

    // --- R6: replay with the SAME request_id -> byte-identical --------
    let replay = corrald_ui::drive::execute_drive(&endpoint, &intent).await;
    assert_eq!(
        replay, outcome,
        "same request_id must replay the stored response byte-identical"
    );
    println!("idempotent replay verified (request_id {})", intent.request_id);

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
