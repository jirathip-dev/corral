//! #485 — the local owner channel: a unix-domain socket carrying the
//! owner-authenticated enrollment operations (`mint` / `pending` /
//! `approve` / `revoke`).
//!
//! Deliberately NOT an HTTP route. The frozen contract (O1 owner pin:
//! "genuinely local-only host-owner approval/revocation channel; do NOT
//! broaden existing network HTTP admin endpoints to grant mutations")
//! authenticates the channel with the host OS identity:
//!
//! - the socket lives in the 0700 config dir at mode 0600 (filesystem
//!   access control — the primary boundary), and
//! - every accepted connection passes a peer-credential check
//!   (`LOCAL_PEERCRED` on macOS / `SO_PEERCRED` on Linux) against the
//!   socket file's owner uid; a mismatch is refused before any request
//!   byte is parsed ([`PeerIdentity`] is injectable so tests can drive
//!   the denial path).
//!
//! The admin token is NOT accepted here (the OS credential is the owner
//! proof) and no network listener gains any of these operations — #354's
//! "per-device grants … never over HTTP" line is held absolutely.
//!
//! Protocol (v1): one JSON request object per connection, terminated by
//! `\n` or half-close (≤ 8 KiB); one JSON response object follows and the
//! connection closes. Success bodies are the frozen §5.2 shapes; errors
//! are `{"error":…,"code":…}` with the frozen code strings.

use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use super::enrollment::{
    ApproveError, MintError, RedeemError, SessionView, StatusError, StatusSnapshot,
};
use super::{AuthPlane, REGISTRATION_TTL, now_secs};

/// Socket file name inside the config dir (`<config_dir>/owner.sock`).
pub const OWNER_SOCKET_FILE: &str = "owner.sock";
/// Bounded request size (protocol v1; local client, tiny JSON objects).
pub const MAX_REQUEST_BYTES: usize = 8 * 1024;
/// A connection that never completes its request is closed.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Cap on the `endpoint` string minted into the QR payload.
const MAX_ENDPOINT_CHARS: usize = 256;

pub fn socket_path(config_dir: &Path) -> PathBuf {
    config_dir.join(OWNER_SOCKET_FILE)
}

/// Peer-identity seam: production reads the kernel peer credential;
/// tests inject a fake to exercise the denial path (frozen C7(g)).
pub trait PeerIdentity: Send + Sync + 'static {
    fn uid(&self, stream: &UnixStream) -> io::Result<u32>;
}

/// Production implementation: the kernel's peer credential.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealPeerIdentity;

impl PeerIdentity for RealPeerIdentity {
    fn uid(&self, stream: &UnixStream) -> io::Result<u32> {
        peer_uid(stream.as_raw_fd())
    }
}

#[cfg(target_os = "macos")]
fn peer_uid(fd: std::os::raw::c_int) -> io::Result<u32> {
    // SAFETY: `fd` is a live unix-socket descriptor for the duration of
    // the call; `cred`/`len` are valid out-parameters of the documented
    // size, so the kernel cannot write out of bounds.
    unsafe {
        let mut cred: libc::xucred = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::xucred>() as libc::socklen_t;
        let rc = libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERCRED,
            &mut cred as *mut libc::xucred as *mut libc::c_void,
            &mut len,
        );
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(cred.cr_uid)
    }
}

#[cfg(target_os = "linux")]
fn peer_uid(fd: std::os::raw::c_int) -> io::Result<u32> {
    // SAFETY: as above — `fd` is a live unix-socket descriptor and the
    // out-buffer is the documented `ucred` size.
    unsafe {
        let mut cred: libc::ucred = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        );
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(cred.uid)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn peer_uid(_fd: std::os::raw::c_int) -> io::Result<u32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "peer credentials are only implemented for macOS and Linux",
    ))
}

/// Bind the owner socket: 0700 dir, 0600 socket file, stale-socket probe.
/// A live listener at the path is never replaced (already-in-use error);
/// a refused connect proves the file is stale and it is removed first.
pub async fn bind(path: &Path) -> Result<UnixListener, String> {
    let dir = path.parent().ok_or("owner socket path has no parent")?;
    super::ensure_dir_0700(dir)?;
    if path.symlink_metadata().is_ok() {
        match std::os::unix::net::UnixStream::connect(path) {
            Ok(_) => {
                return Err(format!("owner socket already in use: {}", path.display()));
            }
            Err(_) => {
                std::fs::remove_file(path)
                    .map_err(|e| format!("remove stale socket {}: {e}", path.display()))?;
            }
        }
    }
    let listener = UnixListener::bind(path).map_err(|e| format!("bind {}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod {}: {e}", path.display()))?;
    Ok(listener)
}

/// Serve with the production peer-identity check.
pub async fn serve(listener: UnixListener, auth: Arc<AuthPlane>) {
    serve_with_identity(listener, auth, Arc::new(RealPeerIdentity)).await;
}

/// Serve with an injected peer identity (tests; frozen C7(g)).
pub async fn serve_with_identity(
    listener: UnixListener,
    auth: Arc<AuthPlane>,
    identity: Arc<dyn PeerIdentity>,
) {
    // The owner uid is the socket file's owner — the daemon's effective
    // uid at bind time. Unreadable → refuse everything (fail closed).
    let owner_uid = match listener
        .local_addr()
        .ok()
        .and_then(|a| a.as_pathname().map(Path::to_path_buf))
        .and_then(|p| std::fs::metadata(p).ok())
    {
        Some(meta) => {
            use std::os::unix::fs::MetadataExt;
            meta.uid()
        }
        None => {
            tracing::error!("owner socket: cannot resolve socket owner uid; refusing connections");
            return;
        }
    };
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let auth = auth.clone();
                let identity = identity.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle(stream, auth, identity, owner_uid).await {
                        tracing::debug!(error = %e, "owner connection closed");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "owner socket accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

async fn handle(
    mut stream: UnixStream,
    auth: Arc<AuthPlane>,
    identity: Arc<dyn PeerIdentity>,
    owner_uid: u32,
) -> io::Result<()> {
    // Peer credential check BEFORE any request parse. A denied peer still
    // receives the typed refusal (frozen §2): the response goes out first,
    // then the peer's pending bytes are drained so the close cannot turn
    // into a Linux connection reset that would discard it (see
    // [`drain_pending`] — hosted rust run 34675447060).
    let peer = match identity.uid(&stream) {
        Ok(uid) => uid,
        Err(e) => {
            tracing::warn!(error = %e, "owner socket: peer credential unavailable");
            let out = respond_error(
                &mut stream,
                "owner credential unavailable",
                "owner_credential_mismatch",
            )
            .await;
            drain_pending(&mut stream).await;
            return out;
        }
    };
    if peer != owner_uid {
        let out = respond_error(
            &mut stream,
            "owner credential mismatch",
            "owner_credential_mismatch",
        )
        .await;
        drain_pending(&mut stream).await;
        return out;
    }
    let request = match read_request(&mut stream).await {
        Ok(v) => v,
        Err(e) => {
            return respond_error(
                &mut stream,
                &format!("malformed owner request: {e}"),
                "malformed_request",
            )
            .await;
        }
    };
    let response = dispatch(&auth, request);
    write_json(&mut stream, &response).await
}

/// Read one framed request's raw BYTES: terminated by `\n` or EOF
/// (half-close), capped at [`MAX_REQUEST_BYTES`], bounded by
/// [`REQUEST_TIMEOUT`]. Returns the bytes unparsed.
async fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>, String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if buf.len() >= MAX_REQUEST_BYTES {
            return Err("request too large".to_string());
        }
        let n = tokio::time::timeout(REQUEST_TIMEOUT, stream.read(&mut byte))
            .await
            .map_err(|_| "request read timed out".to_string())?
            .map_err(|e| e.to_string())?;
        if n == 0 || byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
    }
    Ok(buf)
}

/// Read one JSON request: [`read_frame`] plus the parse.
async fn read_request(stream: &mut UnixStream) -> Result<serde_json::Value, String> {
    let buf = read_frame(stream).await?;
    serde_json::from_slice(&buf).map_err(|e| e.to_string())
}

/// Consume whatever the peer sent, bounded ([`MAX_REQUEST_BYTES`] /
/// [`REQUEST_TIMEOUT`]), WITHOUT parsing or acting on it.
///
/// This runs before a refused connection is closed. Closing an AF_UNIX
/// stream socket while unread bytes sit in its receive queue turns the
/// close into a connection RESET for the peer on Linux — the peer's read
/// fails with `ECONNRESET` (os 104) and the queued refusal is DISCARDED.
/// Observed in hosted rust run 34675447060: the denied-peer test failed
/// with `ConnectionReset` on Linux while macOS delivered the refusal
/// cleanly. The frozen contract (§2) requires the denied peer to receive
/// the typed `owner_credential_mismatch` refusal, so the pending bytes are
/// drained first; errors/timeouts are ignored (the refusal must still go
/// out). Residual: a peer that sends more than the frame bound may still
/// trigger the reset — it receives no refusal, but the connection is
/// always terminated (fail closed).
async fn drain_pending(stream: &mut UnixStream) {
    let _ = read_frame(stream).await;
}

async fn write_json(stream: &mut UnixStream, value: &serde_json::Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value).expect("owner response serializes");
    bytes.push(b'\n');
    stream.write_all(&bytes).await?;
    stream.flush().await
}

async fn respond_error(stream: &mut UnixStream, error: &str, code: &str) -> io::Result<()> {
    write_json(stream, &serde_json::json!({ "error": error, "code": code })).await
}

fn error_json(error: &str, code: &str) -> serde_json::Value {
    serde_json::json!({ "error": error, "code": code })
}

/// The frozen QR payload v1 (field order fixed by the struct; no extra
/// fields, no whitespace).
#[derive(serde::Serialize)]
struct QrPayloadV1<'a> {
    v: u8,
    host_key: &'a str,
    endpoint: &'a str,
    code: &'a str,
    expires_ts: u64,
    scope: &'a str,
}

fn qr_payload_v1(host_key: &str, endpoint: &str, code_b64: &str, expires_ts: u64) -> String {
    serde_json::to_string(&QrPayloadV1 {
        v: 1,
        host_key,
        endpoint,
        code: code_b64,
        expires_ts,
        scope: "read_tail",
    })
    .expect("QR payload serializes")
}

/// Exact-field enforcement (frozen P3: fixed request shapes, no `grants`
/// parameter — a request carrying anything outside its op's field set is
/// refused as malformed, so this channel cannot express grant edits).
fn only_fields(obj: &serde_json::Map<String, serde_json::Value>, allowed: &[&str]) -> bool {
    obj.keys().all(|k| allowed.contains(&k.as_str()))
}

fn dispatch(auth: &AuthPlane, request: serde_json::Value) -> serde_json::Value {
    let Some(obj) = request.as_object() else {
        return error_json("owner request must be a JSON object", "malformed_request");
    };
    let op = obj.get("op").and_then(|v| v.as_str()).unwrap_or_default();
    match op {
        "mint" => {
            if !only_fields(obj, &["op", "endpoint"]) {
                return error_json("unsupported field for mint", "malformed_request");
            }
            // The owner client knows the public endpoint; the daemon has
            // no way to derive `<host>.<tailnet>.ts.net`. Required and
            // validated (https only) so a frozen payload never ships an
            // underived placeholder.
            let Some(endpoint) = obj.get("endpoint").and_then(|v| v.as_str()) else {
                return error_json(
                    "mint requires the public https endpoint",
                    "malformed_request",
                );
            };
            if !endpoint.starts_with("https://")
                || endpoint.chars().count() > MAX_ENDPOINT_CHARS
                || endpoint
                    .chars()
                    .any(|c| c.is_control() || c.is_whitespace())
            {
                return error_json(
                    "endpoint must be a single https:// URL (no whitespace)",
                    "malformed_request",
                );
            }
            match auth.enrollment.mint(now_secs()) {
                Ok(minted) => serde_json::json!({
                    "enrollment_id": minted.enrollment_id,
                    "expires_ts": minted.expires_ts,
                    "scope": ["read_tail"],
                    "payload": qr_payload_v1(
                        &auth.host.public_key_b64(),
                        endpoint,
                        &minted.code_b64,
                        minted.expires_ts,
                    ),
                }),
                Err(MintError::PendingCap) => error_json(
                    "too many enrollment sessions awaiting action",
                    "enroll_pending_cap",
                ),
            }
        }
        "pending" => {
            if !only_fields(obj, &["op"]) {
                return error_json("unsupported field for pending", "malformed_request");
            }
            let sessions: Vec<serde_json::Value> = auth
                .enrollment
                .pending(now_secs())
                .into_iter()
                .map(session_view_json)
                .collect();
            serde_json::json!({ "sessions": sessions })
        }
        "approve" => {
            if !only_fields(obj, &["op", "enrollment_id"]) {
                return error_json("unsupported field for approve", "malformed_request");
            }
            let Some(enrollment_id) = obj.get("enrollment_id").and_then(|v| v.as_str()) else {
                return error_json("approve requires enrollment_id", "malformed_request");
            };
            match auth.enrollment.approve(
                enrollment_id,
                now_secs(),
                &auth.registry,
                REGISTRATION_TTL,
            ) {
                Ok(rec) => serde_json::json!({
                    "ok": true,
                    "key_id": rec.key_id,
                    "grants": rec.grants,
                    "expiry_ts": rec.expiry_ts,
                }),
                Err(ApproveError::UnknownSession) => {
                    error_json("unknown enrollment session", "enroll_unknown_session")
                }
                Err(ApproveError::Expired) => {
                    error_json("enrollment session expired", "enroll_expired")
                }
                Err(ApproveError::NotRedeemed) => error_json(
                    "session not redeemed by a device yet",
                    "enroll_not_redeemed",
                ),
                Err(ApproveError::AlreadyApproved(snap)) => serde_json::json!({
                    "error": "enrollment already approved",
                    "code": "enroll_already_approved",
                    "key_id": snap.key_id,
                    "grants": snap.grants,
                    "expiry_ts": snap.expiry_ts,
                }),
                Err(ApproveError::Persist(e)) => error_json(
                    &format!("registry persist failed: {e}"),
                    "registry_persist_failed",
                ),
            }
        }
        "revoke" => {
            if !only_fields(obj, &["op", "key_id"]) {
                return error_json("unsupported field for revoke", "malformed_request");
            }
            let Some(key_id) = obj.get("key_id").and_then(|v| v.as_str()) else {
                return error_json("revoke requires key_id", "malformed_request");
            };
            match auth.registry.revoke_committed(key_id) {
                Ok(rec) => serde_json::json!({
                    "ok": true,
                    "key_id": rec.key_id,
                    "revoked": true,
                }),
                Err(super::registry::RegistryMutationError::UnknownKey(k)) => {
                    error_json(&format!("unknown key: {k}"), "unknown_key")
                }
                Err(super::registry::RegistryMutationError::Persist(e)) => error_json(
                    &format!("registry persist failed: {e}"),
                    "registry_persist_failed",
                ),
            }
        }
        _ => error_json("unknown owner operation", "malformed_request"),
    }
}

fn session_view_json(view: SessionView) -> serde_json::Value {
    serde_json::json!({
        "enrollment_id": view.enrollment_id,
        "key_id": view.key_id,
        "name": view.name,
        "state": view.state,
        "expires_ts": view.expires_ts,
    })
}

/// Redeem/status shared by the HTTP handlers (`crate::auth::http`): map
/// store results onto the frozen (status, code) vocabulary. Exposed
/// pub(crate) so the HTTP layer has exactly one mapping implementation.
pub(crate) fn redeem_outcome(
    result: Result<super::enrollment::PendingSnapshot, RedeemError>,
) -> (axum::http::StatusCode, serde_json::Value) {
    use axum::http::StatusCode;
    match result {
        Ok(p) => (
            StatusCode::OK,
            serde_json::json!({
                "state": "pending",
                "key_id": p.key_id,
                "expires_ts": p.expires_ts,
            }),
        ),
        Err(RedeemError::UnknownCode) => (
            StatusCode::NOT_FOUND,
            error_json(&RedeemError::UnknownCode.to_string(), "enroll_unknown_code"),
        ),
        Err(e @ RedeemError::Expired) => (
            StatusCode::GONE,
            error_json(&e.to_string(), "enroll_expired"),
        ),
        Err(e @ RedeemError::Consumed) => (
            StatusCode::CONFLICT,
            error_json(&e.to_string(), "enroll_redeemed"),
        ),
        Err(e @ RedeemError::AlreadyRegistered) => (
            StatusCode::CONFLICT,
            error_json(&e.to_string(), "already_registered"),
        ),
        Err(e @ RedeemError::BadPublicKey) => (
            StatusCode::BAD_REQUEST,
            error_json(&e.to_string(), "bad_public_key"),
        ),
        Err(e @ RedeemError::BadName) => (
            StatusCode::BAD_REQUEST,
            error_json(&e.to_string(), "bad_name"),
        ),
    }
}

/// Status outcome mapping (frozen §5.3; `/enroll/status` NEVER 409).
pub(crate) fn status_outcome(
    result: Result<StatusSnapshot, StatusError>,
) -> (axum::http::StatusCode, serde_json::Value) {
    use axum::http::StatusCode;
    match result {
        Ok(StatusSnapshot::Pending { expires_ts }) => (
            StatusCode::OK,
            serde_json::json!({ "state": "pending", "expires_ts": expires_ts }),
        ),
        Ok(StatusSnapshot::Approved {
            key_id,
            grants,
            expiry_ts,
            revoked,
        }) => (
            StatusCode::OK,
            serde_json::json!({
                "state": "approved",
                "key_id": key_id,
                "grants": grants,
                "expiry_ts": expiry_ts,
                "revoked": revoked,
            }),
        ),
        Err(e @ StatusError::UnknownCode) => (
            StatusCode::NOT_FOUND,
            error_json(&e.to_string(), "enroll_unknown_code"),
        ),
        Err(e @ StatusError::Expired) => (
            StatusCode::GONE,
            error_json(&e.to_string(), "enroll_expired"),
        ),
    }
}
