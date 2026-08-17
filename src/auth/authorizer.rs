//! [`DeviceAuthorizer`] — the contract [`DriveAuthorizer`] implementation.
//!
//! Verify order (default deny at every stage, per the mission contract):
//! 1. key known?    -> `UnknownKey`
//! 2. revoked?      -> `Revoked`
//! 3. expired?      -> `Expired`
//! 4. signature OK over `canonical_envelope_bytes(envelope)` with the
//!    device's Ed25519 public key? -> `BadSignature` (strict RFC 8032
//!    verification: non-canonical points and malleable signatures rejected)
//! 5. capability granted? -> `NotGranted(cap)`
//!
//! Signature: base64 (RFC 4648 §4) of the 64-byte Ed25519 signature.
//! Revocation and expiry are checked per request — every drive is a fresh
//! authorization, so revocation takes effect immediately and there is no
//! session to invalidate.

use std::fmt;
use std::sync::Arc;

use crate::drive::{
    AuthError, AuthorizedDrive, DriveAuthorizer, SignedDrive, canonical_envelope_bytes,
};

use super::DeviceRegistry;
use super::registry::now_secs;

pub struct DeviceAuthorizer {
    registry: Arc<DeviceRegistry>,
}

impl DeviceAuthorizer {
    pub fn new(registry: Arc<DeviceRegistry>) -> Self {
        Self { registry }
    }
}

impl DriveAuthorizer for DeviceAuthorizer {
    fn verify(&self, signed: &SignedDrive) -> Result<AuthorizedDrive, AuthError> {
        let rec = self
            .registry
            .get(&signed.key_id)
            .ok_or(AuthError::UnknownKey)?;
        if rec.revoked {
            return Err(AuthError::Revoked);
        }
        if now_secs() >= rec.expiry_ts {
            return Err(AuthError::Expired);
        }
        let sig = super::decode_b64(&signed.signature).ok_or(AuthError::BadSignature)?;
        let sig: [u8; 64] = sig.try_into().map_err(|_| AuthError::BadSignature)?;
        let public_key = ed25519_dalek::VerifyingKey::from_bytes(&rec.public_key)
            .expect("registry holds validated keys");
        let signature = ed25519_dalek::Signature::from_bytes(&sig);
        let message = canonical_envelope_bytes(&signed.envelope);
        public_key
            .verify_strict(&message, &signature)
            .map_err(|_| AuthError::BadSignature)?;
        if !rec.grants.contains(&signed.envelope.capability) {
            return Err(AuthError::NotGranted(signed.envelope.capability));
        }
        Ok(AuthorizedDrive {
            key_id: signed.key_id.clone(),
            envelope: signed.envelope.clone(),
        })
    }
}

impl fmt::Debug for DeviceAuthorizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceAuthorizer")
            .field("registry", &self.registry)
            .finish()
    }
}
