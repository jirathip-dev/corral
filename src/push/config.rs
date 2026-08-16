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
        let team_id = std::env::var("CORRAL_APNS_TEAM_ID").ok()?;
        let key_id = std::env::var("CORRAL_APNS_KEY_ID").ok()?;
        let auth_key_path = std::env::var("CORRAL_APNS_AUTH_KEY_PATH").ok()?;
        let endpoint = match std::env::var("CORRAL_APNS_ENDPOINT").as_deref() {
            Ok("production") | Ok("") => Endpoint::Production,
            Ok("sandbox") => Endpoint::Sandbox,
            Ok(other) => {
                tracing::warn!(
                    endpoint = other,
                    "CORRAL_APNS_ENDPOINT must be production or sandbox; defaulting to production"
                );
                Endpoint::Production
            }
            Err(_) => Endpoint::Production,
        };
        let topic = std::env::var("CORRAL_APNS_TOPIC")
            .ok()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "com.corral.fleetnotifier".to_string());
        Some(Self {
            team_id,
            key_id,
            auth_key_path,
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

    #[test]
    fn from_env_requires_all_three() {
        // Unset everything (tests never set CORRAL_APNS_*): not configured.
        // Rust 2024 marks set/remove_var unsafe (thread-safety); tests are
        // single-threaded and run before any notifier could be armed.
        unsafe {
            for var in [
                "CORRAL_APNS_TEAM_ID",
                "CORRAL_APNS_KEY_ID",
                "CORRAL_APNS_AUTH_KEY_PATH",
                "CORRAL_APNS_ENDPOINT",
                "CORRAL_APNS_TOPIC",
            ] {
                std::env::remove_var(var);
            }
        }
        assert!(Config::from_env().is_none(), "unconfigured -> disabled");
    }

    #[test]
    fn partial_config_is_disabled() {
        unsafe {
            std::env::remove_var("CORRAL_APNS_TEAM_ID");
            std::env::remove_var("CORRAL_APNS_KEY_ID");
            std::env::remove_var("CORRAL_APNS_AUTH_KEY_PATH");
            std::env::remove_var("CORRAL_APNS_ENDPOINT");
            std::env::remove_var("CORRAL_APNS_TOPIC");
            std::env::set_var("CORRAL_APNS_TEAM_ID", "T");
        }
        assert!(Config::from_env().is_none(), "partial env must not arm");
    }
}
