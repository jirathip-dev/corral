//! W3 — device-keypair signatures (D10/D13): the authentication + audit
//! plane for the drive plane.
//!
//! ## Credentials (3, never 1 — D13)
//!
//! 1. **Registration token** — `registration-token` file in the config dir
//!    (`~/.config/corral`, override `CORRAL_CONFIG_DIR`), 256-bit random,
//!    routing only: it gates `POST /register` so only a host-provisioned
//!    client can enroll a device key. It never authenticates a drive write.
//! 2. **Per-device keypair** — Ed25519 keys held by the client (Secure
//!    Enclave/Keychain on iOS). The device registers its public key once;
//!    every drive command is signed with the device key and verified by
//!    [`DeviceAuthorizer`] over the contract's canonical envelope bytes.
//! 3. **Per-capability grants** — registered devices default to **read-only**
//!    (no drive grants; the read plane `/snapshot` + `/events` is loopback
//!    local). Writes require an explicit host grant via `POST /grants`
//!    (admin token). Revocation + expiry are checked on every verify.
//!
//! ## Signature scheme
//!
//! - **Device keys: Ed25519 (RFC 8032)**. Signing bytes are
//!   [`canonical_envelope_bytes`](crate::drive::canonical_envelope_bytes) —
//!   the fixed-order JSON of the drive envelope — per the contract. The
//!   signature is base64 (RFC 4648 §4, with padding); the key id is
//!   `dev_<hex(sha256(pubkey)[..16])>`. Verification uses
//!   `VerifyingKey::verify_strict` (rejects non-canonical points and
//!   malleable signatures).
//! - **Host identity: X25519 (NOT a hostname)**, published by `GET
//!   /host-key` as `{"algorithm":"X25519","public_key":<base64>}`. X25519 is
//!   deliberately NOT used for signing: it exists for host identification
//!   and any future ECDH need (e.g. an encrypted pairing channel). The host
//!   secret lives in `host-key` (0600) and is never logged or serialized.
//!
//! ## Files in the config dir (all 0600, dir 0700)
//!
//! | file                 | contents                                                       |
//! |----------------------|----------------------------------------------------------------|
//! | `host-key`           | X25519 secret, base64 of 32 bytes. Generated on first run.    |
//! | `registration-token` | 256-bit token, base64. Routing only.                          |
//! | `admin-token`        | 256-bit token, base64. Host's own credential: authorizes the  |
//! |                      | admin endpoints (`GET /audit`, `POST /grants`). Never handed  |
//! |                      | to devices.                                                   |
//! | `registry.json`      | Versioned device registry: key id, Ed25519 public key,        |
//! |                      | created/expiry ts, grants, revoked flag.                      |
//! | `audit.log`          | Append-only, SHA-256 hash-chained audit entries (one JSON     |
//! |                      | line per entry).                                              |
//!
//! ## Rotation story
//!
//! - **Host identity**: stop the daemon, delete `host-key`, restart. The
//!   new key is published by `GET /host-key`; clients should re-verify it
//!   out of band. Device auth is unaffected (it is keyed on device keys,
//!   not the host key).
//! - **Registration token**: delete `registration-token`, restart. Existing
//!   devices keep working; new enrollments need the new token.
//! - **Admin token**: delete `admin-token`, restart.
//! - **Device keys**: devices expire after [`REGISTRATION_TTL`]; re-register
//!   with a fresh key (new `key_id`) before expiry. Host revokes via
//!   `POST /grants` with `action: "revoke"`; revocation takes effect on the
//!   next verify (there are no long-lived authenticated sessions to cut
//!   short — every drive is verified per request).
//!
//! ## Biometric step-up (destructive patterns)
//!
//! Payloads matching a destructive pattern (`rm -rf`, `push --force`,
//! `curl|sh` and spaced pipe-to-shell forms, `dd of=…` / `dd if=…of=`,
//! `~/.aws`, `~/.ssh`, `.env`, remote-eval and process-substitution
//! forms, …) require a step-up proof: a short-lived (5 min), single-use
//! token minted by `POST /step-up` only after the client proves possession
//! of the device signing key (signed [`StepUpRequest`], freshness
//! enforced: `|now - ts| < 60s`). The drive seam then demands
//! `X-Step-Up-Token: <token>` and binds it to the same `key_id`. No
//! auto-approve in v1.
//!
//! **Honest scope note**: pattern detection canonicalizes the payload
//! (lowercase, `\s+` → single space, `$HOME` → `~`, quote normalization)
//! and is deliberately conservative, but it is a **deterrent layer, not a
//! security boundary** — a determined attacker with full prompt control
//! can always obfuscate (e.g. `rm$'\x2d\x2dr\x2df'`). The real boundaries
//! are the per-capability prompt grant, W2's claim-based approval, and the
//! step-up itself as friction. Patterns live in one table as the W4
//! extension point.
//!
//! Note: `AuthError` is contract-fixed and has no `StepUpRequired` variant
//! (src/drive/mod.rs is frozen), so step-up is enforced as a **second gate
//! after `verify()`** — see [`StepUpGate`] and the drive seam in `http.rs`.
//!
//! ## Audit log (AC5)
//!
//! [`HashChainAuditLog`] appends one hash-chained entry per write and
//! exposes the chain via `GET /audit` (admin token). **Growth policy is
//! enforced by the caller**: entries are appended only for drive
//! executions and typed refusals at dispatch — never for GETs and never
//! for authentication failures. The drive seam follows this; tests prove
//! it with a caller stub. A crash mid-append is repaired at next open as a
//! flagged tombstone line (skipped by the chain verifier). **TODO(W4)**:
//! the chain head is self-referential (derived from the file itself), so
//! wholesale truncation of trailing entries is undetectable without an
//! external anchor (e.g. a second checkpoint file, or a host-key-signed
//! head shipped off-machine) — W4 must add that anchor.
//!
//! ## Security invariants
//!
//! - Default deny: `verify` returns `NotGranted` unless the key holds the
//!   capability.
//! - Secrets are never logged; `Debug` impls in this module print only
//!   public material (public keys, key ids, counts).
//! - Registration/admin tokens are compared in constant time (`subtle`).
//! - Key material is persisted 0600 under a 0700 directory, enforced on
//!   every load path as well as creation (F5).
//! - Step-up tokens are reaped when expired (mint + spend) and capped at
//!   [`step_up::MAX_LIVE_TOKENS`] (F3).
//! - Secret accessors (`test_support`, admin-token getter, host secret) are
//!   compiled only under `#[cfg(any(test, feature = "test-utils"))]` — the
//!   release binary exposes none of them (F12).

pub mod audit;
pub mod authorizer;
pub mod host_identity;
pub mod http;
pub mod registry;
pub mod step_up;

pub use audit::HashChainAuditLog;
pub use authorizer::DeviceAuthorizer;
pub use host_identity::HostIdentity;
pub use registry::{DeviceRecord, DeviceRegistry, RegisterError, now_secs};
pub use step_up::{StepUpError, StepUpGate};

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::HeaderMap;
use zeroize::Zeroize;

use crate::drive::DriveAuthorizer;

/// Registration token (routing only), admin token, step-up tokens: 32
/// random bytes each.
const TOKEN_BYTES: usize = 32;
/// Default device lifetime: 90 days. The host controls TTLs at
/// registration; clients cannot extend their own validity.
pub const REGISTRATION_TTL: std::time::Duration = std::time::Duration::from_secs(90 * 24 * 3600);
/// Step-up tokens live 5 minutes and are single-use.
pub const STEP_UP_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Random bytes for token/key material. Panics only if the OS RNG fails
/// (unrecoverable — there is no safe fallback for key material).
pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).expect("OS RNG failure: cannot mint key material");
    out
}

/// Lowercase hex encoding for key ids and audit hashes.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        write!(&mut s, "{b:02x}").expect("hex write to String cannot fail");
    }
    s
}

/// RFC 4648 §4 base64 with padding — used for all public keys,
/// signatures, and opaque tokens on the wire and on disk.
pub(crate) fn b64_encode(bytes: &[u8]) -> String {
    use base64::engine::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The auth plane wired into `AppState`: host identity, registry,
/// authorizer, step-up gate, audit log, and the host's admin credential.
pub struct AuthPlane {
    pub host: HostIdentity,
    pub registry: Arc<DeviceRegistry>,
    /// The contract trait object W1's `POST /drive` handler calls.
    pub authorizer: Arc<dyn DriveAuthorizer>,
    pub step_up: Arc<StepUpGate>,
    /// Implements the contract [`AuditLog`] trait (append-only, chained).
    pub audit: Arc<HashChainAuditLog>,
    config_dir: PathBuf,
    admin_token: String,
}

impl AuthPlane {
    /// Load-or-create every credential and store in `config_dir`
    /// (`CORRAL_CONFIG_DIR` or `~/.config/corral`). Fails fast on corrupt
    /// state — never silently re-keys.
    pub fn load_or_create(config_dir: PathBuf) -> Result<Self, String> {
        let host = HostIdentity::load_or_create(&config_dir)?;
        let registry = Arc::new(DeviceRegistry::load_or_create(&config_dir)?);
        let authorizer: Arc<dyn DriveAuthorizer> =
            Arc::new(DeviceAuthorizer::new(registry.clone()));
        let step_up = Arc::new(StepUpGate::new());
        let audit = Arc::new(HashChainAuditLog::open(&config_dir)?);
        let admin_token = load_or_create_secret(&config_dir.join("admin-token"))?;
        Ok(Self {
            host,
            registry,
            authorizer,
            step_up,
            audit,
            config_dir,
            admin_token,
        })
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// `Authorization: Bearer <admin-token>` — the host's own credential
    /// (loopback local access is not enough to distinguish the host from a
    /// device, so the admin token is the host's proof of identity).
    pub fn check_admin(&self, headers: &HeaderMap) -> bool {
        headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|tok| constant_time_eq(tok.as_bytes(), self.admin_token.as_bytes()))
    }
}

/// Constant-time string equality for opaque tokens (`subtle`).
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

/// Read a 0600 file containing base64 of [`TOKEN_BYTES`] random bytes,
/// creating it on first run. Enforces 0600/0700 on the load path too
/// (F5): a pre-seeded file with permissive modes is tightened here, not
/// only at creation.
fn load_or_create_secret(path: &Path) -> Result<String, String> {
    ensure_dir_0700(path.parent().ok_or("secret path has no parent")?)?;
    if let Ok(content) = std::fs::read_to_string(path) {
        ensure_file_0600(path)?;
        let trimmed = content.trim();
        let mut decoded = decode_b64(trimmed).ok_or_else(|| format!("corrupt secret file {}", path.display()))?;
        if decoded.len() != TOKEN_BYTES {
            return Err(format!(
                "corrupt secret file {}: expected {} bytes, got {}",
                path.display(),
                TOKEN_BYTES,
                decoded.len()
            ));
        }
        let token = trimmed.to_string();
        decoded.zeroize();
        return Ok(token);
    }
    let mut raw = random_bytes::<TOKEN_BYTES>();
    let encoded = b64_encode(&raw);
    raw.zeroize();
    write_secret(path, encoded.as_bytes())?;
    Ok(encoded)
}

/// Ensure a config dir exists and is 0700 (F5: enforced on every load).
pub(crate) fn ensure_dir_0700(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("chmod {}: {e}", dir.display()))
}

/// Ensure a key-material file is 0600 (F5: enforced on every load, not
/// only at creation).
pub(crate) fn ensure_file_0600(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod {}: {e}", path.display()))
}

/// Write a secret file atomically (temp + rename) with 0600 perms.
pub(crate) fn write_secret(path: &Path, contents: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let dir = path
        .parent()
        .ok_or_else(|| format!("secret path has no parent: {}", path.display()))?;
    ensure_dir_0700(dir)?;

    let tmp = dir.join(format!(".{}.tmp.{}", file_stem(path), std::process::id()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("open {}: {e}", tmp.display()))?;
    f.write_all(contents)
        .and_then(|_| f.write_all(b"\n"))
        .and_then(|_| f.flush())
        .and_then(|_| f.sync_all())
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod {}: {e}", path.display()))?;
    Ok(())
}

fn file_stem(path: &Path) -> &str {
    path.file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.split('.').next())
        .unwrap_or("secret")
}

/// Decode base64 (RFC 4648 §4, padding required).
pub(crate) fn decode_b64(s: &str) -> Option<Vec<u8>> {
    use base64::engine::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .ok()
}

/// Public-key registry lookup result used by the HTTP layer.
pub(crate) fn b64_decode_array_32(s: &str) -> Option<[u8; 32]> {
    decode_b64(s).and_then(|v| v.try_into().ok())
}

impl fmt::Debug for AuthPlane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No secrets: public key material and counts only.
        f.debug_struct("AuthPlane")
            .field("host", &self.host)
            .field("config_dir", &self.config_dir)
            .field("registry", &self.registry)
            .field("step_up", &self.step_up)
            .field("audit", &self.audit)
            .finish_non_exhaustive()
    }
}

/// Host-side accessor for the admin credential (tests + host tooling only;
/// never transmitted to devices). Compiled only for tests or the explicit
/// `test-utils` feature — absent from production builds (F12).
#[cfg(any(test, feature = "test-utils"))]
#[doc(hidden)]
pub fn admin_token_for_test(plane: &AuthPlane) -> String {
    plane.admin_token.clone()
}

/// Signing + envelope helpers for the test suite and the live self-test
/// client. Compiled only for tests or the explicit `test-utils` feature
/// (F12).
#[cfg(any(test, feature = "test-utils"))]
#[doc(hidden)]
pub mod test_support {
    use std::sync::Arc;

    use crate::drive::{Capability, DriveEnvelope, SignedDrive, canonical_envelope_bytes};

    use super::{DeviceAuthorizer, DeviceRegistry, b64_encode};

    pub fn keypair() -> (ed25519_dalek::SigningKey, [u8; 32]) {
        let signing = ed25519_dalek::SigningKey::from_bytes(&super::random_bytes::<32>());
        let pubkey = signing.verifying_key().to_bytes();
        (signing, pubkey)
    }

    pub fn envelope(request_id: &str, capability: Capability, text: &str) -> DriveEnvelope {
        DriveEnvelope {
            request_id: request_id.to_string(),
            capability,
            target: "herdr:agent-a".to_string(),
            payload: serde_json::json!({ "kind": "prompt", "text": text }),
            rev: None,
        }
    }

    pub fn sign(signing: &ed25519_dalek::SigningKey, envelope: &DriveEnvelope) -> String {
        use ed25519_dalek::Signer as _;
        let sig = signing.sign(&canonical_envelope_bytes(envelope));
        b64_encode(&sig.to_bytes())
    }

    pub fn sign_bytes(signing: &ed25519_dalek::SigningKey, bytes: &[u8]) -> String {
        use ed25519_dalek::Signer as _;
        let sig = signing.sign(bytes);
        b64_encode(&sig.to_bytes())
    }

    pub fn signed(
        registry: &DeviceRegistry,
        token: &str,
        signing: &ed25519_dalek::SigningKey,
        pubkey: [u8; 32],
        envelope: &DriveEnvelope,
    ) -> SignedDrive {
        let rec = registry
            .register(token, pubkey, std::time::Duration::from_secs(3600))
            .expect("register");
        SignedDrive {
            key_id: rec.key_id,
            signature: sign(signing, envelope),
            envelope: envelope.clone(),
        }
    }

    /// Fresh in-memory registry + authorizer over a temp dir (unit tests).
    /// Hand-rolled tempdir: `tempfile` is a dev-dependency and must not
    /// reach the lib build used by integration tests.
    pub fn setup() -> (
        Arc<DeviceRegistry>,
        Arc<DeviceAuthorizer>,
        String,
        std::path::PathBuf,
    ) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "corral-auth-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("test dir");
        let registry = Arc::new(DeviceRegistry::load_or_create(&dir).expect("registry"));
        let token = registry.registration_token();
        let authorizer = Arc::new(DeviceAuthorizer::new(registry.clone()));
        (registry, authorizer, token, dir)
    }

    pub fn public_b64(pubkey: &[u8; 32]) -> String {
        b64_encode(pubkey)
    }

    pub fn now_secs() -> u64 {
        super::now_secs()
    }

    /// Entropy for client-side key generation (live self-test client).
    pub fn random_bytes_for_client() -> [u8; 32] {
        super::random_bytes::<32>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_round_trip() {
        let raw = b"the quick brown fox";
        assert_eq!(decode_b64(&b64_encode(raw)).unwrap(), raw);
        assert!(decode_b64("not base64!!!").is_none());
        assert!(decode_b64("aGVsbG8").is_none(), "padding required");
    }

    #[test]
    fn constant_time_eq_matches_only_identical() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(&[], &[]));
    }
}
