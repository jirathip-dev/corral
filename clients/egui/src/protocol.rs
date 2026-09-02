//! Read-path protocol client: `GET /snapshot`, `GET /events` (SSE with
//! `Last-Event-ID` resume), `POST /register` (signed read auth). The signed
//! drive read path lives in [`crate::drive`].
//!
//! #354 read-only cut: the host-admin surfaces (`GET /audit`, grant admin)
//! and the repo-level Issues view were removed with their UIs; this file
//! keeps only the read-model protocol.
//!
//! The SSE reader owns the resume loop: connect → parse events → on any
//! disconnect, back off (doubling, capped) and reconnect carrying the last
//! seen rev as `Last-Event-ID`. The daemon answers a stale cursor with a
//! full snapshot or delta replay (contract: never misses a window). The
//! reader also falls back to a plain `GET /snapshot` when the SSE endpoint
//! is unavailable, so a restart with dropped SSE support still shows live
//! state (client-side polling only — never in the daemon).

use std::time::Duration;

use serde::Deserialize;

use crate::model::{Delta, Snapshot};

pub const DEFAULT_HOST_URL: &str = "http://127.0.0.1:8474";

/// The retained grant vocabulary this client ever requests or renders
/// (#354): the closed READ set. Read-only default registration requests no
/// grants at all; the recovery path re-applies exactly this set when a
/// rebuild/reinstall leaves the board with a fresh key.
pub const READ_GRANT_CAPABILITIES: [&str; 2] = ["read_tail", "read_diff"];

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
/// SSE connection must NOT carry one (the daemon's keepalive comments
/// keep the stream alive; a total timeout severed it every minute).
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

/// `POST /register` with the routing-only registration token and the
/// device's base64 Ed25519 public key. `name` is the optional cosmetic
/// device label. Returns `(key_id, grants)` — the daemon's read-only
/// default grant set is empty; grants are provisioned out-of-band by the
/// host.
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

/// Consume an SSE response body until the stream ends, invoking `on_event`
/// per parsed frame. Native (tokio) path — the wasm build streams with
/// `bytes_stream()` through the same [`SseParser`].
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

    /// #354 RED/GREEN probe — the grant vocabulary this client requests or
    /// renders is the closed READ set. Reintroducing a mutating capability
    /// string fails here AND in the drive-surface probe.
    #[test]
    fn read_grant_capabilities_are_the_closed_read_set() {
        assert_eq!(READ_GRANT_CAPABILITIES, ["read_tail", "read_diff"]);
    }

    #[test]
    fn sse_parser_joins_data_lines_and_ignores_comments() {
        let mut parser = SseParser::default();
        let frames = parser.push(b": keepalive\n\n");
        assert!(frames.is_empty());
        let frames =
            parser.push(b"event: snapshot\nid: 12\ndata: {\"rev\":1}\ndata: {\"rev\":2}\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event, "snapshot");
        assert_eq!(frames[0].id.as_deref(), Some("12"));
        assert_eq!(frames[0].data, "{\"rev\":1}\n{\"rev\":2}");
        assert!(parser.finish().is_empty());
    }

    #[test]
    fn parse_frame_maps_snapshot_delta_and_unknown() {
        let snap = crate::model::Snapshot {
            schema_version: 5,
            rev: 1,
            generated_at: 0,
            agents: Default::default(),
        };
        let frame = RawFrame {
            event: "snapshot".into(),
            id: Some("1".into()),
            data: serde_json::to_string(&snap).unwrap(),
        };
        assert!(matches!(parse_frame(&frame), SseEvent::Snapshot(_)));

        let delta = Delta {
            rev: 2,
            upd: vec![],
            del: vec![],
        };
        let frame = RawFrame {
            event: "delta".into(),
            id: None,
            data: serde_json::to_string(&delta).unwrap(),
        };
        assert!(matches!(parse_frame(&frame), SseEvent::Delta(_)));

        let frame = RawFrame {
            event: "mystery".into(),
            id: None,
            data: "{}".into(),
        };
        assert!(matches!(parse_frame(&frame), SseEvent::Unknown { .. }));
        assert!(matches!(
            parse_frame(&RawFrame {
                event: "snapshot".into(),
                id: None,
                data: "{".into()
            }),
            SseEvent::Unknown { .. }
        ));
    }

    #[test]
    fn parse_frame_treats_empty_data_as_unknown() {
        let frame = RawFrame {
            event: "snapshot".into(),
            id: None,
            data: String::new(),
        };
        assert!(matches!(parse_frame(&frame), SseEvent::Unknown { .. }));
    }
}
