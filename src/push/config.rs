//! APNs configuration (D16). Env-driven, mirroring `CORRAL_CONFIG_DIR` /
//! `CORRAL_REPO_ROOT`: `CORRAL_APNS_*`. Certificate provisioning (the `.p8`
//! push key, an Apple developer-account artifact) is Guy's — this module
//! only reads what he provisions.
//!
//! Required for the notifier to arm:
//! - `CORRAL_APNS_TEAM_ID` — Apple team id (JWT `iss`).
//! - `CORRAL_APNS_KEY_ID` — the APNs auth key's id (JWT `kid`).
//! - `CORRAL_APNS_AUTH_KEY_PATH` — path to the `.p8` PKCS#8 PEM file.
//!
//! Optional:
//! - `CORRAL_APNS_ENDPOINT` — `production` (default) | `sandbox`.
//! - `CORRAL_APNS_TOPIC` — app bundle id (default `com.corral.fleetnotifier`).
//!
//! When any required input is missing the daemon logs once and runs with
//! the notifier disabled (read path untouched); a bad `.p8` at arm time is
//! a hard error so misprovisioning surfaces at startup, not on the first
//! blocked agent.

use std::path::PathBuf;

use p256::ecdsa::SigningKey;
use serde::{Deserialize, Serialize};

/// Which APNs endpoint to talk to. Tokens are environment-bound: a
/// development-profile token on the production endpoint gets 400
/// `BadDeviceToken` and vice versa — get this wrong and pushes are
/// silently dropped by Apple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Endpoint {
    Production,
    Sandbox,
}

/// Everything the provider + notifier need. `Debug` prints no secrets (key
/// material lives in the `.p8` file, never here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub team_id: String,
    pub key_id: String,
    pub auth_key_path: String,
    pub endpoint: Endpoint,
    /// The receiving app's bundle id (`apns-topic` header).
    pub topic: String,
}

impl Config {
    /// Load from `CORRAL_APNS_*` env. `None` when not configured — the
    /// daemon then runs with the notifier disabled (a documented
    /// first-run state; everything else still works).
    pub fn from_env() -> Option<Self> {
        let mut vars = std::collections::HashMap::new();
        for var in [
            "CORRAL_APNS_TEAM_ID",
            "CORRAL_APNS_KEY_ID",
            "CORRAL_APNS_AUTH_KEY_PATH",
            "CORRAL_APNS_ENDPOINT",
            "CORRAL_APNS_TOPIC",
        ] {
            if let Ok(value) = std::env::var(var) {
                vars.insert(var.to_string(), value);
            }
        }
        Self::from_map(&vars)
    }

    /// The testable seam: build a [`Config`] from an explicit variable map,
    /// never the process env. Tests exercise every branch through this;
    /// the only env reader is [`Config::from_env`], which production (and
    /// main.rs, which arms the notifier) uses — never the test suite.
    pub fn from_map(vars: &std::collections::HashMap<String, String>) -> Option<Self> {
        let team_id = vars.get("CORRAL_APNS_TEAM_ID")?;
        let key_id = vars.get("CORRAL_APNS_KEY_ID")?;
        let auth_key_path = vars.get("CORRAL_APNS_AUTH_KEY_PATH")?;
        let endpoint = match vars.get("CORRAL_APNS_ENDPOINT").map(String::as_str) {
            Some("production") | Some("") => Endpoint::Production,
            Some("sandbox") => Endpoint::Sandbox,
            Some(other) => {
                tracing::warn!(
                    endpoint = other,
                    "CORRAL_APNS_ENDPOINT must be production or sandbox; defaulting to production"
                );
                Endpoint::Production
            }
            None => Endpoint::Production,
        };
        let topic = vars
            .get("CORRAL_APNS_TOPIC")
            .filter(|t| !t.is_empty())
            .cloned()
            .unwrap_or_else(|| "com.corral.fleetnotifier".to_string());
        Some(Self {
            team_id: team_id.clone(),
            key_id: key_id.clone(),
            auth_key_path: auth_key_path.clone(),
            endpoint,
            topic,
        })
    }

    /// Read + parse the `.p8` push key. The only place key material is
    /// held; callers drop it after JWT signing.
    pub fn load_signing_key(path: &str) -> Result<SigningKey, super::provider::PushError> {
        use p256::pkcs8::DecodePrivateKey;
        let pem = std::fs::read_to_string(PathBuf::from(path))
            .map_err(|e| super::provider::PushError::Configuration(format!("read {path}: {e}")))?;
        SigningKey::from_pkcs8_pem(&pem)
            .map_err(|e| super::provider::PushError::Configuration(format!("bad p8 key: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a map with the three required vars set, for branch testing.
    fn required() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("CORRAL_APNS_TEAM_ID".to_string(), "TEAM123456".to_string());
        m.insert("CORRAL_APNS_KEY_ID".to_string(), "KEY12345678".to_string());
        m.insert(
            "CORRAL_APNS_AUTH_KEY_PATH".to_string(),
            "/keys/push.p8".to_string(),
        );
        m
    }

    #[test]
    fn from_map_requires_all_three() {
        // No env is ever touched (N6): set_var/remove_var on the process
        // env is UB under Rust 2024 and races the rest of the test binary.
        // The seam is a plain map, so the ambient environment can neither
        // break these tests nor silently arm a notifier.
        assert!(Config::from_map(&std::collections::HashMap::new()).is_none());

        let mut m = std::collections::HashMap::new();
        m.insert("CORRAL_APNS_TEAM_ID".to_string(), "T".to_string());
        assert!(Config::from_map(&m).is_none(), "partial config is disabled");
        m.insert("CORRAL_APNS_KEY_ID".to_string(), "K".to_string());
        assert!(
            Config::from_map(&m).is_none(),
            "still missing the auth key path"
        );
        m.insert("CORRAL_APNS_AUTH_KEY_PATH".to_string(), "/p8".to_string());
        let c = Config::from_map(&m).expect("complete config parses");
        assert_eq!(c.team_id, "T");
        assert_eq!(c.key_id, "K");
        assert_eq!(c.auth_key_path, "/p8");
        assert_eq!(c.endpoint, Endpoint::Production, "default endpoint");
        assert_eq!(c.topic, "com.corral.fleetnotifier", "default topic");
    }

    #[test]
    fn from_map_parses_endpoint_and_topic() {
        let mut m = required();
        m.insert("CORRAL_APNS_ENDPOINT".to_string(), "sandbox".to_string());
        assert_eq!(Config::from_map(&m).unwrap().endpoint, Endpoint::Sandbox);

        // A bogus endpoint warns and defaults to production.
        m.insert("CORRAL_APNS_ENDPOINT".to_string(), "bogus".to_string());
        assert_eq!(Config::from_map(&m).unwrap().endpoint, Endpoint::Production);

        // An empty topic falls back to the bundle id.
        m.insert("CORRAL_APNS_ENDPOINT".to_string(), "production".to_string());
        m.insert("CORRAL_APNS_TOPIC".to_string(), String::new());
        assert_eq!(
            Config::from_map(&m).unwrap().topic,
            "com.corral.fleetnotifier"
        );
        m.insert("CORRAL_APNS_TOPIC".to_string(), "com.example".to_string());
        assert_eq!(Config::from_map(&m).unwrap().topic, "com.example");
    }
}
