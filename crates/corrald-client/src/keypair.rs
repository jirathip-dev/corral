//! Device keypair (D10/D13): Ed25519 signing keys held by the client.
//!
//! The public key is registered once via `POST /register`; every drive
//! write is signed with the device key over the canonical envelope bytes.
//! The key id derivation mirrors the daemon's
//! `src/auth/registry.rs::key_id_for` (`dev_<hex(sha256(pubkey)[..16])>`) so
//! the client can derive its own identity before the daemon echoes it.

use base64::Engine;
use ed25519_dalek::Signer;
use sha2::{Digest, Sha256};

use crate::drive::{DriveEnvelope, canonical_envelope_bytes};

/// Prefix + truncated SHA-256 of the public key — the daemon's key-id
/// scheme (`dev_<hex(sha256(pubkey)[..16])>`).
pub fn key_id_for(public_key: &[u8; 32]) -> String {
    let digest = Sha256::digest(public_key);
    let mut hex = String::with_capacity(2 + 32);
    hex.push_str("dev_");
    for byte in &digest[..16] {
        use std::fmt::Write;
        write!(&mut hex, "{byte:02x}").expect("hex write to String cannot fail");
    }
    hex
}

/// The client's device identity: signing key + derived key id.
#[derive(Clone)]
pub struct DeviceKeypair {
    signing: ed25519_dalek::SigningKey,
    public_key: [u8; 32],
    key_id: String,
}

impl DeviceKeypair {
    /// Generate a fresh device keypair (OS RNG).
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        getrandom_seed(&mut seed);
        Self::from_seed(seed)
    }

    /// Reconstruct from a 32-byte seed (key storage/restore).
    pub fn from_seed(seed: [u8; 32]) -> Self {
        let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
        let public_key = signing.verifying_key().to_bytes();
        let key_id = key_id_for(&public_key);
        Self {
            signing,
            public_key,
            key_id,
        }
    }

    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }

    /// RFC 4648 §4 base64 with padding — the wire form for `POST /register`.
    pub fn public_key_b64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.public_key)
    }

    /// `dev_<hex(sha256(pubkey)[..16])>` — must match the daemon's registry.
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// Sign raw bytes (the canonical step-up request bytes), base64.
    pub fn sign_bytes(&self, bytes: &[u8]) -> String {
        let signature = self.signing.sign(bytes);
        base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
    }

    /// Sign a drive envelope over its canonical bytes, base64. Ed25519 is
    /// deterministic: re-signing the same envelope yields the same
    /// signature, which is what makes idempotent request_id retries safe.
    pub fn sign_envelope(&self, envelope: &DriveEnvelope) -> String {
        self.sign_bytes(&canonical_envelope_bytes(envelope))
    }
}

fn getrandom_seed(out: &mut [u8; 32]) {
    // 256-bit random for key material; fail hard (no safe fallback).
    getrandom::fill(out).expect("OS RNG failure: cannot generate device key");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_id_derivation_matches_daemon_shape() {
        let kp = DeviceKeypair::generate();
        assert!(kp.key_id().starts_with("dev_"));
        assert_eq!(kp.key_id().len(), "dev_".len() + 32);
        // Derivation is a pure function of the public key.
        let kp2 = DeviceKeypair::from_seed(kp.signing.to_bytes());
        assert_eq!(kp.key_id(), kp2.key_id());
        assert_eq!(kp.public_key(), kp2.public_key());
    }

    #[test]
    fn signature_round_trips_and_is_deterministic() {
        let kp = DeviceKeypair::generate();
        let bytes = b"the canonical envelope bytes";
        let sig = kp.sign_bytes(bytes);
        let sig2 = kp.sign_bytes(bytes);
        assert_eq!(sig, sig2, "Ed25519 signing must be deterministic");
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&sig)
            .unwrap();
        assert_eq!(raw.len(), 64);
        let signature =
            ed25519_dalek::Signature::from_bytes(raw.as_slice().try_into().expect("64 bytes"));
        ed25519_dalek::VerifyingKey::from_bytes(kp.public_key())
            .unwrap()
            .verify_strict(bytes, &signature)
            .expect("self-signature verifies");
    }

    #[test]
    fn sign_envelope_covers_canonical_bytes() {
        let kp = DeviceKeypair::generate();
        let envelope = crate::drive::DriveEnvelope {
            request_id: "r-1".to_string(),
            capability: crate::drive::Capability::ReadTail,
            target: "herdr:pane:wQ:p1".to_string(),
            payload: crate::drive::DrivePayload::ReadTail { lines: Some(50) }.to_json(),
            rev: None,
        };
        let sig = kp.sign_envelope(&envelope);
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&sig)
            .unwrap();
        let signature =
            ed25519_dalek::Signature::from_bytes(raw.as_slice().try_into().expect("64 bytes"));
        ed25519_dalek::VerifyingKey::from_bytes(kp.public_key())
            .unwrap()
            .verify_strict(&canonical_envelope_bytes(&envelope), &signature)
            .expect("signature covers the canonical envelope bytes");
    }
}
