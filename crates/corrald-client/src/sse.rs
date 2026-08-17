//! The SSE read path: `GET /events` with `Last-Event-ID` resume.
//!
//! Wire behavior (frozen daemon, `src/api/mod.rs`):
//!
//! - `event: snapshot` — full state (first frame when the client has no
//!   cursor, a stale cursor, or fell behind the live ring mid-stream
//!   (lag → resnapshot)).
//! - `event: delta` — `{rev, upd, del}` batches; one per coalesced store
//!   change. `id: <rev>` is the monotonic cursor.
//! - No cursor / stale cursor / future cursor → snapshot; covered cursor →
//!   delta replay then live; current cursor → straight to live (nothing is
//!   emitted until the next change).
//!
//! The client reconnects with bounded backoff after any connection failure
//! and re-anchors on the next snapshot; while replaying, it passes the last
//! processed rev as `Last-Event-ID` so the daemon resumes exactly where the
//! client left off (no gap — and deltas are idempotent upserts anyway).

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::{Stream, StreamExt};
use tokio::time::{Sleep, sleep};

use crate::errors::ApiError;
use crate::model::{Delta, Snapshot};

/// One SSE frame, typed. A `Snapshot` resets the client epoch; a `Delta`
/// applies to the current state.
#[derive(Debug, Clone, PartialEq)]
pub enum SseEvent {
    Snapshot(Snapshot),
    Delta(Delta),
}

impl SseEvent {
    /// The event's monotonic rev (the SSE `id:` / the embedded `rev`).
    pub fn rev(&self) -> u64 {
        match self {
            Self::Snapshot(s) => s.rev,
            Self::Delta(d) => d.rev,
        }
    }

    pub fn is_snapshot(&self) -> bool {
        matches!(self, Self::Snapshot(_))
    }

    pub fn as_snapshot(&self) -> Option<&Snapshot> {
        match self {
            Self::Snapshot(s) => Some(s),
            Self::Delta(_) => None,
        }
    }
}

const RECONNECT_BASE: Duration = Duration::from_millis(250);
const RECONNECT_MAX: Duration = Duration::from_secs(30);
/// Per-chunk read deadline. The daemon's keep-alive cadence is 15s
/// (`src/api/mod.rs`), so a silent stream for 2x that means the connection
/// is stalled or dead — reconnect instead of hanging forever (F3: W2's
/// board must never sit on a permanently silent stream).
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Backoff reset when a connection delivered at least one frame.
#[derive(Debug, Clone, Copy)]
struct Backoff {
    base: Duration,
    max: Duration,
    current: Duration,
}

impl Backoff {
    fn new() -> Self {
        Self {
            base: RECONNECT_BASE,
            max: RECONNECT_MAX,
            current: Duration::ZERO,
        }
    }

    /// The delay before the next connection attempt, doubling up to max.
    fn next(&mut self) -> Duration {
        self.current = if self.current.is_zero() {
            self.base
        } else {
            (self.current * 2).min(self.max)
        };
        self.current
    }

    fn reset(&mut self) {
        self.current = Duration::ZERO;
    }
}

/// The streamed HTTP body of an `event-stream` response.
type BodyStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>;

/// In-flight state of the reconnecting event stream.
struct SseState {
    url: String,
    http: reqwest::Client,
    /// The last processed rev — sent as `Last-Event-ID` on reconnect.
    last_rev: Option<u64>,
    /// Active response body, when connected.
    body: Option<BodyStream>,
    /// Sleep before the next (re)connect attempt.
    delay: Option<Pin<Box<Sleep>>>,
    backoff: Backoff,
    /// Per-chunk read deadline (see [`READ_TIMEOUT`]).
    read_timeout: Duration,
    /// Partial-frame parsing state.
    event_name: Option<String>,
    event_id: Option<u64>,
    data_lines: Vec<String>,
    line_buf: Vec<u8>,
}

/// A reconnecting SSE stream of typed snapshot/delta events. Never
/// terminates on its own: connection failures are surfaced as `Err` items
/// and the stream retries with bounded backoff. Drop to stop.
pub struct SseStream {
    inner: Pin<Box<dyn Stream<Item = Result<SseEvent, ApiError>> + Send>>,
}

impl SseStream {
    pub(crate) fn new(client: reqwest::Client, url: String, last_rev: Option<u64>) -> Self {
        let state = SseState {
            url,
            http: client,
            last_rev,
            body: None,
            delay: None,
            backoff: Backoff::new(),
            read_timeout: READ_TIMEOUT,
            event_name: None,
            event_id: None,
            data_lines: Vec::new(),
            line_buf: Vec::new(),
        };
        Self {
            inner: Box::pin(futures::stream::unfold(state, poll_state)),
        }
    }
}

impl Stream for SseStream {
    type Item = Result<SseEvent, ApiError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl std::fmt::Debug for SseStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SseStream").finish_non_exhaustive()
    }
}

/// One poll-cycle of the unfold state machine. Returns when an item is
/// ready (event or connection error); the connection/reconnect bookkeeping
/// happens in between.
async fn poll_state(mut state: SseState) -> Option<(Result<SseEvent, ApiError>, SseState)> {
    loop {
        // Ensure an active connection. Connect + response headers are under
        // the same read deadline as the body chunks: a server that accepts
        // but never answers must not hang the stream's first poll forever.
        if state.body.is_none() {
            if let Some(delay) = state.delay.take() {
                delay.await;
            }
            let url = state.url.clone();
            let http = state.http.clone();
            let last_rev = state.last_rev;
            let read_timeout = state.read_timeout;
            match tokio::time::timeout(read_timeout, open(&http, &url, last_rev)).await {
                Ok(Ok(body)) => state.body = Some(body),
                Ok(Err(e)) => {
                    let delay = state.backoff.next();
                    state.delay = Some(Box::pin(sleep(delay)));
                    return Some((Err(e), state));
                }
                Err(_elapsed) => {
                    let delay = state.backoff.next();
                    state.delay = Some(Box::pin(sleep(delay)));
                    return Some((
                        Err(ApiError::Plain {
                            status: reqwest::StatusCode::OK,
                            error: format!("event stream connect timed out after {read_timeout:?}"),
                        }),
                        state,
                    ));
                }
            }
        }

        let body = state.body.as_mut().expect("body set above");
        let read_timeout = state.read_timeout;
        let chunk = match tokio::time::timeout(read_timeout, body.next()).await {
            Ok(Some(Ok(chunk))) => chunk,
            Ok(Some(Err(e))) => {
                // Mid-stream transport failure: reconnect from the last
                // processed rev (the daemon's resume covers any gap).
                state.body = None;
                let delay = state.backoff.next();
                state.delay = Some(Box::pin(sleep(delay)));
                return Some((Err(ApiError::Transport(e)), state));
            }
            Ok(None) => {
                // Clean EOF (daemon closed the stream): reconnect.
                state.body = None;
                let delay = state.backoff.next();
                state.delay = Some(Box::pin(sleep(delay)));
                return Some((
                    Err(ApiError::Plain {
                        status: reqwest::StatusCode::OK,
                        error: "event stream closed by the daemon".to_string(),
                    }),
                    state,
                ));
            }
            Err(_elapsed) => {
                // Stalled connection: no data for 2x the daemon's keep-alive
                // cadence. Drop the connection and reconnect from the last
                // processed rev; the daemon's resume covers the gap.
                state.body = None;
                let delay = state.backoff.next();
                state.delay = Some(Box::pin(sleep(delay)));
                return Some((
                    Err(ApiError::Plain {
                        status: reqwest::StatusCode::OK,
                        error: format!("event stream read timed out after {READ_TIMEOUT:?}"),
                    }),
                    state,
                ));
            }
        };

        // Feed the line parser; a complete frame yields an event.
        state.line_buf.extend_from_slice(&chunk);
        while let Some(line) = take_line(&mut state.line_buf) {
            if let Some(event) = feed_line(&mut state, line) {
                state.backoff.reset();
                return Some((event, state));
            }
        }
    }
}

/// Extract one complete line (strip `\r\n`/`\n`); `None` when no newline
/// has arrived yet.
fn take_line(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    let newline = buf.iter().position(|b| *b == b'\n')?;
    let mut line: Vec<u8> = buf.drain(..=newline).collect();
    line.pop(); // '\n'
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Some(line)
}

/// Feed one line into the SSE frame state machine (RFC 8895 field rules:
/// `event:`, `id:`, `data:`, comments, blank-line dispatch). Returns the
/// completed frame, if any.
fn feed_line(state: &mut SseState, line: Vec<u8>) -> Option<Result<SseEvent, ApiError>> {
    let line = String::from_utf8_lossy(&line);
    if line.is_empty() {
        return dispatch_frame(state);
    }
    if line.starts_with(':') {
        return None; // comment / keep-alive
    }
    let (field, value) = match line.split_once(':') {
        Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
        None => return None,
    };
    match field {
        "event" => state.event_name = Some(value.to_string()),
        "id" => {
            state.event_id = value.parse::<u64>().ok();
        }
        "data" => state.data_lines.push(value.to_string()),
        _ => {}
    }
    None
}

/// Dispatch a completed frame (blank line seen). Frames without `event`
/// and `data` are keep-alives and are skipped. Unknown event kinds are
/// skipped (additive tolerance). A decode failure of a known kind is
/// surfaced as an error item — the connection stays up.
fn dispatch_frame(state: &mut SseState) -> Option<Result<SseEvent, ApiError>> {
    let event_name = state.event_name.take();
    let data = std::mem::take(&mut state.data_lines).join("\n");
    let id = state.event_id.take();
    if data.is_empty() {
        return None; // keep-alive frame
    }
    let event = match event_name.as_deref() {
        Some("snapshot") => match serde_json::from_str::<Snapshot>(&data) {
            Ok(snap) => SseEvent::Snapshot(snap),
            Err(e) => return Some(Err(ApiError::Decode(e))),
        },
        Some("delta") => match serde_json::from_str::<Delta>(&data) {
            Ok(delta) => SseEvent::Delta(delta),
            Err(e) => return Some(Err(ApiError::Decode(e))),
        },
        _ => return None, // unknown event kind: skip
    };
    state.last_rev = Some(id.unwrap_or_else(|| event.rev()));
    Some(Ok(event))
}

async fn open(
    http: &reqwest::Client,
    url: &str,
    last_rev: Option<u64>,
) -> Result<BodyStream, ApiError> {
    let mut request = http
        .get(url)
        .header(reqwest::header::ACCEPT, "text/event-stream");
    if let Some(rev) = last_rev {
        request = request.header("Last-Event-ID", rev.to_string());
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(ApiError::Plain {
            status: response.status(),
            error: "event stream not available".to_string(),
        });
    }
    Ok(Box::pin(response.bytes_stream()))
}

/// Connect once and wait for the first event (snapshot or delta) with a
/// timeout. Test/consumer convenience; the full reconnecting behavior is
/// on [`SseStream`].
pub async fn first_event(
    client: reqwest::Client,
    url: String,
    last_rev: Option<u64>,
    timeout: Duration,
) -> Result<SseEvent, ApiError> {
    let mut stream = SseStream::new(client, url, last_rev);
    match tokio::time::timeout(timeout, stream.next()).await {
        Ok(Some(Ok(event))) => Ok(event),
        Ok(Some(Err(e))) => Err(e),
        Ok(None) => Err(ApiError::Plain {
            status: reqwest::StatusCode::OK,
            error: "event stream ended before the first event".to_string(),
        }),
        Err(_) => Err(ApiError::Plain {
            status: reqwest::StatusCode::OK,
            error: format!("timed out waiting for the first event ({timeout:?})"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F3 regression: a connection that accepts and then stalls (no data,
    /// no keep-alive) must time out and drop the connection instead of
    /// hanging forever — with an injected short deadline so the test is
    /// fast. The daemon's keep-alive is 15s; the default deadline is 30s.
    #[tokio::test]
    async fn stalled_stream_times_out_instead_of_hanging() {
        // Server accepts and holds the connection open without ever
        // answering (not even response headers).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _held_open = stream; // keep the socket alive, never write
            tokio::time::sleep(Duration::from_secs(60)).await;
        });

        let state = SseState {
            url: format!("http://{addr}/events"),
            http: reqwest::Client::new(),
            last_rev: None,
            body: None,
            delay: None,
            backoff: Backoff::new(),
            read_timeout: Duration::from_millis(100),
            event_name: None,
            event_id: None,
            data_lines: Vec::new(),
            line_buf: Vec::new(),
        };
        let start = tokio::time::Instant::now();
        let (item, next) = poll_state(state).await.expect("one item");
        let Err(ApiError::Plain { error, .. }) = item else {
            panic!("stalled stream must surface a timeout error, got {item:?}");
        };
        assert!(error.contains("timed out"), "{error}");
        assert!(
            next.body.is_none(),
            "stalled connection must be dropped for reconnect"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the read deadline must fire promptly"
        );
    }
}
