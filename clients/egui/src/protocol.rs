//! Read-path protocol client: `GET /snapshot`, `GET /events` (SSE with
//! `Last-Event-ID` resume), `POST /register`, host-admin `GET /grants`,
//! and `GET /audit`. The drive write path lives in [`crate::drive`].
//!
//! The SSE reader owns the resume loop: connect → parse events → on any
//! disconnect, back off (doubling, capped) and reconnect carrying the last
//! seen rev as `Last-Event-ID`. The daemon answers a stale cursor with a
//! full snapshot or delta replay (contract: never misses a window). The
//! reader also falls back to a plain `GET /snapshot` when the SSE endpoint
//! is unavailable, so a restart with dropped SSE support still shows live
//! state (client-side polling only — never in the daemon).

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;

use crate::model::{Delta, FleetIdentities, GhIssueRef, Snapshot};

pub const DEFAULT_HOST_URL: &str = "http://127.0.0.1:8474";

/// Canonical grant order for the board's grant editor. This is the same
/// closed set the daemon's `Capability` parser accepts; `start_worktree`
/// is rendered separately as a fleet-level capability.
pub const GRANT_CAPABILITIES: [&str; 9] = [
    "prompt",
    "interrupt",
    "approve",
    "read_tail",
    "read_diff",
    "read_issues",
    "kill",
    "attach",
    "start_worktree",
];

/// #249 recovery grant set: the signed drive-plane caps re-applied when a
/// rebuild/reinstall leaves the board with a fresh key. Deliberately
/// excludes `start_worktree` (a binding operation, not part of the drive
/// plane the issue names) and is used for the one-tap
/// "Re-register + grant" recovery when the previous registration carries
/// no recorded grants.
pub const RECOVERY_GRANT_CAPS: [&str; 7] = [
    "read_tail",
    "read_diff",
    "prompt",
    "interrupt",
    "approve",
    "kill",
    "attach",
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HostKey {
    pub algorithm: String,
    pub public_key: String,
}

/// One parsed SSE event frame.
#[derive(Debug, Clone, PartialEq)]
pub enum SseEvent {
    Snapshot(Snapshot),
    Delta(Delta),
    /// An event type we do not understand (forward-compatible: ignore).
    Unknown {
        event: String,
        id: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamOutcome {
    /// Stream ended cleanly (daemon closed); reconnect per policy.
    Closed,
    /// Transport/parse failure; reconnect per policy.
    Error,
}

/// Bounded reconnect backoff for the SSE loop.
pub const SSE_BACKOFF_BASE_MS: u64 = 500;
pub const SSE_BACKOFF_MAX_MS: u64 = 30_000;

/// Per-chunk read deadline for the SSE stream. reqwest's `.timeout()` is a
/// TOTAL deadline that keeps ticking through body streaming, so a long-lived
/// SSE connection must NOT carry one (the daemon's 15s keepalive comments
/// keep the stream alive; a 60s total timeout severed it every minute).
/// Instead, each `chunk()` await gets a read deadline well above the
/// keepalive cadence (3x), so a genuinely dead socket still forces a
/// reconnect without ever killing a healthy stream.
pub const SSE_CHUNK_READ_TIMEOUT: Duration = Duration::from_secs(45);

/// Open a streaming connection to `/events`, resuming from `last_rev`.
/// Returns an error for non-2xx so the caller can fall back to snapshot.
/// No total request timeout: the connection is meant to live indefinitely
/// (connect timeout comes from the shared client builder).
pub async fn open_events(
    client: &reqwest::Client,
    base_url: &str,
    last_rev: Option<u64>,
) -> Result<reqwest::Response, String> {
    let url = format!("{}/events", base_url.trim_end_matches('/'));
    let mut builder = client.get(&url);
    if let Some(rev) = last_rev {
        builder = builder.header("Last-Event-ID", rev.to_string());
    }
    let response = builder.send().await.map_err(|e| format!("connect: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("GET /events -> {status}"));
    }
    Ok(response)
}

pub async fn fetch_host_key(client: &reqwest::Client, base_url: &str) -> Result<HostKey, String> {
    let url = format!("{}/host-key", base_url.trim_end_matches('/'));
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("connect: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("GET /host-key -> {}", response.status()));
    }
    #[derive(Deserialize)]
    struct Wire {
        algorithm: String,
        public_key: String,
    }
    let wire: Wire = response.json().await.map_err(|e| format!("body: {e}"))?;
    Ok(HostKey {
        algorithm: wire.algorithm,
        public_key: wire.public_key,
    })
}

pub async fn fetch_snapshot(client: &reqwest::Client, base_url: &str) -> Result<Snapshot, String> {
    let url = format!("{}/snapshot", base_url.trim_end_matches('/'));
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("connect: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("GET /snapshot -> {}", response.status()));
    }
    response.json().await.map_err(|e| format!("body: {e}"))
}

/// #113: `GET /issues` — the daemon's read-only repo-level issue view, the
/// same set the worktree action validates a selected issue against. The
/// response is `{ "repos": { repo: [GhIssueRef...] } }`; older daemons
/// without the endpoint return an error, which the UI surfaces politely
/// (the Board still renders per-agent issues from the snapshot).
pub async fn fetch_issues(
    client: &reqwest::Client,
    base_url: &str,
) -> Result<BTreeMap<String, Vec<GhIssueRef>>, String> {
    let url = format!("{}/issues", base_url.trim_end_matches('/'));
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("connect: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("GET /issues -> {}", response.status()));
    }
    let body: serde_json::Value = response.json().await.map_err(|e| format!("body: {e}"))?;
    decode_issues(body)
}

/// Decode the complete `/issues` envelope in one place before it crosses the
/// async boundary. Keeping the grouped map intact is important: the daemon
/// includes configured repos with zero issues alongside repos with a full
/// snapshot, and the UI must not accidentally project this into one repo or
/// one issue per group.
fn decode_issues(body: serde_json::Value) -> Result<BTreeMap<String, Vec<GhIssueRef>>, String> {
    #[derive(Deserialize)]
    struct Wire {
        repos: BTreeMap<String, Vec<GhIssueRef>>,
    }
    serde_json::from_value::<Wire>(body)
        .map(|wire| wire.repos)
        .map_err(|e| format!("body: {e}"))
}

/// #237: `GET /fleets` — the fleet-ops CLI validated identity catalog
/// (configless: corral never reads fleets.json). Transport, non-2xx, and
/// body decode failures are strict `Err(String)`s; a daemon-level fleet-ops
/// CLI failure is a successful HTTP 200 `FleetIdentities { status: "error",
/// .. }` and is rendered prominently.
pub async fn fetch_fleet_identities(
    client: &reqwest::Client,
    base_url: &str,
) -> Result<FleetIdentities, String> {
    let url = format!("{}/fleets", base_url.trim_end_matches('/'));
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("connect: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("GET /fleets -> {}", response.status()));
    }
    response.json().await.map_err(|e| format!("body: {e}"))
}

/// `POST /register` with the routing-only registration token and the
/// device's base64 Ed25519 public key. `name` is the optional cosmetic
/// device label (#209) — the daemon stores it so every Devices/Grants
/// surface can name this machine/phone. Returns `(key_id, grants)`.
pub async fn register_device(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    public_key_b64: &str,
    name: Option<&str>,
) -> Result<(String, Vec<String>), String> {
    let url = format!("{}/register", base_url.trim_end_matches('/'));
    let mut body = serde_json::Map::new();
    body.insert("token".to_string(), serde_json::json!(token));
    body.insert("public_key".to_string(), serde_json::json!(public_key_b64));
    if let Some(name) = name {
        body.insert("name".to_string(), serde_json::json!(name));
    }
    let response = client
        .post(&url)
        .json(&serde_json::Value::Object(body))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let status = response.status();
    let json: serde_json::Value = response.json().await.map_err(|e| format!("body: {e}"))?;
    if !status.is_success() {
        let error = json
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("registration refused");
        return Err(format!("{status} {error}"));
    }
    let key_id = json
        .get("key_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "registration response missing key_id".to_string())?
        .to_string();
    let grants = json
        .get("grants")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|g| g.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok((key_id, grants))
}

/// `GET /audit` with the host admin bearer token (the host's own
/// credential — the audit log is host-admin, never device-accessible).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AuditEntry {
    pub seq: u64,
    pub ts: u64,
    pub key_id: String,
    pub request_id: String,
    pub capability: String,
    pub target: String,
    /// `"executed"` or `{"refused": d}` / `{"failed": d}` (externally
    /// tagged, mirrors the daemon's `OutcomeJson`).
    pub outcome: serde_json::Value,
    pub prev: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AuditView {
    pub head: String,
    pub valid: bool,
    pub entries: Vec<AuditEntry>,
}

pub async fn fetch_audit(
    client: &reqwest::Client,
    base_url: &str,
    admin_token: &str,
) -> Result<AuditView, String> {
    let url = format!("{}/audit", base_url.trim_end_matches('/'));
    let response = client
        .get(&url)
        .bearer_auth(admin_token)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("connect: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("GET /audit -> {}", response.status()));
    }
    response.json().await.map_err(|e| format!("body: {e}"))
}

/// A registered device as projected by the host-admin `GET /grants` read
/// surface. Public keys and push tokens stay host-side and never cross
/// this wire shape. `name` is the optional cosmetic label the device
/// supplied at registration (#209).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GrantDevice {
    pub key_id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub grants: Vec<String>,
    pub revoked: bool,
    /// When the host revoked this device (#257, additive: `None` on
    /// pre-#257 ledgers and on old daemons that do not project it). The
    /// row then shows the true revocation age; `None` falls back to a
    /// plain "revoked" subline (never the creation age).
    #[serde(default)]
    pub revoked_ts: Option<u64>,
    pub expiry_ts: u64,
    pub created_ts: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AdminGrantsView {
    pub ok: bool,
    pub devices: Vec<GrantDevice>,
}

/// `GET /grants` with the host admin bearer token. Without `key_id` it
/// returns every registered device for the Settings selector; with a
/// `key_id` it narrows the daemon's projection to one device.
pub async fn fetch_admin_grants(
    client: &reqwest::Client,
    base_url: &str,
    admin_token: &str,
    key_id: Option<&str>,
) -> Result<AdminGrantsView, String> {
    let mut url = format!("{}/grants", base_url.trim_end_matches('/'));
    if let Some(key_id) = key_id {
        url.push_str(&format!("?key_id={}", urlencode(key_id)));
    }
    let response = client
        .get(&url)
        .bearer_auth(admin_token)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.map_err(|e| format!("body: {e}"))?;
    if !status.is_success() {
        return Err(grant_error(status.as_u16(), &body, "GET /grants"));
    }
    serde_json::from_value(body).map_err(|e| format!("GET /grants malformed response: {e}"))
}

/// The exact `POST /grants` `set_grants` body used by
/// `scripts/corrald-grant.sh`: replacing the full grant set; empty =
/// read-only.
pub fn grant_set_body(key_id: &str, grants: &[String]) -> serde_json::Value {
    serde_json::json!({
        "action": "set_grants",
        "key_id": key_id,
        "grants": grants,
    })
}

/// The exact `POST /grants` `revoke` body used by
/// `scripts/corrald-grant.sh`.
pub fn grant_revoke_body(key_id: &str) -> serde_json::Value {
    grant_revoke_body_with(key_id, true)
}

/// `POST /grants` `revoke` body with an explicit flag: `true` revokes the
/// device (#209's Revoke action), `false` re-grants it (Re-grant action).
pub fn grant_revoke_body_with(key_id: &str, revoked: bool) -> serde_json::Value {
    serde_json::json!({
        "action": "revoke",
        "key_id": key_id,
        "revoked": revoked,
    })
}

/// Replace a registered device's full grant set via the host admin token.
pub async fn set_admin_grants(
    client: &reqwest::Client,
    base_url: &str,
    admin_token: &str,
    key_id: &str,
    grants: &[String],
) -> Result<String, String> {
    post_grant_body(
        client,
        base_url,
        admin_token,
        grant_set_body(key_id, grants),
    )
    .await
}

/// Revoke a registered device (the `--revoke` alternate path) via the host
/// admin token.
pub async fn revoke_admin_device(
    client: &reqwest::Client,
    base_url: &str,
    admin_token: &str,
    key_id: &str,
) -> Result<String, String> {
    set_admin_revoked(client, base_url, admin_token, key_id, true).await
}

/// Set a device's revoked flag via the host admin token (`true` revokes,
/// `false` re-grants — #209's Revoke/Re-grant actions).
pub async fn set_admin_revoked(
    client: &reqwest::Client,
    base_url: &str,
    admin_token: &str,
    key_id: &str,
    revoked: bool,
) -> Result<String, String> {
    post_grant_body(
        client,
        base_url,
        admin_token,
        grant_revoke_body_with(key_id, revoked),
    )
    .await
}

async fn post_grant_body(
    client: &reqwest::Client,
    base_url: &str,
    admin_token: &str,
    body: serde_json::Value,
) -> Result<String, String> {
    let url = format!("{}/grants", base_url.trim_end_matches('/'));
    let response = client
        .post(&url)
        .bearer_auth(admin_token)
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let status = response.status();
    let json: serde_json::Value = response.json().await.map_err(|e| format!("body: {e}"))?;
    if !status.is_success() {
        return Err(grant_error(status.as_u16(), &json, "POST /grants"));
    }
    let ok = json.get("ok").and_then(serde_json::Value::as_bool);
    let key_id = json
        .get("key_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    match (ok, key_id) {
        (Some(true), Some(key_id)) => Ok(key_id),
        _ => Err("POST /grants -> malformed success body".to_string()),
    }
}

fn grant_error(status: u16, body: &serde_json::Value, endpoint: &str) -> String {
    let detail = body
        .get("error")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("request failed");
    format!("{endpoint} -> {status}: {detail}")
}

/// Minimal query-value percent-encoding (agent ids carry `:`; cursors
/// are dot/hex). Everything unreserved passes through; the rest is
/// percent-encoded byte-wise — no new dependency for a URL crate.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Drain one SSE byte stream to the `on_event` callback. Returns
/// `StreamOutcome::Closed` on a clean EOF and `Error` on transport/parse
/// trouble — the caller owns reconnect/backoff. Each read carries a
/// bounded deadline (see [`SSE_CHUNK_READ_TIMEOUT`]); the stream itself
/// has no total lifetime.
///
/// Desktop-only: `reqwest`'s wasm `Response` has no `chunk()` and the
/// timeout needs a tokio timer, neither of which exists in the read-only
/// web build — [`crate::web`] owns its own chunk drain against the same
/// [`SseParser`].
#[cfg(not(target_arch = "wasm32"))]
pub async fn stream_events<F: FnMut(SseEvent)>(
    mut response: reqwest::Response,
    mut on_event: F,
) -> StreamOutcome {
    let mut parser = SseParser::default();
    loop {
        let chunk = match tokio::time::timeout(SSE_CHUNK_READ_TIMEOUT, response.chunk()).await {
            Ok(Ok(Some(chunk))) => chunk,
            Ok(Ok(None)) => break,
            Ok(Err(_)) => return StreamOutcome::Error,
            Err(_) => {
                tracing::warn!(
                    timeout_ms = SSE_CHUNK_READ_TIMEOUT.as_millis(),
                    "SSE chunk read deadline exceeded — reconnecting"
                );
                return StreamOutcome::Error;
            }
        };
        for frame in parser.push(&chunk) {
            on_event(parse_frame(&frame));
        }
    }
    for frame in parser.finish() {
        on_event(parse_frame(&frame));
    }
    StreamOutcome::Closed
}

/// A raw parsed SSE frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RawFrame {
    pub event: String,
    pub id: Option<String>,
    pub data: String,
}

pub(crate) fn parse_frame(frame: &RawFrame) -> SseEvent {
    if frame.data.is_empty() {
        return SseEvent::Unknown {
            event: frame.event.clone(),
            id: frame.id.clone(),
        };
    }
    match frame.event.as_str() {
        "snapshot" => match serde_json::from_str(&frame.data) {
            Ok(snap) => SseEvent::Snapshot(snap),
            Err(e) => SseEvent::Unknown {
                event: format!("snapshot:parse:{e}"),
                id: frame.id.clone(),
            },
        },
        "delta" => match serde_json::from_str(&frame.data) {
            Ok(delta) => SseEvent::Delta(delta),
            Err(e) => SseEvent::Unknown {
                event: format!("delta:parse:{e}"),
                id: frame.id.clone(),
            },
        },
        other => SseEvent::Unknown {
            event: other.to_string(),
            id: frame.id.clone(),
        },
    }
}

/// Incremental SSE parser: line-oriented, comments (`:`) ignored,
/// `data:` lines joined with `\n`, blank line flushes the frame.
#[derive(Debug, Clone, Default)]
pub struct SseParser {
    line_buf: Vec<u8>,
    event: String,
    id: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<RawFrame> {
        let mut frames = Vec::new();
        for &byte in chunk {
            match byte {
                b'\n' => {
                    self.feed_line(&mut frames);
                }
                other => self.line_buf.push(other),
            }
        }
        frames
    }

    pub fn finish(&mut self) -> Vec<RawFrame> {
        let mut frames = Vec::new();
        if !self.line_buf.is_empty() {
            self.feed_line(&mut frames);
        }
        // A trailing blank line flushes the last frame.
        if self.line_buf.is_empty() && self.has_pending() {
            let frame = RawFrame {
                event: std::mem::take(&mut self.event),
                id: self.id.take(),
                data: self.data.join("\n"),
            };
            if !frame.data.is_empty() {
                frames.push(frame);
            }
        }
        frames
    }

    fn has_pending(&self) -> bool {
        !self.event.is_empty() || self.id.is_some() || !self.data.is_empty()
    }

    fn feed_line(&mut self, frames: &mut Vec<RawFrame>) {
        let line = std::mem::take(&mut self.line_buf);
        if line.is_empty() {
            // Blank line: dispatch the frame.
            if self.has_pending() {
                let frame = RawFrame {
                    event: std::mem::take(&mut self.event),
                    id: self.id.take(),
                    data: self.data.join("\n"),
                };
                if !frame.data.is_empty() {
                    frames.push(frame);
                }
                self.data.clear();
            }
            return;
        }
        if line[0] == b':' {
            return; // comment / keepalive
        }
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            return;
        };
        let field = String::from_utf8_lossy(&line[..colon]).to_string();
        // One optional space after the colon is dropped (SSE spec).
        let mut value = String::from_utf8_lossy(&line[colon + 1..]).to_string();
        if value.starts_with(' ') {
            value.remove(0);
        }
        match field.as_str() {
            "event" => self.event = value,
            "id" => self.id = Some(value),
            "data" => self.data.push(value),
            _ => {}
        }
    }
}

/// Spawns the read-path loop task (SSE with resume + backoff, snapshot
/// fallback) on the app's runtime handle. The UI thread is NOT inside the
/// runtime, so plain `tokio::spawn` would panic — `Handle::spawn` is the
/// portable entry point.
///
/// Desktop-only (#215): the web build has no tokio runtime; [`crate::web`]
/// runs the same resume/backoff policy on a wasm local executor.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_read_loop(
    rt: tokio::runtime::Handle,
    client: reqwest::Client,
    base_url: String,
    loop_generation: u64,
    tx_apply: tokio::sync::mpsc::UnboundedSender<ApplyMsg>,
    stop_rx: tokio::sync::watch::Receiver<bool>,
    auto_reconnect: bool,
) {
    rt.spawn(async move {
        let mut last_rev: Option<u64> = None;
        let mut backoff_ms = SSE_BACKOFF_BASE_MS;
        let mut consecutive_errors = 0u32;
        loop {
            if *stop_rx.borrow() {
                return;
            }
            match open_events(&client, &base_url, last_rev).await {
                Ok(response) => {
                    consecutive_errors = 0;
                    backoff_ms = SSE_BACKOFF_BASE_MS;
                    tracing::info!(
                        base_url = %base_url,
                        last_event_id = last_rev,
                        "SSE connected to corrald /events"
                    );
                    let _ = tx_apply.send(ApplyMsg::Conn {
                        loop_generation,
                        event: Live::Connected,
                    });
                    // Track the newest rev seen so the reconnect resumes
                    // from exactly where we stopped (Last-Event-ID).
                    let mut stream_rev = last_rev;
                    let outcome = stream_events(response, &mut |event| {
                        if let SseEvent::Snapshot(snap) = &event {
                            stream_rev = Some(snap.rev);
                        } else if let SseEvent::Delta(delta) = &event {
                            stream_rev = Some(delta.rev);
                        }
                        let _ = tx_apply.send(ApplyMsg::Sse {
                            loop_generation,
                            event,
                        });
                    })
                    .await;
                    last_rev = stream_rev;
                    let _ = tx_apply.send(ApplyMsg::Conn {
                        loop_generation,
                        event: Live::Disconnected,
                    });
                    match outcome {
                        StreamOutcome::Closed => {
                            // Clean close: reconnect per policy.
                        }
                        StreamOutcome::Error => {
                            consecutive_errors += 1;
                        }
                    }
                }
                Err(e) => {
                    consecutive_errors += 1;
                    let _ = tx_apply.send(ApplyMsg::ConnError {
                        loop_generation,
                        error: e,
                    });
                    // Fall back to a one-shot snapshot so the board still
                    // renders while SSE is unavailable (client-side poll
                    // on reconnect only).
                    if consecutive_errors >= 3
                        && let Ok(snap) = fetch_snapshot(&client, &base_url).await
                    {
                        let _ = tx_apply.send(ApplyMsg::Sse {
                            loop_generation,
                            event: SseEvent::Snapshot(snap),
                        });
                    }
                }
            }
            if !auto_reconnect {
                let _ = tx_apply.send(ApplyMsg::ConnError {
                    loop_generation,
                    error: "connection stopped (auto-reconnect is disabled)".to_string(),
                });
                return;
            }
            // Stop if requested (settings changed host or app is quitting).
            if *stop_rx.borrow() {
                return;
            }
            let _ = tx_apply.send(ApplyMsg::Conn {
                loop_generation,
                event: Live::Reconnecting {
                    backoff_ms,
                    rev: last_rev,
                },
            });
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms = (backoff_ms * 2).min(SSE_BACKOFF_MAX_MS);
        }
    });
}

/// Messages from background tasks to the UI state.
#[derive(Debug, Clone)]
pub enum ApplyMsg {
    /// Snapshot/delta data from a particular spawned read loop. The app
    /// drops events from loops that were stopped by a host switch, including
    /// events a stream had already decoded before cancellation.
    Sse {
        loop_generation: u64,
        event: SseEvent,
    },
    /// A lifecycle event from a particular spawned read loop. The app drops
    /// events from loops that were stopped by a host switch.
    Conn {
        loop_generation: u64,
        event: Live,
    },
    ConnError {
        loop_generation: u64,
        error: String,
    },
    /// Host identity resolved (drives device-key scoping + registration).
    Fingerprint(String),
    /// Registration round-trip result: `(key_id, grants)`.
    RegisterResult(Result<(String, Vec<String>), String>),
    /// #113: repo-level issue view arrived from the read-only `GET /issues`
    /// endpoint (keyed by repo/fleet name). The generation is stamped when
    /// the request starts so a response from a prior connection/host cannot
    /// populate the current identity model. Keep failures in the message so
    /// the UI can retry instead of treating a dropped response as "empty".
    Issues {
        generation: u64,
        result: Result<BTreeMap<String, Vec<GhIssueRef>>, String>,
    },
    /// #237: the fleet-ops CLI validated identity catalog arrived from
    /// `GET /fleets`. The generation is stamped when the request starts so
    /// a response from a prior connection/host cannot make stale fleet names
    /// actionable. `Err` is a transport/endpoint failure; daemon fleet-ops
    /// failures ride an `Ok(FleetIdentities)` with `status="error"`.
    FleetIdentities {
        generation: u64,
        result: Result<FleetIdentities, String>,
    },
    /// #137: host-admin device/grants view arrived from `GET /grants`.
    GrantDevices(Result<AdminGrantsView, String>),
    /// #137: a host-admin grant set replacement or device revocation
    /// finished. `grants` are the submitted set (empty for revoke) so the
    /// local board ledger can reflect the change when the selected device is
    /// this board's own key.
    GrantMutation(GrantMutationMsg),
}

/// Completion payload for the Settings grant editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantMutationMsg {
    pub key_id: String,
    pub grants: Vec<String>,
    pub revoke: bool,
    pub result: Result<(), String>,
}

#[derive(Debug, Clone)]
pub enum Live {
    Connected,
    Disconnected,
    Reconnecting { backoff_ms: u64, rev: Option<u64> },
}

/// Record the last id seen in a frame batch (the resume cursor is the
/// rev from snapshot/delta payloads, per the contract; the SSE `id` field
/// echoes the same rev).
#[allow(dead_code)]
pub fn track_last_id(frame: &RawFrame, cursor: &mut Option<u64>) {
    if let Some(id) = &frame.id
        && let Ok(rev) = id.parse::<u64>()
    {
        *cursor = Some(rev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_capabilities_are_stable_canonical_and_complete() {
        assert_eq!(
            GRANT_CAPABILITIES,
            [
                "prompt",
                "interrupt",
                "approve",
                "read_tail",
                "read_diff",
                "read_issues",
                "kill",
                "attach",
                "start_worktree"
            ]
        );
        assert_eq!(
            RECOVERY_GRANT_CAPS,
            [
                "read_tail",
                "read_diff",
                "prompt",
                "interrupt",
                "approve",
                "kill",
                "attach"
            ]
        );
        assert!(
            GRANT_CAPABILITIES
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len()
                == GRANT_CAPABILITIES.len()
        );
    }

    #[test]
    fn issues_decoder_keeps_every_repo_and_issue_in_the_wire_snapshot() {
        let body = serde_json::json!({
            "repos": {
                "corral": [
                    {
                        "repo": "corral",
                        "number": 207,
                        "state": "OPEN",
                        "title": "fetch path",
                        "labels": [{"name": "bug", "color": "f85149"}],
                        "url": "https://github.com/example/corral/issues/207"
                    },
                    {
                        "repo": "corral",
                        "number": 208,
                        "state": "CLOSED",
                        "title": "older issue"
                    }
                ],
                "fleet-ops": [],
                "plush": [{
                    "repo": "plush",
                    "number": 10,
                    "state": "OPEN",
                    "title": "another repo"
                }]
            }
        });

        let issues = decode_issues(body).expect("daemon /issues envelope parses");
        assert_eq!(issues.len(), 3);
        assert_eq!(issues["corral"].len(), 2);
        assert_eq!(issues["corral"][0].number, 207);
        assert_eq!(issues["corral"][1].number, 208);
        assert!(issues["fleet-ops"].is_empty(), "empty repo is retained");
        assert_eq!(issues["plush"][0].title, "another repo");
    }

    #[test]
    fn issues_decoder_preserves_body_for_inline_expansion() {
        let body = serde_json::json!({
            "repos": {
                "corral": [{
                    "repo": "corral",
                    "number": 270,
                    "state": "OPEN",
                    "title": "issues browser",
                    "body": "Body shown when the row expands.",
                    "labels": [],
                    "url": "https://github.com/example/corral/issues/270"
                }]
            }
        });

        let issues = decode_issues(body).expect("body-bearing /issues envelope parses");
        assert_eq!(
            issues["corral"][0].body.as_deref(),
            Some("Body shown when the row expands.")
        );
    }

    #[test]
    fn grant_bodies_match_corrald_grant_script_shape() {
        let set = grant_set_body("dev_abc", &["read_tail".into(), "prompt".into()]);
        assert_eq!(set["action"], "set_grants");
        assert_eq!(set["key_id"], "dev_abc");
        assert_eq!(set["grants"], serde_json::json!(["read_tail", "prompt"]));

        let revoke = grant_revoke_body("dev_abc");
        assert_eq!(revoke["action"], "revoke");
        assert_eq!(revoke["key_id"], "dev_abc");
        assert_eq!(revoke["revoked"], true);
        assert!(revoke.get("grants").is_none());
    }

    #[test]
    fn grant_error_parsing_keeps_the_token_out_of_ui_errors() {
        let body = serde_json::json!({
            "error": "unknown key: dev_x",
            "secret": "admin-token-must-not-leak",
        });
        let error = grant_error(404, &body, "POST /grants");
        assert_eq!(error, "POST /grants -> 404: unknown key: dev_x");
        assert!(!error.contains("admin-token"));
    }

    #[test]
    fn admin_grant_views_reject_missing_required_fields() {
        assert!(
            serde_json::from_value::<GrantDevice>(serde_json::json!({
                "key_id": "dev_x",
                "grants": [],
                "revoked": false,
                "expiry_ts": 1,
                "created_ts": 2,
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<GrantDevice>(serde_json::json!({
                "key_id": "dev_x",
                "revoked": false,
                "expiry_ts": 1,
                "created_ts": 2,
            }))
            .is_err(),
            "a malformed device projection must not silently become no grants"
        );
        assert!(
            serde_json::from_value::<AdminGrantsView>(serde_json::json!({
                "ok": true,
            }))
            .is_err(),
            "missing devices array must fail loudly"
        );
    }

    #[test]
    fn sse_parser_parses_snapshot_delta_and_keepalive() {
        let mut parser = SseParser::default();
        let frames = parser.push(
            b"event: snapshot\nid: 12\ndata: {\"schema_version\":5,\"rev\":12,\"generated_at\":0,\"agents\":{}}\n\n: keepalive\n\nevent: delta\nid: 13\ndata: {\"rev\":13,\"upd\":[],\"del\":[]}\n\n",
        );
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].event, "snapshot");
        assert_eq!(frames[0].id.as_deref(), Some("12"));
        assert!(frames[0].data.contains("\"rev\":12"));
        assert_eq!(frames[1].event, "delta");
        assert!(frames[1].data.contains("\"rev\":13"));
        assert!(matches!(parse_frame(&frames[0]), SseEvent::Snapshot(_)));
        assert!(matches!(parse_frame(&frames[1]), SseEvent::Delta(_)));
    }

    #[test]
    fn sse_parser_joins_multiline_data() {
        let mut parser = SseParser::default();
        let frames = parser.push(b"event: delta\nid: 14\ndata: {\"rev\":14,\n: comment\n\n");
        // No flush yet (blank line missing after the split).
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "{\"rev\":14,");
        // A trailing flush delivers the frame with the joined data.
        let frames = parser.finish();
        assert_eq!(frames.len(), 0);
        let mut parser = SseParser::default();
        let frames =
            parser.push(b"event: delta\nid: 14\ndata: {\"rev\":14,\ndata: \"upd\":[]}\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "{\"rev\":14,\n\"upd\":[]}");
    }

    #[test]
    fn sse_parser_chunk_boundaries_are_safe() {
        let mut parser = SseParser::default();
        let full = b"event: snapshot\nid: 2\ndata: {\"agents\":{}}\n\n";
        let mut frames = Vec::new();
        for &b in full {
            frames.extend(parser.push(&[b]));
        }
        frames.extend(parser.finish());
        assert_eq!(frames.len(), 1);
        assert!(frames[0].data.contains("\"agents\""));
    }

    #[test]
    fn sse_parser_ignores_unknown_fields_and_comments() {
        let mut parser = SseParser::default();
        let frames =
            parser.push(b": keepalive\nevent: snapshot\nid: 5\nretry: 100\ndata: {\"rev\":5}\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event, "snapshot");
        assert_eq!(frames[0].data, "{\"rev\":5}");
    }

    #[test]
    fn empty_data_frames_are_ignored() {
        let mut parser = SseParser::default();
        let frames = parser.push(b"event: ping\n\n");
        assert_eq!(frames.len(), 0);
    }

    #[test]
    fn track_last_id_records_the_sse_id() {
        let mut cursor = None;
        track_last_id(
            &RawFrame {
                event: "delta".into(),
                id: Some("42".into()),
                data: "{}".into(),
            },
            &mut cursor,
        );
        assert_eq!(cursor, Some(42));
    }
}
