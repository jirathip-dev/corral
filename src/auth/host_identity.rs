//! Host identity: an X25519 keypair (NOT a hostname), generated on first
//! run and persisted at `<config_dir>/host-key` (0600).
//!
//! The X25519 key is identity + future ECDH material only — never used for
//! signing (device writes are signed with per-device Ed25519 keys, see
//! `mod.rs`). Rotation: stop the daemon, delete `host-key`, restart; the
//! new key is published by `GET /host-key`.

use std::fmt;
use std::path::{Path, PathBuf};

use x25519_dalek::{PublicKey, StaticSecret};

use super::{b64_encode, random_bytes, write_secret};

/// File name of the host identity secret inside the config dir.
pub const HOST_KEY_FILE: &str = "host-key";
/// Algorithm string published by `GET /host-key`.
pub const HOST_KEY_ALGORITHM: &str = "X25519";

pub struct HostIdentity {
    /// Held in memory (zeroized on drop via x25519-dalek/zeroize) for a
    /// future ECDH pairing channel; no production read path yet, so the
    /// non-test build sees it as unread.
    #[cfg_attr(not(any(test, feature = "test-utils")), allow(dead_code))]
    secret: StaticSecret,
    public_key: [u8; 32],
    path: PathBuf,
}

impl HostIdentity {
    /// Load the host key from `<config_dir>/host-key`, creating it on
    /// first run. Fails fast on corrupt material — never silently re-keys.
    /// Enforces 0700/0600 on the load path too (F5).
    pub fn load_or_create(config_dir: &Path) -> Result<Self, String> {
        use zeroize::Zeroize;
        super::ensure_dir_0700(config_dir)?;
        let path = config_dir.join(HOST_KEY_FILE);
        let secret: [u8; 32] = match std::fs::read_to_string(&path) {
            Ok(content) => {
                super::ensure_file_0600(&path)?;
                let mut decoded = super::decode_b64(content.trim())
                    .ok_or_else(|| format!("corrupt host key {}: not base64", path.display()))?;
                if decoded.len() != 32 {
                    decoded.zeroize();
                    return Err(format!("corrupt host key {}: wrong length", path.display()));
                }
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&decoded);
                decoded.zeroize();
                bytes
            }
            Err(_) => {
                let mut raw = random_bytes::<32>();
                let encoded = b64_encode(&raw);
                write_secret(&path, encoded.as_bytes())?;
                let out = raw;
                raw.zeroize();
                out
            }
        };
        let secret = StaticSecret::from(secret);
        let public_key: [u8; 32] = PublicKey::from(&secret).as_bytes().to_owned();
        Ok(Self {
            secret,
            public_key,
            path,
        })
    }

    /// The X25519 public key — the host's identity, base64 on the wire.
    pub fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    pub fn public_key_b64(&self) -> String {
        b64_encode(&self.public_key)
    }

    pub fn algorithm(&self) -> &'static str {
        HOST_KEY_ALGORITHM
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Secret accessor for a future ECDH pairing channel. Compiled only
    /// for tests or the explicit `test-utils` feature (F12) — the release
    /// binary cannot lift the host secret.
    #[cfg(any(test, feature = "test-utils"))]
    #[doc(hidden)]
    pub fn secret(&self) -> &StaticSecret {
        &self.secret
    }
}

impl fmt::Debug for HostIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Public key only — the secret never reaches a Debug/Display path.
        f.debug_struct("HostIdentity")
            .field("algorithm", &self.algorithm())
            .field("public_key", &super::hex(&self.public_key))
            .field("path", &self.path)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn created_with_0600_and_stable_across_restart() {
        let d = dir();
        let first = HostIdentity::load_or_create(d.path()).unwrap();
        let path = d.path().join(HOST_KEY_FILE);
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o600,
            "host key must be 0600"
        );
        let dir_mode = std::fs::metadata(d.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "config dir must be 0700");

        let second = HostIdentity::load_or_create(d.path()).unwrap();
        assert_eq!(
            first.public_key(),
            second.public_key(),
            "key must not rotate"
        );
        assert_eq!(first.public_key_b64(), second.public_key_b64());
    }

    #[test]
    fn corrupt_key_fails_fast() {
        let d = dir();
        std::fs::write(d.path().join(HOST_KEY_FILE), "not-base64!").unwrap();
        assert!(HostIdentity::load_or_create(d.path()).is_err());
        std::fs::write(d.path().join(HOST_KEY_FILE), b64_encode(&[1, 2, 3])).unwrap();
        assert!(
            HostIdentity::load_or_create(d.path()).is_err(),
            "wrong length"
        );
    }

    #[test]
    fn debug_never_leaks_secret() {
        let d = dir();
        let id = HostIdentity::load_or_create(d.path()).unwrap();
        let dbg = format!("{id:?}");
        assert!(dbg.contains("X25519"));
        assert!(
            !dbg.contains("secret"),
            "no secret-bearing field name in Debug"
        );
        // The base64 secret cannot appear in the debug output.
        let secret_b64 = super::b64_encode(&id.secret().to_bytes());
        assert!(!dbg.contains(&secret_b64), "secret leaked in Debug");
    }
}
