//! APNs provider seam (D16): the trait the notifier pushes through, the
//! real HTTP/2 provider, and the test double.
//!
//! ## Certificate provisioning (Guy's)
//!
//! APNs authentication uses an **Apple developer account push key** (`.p8`,
//! ES256, "APNs Auth Key" created in Certificates, Identifiers & Profiles →
//! Keys → Apple Push Notifications service (APNs)). Nothing in this module
//! can generate it: the `.p8` file is provisioned from Guy's developer
//! account and placed on the daemon host. Required inputs (env):
//!
//! - `CORRAL_APNS_TEAM_ID` — 10-char team id (Apple developer account →
//!   Membership). The JWT `iss` claim.
//! - `CORRAL_APNS_KEY_ID` — the push key's 10-char id (shown on the Keys
//!   page next to the created key). The JWT header `kid`.
//! - `CORRAL_APNS_AUTH_KEY_PATH` — path to the `.p8` file (PKCS#8 PEM:
//!   `-----BEGIN PRIVATE KEY-----`).
//! - `CORRAL_APNS_ENDPOINT` — `production` (default) or `sandbox`.
//!   Sandbox tokens are issued by the development provisioning profile;
//!   TestFlight/App Store builds get production tokens. A mismatch returns
//!   400 `BadDeviceToken` — get this wrong and the push is silently
//!   rejected by Apple.
//! - `CORRAL_APNS_TOPIC` — app bundle id; default `com.corral.fleetnotifier`.
//!
//! The device side needs the app's `aps-environment` entitlement, which
//! comes from the provisioning profile (Guy's signing config), not from
//! code. Simulator builds have no APNs at all (see the DEBUG-only local
//! notification bridge in the iOS app).
//!
//! ## Provider token (ES256 JWT)
//!
//! Every request carries `Authorization: Bearer <provider token>`: a JWT
//! with header `{alg: ES256, kid: <key id>}` and claims `{iss: <team id>,
//! iat: <now>}` signed with the `.p8` key. Tokens are valid for an hour
//! (Apple suggests minting fresh well before expiry); [`RealApnsProvider`]
//! caches one and refreshes it after [`TOKEN_REFRESH`] to bound the signing
//! cost while never presenting a token older than Apple's limit.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::{SigningKey, signature::Signer as _};
use p256::pkcs8::DecodePrivateKey as _;
use serde_json::Value;
use tracing::{debug, warn};

use super::config::{Config, Endpoint};

/// Provider token lifetime gate: Apple accepts tokens up to an hour old;
/// refresh after this so the presented token is always comfortably valid.
const TOKEN_REFRESH: Duration = Duration::from_secs(30 * 60);
/// Retryable HTTP statuses (Apple throttles and has outages too).
const RETRYABLE_STATUS: [u16; 4] = [429, 500, 502, 503];

/// Typed APNs failures. The notifier's retry policy keys off this:
/// transport/5xx/429 are retried with backoff; everything else is
/// permanent (log + drop) — except [`PushError::Unregistered`], which
/// removes the device token from the registry (revocation, D16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushError {
    /// The device token is not valid for this topic/environment (HTTP 400
    /// `BadDeviceToken`, or Apple says the app is gone — HTTP 410
    /// `Unregistered`). The registry entry is dropped.
    Unregistered,
    /// Provider configuration/credential problem (HTTP 401/403, malformed
    /// key, bad endpoint). Fixing it needs Guy, not a retry.
    Configuration(String),
    /// HTTP 4xx that is neither of the above (e.g. payload too large).
    Rejected { status: u16, reason: String },
    /// Network failure or server-side throttle/outage (HTTP 429/5xx).
    Retryable { status: Option<u16>, reason: String },
}

impl std::fmt::Display for PushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unregistered => write!(f, "device token unregistered"),
            Self::Configuration(e) => write!(f, "apns configuration error: {e}"),
            Self::Rejected { status, reason } => {
                write!(f, "apns rejected (HTTP {status}): {reason}")
            }
            Self::Retryable { status, reason } => match status {
                Some(s) => write!(f, "apns retryable (HTTP {s}): {reason}"),
                None => write!(f, "apns transport error: {reason}"),
            },
        }
    }
}

impl std::error::Error for PushError {}

impl PushError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable { .. })
    }
}

/// The provider seam. Tests inject [`MockProvider`]; the daemon uses
/// [`RealApnsProvider`]. `push` sends one payload to one device and returns
/// a typed result — network behavior lives here, policy in the notifier.
///
/// A boxed future keeps the trait dyn-compatible (callers hold
/// `Arc<dyn ApnsProvider>`); same shape as `Adapter::read_tail`.
pub trait ApnsProvider: Send + Sync {
    fn push<'a>(
        &'a self,
        device_token: &'a str,
        payload: &'a Value,
    ) -> futures::future::BoxFuture<'a, Result<(), PushError>>;
}

// ---------------------------------------------------------------------------
// Real provider (HTTP/2 to Apple)
// ---------------------------------------------------------------------------

/// APNs HTTP/2 provider. One reqwest client (HTTP/2, kept warm) plus a
/// cached ES256 provider token refreshed on the configured cadence.
pub struct RealApnsProvider {
    client: reqwest::Client,
    config: Config,
    token: Mutex<CachedToken>,
}

struct CachedToken {
    jwt: String,
    minted_at: Duration,
}

impl std::fmt::Debug for RealApnsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // No secrets: config is team/key ids + endpoint, never the p8.
        f.debug_struct("RealApnsProvider")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl RealApnsProvider {
    /// Build the provider from a loaded [`Config`] and the parsed p8
    /// signing key. The key material is held for JWT signing only and is
    /// never serialized or logged.
    pub fn new(config: Config, signing_key: SigningKey) -> Result<Self, PushError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| PushError::Configuration(format!("reqwest client: {e}")))?;
        let provider = Self {
            client,
            config,
            token: Mutex::new(CachedToken {
                jwt: String::new(),
                minted_at: Duration::ZERO,
            }),
        };
        // Fail fast on a bad key: mint the first token now so a typo'd
        // CORRAL_APNS_* surfaces at startup, not on the first real block.
        provider
            .refresh_token(&signing_key)
            .map_err(|e| PushError::Configuration(format!("provider token: {e}")))?;
        Ok(provider)
    }

    /// Parse the `.p8` file (PKCS#8 PEM) into an ES256 signing key.
    /// This is the only place the p8 bytes are touched; they are dropped
    /// immediately after parsing.
    pub fn parse_p8(pem: &str) -> Result<SigningKey, PushError> {
        SigningKey::from_pkcs8_pem(pem)
            .map_err(|e| PushError::Configuration(format!("bad p8 key: {e}")))
    }

    /// Mint (or refresh) the cached provider token.
    fn refresh_token(&self, key: &SigningKey) -> Result<String, PushError> {
        let now = now_elapsed();
        let header = json_b64(&serde_json::json!({ "alg": "ES256", "kid": self.config.key_id }));
        let claims = json_b64(&serde_json::json!({
            "iss": self.config.team_id,
            "iat": now.as_secs(),
        }));
        let signing_input = format!("{header}.{claims}");
        let signature: p256::ecdsa::Signature = key.sign(signing_input.as_bytes());
        let jwt = format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        );
        *self.token.lock().expect("token lock poisoned") = CachedToken {
            jwt: jwt.clone(),
            minted_at: now,
        };
        Ok(jwt)
    }

    /// The current token, refreshing when stale.
    fn token(&self) -> Result<String, PushError> {
        let cached = self.token.lock().expect("token lock poisoned");
        if !cached.jwt.is_empty() && now_elapsed().saturating_sub(cached.minted_at) < TOKEN_REFRESH
        {
            return Ok(cached.jwt.clone());
        }
        drop(cached);
        // Token is stale (or a fresh provider): the p8 key is long gone,
        // so this can only happen if TOKEN_REFRESH elapsed — re-read the
        // key from disk and re-mint.
        let key = Config::load_signing_key(&self.config.auth_key_path)?;
        {
            let token = self.token.lock().expect("token lock poisoned");
            if !token.jwt.is_empty()
                && now_elapsed().saturating_sub(token.minted_at) < TOKEN_REFRESH
            {
                return Ok(token.jwt.clone());
            }
        } // guard dropped before refresh_token re-locks
        self.refresh_token(&key)
    }
}

impl ApnsProvider for RealApnsProvider {
    fn push<'a>(
        &'a self,
        device_token: &'a str,
        payload: &'a Value,
    ) -> futures::future::BoxFuture<'a, Result<(), PushError>> {
        Box::pin(async move {
            let token = self.token()?;
            let endpoint = match self.config.endpoint {
                Endpoint::Production => "https://api.push.apple.com/3/device/",
                Endpoint::Sandbox => "https://api.sandbox.push.apple.com/3/device/",
            };
            let url = format!("{endpoint}{device_token}");
            let response = self
                .client
                .post(&url)
                .bearer_auth(&token)
                .header("apns-topic", &self.config.topic)
                .header("apns-push-type", "alert")
                .header("apns-priority", "10")
                .json(payload)
                .send()
                .await
                .map_err(|e| PushError::Retryable {
                    status: None,
                    reason: transport_reason(&e),
                })?;
            let status = response.status();
            if status.is_success() {
                debug!(device_token = %token_hash(device_token), "apns push delivered");
                return Ok(());
            }
            // Apple replies with a JSON reason body on errors.
            let reason = response
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v.get("reason").and_then(|r| r.as_str()).map(String::from))
                .unwrap_or_else(|| format!("HTTP {status}"));
            // N5: a suspended laptop resumes with a stale clock; Apple
            // rejects the cached JWT with 403 ExpiredProviderToken. Clear
            // the cache so the retry re-mints, and treat this ONE case as
            // retryable — clock skew heals, a bad key (InvalidProviderToken)
            // does not and stays Configuration.
            if is_expired_provider_token(status.as_u16(), &reason) {
                self.token.lock().expect("token lock poisoned").jwt.clear();
                return Err(PushError::Retryable {
                    status: Some(403),
                    reason: "ExpiredProviderToken (cached token invalidated; retry re-mints)"
                        .to_string(),
                });
            }
            Err(classify_apns_error(status.as_u16(), &reason))
        })
    }
}

/// Classify a reqwest transport failure into a short reason string.
///
/// NEVER the error's `Display`: reqwest appends ` for url ({url})`, and the
/// URL is `…/3/device/<raw device token>` (F5). Callers log the hash
/// ([`token_hash`]) as a separate field instead.
fn transport_reason(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout".to_string()
    } else if e.is_connect() {
        "connect".to_string()
    } else if e.is_body() {
        "body".to_string()
    } else {
        "transport".to_string()
    }
}

/// Whether a 403 `ExpiredProviderToken` response is the clock-skew case
/// (the caller invalidates its cached JWT and retries once — N5) rather
/// than a bad-key Configuration. Only this exact pair is special-cased:
/// every other 401/403 stays Configuration (permanent).
fn is_expired_provider_token(status: u16, reason: &str) -> bool {
    status == 403 && reason == "ExpiredProviderToken"
}

/// Map an APNs HTTP status + Apple `reason` to a typed [`PushError`].
///
/// Only a genuinely dead device drops the registry token: HTTP 400
/// `BadDeviceToken`/`Unregistered`, or HTTP 410 `Unregistered` (F2). Our
/// OWN config/payload bugs that Apple reports as 400 (PayloadTooLarge,
/// DeviceTokenNotForTopic, BadTopic, …) must NOT deregister a healthy
/// device — they are `Rejected` (logged + dropped, token retained), so a
/// fixable config error never becomes silent push loss.
fn classify_apns_error(status: u16, reason: &str) -> PushError {
    match status {
        400 => match reason {
            "BadDeviceToken" | "Unregistered" => {
                warn!(status, reason, "apns rejected the device token");
                PushError::Unregistered
            }
            _ => {
                warn!(status, reason, "apns rejected the push");
                PushError::Rejected {
                    status,
                    reason: reason.to_string(),
                }
            }
        },
        410 => {
            warn!(status, reason, "apns: device unregistered (app removed)");
            PushError::Unregistered
        }
        401 | 403 => {
            warn!(status, reason, "apns credential problem");
            PushError::Configuration(reason.to_string())
        }
        s if RETRYABLE_STATUS.contains(&s) => PushError::Retryable {
            status: Some(s),
            reason: reason.to_string(),
        },
        _ => PushError::Rejected {
            status,
            reason: reason.to_string(),
        },
    }
}

fn now_elapsed() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

fn json_b64(v: &Value) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).expect("jwt part serializes"))
}

/// One-way fingerprint of a device token for logs — never the raw token,
/// not even a prefix (F5): the first 8 hex chars of SHA-256 give enough
/// entropy to correlate a log line without disclosing a per-device id.
pub(crate) fn token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(token.as_bytes());
    crate::auth::hex(&digest[..4])
}

// ---------------------------------------------------------------------------
// Mock provider (tests + the acceptance timing assertion)
// ---------------------------------------------------------------------------

/// Records every push. `received` yields `(device_token, payload)` pairs in
/// order; `fail_next` makes the next `fail_next` pushes return
/// [`PushError::Retryable`] (retry-policy tests); `fail_with` makes the
/// NEXT push return an exact [`PushError`] (error-mapping tests). Never
/// touches the network — the only provider the test suite uses.
pub struct MockProvider {
    tx: tokio::sync::mpsc::UnboundedSender<(String, Value)>,
    fail_next: Mutex<usize>,
    fail_with: Mutex<Option<PushError>>,
}

impl MockProvider {
    /// Returns the provider (as `Arc<dyn ApnsProvider>`-able) plus the
    /// channel every delivered push lands on.
    pub fn new() -> (
        Arc<Self>,
        tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
    ) {
        let (tx, received) = tokio::sync::mpsc::unbounded_channel();
        (
            Arc::new(Self {
                tx,
                fail_next: Mutex::new(0),
                fail_with: Mutex::new(None),
            }),
            received,
        )
    }

    pub fn fail_next(&self, n: usize) {
        *self.fail_next.lock().expect("mock lock") = n;
    }

    /// The next push returns `error` exactly once.
    pub fn fail_with(&self, error: PushError) {
        *self.fail_with.lock().expect("mock lock") = Some(error);
    }
}

impl Default for MockProvider {
    fn default() -> Self {
        let (tx, _received) = tokio::sync::mpsc::unbounded_channel();
        Self {
            tx,
            fail_next: Mutex::new(0),
            fail_with: Mutex::new(None),
        }
    }
}

impl ApnsProvider for MockProvider {
    fn push<'a>(
        &'a self,
        device_token: &'a str,
        payload: &'a Value,
    ) -> futures::future::BoxFuture<'a, Result<(), PushError>> {
        Box::pin(async move {
            let mut remaining = self.fail_next.lock().expect("mock lock");
            if *remaining > 0 {
                *remaining -= 1;
                return Err(PushError::Retryable {
                    status: Some(503),
                    reason: "mock outage".to_string(),
                });
            }
            drop(remaining);
            if let Some(error) = self.fail_with.lock().expect("mock lock").take() {
                return Err(error);
            }
            self.tx
                .send((device_token.to_string(), payload.clone()))
                .map_err(|_| PushError::Rejected {
                    status: 500,
                    reason: "mock receiver dropped".to_string(),
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_p8_rejects_garbage() {
        assert!(RealApnsProvider::parse_p8("not a pem").is_err());
        assert!(RealApnsProvider::parse_p8("").is_err());
    }

    #[test]
    fn parse_p8_accepts_ec_pkcs8_pem() {
        let pem = test_p8();
        let key = RealApnsProvider::parse_p8(&pem).expect("valid p8 parses");
        // The parsed key signs (and is a P-256 key, i.e. usable for ES256).
        let sig: p256::ecdsa::Signature = key.sign(b"hello");
        assert_eq!(sig.to_bytes().len(), 64);
    }

    #[test]
    fn provider_token_is_an_es256_jwt_with_expected_claims() {
        let key = RealApnsProvider::parse_p8(&test_p8()).unwrap();
        let config = Config {
            team_id: "TEAM123456".to_string(),
            key_id: "KEY12345678".to_string(),
            auth_key_path: String::new(),
            endpoint: Endpoint::Sandbox,
            topic: "com.corral.fleetnotifier".to_string(),
        };
        let provider = RealApnsProvider::new(config, key).expect("provider builds");
        let cached = provider.token.lock().unwrap();
        let parts: Vec<&str> = cached.jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT is header.claims.signature");

        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["kid"], "KEY12345678");

        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], "TEAM123456");
        let iat = claims["iat"].as_u64().unwrap();
        let now = now_elapsed().as_secs();
        assert!(
            iat.abs_diff(now) <= 2,
            "iat is the mint time, within clock skew"
        );
    }

    /// A generated P-256 key in PKCS#8 PEM — same shape Apple's .p8 uses.
    fn test_p8() -> String {
        use p256::pkcs8::{EncodePrivateKey, LineEnding};
        // Deterministic test key (never secrets): from_bytes validates the
        // scalar is canonical; the fixed bytes derive a valid P-256 key.
        let signing = SigningKey::from_bytes((&[0x42; 32]).into()).expect("valid p-256 scalar");
        signing.to_pkcs8_pem(LineEnding::LF).unwrap().to_string()
    }

    #[test]
    fn apns_400_mapping_is_reason_aware() {
        // A genuinely dead device: only these drop the registry token.
        assert_eq!(
            classify_apns_error(400, "BadDeviceToken"),
            PushError::Unregistered
        );
        assert_eq!(
            classify_apns_error(400, "Unregistered"),
            PushError::Unregistered
        );
        // Our OWN config/payload bug (F2): rejected, NOT unregistered —
        // the token must be retained so a fixable error stays fixable.
        assert_eq!(
            classify_apns_error(400, "PayloadTooLarge"),
            PushError::Rejected {
                status: 400,
                reason: "PayloadTooLarge".to_string()
            }
        );
        assert_eq!(
            classify_apns_error(400, "DeviceTokenNotForTopic"),
            PushError::Rejected {
                status: 400,
                reason: "DeviceTokenNotForTopic".to_string()
            }
        );
        assert_eq!(
            classify_apns_error(400, "BadTopic"),
            PushError::Rejected {
                status: 400,
                reason: "BadTopic".to_string()
            }
        );
        // 410 always means the device is gone.
        assert_eq!(
            classify_apns_error(410, "Unregistered"),
            PushError::Unregistered
        );
        // Credentials and transient statuses are unchanged.
        assert!(matches!(
            classify_apns_error(401, "BadProviderToken"),
            PushError::Configuration(_)
        ));
        assert!(classify_apns_error(503, "Unavailable").is_retryable());
        assert!(classify_apns_error(429, "TooManyProviderRequests").is_retryable());
    }

    #[test]
    fn apns_400_payload_too_large_is_not_unregistered() {
        // The exact F2 regression: a 400 PayloadTooLarge body must yield
        // Rejected (token retained), never Unregistered (token dropped).
        let err = classify_apns_error(400, "PayloadTooLarge");
        assert_ne!(err, PushError::Unregistered, "our bug is not a dead device");
        match err {
            PushError::Rejected { status, reason } => {
                assert_eq!(status, 400);
                assert_eq!(reason, "PayloadTooLarge");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn token_hash_is_one_way_and_stable() {
        // Deterministic, 8 hex chars, and it never contains the token.
        assert_eq!(token_hash("a1b2c3d4e5f6"), token_hash("a1b2c3d4e5f6"));
        assert_eq!(token_hash("a1b2c3d4e5f6").len(), 8);
        assert_ne!(token_hash("a1b2c3d4e5f6"), token_hash("a1b2c3d4e5f7"));
        let h = token_hash("deadbeefdeadbeef");
        assert!(!h.contains("dead"), "no raw token material in the log form");
    }

    #[tokio::test]
    async fn transport_error_reason_is_classified_not_displayed() {
        // F5/N2: the reason string a transport failure carries into the
        // logs must NEVER embed the URL (which contains the raw device
        // token). Classify into a fixed vocabulary instead of the error's
        // Display, which reqwest appends ` for url ({url})` to.
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(500))
            .build()
            .expect("client builds");
        // Connection refused on loopback is deterministic and immediate —
        // a real reqwest::Error, exercising the real classification.
        let err = client
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .expect_err("closed loopback port must fail");
        assert!(err.is_connect(), "connection-refused classifies as connect");
        let reason = transport_reason(&err);
        assert_eq!(reason, "connect");
        assert!(
            !reason.contains("127.0.0.1") && !reason.contains("url"),
            "classified reason must not embed the URL/token: {reason}"
        );
    }

    #[test]
    fn expired_provider_token_is_retryable_but_only_that_exact_case() {
        // N5: only 403 ExpiredProviderToken is the clock-skew case; a bad
        // key (InvalidProviderToken) or a 401 must stay Configuration.
        assert!(is_expired_provider_token(403, "ExpiredProviderToken"));
        assert!(!is_expired_provider_token(403, "InvalidProviderToken"));
        assert!(!is_expired_provider_token(401, "ExpiredProviderToken"));
        assert!(!is_expired_provider_token(400, "ExpiredProviderToken"));
    }
}
