//! Device identity (D10/D13): Ed25519 keypair generated on first run,
//! stored in the OS keychain where available (macOS Keychain / Linux
//! kernel keyring via `keyring`), else a 0600 file under the client
//! config dir with a startup warning. Registration records (key_id per
//! host fingerprint) live in a small JSON config, never key material.

use std::fmt;
use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use zeroize::Zeroize;

/// Client config dir: `$CORRAL_UI_CONFIG_DIR` or `$HOME/.config/corral/ui`
/// (matches the daemon's HOME-based convention so localhost auto-register
/// can read `$HOME/.config/corral/registration-token`).
pub fn client_config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CORRAL_UI_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/corral/ui")
}

fn daemon_config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CORRAL_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/corral")
}

/// Where the device key actually lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyStore {
    Keyring,
    /// 0600 file fallback (documented warning surfaced at startup).
    File {
        path: PathBuf,
    },
}

impl fmt::Display for KeyStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keyring => write!(f, "OS keychain"),
            Self::File { path } => write!(f, "0600 file {}", path.display()),
        }
    }
}

const KEYRING_SERVICE: &str = "corrald-ui";

fn keyring_enabled() -> bool {
    !matches!(
        std::env::var("CORRAL_UI_DISABLE_KEYRING").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// A loaded device identity.
pub struct DeviceKey {
    pub signing: SigningKey,
    pub store: KeyStore,
}

fn account_for(host_fingerprint: &str) -> String {
    format!("corral-device:{host_fingerprint}")
}

/// Load the device seed for `host_fingerprint`, generating + storing it on
/// first use. Keychain first, 0600 file fallback (per P4-conformance:
/// "macOS Keychain where available, 0600 file fallback with a startup
/// warning"). Never silently weakens: the store kind is returned so the
/// UI can surface the warning banner.
pub fn load_or_create_key(host_fingerprint: &str) -> Result<DeviceKey, String> {
    let account = account_for(host_fingerprint);
    if keyring_enabled()
        && let Ok(seed) = read_keyring(&account)
    {
        let signing = SigningKey::from_bytes(&seed);
        return Ok(DeviceKey {
            signing,
            store: KeyStore::Keyring,
        });
    }
    let dir = client_config_dir().join("keys");
    let path = dir.join(format!("{host_fingerprint}.key"));
    if let Ok(seed) = read_key_file(&path) {
        let signing = SigningKey::from_bytes(&seed);
        return Ok(DeviceKey {
            signing,
            store: KeyStore::File { path },
        });
    }
    // First run: generate fresh entropy.
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| format!("OS RNG failure: {e}"))?;
    let signing = SigningKey::from_bytes(&seed);

    // Prefer the keychain; fall back to the 0600 file. The explicit disabled
    // mode is used by unattended scratch/evidence runs so macOS cannot stop
    // the native window behind an interactive Keychain prompt.
    if keyring_enabled() {
        match write_keyring(&account, &seed) {
            Ok(()) => {
                seed.zeroize();
                Ok(DeviceKey {
                    signing,
                    store: KeyStore::Keyring,
                })
            }
            Err(keyring_err) => {
                write_key_file(&path, &seed).map_err(|file_err| {
                    format!(
                        "keychain unavailable ({keyring_err}) and file write failed: {file_err}"
                    )
                })?;
                seed.zeroize();
                Ok(DeviceKey {
                    signing,
                    store: KeyStore::File { path },
                })
            }
        }
    } else {
        write_key_file(&path, &seed)?;
        seed.zeroize();
        Ok(DeviceKey {
            signing,
            store: KeyStore::File { path },
        })
    }
}

/// Rotate: replace the stored seed (used on re-registration).
pub fn rotate_key(host_fingerprint: &str, seed: &[u8; 32]) -> Result<(), String> {
    let account = account_for(host_fingerprint);
    if !keyring_enabled() {
        let path = client_config_dir()
            .join("keys")
            .join(format!("{host_fingerprint}.key"));
        return write_key_file(&path, seed);
    }
    match write_keyring(&account, seed) {
        Ok(()) => {
            // Best-effort remove of any earlier file fallback.
            let path = client_config_dir()
                .join("keys")
                .join(format!("{host_fingerprint}.key"));
            let _ = std::fs::remove_file(path);
            Ok(())
        }
        Err(keyring_err) => {
            let path = client_config_dir()
                .join("keys")
                .join(format!("{host_fingerprint}.key"));
            write_key_file(&path, seed)
                .map_err(|file_err| format!("keychain unavailable ({keyring_err}): {file_err}"))
        }
    }
}

/// Host admin token for host-side administration (audit + grants):
/// keychain-only (never written to a plaintext config). Returns the store
/// kind used.
pub fn store_admin_token(host_fingerprint: &str, token: &str) -> Result<KeyStore, String> {
    if !keyring_enabled() {
        return Err("keychain disabled by CORRAL_UI_DISABLE_KEYRING".to_string());
    }
    let account = format!("corral-admin:{host_fingerprint}");
    match write_keyring(&account, token.as_bytes()) {
        Ok(()) => Ok(KeyStore::Keyring),
        Err(e) => Err(format!("keychain unavailable: {e}")),
    }
}

pub fn load_admin_token(host_fingerprint: &str) -> Option<String> {
    if !keyring_enabled() {
        return None;
    }
    let account = format!("corral-admin:{host_fingerprint}");
    read_keyring(&account)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
}

/// Fingerprint scoping a device key to one host: first 16 hex chars of the
/// SHA-256 of the host's X25519 public key, falling back to the host URL
/// when the daemon is unreachable (so the key does not churn before the
/// first successful contact).
pub fn host_fingerprint(host_public_key_b64: Option<&str>, host_url: &str) -> String {
    use sha2::{Digest, Sha256};
    let material = host_public_key_b64.unwrap_or(host_url);
    let digest = Sha256::digest(material.as_bytes());
    let mut hex = String::with_capacity(16);
    for b in digest.iter().take(8) {
        use std::fmt::Write;
        write!(hex, "{b:02x}").expect("hex write to String cannot fail");
    }
    hex
}

// ---------------------------------------------------------------------------
// Keychain + file storage
// ---------------------------------------------------------------------------

fn read_keyring(account: &str) -> Result<[u8; 32], String> {
    let entry =
        keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| format!("keyring entry: {e}"))?;
    let pw = entry.get_password().map_err(|e| e.to_string())?;
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, pw.trim())
        .map_err(|e| format!("corrupt keychain payload: {e}"))?;
    bytes
        .try_into()
        .map_err(|_| "corrupt keychain payload: not 32 bytes".to_string())
}

fn write_keyring(account: &str, seed: &[u8]) -> Result<(), String> {
    let entry =
        keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| format!("keyring entry: {e}"))?;
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, seed);
    entry.set_password(&encoded).map_err(|e| e.to_string())
}

fn ensure_dir_0700(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let mut perms = std::fs::metadata(dir)
        .map_err(|e| e.to_string())?
        .permissions();
    use std::os::unix::fs::PermissionsExt;
    if perms.mode() & 0o077 != 0 {
        perms.set_mode(0o700);
        std::fs::set_permissions(dir, perms)
            .map_err(|e| format!("chmod {}: {e}", dir.display()))?;
    }
    Ok(())
}

fn read_key_file(path: &Path) -> Result<[u8; 32], String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content.trim())
        .map_err(|e| format!("corrupt key file {}: {e}", path.display()))?;
    bytes
        .try_into()
        .map_err(|_| format!("corrupt key file {}: not 32 bytes", path.display()))
}

fn write_key_file(path: &Path, seed: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    if let Some(parent) = path.parent() {
        ensure_dir_0700(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("open {}: {e}", tmp.display()))?;
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, seed);
    f.write_all(encoded.as_bytes())
        .and_then(|_| f.write_all(b"\n"))
        .and_then(|_| f.flush())
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod {}: {e}", path.display()))?;
    Ok(())
}

/// Try to read the daemon's registration token (localhost auto-register).
/// The token is routing-only and lives in a 0600 file owned by the same
/// user, so reading it locally is the documented localhost path.
pub fn read_daemon_registration_token() -> Result<String, String> {
    let path = daemon_config_dir().join("registration-token");
    std::fs::read_to_string(&path)
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("read {}: {e}", path.display()))
}

/// Try to read the daemon's host admin token (audit + grant management on
/// localhost). Host-admin credential — same-machine, same-user file access
/// only; sent only to loopback host-admin endpoints, never to a device.
pub fn read_daemon_admin_token() -> Option<String> {
    let path = daemon_config_dir().join("admin-token");
    std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_are_stable_and_distinct() {
        let a = host_fingerprint(Some("AAAA"), "http://127.0.0.1:8474");
        let b = host_fingerprint(Some("AAAA"), "http://127.0.0.1:8474");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
        let c = host_fingerprint(Some("BBBB"), "http://127.0.0.1:8474");
        assert_ne!(a, c);
        let d = host_fingerprint(None, "http://127.0.0.1:8474");
        assert_ne!(a, d, "host key material and URL must not collide");
        assert_eq!(host_fingerprint(None, "http://127.0.0.1:8474"), d);
    }

    #[test]
    fn key_file_round_trips_0600() {
        let dir = std::env::temp_dir().join(format!("corrald-ui-keys-test-{}", std::process::id()));
        let path = dir.join("fp.key");
        let seed = [7u8; 32];
        write_key_file(&path, &seed).unwrap();
        assert_eq!(read_key_file(&path).unwrap(), seed);
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "key material must be 0600");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keyring_fallback_keeps_the_seed() {
        // Simulated keyring-unavailable path is exercised via rotate_key's
        // file branch; assert the file branch keeps a fresh seed readable.
        let fp = host_fingerprint(Some("key"), "http://unit-test.invalid");
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).unwrap();
        // Force file-store path by pointing at a config dir (keyring may
        // actually work on the host, so rotate_key alone is not
        // deterministic; only exercise the pure file helpers here).
        let path = client_config_dir().join("keys").join(format!("{fp}.key"));
        write_key_file(&path, &seed).unwrap();
        assert_eq!(read_key_file(&path).unwrap(), seed);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn client_config_dir_uses_home_convention() {
        let home = std::env::var("HOME").unwrap();
        // SAFETY: single-threaded test; env mutation is contained here.
        unsafe { std::env::remove_var("CORRAL_UI_CONFIG_DIR") };
        assert!(client_config_dir().starts_with(home));
        // SAFETY: single-threaded test; env mutation is contained here.
        unsafe { std::env::set_var("CORRAL_UI_CONFIG_DIR", "/tmp/corral-ui-test") };
        assert_eq!(client_config_dir(), PathBuf::from("/tmp/corral-ui-test"));
        // SAFETY: single-threaded test; env mutation is contained here.
        unsafe { std::env::remove_var("CORRAL_UI_CONFIG_DIR") };
    }
}
