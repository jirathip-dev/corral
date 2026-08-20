//! Host-side device registry: registered device keys with grants, expiry
//! and revocation, persisted as `registry.json` (0600) in the config dir.
//!
//! Default deny: a freshly registered device has NO drive grants (the read
//! plane — `/healthz`, `/snapshot`, `/events`, `/history` — is
//! credential-free; its boundary is the bound interface's network, #65).
//! Drive capabilities are promoted by the host via `POST /grants`
//! (admin token). Registration is gated by the routing-only registration
//! token (constant-time compare).
//!
//! Registry state is versioned (schema_version 1, additive-only) and
//! persisted atomically on every mutation. Corrupt state fails fast at
//! load — never silently reset (that would rotate the registration token
//! under the host's feet).

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::drive::Capability;

use super::{b64_decode_array_32, b64_encode, random_bytes, write_secret};

/// File name of the device registry inside the config dir.
pub const REGISTRY_FILE: &str = "registry.json";
/// File name of the routing-only registration token.
pub const REGISTRATION_TOKEN_FILE: &str = "registration-token";
pub const SCHEMA_VERSION: u32 = 1;

/// One registered device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub key_id: String,
    #[serde(with = "b64_bytes")]
    pub public_key: [u8; 32],
    pub created_ts: u64,
    pub expiry_ts: u64,
    pub grants: Vec<Capability>,
    pub revoked: bool,
    /// APNs device token (D16, additive: `None` on pre-push registries).
    /// Set via the signed `POST /device-token`; cleared by the device (send
    /// an empty token) or by the notifier when Apple says the token died
    /// (HTTP 410 `Unregistered`) — that is the push-side revocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_token: Option<String>,
}

impl DeviceRecord {
    /// Push eligibility: a live, non-revoked key WITH a registered token.
    /// The notifier filters on this — an expired/revoked key never receives
    /// a notification, and a token-less device silently gets nothing.
    pub fn push_eligible(&self) -> bool {
        !self.revoked && self.device_token.is_some() && !self.expired()
    }

    /// Whether the key has passed its expiry (checked by the notifier and
    /// the authorizer on every verify).
    pub fn expired(&self) -> bool {
        now_secs() >= self.expiry_ts
    }
}

/// base64 (de)serialization of the raw Ed25519 public key.
mod b64_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::b64_encode;

    pub fn serialize<S>(key: &[u8; 32], s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(&b64_encode(key))
    }

    pub fn deserialize<'de, D>(d: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        super::b64_decode_array_32(&s).ok_or_else(|| serde::de::Error::custom("bad base64 key"))
    }
}

#[derive(Serialize, Deserialize)]
struct RegistryFile {
    schema_version: u32,
    devices: BTreeMap<String, DeviceRecord>,
}

#[derive(Default)]
struct RegistryData {
    devices: BTreeMap<String, DeviceRecord>,
}

pub struct DeviceRegistry {
    path: PathBuf,
    token_path: PathBuf,
    inner: Mutex<RegistryData>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    /// The registration token presented did not match (constant-time
    /// compare). Routing-only credential — never authenticates writes.
    BadToken,
    /// The public key was not 32 bytes of valid base64, or is not a
    /// canonical Ed25519 point.
    BadPublicKey,
    /// The registry could not be persisted (disk full, perms, …). The
    /// in-memory registration is applied; the error propagates instead of
    /// panicking under the registry lock (F8).
    Persist(String),
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadToken => write!(f, "bad registration token"),
            Self::BadPublicKey => write!(f, "malformed device public key"),
            Self::Persist(e) => write!(f, "registry persist failed: {e}"),
        }
    }
}

impl std::error::Error for RegisterError {}

/// The registration token, refreshed once at first run. The client-facing
/// side of the D13 "routing only" credential.
pub fn registration_token_path(config_dir: &Path) -> PathBuf {
    config_dir.join(REGISTRATION_TOKEN_FILE)
}

impl DeviceRegistry {
    pub fn load_or_create(config_dir: &Path) -> Result<Self, String> {
        let path = config_dir.join(REGISTRY_FILE);
        let token_path = registration_token_path(config_dir);
        std::fs::create_dir_all(config_dir).map_err(|e| format!("mkdir: {e}"))?;
        super::ensure_dir_0700(config_dir)?;

        // Registration token: load or create (routing only). Mode enforced
        // on the load path too (F5).
        let token = match std::fs::read_to_string(&token_path) {
            Ok(content) => {
                super::ensure_file_0600(&token_path)?;
                content.trim().to_string()
            }
            Err(_) => {
                let mut raw = random_bytes::<32>();
                let encoded = b64_encode(&raw);
                raw.zeroize();
                write_secret(&token_path, encoded.as_bytes())?;
                encoded
            }
        };
        if token.is_empty() {
            return Err(format!(
                "corrupt registration token {}",
                token_path.display()
            ));
        }

        let data = match std::fs::read_to_string(&path) {
            Ok(content) => {
                super::ensure_file_0600(&path)?;
                let file: RegistryFile = serde_json::from_str(&content)
                    .map_err(|e| format!("corrupt registry {}: {e}", path.display()))?;
                if file.schema_version != SCHEMA_VERSION {
                    return Err(format!(
                        "registry {} has schema_version {} (expected {SCHEMA_VERSION}); \
                         refusing to migrate silently",
                        path.display(),
                        file.schema_version
                    ));
                }
                RegistryData {
                    devices: file.devices,
                }
            }
            Err(_) => RegistryData::default(),
        };
        Ok(Self {
            path,
            token_path,
            inner: Mutex::new(data),
        })
    }

    /// The routing-only token (for the `POST /register` check and for
    /// host-side provisioning).
    pub fn registration_token(&self) -> String {
        self.load_token()
    }

    fn load_token(&self) -> String {
        std::fs::read_to_string(&self.token_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    /// Register a device Ed25519 public key. Returns the (possibly
    /// existing) record — registering the same key twice is idempotent and
    /// never refreshes expiry or grants.
    pub fn register(
        &self,
        token: &str,
        public_key: [u8; 32],
        ttl: std::time::Duration,
    ) -> Result<DeviceRecord, RegisterError> {
        if !super::constant_time_eq(token.as_bytes(), self.load_token().as_bytes()) {
            return Err(RegisterError::BadToken);
        }
        // Reject non-canonical / weak Ed25519 points at the door so the
        // registry only ever holds keys verify() can use. (A weak key —
        // low order, e.g. all zeros — would be rejected by verify_strict
        // anyway; failing at registration makes the failure typed.)
        let pk = ed25519_dalek::VerifyingKey::from_bytes(&public_key)
            .map_err(|_| RegisterError::BadPublicKey)?;
        if pk.is_weak() {
            return Err(RegisterError::BadPublicKey);
        }

        let mut inner = self.inner.lock().expect("registry lock poisoned");
        for rec in inner.devices.values() {
            if rec.public_key == public_key {
                return Ok(rec.clone());
            }
        }
        let now = now_secs();
        let key_id = key_id_for(&public_key);
        let rec = DeviceRecord {
            key_id: key_id.clone(),
            public_key,
            created_ts: now,
            expiry_ts: now.saturating_add(ttl.as_secs()),
            grants: Vec::new(),
            revoked: false,
            device_token: None,
        };
        inner.devices.insert(key_id, rec.clone());
        self.persist_locked(&inner)
            .map_err(RegisterError::Persist)?;
        Ok(rec)
    }

    /// Look up a device by key id (the authorizer's first check).
    pub fn get(&self, key_id: &str) -> Option<DeviceRecord> {
        self.inner
            .lock()
            .expect("registry lock poisoned")
            .devices
            .get(key_id)
            .cloned()
    }

    /// Replace the grant set (host promotion/demotion). Empty = read-only.
    /// Persist failures propagate (F8): a disk error must not panic while
    /// holding the registry lock (that would poison it for every verify).
    pub fn set_grants(
        &self,
        key_id: &str,
        grants: Vec<Capability>,
    ) -> Result<(), RegistryMutationError> {
        let mut inner = self.inner.lock().expect("registry lock poisoned");
        let rec = inner
            .devices
            .get_mut(key_id)
            .ok_or_else(|| RegistryMutationError::UnknownKey(key_id.to_string()))?;
        rec.grants = grants;
        self.persist_locked(&inner)
            .map_err(RegistryMutationError::Persist)?;
        Ok(())
    }

    /// Set or clear the revoked flag. Revocation is checked on every
    /// verify — there are no authenticated sessions to cut short.
    pub fn set_revoked(&self, key_id: &str, revoked: bool) -> Result<(), RegistryMutationError> {
        let mut inner = self.inner.lock().expect("registry lock poisoned");
        let rec = inner
            .devices
            .get_mut(key_id)
            .ok_or_else(|| RegistryMutationError::UnknownKey(key_id.to_string()))?;
        rec.revoked = revoked;
        self.persist_locked(&inner)
            .map_err(RegistryMutationError::Persist)?;
        Ok(())
    }

    /// Set or clear a device's APNs push token (D16). `None`/empty clears
    /// the registration (per-device revocation; the notifier also clears
    /// it when Apple reports the token dead).
    pub fn set_device_token(
        &self,
        key_id: &str,
        device_token: Option<&str>,
    ) -> Result<(), RegistryMutationError> {
        let token = device_token.filter(|t| !t.is_empty()).map(String::from);
        let mut inner = self.inner.lock().expect("registry lock poisoned");
        let rec = inner
            .devices
            .get_mut(key_id)
            .ok_or_else(|| RegistryMutationError::UnknownKey(key_id.to_string()))?;
        rec.device_token = token;
        self.persist_locked(&inner)
            .map_err(RegistryMutationError::Persist)?;
        Ok(())
    }

    /// Clear the token for every record holding it (used by the notifier
    /// on `Unregistered` — Apple no longer knows the device). No-op when
    /// no record matches; persist failures propagate (F8). All matches are
    /// cleared (F4): the same token can legitimately appear on more than
    /// one record (one install re-registered under two keys), and leaving
    /// one stale copy would fail on every future push.
    pub fn set_device_token_by_token(
        &self,
        device_token: &str,
        replacement: Option<&str>,
    ) -> Result<(), RegistryMutationError> {
        let replacement = replacement.filter(|t| !t.is_empty()).map(String::from);
        let mut inner = self.inner.lock().expect("registry lock poisoned");
        let mut changed = false;
        for rec in inner.devices.values_mut() {
            if rec.device_token.as_deref() == Some(device_token) {
                rec.device_token = replacement.clone();
                changed = true;
            }
        }
        if changed {
            self.persist_locked(&inner)
                .map_err(RegistryMutationError::Persist)
        } else {
            Ok(())
        }
    }

    /// Snapshot of all records (tests, admin display).
    pub fn records(&self) -> Vec<DeviceRecord> {
        self.inner
            .lock()
            .expect("registry lock poisoned")
            .devices
            .values()
            .cloned()
            .collect()
    }

    pub fn device_count(&self) -> usize {
        self.inner
            .lock()
            .expect("registry lock poisoned")
            .devices
            .len()
    }

    /// Persist the registry atomically. Note: on failure the in-memory
    /// mutation is already applied and reported via the error; the next
    /// successful mutation persists the full current state.
    fn persist_locked(&self, inner: &RegistryData) -> Result<(), String> {
        let file = RegistryFile {
            schema_version: SCHEMA_VERSION,
            devices: inner.devices.clone(),
        };
        let json = serde_json::to_vec(&file).map_err(|e| format!("registry serializes: {e}"))?;
        write_secret(&self.path, &json)
    }
}

/// Typed registry mutation failures (F8: persist errors propagate instead
/// of panicking under the lock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryMutationError {
    UnknownKey(String),
    Persist(String),
}

impl fmt::Display for RegistryMutationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey(k) => write!(f, "unknown key: {k}"),
            Self::Persist(e) => write!(f, "registry persist failed: {e}"),
        }
    }
}

impl std::error::Error for RegistryMutationError {}

/// `dev_<hex(sha256(pubkey)[..16])>` — stable, collision-resistant at
/// personal-fleet scale, and independent of registration order.
pub fn key_id_for(public_key: &[u8; 32]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(public_key);
    format!("dev_{}", super::hex(&digest[..16]))
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl fmt::Debug for DeviceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No secrets: registration token and key material never print.
        f.debug_struct("DeviceRegistry")
            .field("path", &self.path)
            .field(
                "devices",
                &self
                    .records()
                    .iter()
                    .map(|r| {
                        (
                            &r.key_id,
                            format!(
                                "grants={:?} revoked={} expiry={}",
                                r.grants, r.revoked, r.expiry_ts
                            ),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    /// A valid Ed25519 public key (random bytes are canonical points only
    /// ~1/8 of the time, so keys must come from a real keypair).
    fn key() -> [u8; 32] {
        let signing = ed25519_dalek::SigningKey::from_bytes(&super::super::random_bytes::<32>());
        signing.verifying_key().to_bytes()
    }

    #[test]
    fn register_gates_on_token_constant_time() {
        let d = dir();
        let reg = DeviceRegistry::load_or_create(d.path()).unwrap();
        let token = reg.registration_token();
        assert!(!token.is_empty());

        let pk = key();
        assert_eq!(
            reg.register("wrong-token", pk, std::time::Duration::from_secs(60)),
            Err(RegisterError::BadToken)
        );
        let rec = reg
            .register(&token, pk, std::time::Duration::from_secs(60))
            .unwrap();
        assert!(rec.key_id.starts_with("dev_"));
        assert!(rec.grants.is_empty(), "default deny: no drive grants");
        assert!(!rec.revoked);
        assert!(rec.expiry_ts > rec.created_ts);
    }

    #[test]
    fn bad_public_keys_rejected() {
        let d = dir();
        let reg = DeviceRegistry::load_or_create(d.path()).unwrap();
        let token = reg.registration_token();
        // All-zero bytes are a weak (low-order) Ed25519 point.
        assert_eq!(
            reg.register(&token, [0u8; 32], std::time::Duration::ZERO),
            Err(RegisterError::BadPublicKey)
        );
    }

    #[test]
    fn re_registration_is_idempotent_and_never_extends() {
        let d = dir();
        let reg = DeviceRegistry::load_or_create(d.path()).unwrap();
        let token = reg.registration_token();
        let pk = key();
        let first = reg
            .register(&token, pk, std::time::Duration::from_secs(100))
            .unwrap();
        let second = reg
            .register(&token, pk, std::time::Duration::from_secs(9999))
            .unwrap();
        assert_eq!(first, second, "same key -> same record, expiry untouched");
        assert_eq!(reg.device_count(), 1);
    }

    #[test]
    fn grants_and_revocation_persist_and_survive_reload() {
        let d = dir();
        let reg = DeviceRegistry::load_or_create(d.path()).unwrap();
        let token = reg.registration_token();
        let pk = key();
        let rec = reg
            .register(&token, pk, std::time::Duration::from_secs(60))
            .unwrap();

        reg.set_grants(&rec.key_id, vec![Capability::ReadTail])
            .unwrap();
        reg.set_revoked(&rec.key_id, true).unwrap();

        let reloaded = DeviceRegistry::load_or_create(d.path()).unwrap();
        let got = reloaded.get(&rec.key_id).unwrap();
        assert_eq!(got.grants, vec![Capability::ReadTail]);
        assert!(got.revoked);

        // Registry file is 0600.
        let meta = std::fs::metadata(d.path().join(REGISTRY_FILE)).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn corrupt_registry_fails_fast() {
        let d = dir();
        std::fs::write(d.path().join(REGISTRY_FILE), "{not json").unwrap();
        assert!(DeviceRegistry::load_or_create(d.path()).is_err());
    }

    /// D16: the device-token column is additive — a pre-push registry.json
    /// (schema 1, no `device_token`) loads with `None`, and a set token
    /// survives reload.
    #[test]
    fn device_token_is_additive_and_persists() {
        let d = dir();
        // Pre-seed a v1 registry WITHOUT the device_token field.
        let content = serde_json::json!({
            "schema_version": 1,
            "devices": {},
        });
        std::fs::write(
            d.path().join(REGISTRY_FILE),
            serde_json::to_vec(&content).unwrap(),
        )
        .unwrap();
        let reg = DeviceRegistry::load_or_create(d.path()).unwrap();
        let token = reg.registration_token();
        let pk = key();
        let rec = reg
            .register(&token, pk, std::time::Duration::from_secs(60))
            .unwrap();
        assert_eq!(rec.device_token, None, "additive column defaults to None");

        reg.set_device_token(&rec.key_id, Some("a1b2c3d4e5f6"))
            .unwrap();
        let reloaded = DeviceRegistry::load_or_create(d.path()).unwrap();
        assert_eq!(
            reloaded.get(&rec.key_id).unwrap().device_token.as_deref(),
            Some("a1b2c3d4e5f6")
        );

        // Empty clears (per-device revocation).
        reloaded.set_device_token(&rec.key_id, Some("")).unwrap();
        assert_eq!(reloaded.get(&rec.key_id).unwrap().device_token, None);

        // by-token clear (the notifier's Unregistered path).
        reloaded
            .set_device_token(&rec.key_id, Some("a1b2c3d4e5f6"))
            .unwrap();
        reloaded
            .set_device_token_by_token("a1b2c3d4e5f6", None)
            .unwrap();
        assert_eq!(reloaded.get(&rec.key_id).unwrap().device_token, None);
        // Unknown token is a no-op, not an error.
        reloaded.set_device_token_by_token("ghost", None).unwrap();
    }

    /// F4: the same token on MORE THAN ONE record (one install
    /// re-registered under two keys) must clear ALL of them, not just the
    /// first match — a leftover dead token fails on every future push.
    #[test]
    fn set_device_token_by_token_clears_all_matching_records() {
        let d = dir();
        let reg = DeviceRegistry::load_or_create(d.path()).unwrap();
        let mut key_ids = vec![];
        for _ in 0..3 {
            let token = reg.registration_token();
            let rec = reg
                .register(&token, key(), std::time::Duration::from_secs(3600))
                .unwrap();
            reg.set_device_token(&rec.key_id, Some("deadbeef0001"))
                .unwrap();
            key_ids.push(rec.key_id);
        }

        reg.set_device_token_by_token("deadbeef0001", None).unwrap();
        for id in &key_ids {
            assert_eq!(
                reg.get(id).unwrap().device_token,
                None,
                "every record sharing the token is cleared (F4)"
            );
        }
    }

    /// D16: push eligibility = live key + registered token; revoked or
    /// expired keys never push even with a token.
    #[test]
    fn push_eligibility_gates_revoked_and_expired_keys() {
        let d = dir();
        let reg = DeviceRegistry::load_or_create(d.path()).unwrap();
        let token = reg.registration_token();
        let pk = key();
        let rec = reg
            .register(&token, pk, std::time::Duration::from_secs(3600))
            .unwrap();

        assert!(!rec.push_eligible(), "no token -> not eligible");
        reg.set_device_token(&rec.key_id, Some("tok")).unwrap();
        assert!(reg.get(&rec.key_id).unwrap().push_eligible());

        reg.set_revoked(&rec.key_id, true).unwrap();
        assert!(!reg.get(&rec.key_id).unwrap().push_eligible());

        let pk2 = key();
        let rec2 = reg
            .register(&token, pk2, std::time::Duration::ZERO)
            .unwrap();
        reg.set_device_token(&rec2.key_id, Some("tok2")).unwrap();
        assert!(rec2.expired(), "zero TTL key is already expired");
        assert!(!reg.get(&rec2.key_id).unwrap().push_eligible());
    }

    /// F8: a persist failure must propagate as a typed error — never
    /// panic while holding the registry lock (that would poison it for
    /// every subsequent verify). Failure is injected by replacing
    /// `registry.json` with a directory: `write_secret`'s atomic rename
    /// over a directory fails while the dir stays writable.
    #[test]
    fn f8_persist_failure_propagates_and_lock_stays_usable() {
        let d = dir();
        let reg = DeviceRegistry::load_or_create(d.path()).unwrap();
        let token = reg.registration_token();
        let pk = key();
        let rec = reg
            .register(&token, pk, std::time::Duration::from_secs(60))
            .unwrap();

        // Inject the failure: registry.json becomes a directory.
        std::fs::remove_file(d.path().join(REGISTRY_FILE)).unwrap();
        std::fs::create_dir(d.path().join(REGISTRY_FILE)).unwrap();

        let err = reg.register(&token, key(), std::time::Duration::from_secs(60));
        assert!(
            matches!(err, Err(RegisterError::Persist(_))),
            "persist error must propagate: {err:?}"
        );
        let grants_err = reg.set_grants(&rec.key_id, vec![Capability::ReadTail]);
        assert!(
            matches!(grants_err, Err(RegistryMutationError::Persist(_))),
            "set_grants persist error must propagate: {grants_err:?}"
        );

        // The lock is NOT poisoned: reads still answer (F8's whole point).
        assert!(reg.get(&rec.key_id).is_some());
        assert_eq!(reg.device_count(), 2, "in-memory state applied");

        // Restore for TempDir cleanup.
        std::fs::remove_dir(d.path().join(REGISTRY_FILE)).unwrap();
    }

    /// F5: a pre-seeded registry.json with permissive modes is tightened
    /// on load, not just at creation.
    #[test]
    fn f5_load_enforces_0600() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir();
        // Pre-seed a valid registry file with 0644.
        let content = serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "devices": {},
        });
        std::fs::write(
            d.path().join(REGISTRY_FILE),
            serde_json::to_vec(&content).unwrap(),
        )
        .unwrap();
        std::fs::set_permissions(
            d.path().join(REGISTRY_FILE),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::write(
            d.path().join(REGISTRATION_TOKEN_FILE),
            b64_encode(&[7u8; 32]),
        )
        .unwrap();
        std::fs::set_permissions(
            d.path().join(REGISTRATION_TOKEN_FILE),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

        let reg = DeviceRegistry::load_or_create(d.path()).unwrap();
        assert_eq!(
            std::fs::metadata(d.path().join(REGISTRY_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "registry.json tightened to 0600 on load"
        );
        assert_eq!(
            std::fs::metadata(d.path().join(REGISTRATION_TOKEN_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "registration-token tightened to 0600 on load"
        );
        assert_eq!(
            std::fs::metadata(d.path()).unwrap().permissions().mode() & 0o777,
            0o700,
            "config dir tightened to 0700 on load"
        );
        assert_eq!(
            reg.registration_token(),
            b64_encode(&[7u8; 32]),
            "pre-seeded token preserved"
        );
    }

    #[test]
    fn debug_never_leaks_registration_token_or_keys() {
        let d = dir();
        let reg = DeviceRegistry::load_or_create(d.path()).unwrap();
        let token = reg.registration_token();
        let dbg = format!("{reg:?}");
        assert!(!dbg.contains(&token), "registration token leaked in Debug");
        let pk = key();
        let rec = reg
            .register(&token, pk, std::time::Duration::from_secs(60))
            .unwrap();
        let dbg = format!("{reg:?}");
        assert!(dbg.contains(&rec.key_id));
        assert!(
            !dbg.contains(&super::super::b64_encode(&pk)),
            "public key must not print either — keys are not secret but Debug stays minimal"
        );
    }
}
