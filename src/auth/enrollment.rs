//! #485 — host-approved enrollment sessions: short-lived, single-use
//! codes, in-memory only (no new secret at rest).
//!
//! Frozen contract: `.proposal-485-owner-resolved.md` (sha256 bfa5e392…,
//! independent verdict review485-owner-r1). Shape of the flow:
//!
//! 1. the **local owner channel** (unix socket, never HTTP) mints a
//!    session: an `enr_`-style id plus a 256-bit single-use `code`;
//! 2. the **device** redeems the code with its Ed25519 public key — the
//!    session binds that key as a *pending request* (no authority);
//! 3. only an explicit owner **approve** turns the pending request into a
//!    registry record (or restores a revoked one), disk-first, carrying
//!    `read_tail` exactly, never control grants.
//!
//! Precedence (frozen §5.3): unknown code → `UnknownCode`; past
//! `expires_ts` → `Expired`; then by session state. `/enroll/status` never
//! answers 409 for a consumed code (a lost redeem response must not brick
//! polling), and it never exposes anything a caller without the code could
//! read.
//!
//! Every transition re-validates expiry and state INSIDE the session
//! mutex (frozen C4) and the lock order is session → registry only: the
//! store may call [`DeviceRegistry`] while holding its own lock; the
//! registry never calls back into the store.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use crate::drive::Capability;

use super::constant_time_eq;
use super::registry::{DeviceRecord, DeviceRegistry, key_id_for, normalize_display_name};

/// Single deadline for mint → redeem → approve (frozen §5.5).
pub const ENROLL_TTL: Duration = Duration::from_secs(300);
/// Hard cap on concurrent ACTIONABLE pairing sessions (minted-but-unapproved
/// — approved sessions are terminal). Fail-safe against runaway owner loops
/// (frozen §5.5); in-memory growth stays bounded regardless.
pub const MAX_PENDING_SESSIONS: usize = 8;
/// `code` = base64 of this many random bytes (256-bit, frozen §5.5).
const CODE_BYTES: usize = 32;
/// `enrollment_id` = `enr_` + this many lowercase hex chars (16 bytes).
const ENROLLMENT_ID_HEX_CHARS: usize = 32;

/// One pairing session. Created by the owner channel, consumed by one
/// device redeem, resolved by one owner approve.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EnrollmentSession {
    enrollment_id: String,
    /// The single-use redemption secret (raw bytes; never logged, never
    /// serialized outside the owner channel's mint response).
    code: [u8; CODE_BYTES],
    created_ts: u64,
    expires_ts: u64,
    state: SessionState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionState {
    /// Minted, not yet redeemed.
    Pending,
    /// Redeemed: bound to exactly the key presented at redeem.
    Redeemed {
        key_id: String,
        public_key: [u8; 32],
        name: Option<String>,
    },
    /// Approved: terminal (the record itself carries the live state).
    Approved { key_id: String },
}

/// Successful mint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedSession {
    pub enrollment_id: String,
    pub code_b64: String,
    pub expires_ts: u64,
}

/// Successful redeem / pending status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSnapshot {
    pub key_id: String,
    pub expires_ts: u64,
}

/// Status of an approved session (current registry record read live).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusSnapshot {
    Pending {
        expires_ts: u64,
    },
    Approved {
        key_id: String,
        grants: Vec<Capability>,
        expiry_ts: u64,
        revoked: bool,
    },
}

/// One entry of the owner's pending listing (frozen §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub enrollment_id: String,
    /// `None` until a device redeems the code.
    pub key_id: Option<String>,
    pub name: Option<String>,
    /// `"pending"` (minted) or `"redeemed"` (awaiting approval).
    pub state: &'static str,
    pub expires_ts: u64,
}

/// The approve-conflict payload (frozen C3: the 409 carries the approved
/// record so a lost approve response self-heals).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedSnapshot {
    pub key_id: String,
    pub grants: Vec<Capability>,
    pub expiry_ts: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MintError {
    /// The actionable-session cap is reached (frozen §5.5 fail-safe).
    PendingCap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedeemError {
    /// Code never minted here (including a code from another host).
    UnknownCode,
    /// Past the single mint→redeem→approve deadline.
    Expired,
    /// Code already consumed (replay / second device) — the replay witness.
    Consumed,
    /// The presented key is a LIVE registry record; re-pairing an existing
    /// live device is not this flow (409 `already_registered`).
    AlreadyRegistered,
    /// Not 32 bytes of base64, or not a canonical/non-weak Ed25519 point.
    BadPublicKey,
    /// Label fails the #209 display-name rules.
    BadName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusError {
    UnknownCode,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproveError {
    /// No such session id on this daemon.
    UnknownSession,
    Expired,
    /// Approve before any redeem.
    NotRedeemed,
    /// Already resolved — either a retry of this session's approve, or the
    /// same key became live through an earlier session (double mint).
    /// Carries the live record (frozen C3). The session is terminal.
    AlreadyApproved(ApprovedSnapshot),
    /// Disk-first commit failed: NOTHING changed, the session stays
    /// retryable (frozen §5.4).
    Persist(String),
}

impl fmt::Display for StatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownCode => "unknown enrollment code",
            Self::Expired => "enrollment code expired",
        })
    }
}

impl fmt::Display for RedeemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownCode => "unknown enrollment code",
            Self::Expired => "enrollment code expired",
            Self::Consumed => "enrollment code already redeemed",
            Self::AlreadyRegistered => "device key already registered",
            Self::BadPublicKey => "malformed device public key",
            Self::BadName => "malformed device display name",
        })
    }
}

/// In-memory enrollment-session store. `ttl` is the single deadline shared
/// by mint, redeem and approve (production: [`ENROLL_TTL`]; tests inject a
/// shorter one).
pub struct EnrollmentStore {
    ttl: Duration,
    inner: Mutex<HashMap<String, EnrollmentSession>>,
}

impl Default for EnrollmentStore {
    fn default() -> Self {
        Self::new(ENROLL_TTL)
    }
}

impl EnrollmentStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Mint a session: a fresh `enr_<32 hex>` id and a 256-bit code. The
    /// cap bounds concurrent actionable sessions (frozen §5.5). `now` is
    /// injected so tests can drive expiry without sleeping.
    pub fn mint(&self, now: u64) -> Result<MintedSession, MintError> {
        let mut inner = self.inner.lock().expect("enrollment lock poisoned");
        inner.retain(|_, s| now < s.expires_ts);
        let actionable = inner
            .values()
            .filter(|s| !matches!(s.state, SessionState::Approved { .. }))
            .count();
        if actionable >= MAX_PENDING_SESSIONS {
            return Err(MintError::PendingCap);
        }
        let enrollment_id = format!(
            "enr_{}",
            super::hex(&super::random_bytes::<{ ENROLLMENT_ID_HEX_CHARS / 2 }>())
        );
        let code = super::random_bytes::<CODE_BYTES>();
        let session = EnrollmentSession {
            enrollment_id: enrollment_id.clone(),
            code,
            created_ts: now,
            expires_ts: now.saturating_add(self.ttl.as_secs()),
            state: SessionState::Pending,
        };
        let code_b64 = super::b64_encode(&code);
        inner.insert(enrollment_id.clone(), session);
        Ok(MintedSession {
            enrollment_id,
            code_b64,
            expires_ts: now.saturating_add(self.ttl.as_secs()),
        })
    }

    /// Redeem a code. Validation happens BEFORE consumption, and the
    /// consume itself is inside the session mutex, so exactly one of
    /// several racing redeems can win (frozen C4/C7c). A revoked existing
    /// key re-enters as a pending request bound to that known `key_id`
    /// (the O2 restore path — nothing is restored here; approval restores).
    pub fn redeem(
        &self,
        code_b64: &str,
        public_key: [u8; 32],
        name: Option<&str>,
        now: u64,
        registry: &DeviceRegistry,
    ) -> Result<PendingSnapshot, RedeemError> {
        let code = super::decode_b64(code_b64)
            .and_then(|v| <[u8; CODE_BYTES]>::try_from(v).ok())
            .ok_or(RedeemError::UnknownCode)?;
        // Canonical/non-weak Ed25519 only — same rules as `register_named`,
        // so the registry never holds a key verify() cannot use.
        let pk = ed25519_dalek::VerifyingKey::from_bytes(&public_key)
            .map_err(|_| RedeemError::BadPublicKey)?;
        if pk.is_weak() {
            return Err(RedeemError::BadPublicKey);
        }
        let name = match name {
            Some(raw) => Some(normalize_display_name(raw).map_err(|_| RedeemError::BadName)?),
            None => None,
        };
        let mut inner = self.inner.lock().expect("enrollment lock poisoned");
        let session = inner
            .values_mut()
            .find(|s| constant_time_eq(&s.code, &code))
            .ok_or(RedeemError::UnknownCode)?;
        if now >= session.expires_ts {
            return Err(RedeemError::Expired);
        }
        if !matches!(session.state, SessionState::Pending) {
            return Err(RedeemError::Consumed);
        }
        // Session → registry lock order (never the reverse).
        let key_id = match registry.find_by_public_key(&public_key) {
            Some(rec) if !rec.revoked => return Err(RedeemError::AlreadyRegistered),
            Some(rec) => rec.key_id,
            None => key_id_for(&public_key),
        };
        session.state = SessionState::Redeemed {
            key_id: key_id.clone(),
            public_key,
            name,
        };
        Ok(PendingSnapshot {
            key_id,
            expires_ts: session.expires_ts,
        })
    }

    /// Read-only status for a code the caller already holds. NEVER 409:
    /// a consumed-but-unapproved session reports `Pending`, so a lost
    /// redeem response cannot brick device polling (frozen C2).
    pub fn status(
        &self,
        code_b64: &str,
        now: u64,
        registry: &DeviceRegistry,
    ) -> Result<StatusSnapshot, StatusError> {
        let code = super::decode_b64(code_b64)
            .and_then(|v| <[u8; CODE_BYTES]>::try_from(v).ok())
            .ok_or(StatusError::UnknownCode)?;
        let inner = self.inner.lock().expect("enrollment lock poisoned");
        let session = inner
            .values()
            .find(|s| constant_time_eq(&s.code, &code))
            .ok_or(StatusError::UnknownCode)?;
        if now >= session.expires_ts {
            return Err(StatusError::Expired);
        }
        match &session.state {
            SessionState::Pending | SessionState::Redeemed { .. } => Ok(StatusSnapshot::Pending {
                expires_ts: session.expires_ts,
            }),
            SessionState::Approved { key_id } => {
                // Read live: revokes/expiry reach the device without any
                // session bookkeeping. A vanished record is fail-closed.
                let rec = registry.get(key_id).ok_or(StatusError::UnknownCode)?;
                Ok(StatusSnapshot::Approved {
                    key_id: rec.key_id,
                    grants: rec.grants,
                    expiry_ts: rec.expiry_ts,
                    revoked: rec.revoked,
                })
            }
        }
    }

    /// Owner approve: turns a redeemed session into a committed registry
    /// record (create) or restores the bound revoked record — `read_tail`
    /// exactly, disk first. The session becomes terminal only after the
    /// persist succeeded; on failure it stays retryable (frozen §5.4).
    pub fn approve(
        &self,
        enrollment_id: &str,
        now: u64,
        registry: &DeviceRegistry,
        registration_ttl: Duration,
    ) -> Result<DeviceRecord, ApproveError> {
        let mut inner = self.inner.lock().expect("enrollment lock poisoned");
        let session = inner
            .get_mut(enrollment_id)
            .ok_or(ApproveError::UnknownSession)?;
        if now >= session.expires_ts {
            return Err(ApproveError::Expired);
        }
        match session.state.clone() {
            SessionState::Pending => Err(ApproveError::NotRedeemed),
            SessionState::Approved { key_id } => {
                let rec = registry.get(&key_id).ok_or(ApproveError::UnknownSession)?;
                Err(ApproveError::AlreadyApproved(ApprovedSnapshot {
                    key_id: rec.key_id,
                    grants: rec.grants,
                    expiry_ts: rec.expiry_ts,
                }))
            }
            SessionState::Redeemed {
                key_id: _,
                public_key,
                name,
            } => match registry.enroll_approve(public_key, name.as_deref(), registration_ttl) {
                Ok(super::registry::EnrollApprove::Created(rec)) => {
                    session.state = SessionState::Approved {
                        key_id: rec.key_id.clone(),
                    };
                    Ok(rec)
                }
                Ok(super::registry::EnrollApprove::Restored(rec)) => {
                    session.state = SessionState::Approved {
                        key_id: rec.key_id.clone(),
                    };
                    Ok(rec)
                }
                // The same key became live through an earlier session
                // (double mint of one revoked key): fail-safe typed
                // conflict + the live record; the session is terminal, so
                // `/enroll/status` converges the device either way.
                Ok(super::registry::EnrollApprove::AlreadyLive(rec)) => {
                    session.state = SessionState::Approved {
                        key_id: rec.key_id.clone(),
                    };
                    Err(ApproveError::AlreadyApproved(ApprovedSnapshot {
                        key_id: rec.key_id,
                        grants: rec.grants,
                        expiry_ts: rec.expiry_ts,
                    }))
                }
                Err(e) => Err(ApproveError::Persist(e)),
            },
        }
    }

    /// Owner listing: all live sessions awaiting action (`pending` =
    /// minted, `redeemed` = awaiting approval). Approved sessions are
    /// terminal and intentionally not listed; expired ones are pruned.
    /// Ordered oldest first so the owner sees the queue.
    pub fn pending(&self, now: u64) -> Vec<SessionView> {
        let mut inner = self.inner.lock().expect("enrollment lock poisoned");
        inner.retain(|_, s| now < s.expires_ts);
        let mut views: Vec<(u64, SessionView)> = inner
            .values()
            .filter_map(|s| match &s.state {
                SessionState::Pending => Some((
                    s.created_ts,
                    SessionView {
                        enrollment_id: s.enrollment_id.clone(),
                        key_id: None,
                        name: None,
                        state: "pending",
                        expires_ts: s.expires_ts,
                    },
                )),
                SessionState::Redeemed { key_id, name, .. } => Some((
                    s.created_ts,
                    SessionView {
                        enrollment_id: s.enrollment_id.clone(),
                        key_id: Some(key_id.clone()),
                        name: name.clone(),
                        state: "redeemed",
                        expires_ts: s.expires_ts,
                    },
                )),
                SessionState::Approved { .. } => None,
            })
            .collect();
        views.sort_by_key(|(created, _)| *created);
        views.into_iter().map(|(_, v)| v).collect()
    }

    /// Test/introspection seam: live session count (all states).
    pub fn live_count(&self, now: u64) -> usize {
        self.inner
            .lock()
            .expect("enrollment lock poisoned")
            .values()
            .filter(|s| now < s.expires_ts)
            .count()
    }
}

impl fmt::Debug for EnrollmentStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No secrets: counts only (codes never print).
        f.debug_struct("EnrollmentStore")
            .field("ttl_secs", &self.ttl.as_secs())
            .field(
                "sessions",
                &self.inner.lock().map(|g| g.len()).unwrap_or_default(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::registry::now_secs;
    use crate::auth::test_support;
    use crate::drive::Capability;

    fn store(ttl_secs: u64) -> EnrollmentStore {
        EnrollmentStore::new(Duration::from_secs(ttl_secs))
    }

    #[test]
    fn precedence_unknown_then_expired_then_state() {
        let (registry, _authz, token, _dir) = test_support::setup();
        let s = store(300);
        let now = now_secs();
        let minted = s.mint(now).expect("mint");

        // Unknown code wins over everything.
        assert_eq!(
            s.status("bm90LWEtY29kZQ==", now, &registry),
            Err(StatusError::UnknownCode)
        );
        assert_eq!(
            s.redeem(
                "bm90LWEtY29kZQ==",
                test_support::keypair().1,
                None,
                now,
                &registry
            ),
            Err(RedeemError::UnknownCode)
        );

        // Expiry wins over consumed/unapproved state (consumed-but-expired
        // reports 410, not pending — frozen §5.3).
        let later = now + 301;
        assert_eq!(
            s.redeem(
                &minted.code_b64,
                test_support::keypair().1,
                None,
                later,
                &registry
            ),
            Err(RedeemError::Expired)
        );
        // Consume at the live edge, then let it expire.
        let (_, pk) = test_support::keypair();
        s.redeem(&minted.code_b64, pk, None, now, &registry)
            .unwrap();
        assert_eq!(
            s.status(&minted.code_b64, later, &registry),
            Err(StatusError::Expired)
        );
        let _ = token;

        // And the new-key path did NOT create a registry record.
        assert_eq!(registry.device_count(), 0, "redeem grants nothing");
    }

    #[test]
    fn status_never_conflicts_for_consumed_codes() {
        let (registry, _authz, _token, _dir) = test_support::setup();
        let s = store(300);
        let now = now_secs();
        let minted = s.mint(now).unwrap();
        let (_, pk) = test_support::keypair();
        let redeemed = s
            .redeem(&minted.code_b64, pk, None, now, &registry)
            .unwrap();
        // Consumed + unapproved -> 200 pending shape (frozen C2).
        assert_eq!(
            s.status(&minted.code_b64, now, &registry),
            Ok(StatusSnapshot::Pending {
                expires_ts: redeemed.expires_ts
            })
        );
    }

    #[test]
    fn cap_bounds_actionable_sessions_and_prunes_expired() {
        let s = store(10);
        let now = now_secs();
        for _ in 0..MAX_PENDING_SESSIONS {
            s.mint(now).unwrap();
        }
        assert_eq!(s.mint(now), Err(MintError::PendingCap));
        // Expired sessions pruned -> mint succeeds again.
        assert!(s.mint(now + 11).is_ok());
        // Approved sessions free the cap immediately: approving all eight
        // leaves the store actionable-empty, so the next mint succeeds.
        let s2 = store(300);
        let (registry, _authz, _token, _dir) = test_support::setup();
        for _ in 0..MAX_PENDING_SESSIONS {
            let (_, pk) = test_support::keypair();
            let m = s2.mint(now).unwrap();
            s2.redeem(&m.code_b64, pk, None, now, &registry).unwrap();
            s2.approve(
                &m.enrollment_id,
                now,
                &registry,
                Duration::from_secs(90 * 24 * 3600),
            )
            .unwrap();
        }
        assert!(
            s2.mint(now).is_ok(),
            "approval frees the actionable pending cap"
        );
    }

    #[test]
    fn revoked_key_is_not_restored_until_explicit_approve() {
        let (registry, _authz, token, _dir) = test_support::setup();
        let s = store(300);
        let now = now_secs();
        let (_, pk) = test_support::keypair();
        let rec = registry
            .register(&token, pk, Duration::from_secs(3600))
            .unwrap();
        registry
            .set_grants(&rec.key_id, vec![Capability::ReadTail])
            .unwrap();
        registry.revoke_committed(&rec.key_id).unwrap();
        assert!(registry.get(&rec.key_id).unwrap().revoked);

        let minted = s.mint(now).unwrap();
        let pending = s
            .redeem(&minted.code_b64, pk, Some("iPhone"), now, &registry)
            .unwrap();
        assert_eq!(pending.key_id, rec.key_id, "binding is exact");
        // Never automatic: still revoked, still no drive.
        let still = registry.get(&rec.key_id).unwrap();
        assert!(still.revoked, "redeem must not restore");
        assert_eq!(still.grants, vec![Capability::ReadTail]);

        // Approve restores; read_tail exactly; session terminal.
        let restored = s
            .approve(
                &minted.enrollment_id,
                now,
                &registry,
                Duration::from_secs(90 * 24 * 3600),
            )
            .unwrap();
        assert_eq!(restored.key_id, rec.key_id);
        assert!(!restored.revoked);
        assert_eq!(restored.grants, vec![Capability::ReadTail]);
        assert_eq!(restored.revoked_ts, None);
        assert!(restored.expiry_ts >= now + 90 * 24 * 3600 - 5);
    }

    #[test]
    fn double_mint_same_revoked_key_second_approve_is_typed_conflict() {
        let (registry, _authz, token, _dir) = test_support::setup();
        let s = store(300);
        let now = now_secs();
        let (_, pk) = test_support::keypair();
        let rec = registry
            .register(&token, pk, Duration::from_secs(3600))
            .unwrap();
        registry.revoke_committed(&rec.key_id).unwrap();

        let a = s.mint(now).unwrap();
        let b = s.mint(now).unwrap();
        s.redeem(&a.code_b64, pk, None, now, &registry).unwrap();
        s.redeem(&b.code_b64, pk, None, now, &registry).unwrap();
        s.approve(&a.enrollment_id, now, &registry, Duration::from_secs(3600))
            .unwrap();
        let err = s
            .approve(&b.enrollment_id, now, &registry, Duration::from_secs(3600))
            .unwrap_err();
        match err {
            ApproveError::AlreadyApproved(snap) => {
                assert_eq!(snap.key_id, rec.key_id);
                assert_eq!(snap.grants, vec![Capability::ReadTail]);
            }
            other => panic!("expected typed conflict, got {other:?}"),
        }
        // Second approve did not invent a new record.
        assert_eq!(registry.device_count(), 1);
    }

    #[test]
    fn approve_before_redeem_conflicts_and_unknown_session_404s() {
        let (registry, _authz, _token, _dir) = test_support::setup();
        let s = store(300);
        let now = now_secs();
        let minted = s.mint(now).unwrap();
        assert_eq!(
            s.approve(
                &minted.enrollment_id,
                now,
                &registry,
                Duration::from_secs(3600)
            ),
            Err(ApproveError::NotRedeemed)
        );
        assert_eq!(
            s.approve(
                "enr_00000000000000000000000000000000",
                now,
                &registry,
                Duration::from_secs(3600)
            ),
            Err(ApproveError::UnknownSession)
        );
    }
}
