//! herdr adapter: JSON-RPC over the herdr unix socket; event-first with a
//! bounded trusted-catalog refresh.
//!
//! Protocol facts (verified against the live socket):
//! - Newline-delimited JSON-RPC; responses carry `id`, pushed events carry
//!   `{"event": <kind>, "data": {...}}`.
//! - A connection that has sent `events.subscribe` becomes push-only: the
//!   server stops answering further requests on it, and idle non-subscribed
//!   connections are closed by the server. Therefore the adapter uses ONE
//!   connection per subscription set: **event** connections each send
//!   exactly one `events.subscribe` (global lifecycle subs + per-pane subs
//!   for all known agent panes on the main stream; one per newly-detected
//!   pane afterwards), while request/response work (agent.list bootstrap,
//!   drive commands) opens a fresh short-lived connection per call.
//! - On subscribe, the server replays current pane state (pane.updated), so
//!   the adapter converges from replay without an extra listing. If bounded
//!   event delivery
//!   overflows, the stream is retired after its pending subscribe response;
//!   a successfully subscribed global stream re-bootstraps the session after
//!   the same capped outage backoff as connect/subscribe failures, while a
//!   pane stream reconnects and re-subscribes with its capped retry delay.
//!   Connection and subscribe failures do not trigger a global re-bootstrap
//!   until their owning retry delay has elapsed.
//!
//! Bootstrap is one `agent.list` call on connect, then a trusted freshness
//! watchdog repeats it while the global stream is open. The short cadence
//! catches a silently-stalled stream whose socket never closes; the interval
//! is injected in tests and the session loop keeps reconciles serialized, so
//! the watchdog cannot overlap a close-triggered reconcile or hot-loop herdr.
//! `pane.output_matched` is the notification trigger (server-side push, not
//! a tail scrape): while herdr reports the agent `blocked`, a matched line
//! becomes the canonical `waiting_on` record.
//! Reconnects with backoff; identity maps and seqs survive reconnects.
//!
//! ## Secret redaction (D9) — at the boundary, before bytes leave the machine
//!
//! Every pane-derived text field (waiting_on prompt/choices, reason, title,
//! display name) passes through [`crate::core::redact::redact`] BEFORE it
//! becomes a canonical record. Everything downstream (snapshot, SSE, drive
//! responses, audit entries) serializes the store, so the output is redacted
//! by construction. Paths and pane ids are identity, never redacted. Repo
//! identity comes from explicit Corral/Herdr roots and branch identity comes
//! from cached git facts; display names and pane labels are never used. The
//! `read_tail` result path applies the same `redact` to the fetched tail
//! text before it leaves the machine ([`Adapter::read_tail`]).
//!
//! ## Drive policy for `unknown` state
//!
//! `AgentState::Unknown` is first-class in the read model (any herdr status
//! outside idle/working/blocked/done maps to it). Drive gating keys off the
//! pane mapping, NOT the state: an Unknown-state agent whose pane is still
//! tracked is drivable (its pane exists — prompt/interrupt/read_tail work),
//! an agent that disappeared is refused with the typed
//! [`DriveError::StaleAgent`], and an agent with no mapping is refused with
//! [`DriveError::UnknownAgent`]. A command never panics on Unknown state.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json, value::RawValue};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot, watch};
use tracing::{debug, info, warn};

use crate::adapters::{Adapter, DriveCommand, DriveError};
use crate::core::blocks::{scrub_tui_furniture, scrub_unsupported_glyphs};
use crate::core::model::{
    Agent, AgentState, Attachment, CAPABILITIES, WaitingOn, WaitingOnKind, Workspace,
};
use crate::core::redact::redact;
use crate::core::store::Store;
use crate::core::util::{canonicalize_existing_prefix, now_millis};
use crate::core::workspace::{WorkspaceAttribution, paths_match};
use crate::drive::{READ_TAIL_MAX_BYTES, READ_TAIL_MAX_LINES};

/// Where herdr exposes its JSON-RPC API socket (expanded from ~ in main).
pub const DEFAULT_SOCKET: &str = "~/.config/herdr/herdr.sock";

/// Regex for lines that look like an agent asking the human something.
/// A secondary signal: only honored while herdr reports the agent blocked.
/// `strip_ansi` is on (herdr default), so plain patterns suffice.
const PROMPT_REGEX: &str = r"(?i)(approve|approval|permission|allow this|confirm|continue\s*\?|proceed\s*\?|do you want|should i|are you sure|is that (ok|okay|fine)|is this (ok|okay|fine)|waiting for|select|choose|\[y/n\]|\(y/n\)|yes/no|please review|need your input|your decision)";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const RECONNECT_BASE: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(30);
const FRAME_CHANNEL_CAP: usize = 1024;
/// A session that survived at least this long proves the server was
/// reachable; the reconnect backoff resets afterwards so steady-state
/// restarts recover quickly instead of staying at 30s forever.
const RECONNECT_RESET_AFTER: Duration = Duration::from_secs(2);
/// Fresh `agent.list` cadence while the global event stream is open. This is
/// the watchdog that keeps the read model live when herdr's socket remains
/// connected but stops delivering events; the interval is short enough to
/// satisfy the catalog freshness target while remaining far below a hot loop.
const CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
/// A listed pane must be seen without `agent_session` this many consecutive
/// trusted catalog refreshes before an explicit session id is demoted to the
/// pane-derived fallback. One refresh can omit the optional field transiently;
/// demoting immediately would re-identify, tombstone, and 409 a live agent.
const SESSIONLESS_DEMOTION_REFRESHES: usize = 2;
/// Pane event streams use a longer initial delay so a single unhealthy pane
/// cannot compete with the global stream for the herdr socket.
const PANE_RETRY_BASE: Duration = Duration::from_secs(2);
/// Delay before a pane task retries after a live stream closes.
const PANE_RESPAWN_DELAY: Duration = Duration::from_secs(5);
/// Tombstones are only needed long enough to classify late drive/event races;
/// they are not an unbounded session history.
const STALE_TOMBSTONE_TTL: Duration = Duration::from_secs(5 * 60);
const STALE_TOMBSTONE_CAP: usize = 1024;

/// Exponential retry schedule shared by event-stream reconnect loops. Keeping
/// this policy as a small value type makes the no-hot-loop guarantee testable
/// without waiting on wall-clock timers.
#[derive(Debug, Clone, Copy)]
struct RetryBackoff {
    next: Duration,
    base: Duration,
    max: Duration,
}

impl RetryBackoff {
    fn new(base: Duration, max: Duration) -> Self {
        assert!(base <= max, "retry backoff base must not exceed its cap");
        Self {
            next: base,
            base,
            max,
        }
    }

    fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = self.next.saturating_mul(2).min(self.max);
        delay
    }

    fn reset(&mut self) {
        self.next = self.base;
    }
}

#[derive(Debug, Clone, Copy)]
struct StreamRetryPolicy {
    base: Duration,
    max: Duration,
    reset_after: Duration,
}

impl StreamRetryPolicy {
    fn production() -> Self {
        Self {
            base: RECONNECT_BASE,
            max: RECONNECT_MAX,
            reset_after: RECONNECT_RESET_AFTER,
        }
    }
}

/// Testable cadence for trusted `agent.list` reconciliation. Production uses
/// [`CATALOG_REFRESH_INTERVAL`]; tests inject a short interval so the
/// silent-stream path is verified without wall-clock sleeps.
#[derive(Debug, Clone, Copy)]
struct CatalogFreshnessPolicy {
    interval: Duration,
}

impl CatalogFreshnessPolicy {
    fn production() -> Self {
        Self {
            interval: CATALOG_REFRESH_INTERVAL,
        }
    }
}

/// Catalog watchdog failures follow the stream outage policy: the first
/// failure is WARNed, repeated ticks stay at debug until a refresh succeeds.
#[derive(Debug, Default)]
struct CatalogRefreshLog {
    failed: bool,
}

#[derive(Debug)]
struct CatalogMigration {
    old_agent_id: String,
    agent_id: String,
    pane_id: String,
    generation: u64,
}

#[derive(Debug)]
struct ReconcilePlan {
    removals: Vec<String>,
    migrations: Vec<String>,
    newly_subscribed: Vec<String>,
    catalog_migrations: Vec<CatalogMigration>,
    live_agent_ids: HashSet<String>,
}

impl CatalogRefreshLog {
    fn failed(&mut self, error: &RpcError) {
        if self.failed {
            debug!(error = %error, "herdr catalog refresh still failing");
            return;
        }
        warn!(
            error = %error,
            "herdr catalog refresh failed; keeping stream and retrying"
        );
        self.failed = true;
    }

    fn recovered(&mut self) {
        if self.failed {
            info!("herdr catalog refresh recovered");
            self.failed = false;
        }
    }
}

/// Stream failures are one outage, not one new warning per retry. The first
/// failure is useful operational evidence; subsequent attempts stay silent at
/// the default log level until the stream recovers.
#[derive(Debug, Default)]
struct SubscriptionFailureLog {
    attempts: u32,
    warned: bool,
}

impl SubscriptionFailureLog {
    fn failed(&mut self, key: &StreamKey, error: &RpcError) {
        self.attempts = self.attempts.saturating_add(1);
        if !self.warned {
            warn!(
                key = ?key,
                attempt = self.attempts,
                error = %error,
                "event stream subscription failed; retrying with backoff"
            );
            self.warned = true;
        }
    }

    fn recovered(&mut self, key: &StreamKey) {
        if self.warned {
            info!(
                key = ?key,
                attempts = self.attempts,
                "event stream subscription recovered"
            );
        }
        self.attempts = 0;
        self.warned = false;
    }

    fn closed(&mut self, key: &StreamKey, stable_for: Duration) {
        self.attempts = self.attempts.saturating_add(1);
        if !self.warned {
            warn!(
                key = ?key,
                attempt = self.attempts,
                stable_for = ?stable_for,
                "event stream closed; retrying with backoff"
            );
            self.warned = true;
        }
    }
}

/// Global stream failures and accepted-then-closed streams share one outage
/// ladder. The state survives the session's delayed re-bootstrap so a stream
/// that accepts and closes repeatedly cannot reset to the base delay on every
/// new subscription.
#[derive(Debug)]
struct GlobalStreamRetry {
    backoff: RetryBackoff,
    failures: SubscriptionFailureLog,
    reset_after: Duration,
}

impl GlobalStreamRetry {
    fn new(policy: StreamRetryPolicy) -> Self {
        Self {
            backoff: RetryBackoff::new(policy.base, policy.max),
            failures: SubscriptionFailureLog::default(),
            reset_after: policy.reset_after,
        }
    }

    fn subscription_failed(&mut self, key: &StreamKey, error: &RpcError) -> Duration {
        self.failures.failed(key, error);
        self.backoff.next_delay()
    }

    fn stream_closed(&mut self, key: &StreamKey, stable_for: Duration) -> Duration {
        if stable_for >= self.reset_after {
            self.backoff.reset();
            self.failures.recovered(key);
        }
        self.failures.closed(key, stable_for);
        self.backoff.next_delay()
    }
}

// ---------------------------------------------------------------------------
// Wire types (tolerant: herdr may add fields; missing fields default)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct AgentSessionWire {
    agent: Option<String>,
    kind: Option<String>,
    source: Option<String>,
    value: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct PaneInfoWire {
    pane_id: String,
    workspace_id: Option<String>,
    cwd: Option<String>,
    foreground_cwd: Option<String>,
    agent: Option<String>,
    agent_status: Option<String>,
    title: Option<String>,
    terminal_title: Option<String>,
    terminal_title_stripped: Option<String>,
    display_agent: Option<String>,
    state_labels: HashMap<String, String>,
    agent_session: Option<AgentSessionWire>,
    state_change_seq: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct AgentInfoWire {
    agent: Option<String>,
    agent_session: Option<AgentSessionWire>,
    agent_status: String,
    cwd: Option<String>,
    foreground_cwd: Option<String>,
    name: Option<String>,
    pane_id: String,
    state_labels: HashMap<String, String>,
    terminal_title: Option<String>,
    terminal_title_stripped: Option<String>,
    title: Option<String>,
    workspace_id: Option<String>,
    state_change_seq: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct StatusChangedWire {
    pane_id: String,
    agent_status: Option<String>,
    agent: Option<String>,
    title: Option<String>,
    state_labels: HashMap<String, String>,
    state_change_seq: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct OutputMatchedWire {
    pane_id: String,
    matched_line: Option<String>,
    read: Option<OutputReadWire>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct OutputReadWire {
    text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct AgentDetectedWire {
    pane_id: String,
    agent: Option<String>,
    released: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct AgentListWire {
    agents: Vec<AgentInfoWire>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct WireEnvelope {
    id: Option<String>,
    error: Option<Box<RawValue>>,
    result: Option<Box<RawValue>>,
    event: Option<String>,
    data: Option<Box<RawValue>>,
}

#[derive(Debug, Deserialize)]
struct WireError {
    code: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct PaneUpdatedData {
    pane: PaneInfoWire,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct PaneLifecycleData {
    pane_id: String,
}

// ---------------------------------------------------------------------------
// RPC framing
// ---------------------------------------------------------------------------

/// A pushed event (responses are resolved inline by the reader).
#[derive(Debug)]
enum ParsedEvent {
    PaneUpdated(Box<PaneInfoWire>),
    StatusChanged(StatusChangedWire),
    OutputMatched(OutputMatchedWire),
    AgentDetected(AgentDetectedWire),
    PaneClosed { pane_id: String },
    PaneCreated,
}

#[derive(Debug)]
struct EventFrame {
    event: ParsedEvent,
}

/// Decode one pushed event exactly once, at the socket boundary. Unknown
/// event kinds are ignored without building a generic JSON tree; known kinds
/// deserialize directly into the wire type the handler needs.
fn decode_event(
    kind: &str,
    data: Option<&RawValue>,
) -> Result<Option<ParsedEvent>, serde_json::Error> {
    let raw = data.map(RawValue::get).unwrap_or("null");
    match kind {
        "pane_updated" => Ok(Some(ParsedEvent::PaneUpdated(Box::new(
            serde_json::from_str::<PaneUpdatedData>(raw)?.pane,
        )))),
        "pane_agent_status_changed" => {
            Ok(Some(ParsedEvent::StatusChanged(serde_json::from_str(raw)?)))
        }
        "pane_output_matched" => Ok(Some(ParsedEvent::OutputMatched(serde_json::from_str(raw)?))),
        "pane_agent_detected" => Ok(Some(ParsedEvent::AgentDetected(serde_json::from_str(raw)?))),
        "pane_closed" | "pane_exited" => Ok(Some(ParsedEvent::PaneClosed {
            pane_id: serde_json::from_str::<PaneLifecycleData>(raw)?.pane_id,
        })),
        "pane_created" => Ok(Some(ParsedEvent::PaneCreated)),
        _ => Ok(None),
    }
}

fn decode_wire_error(raw: &str) -> RpcError {
    match serde_json::from_str::<WireError>(raw) {
        Ok(error) => RpcError::Server {
            code: error.code.unwrap_or_else(|| "error".to_string()),
            message: error.message.unwrap_or_else(|| "unknown error".to_string()),
        },
        Err(_) => RpcError::Server {
            code: "error".to_string(),
            message: "malformed error response".to_string(),
        },
    }
}

fn decode_wire_result(raw: Option<&RawValue>) -> Value {
    raw.map(RawValue::get)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or(Value::Null)
}

#[derive(Debug)]
enum RpcError {
    Server { code: String, message: String },
    Timeout,
    Disconnected,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Server { code, message } => write!(f, "herdr error {code}: {message}"),
            Self::Timeout => write!(f, "herdr request timed out"),
            Self::Disconnected => write!(f, "herdr connection closed"),
        }
    }
}

impl std::error::Error for RpcError {}

/// Pending requests and the terminal state of their reader, guarded as one
/// unit so call registration cannot race reader teardown.
struct PendingState {
    closed: bool,
    calls: HashMap<String, oneshot::Sender<Result<Value, RpcError>>>,
}

type PendingCalls = Arc<Mutex<PendingState>>;

/// JSON-RPC client over a unix socket. One reader task parses
/// newline-delimited frames: responses (with `id`) resolve pending calls
/// inline; pushed events are forwarded to the returned receiver. The
/// reader is the ONLY task that reads the socket, so the client owns it:
/// dropping the client aborts the reader (and with it the read half), so a
/// connection is never left half-open after its caller is done.
struct RpcClient {
    writer: AsyncMutex<OwnedWriteHalf>,
    pending: PendingCalls,
    id_seq: AtomicU64,
    reader: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl RpcClient {
    fn new(stream: UnixStream) -> (Arc<Self>, mpsc::Receiver<EventFrame>) {
        let (read, write) = stream.into_split();
        let (events_tx, events_rx) = mpsc::channel(FRAME_CHANNEL_CAP);
        let pending: PendingCalls = Arc::new(Mutex::new(PendingState {
            closed: false,
            calls: HashMap::new(),
        }));
        let client = Arc::new(Self {
            writer: AsyncMutex::new(write),
            pending: pending.clone(),
            id_seq: AtomicU64::new(0),
            reader: Mutex::new(None),
        });
        let handle = tokio::spawn(Self::reader(read, pending, events_tx));
        *client.reader.lock().unwrap() = Some(handle);
        (client, events_rx)
    }

    async fn reader(read: OwnedReadHalf, pending: PendingCalls, events: mpsc::Sender<EventFrame>) {
        let mut lines = BufReader::new(read).lines();
        let mut overflowed = false;
        loop {
            let line = match lines.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(e) => {
                    warn!(error = %e, "herdr socket read error");
                    break;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let frame: WireEnvelope = match serde_json::from_str(&line) {
                Ok(frame) => frame,
                Err(e) => {
                    warn!(error = %e, "herdr frame parse error");
                    continue;
                }
            };
            if let Some(id) = frame.id.as_deref() {
                if let Some(tx) = pending.lock().unwrap().calls.remove(id) {
                    let result = match frame.error.as_deref() {
                        Some(error) => Err(decode_wire_error(error.get())),
                        None => Ok(decode_wire_result(frame.result.as_deref())),
                    };
                    let _ = tx.send(result);
                }
                if overflowed {
                    break;
                }
                continue;
            }
            if overflowed {
                // The stream is being retired after an overflow. Keep
                // reading long enough to resolve the in-flight subscribe,
                // but never deliver more potentially incomplete state.
                continue;
            }
            let Some(kind) = frame.event.as_deref() else {
                continue;
            };
            let event = match decode_event(kind, frame.data.as_deref()) {
                Ok(Some(event)) => event,
                Ok(None) => continue,
                Err(e) => {
                    warn!(event = kind, error = %e, "herdr event decode failed");
                    continue;
                }
            };
            match events.try_send(EventFrame { event }) {
                Ok(()) => {}
                // #105: a full channel means canonical state may have
                // missed an event. Retire this stream so the session
                // re-bootstraps; do not silently drop live state. If the
                // overflow happened before the subscribe response, keep
                // reading only until that response has been resolved.
                Err(mpsc::error::TrySendError::Full(_)) => {
                    overflowed = true;
                    if pending.lock().unwrap().calls.is_empty() {
                        break;
                    }
                }
                Err(mpsc::error::TrySendError::Closed(_)) => break,
            }
        }
        let calls = {
            let mut state = pending.lock().unwrap();
            state.closed = true;
            std::mem::take(&mut state.calls)
        };
        for (_, tx) in calls {
            let _ = tx.send(Err(RpcError::Disconnected));
        }
    }

    /// Send a request and await its response. Only valid on a connection
    /// that has never called `events.subscribe` (herdr stops answering on
    /// subscribed connections).
    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = format!("corral:{}", self.id_seq.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self.pending.lock().unwrap();
            if state.closed {
                return Err(RpcError::Disconnected);
            }
            state.calls.insert(id.clone(), tx);
        }
        let frame = json!({ "id": id, "method": method, "params": params });
        let mut line = frame.to_string();
        line.push('\n');
        {
            let mut writer = self.writer.lock().await;
            if writer.write_all(line.as_bytes()).await.is_err() {
                self.pending.lock().unwrap().calls.remove(&id);
                return Err(RpcError::Disconnected);
            }
            let _ = writer.flush().await;
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(err))) => Err(err),
            Ok(Err(_)) => Err(RpcError::Disconnected),
            Err(_) => Err(RpcError::Timeout),
        }
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        // Tear down the reader (and with it the read half) so the fd is
        // released the moment the connection's last owner is gone. Without
        // this, a reader blocked on next_line keeps the socket open until
        // herdr closes the idle connection — every failed subscribe and
        // one-shot rpc_call leaks a descriptor during a timeout storm.
        if let Some(reader) = self.reader.lock().unwrap().take() {
            reader.abort();
        }
    }
}

/// Open a fresh connection, make one request/response call, close. Used for
/// bootstrap and drive commands only (both rare) — herdr closes idle API
/// connections and turns subscribed ones push-only, so a persistent control
/// connection is not viable.
async fn rpc_call(
    socket_path: &std::path::Path,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    let stream = UnixStream::connect(socket_path)
        .await
        .map_err(|e| RpcError::Server {
            code: "connect".to_string(),
            message: e.to_string(),
        })?;
    let (client, _events) = RpcClient::new(stream);
    client.call(method, params).await
}

fn is_agent_not_found(code: &str, message: &str) -> bool {
    let normalized_code = code.to_ascii_lowercase().replace('-', "_");
    normalized_code == "agent_not_found"
        || normalized_code == "agent_not_found_error"
        || message.to_ascii_lowercase().contains("agent not found")
        || message.to_ascii_lowercase().contains("agent-not-found")
}

fn is_pane_not_found(code: &str, message: &str) -> bool {
    let normalized_code = code.to_ascii_lowercase().replace('-', "_");
    normalized_code == "pane_not_found"
        || normalized_code == "source_pane_not_found"
        || normalized_code == "target_pane_not_found"
        || message.to_ascii_lowercase().contains("pane not found")
        || message.to_ascii_lowercase().contains("pane-not-found")
}

fn is_missing_drive_target(code: &str, message: &str) -> bool {
    is_agent_not_found(code, message) || is_pane_not_found(code, message)
}

fn map_drive_rpc_error(agent_id: &str, method: &str, error: RpcError) -> DriveError {
    match error {
        RpcError::Server { code, message } if is_missing_drive_target(&code, &message) => {
            DriveError::StaleAgent(agent_id.to_string())
        }
        other => DriveError::Transport(format!("{method} failed: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Session state (survives reconnects)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SessionState {
    /// pane_id -> canonical agent_id
    pane_agents: HashMap<String, String>,
    /// agent_id -> pane_id (drive target resolution)
    agent_panes: HashMap<String, String>,
    /// agent_id -> herdr agent name (drive target when available)
    agent_names: HashMap<String, String>,
    /// Monotonic mapping generation. Every pane/target transition advances
    /// this value, so a late RPC result cannot retire a newer mapping that
    /// happens to resolve to the same wire target.
    agent_generations: HashMap<String, u64>,
    /// Adapter-lifetime allocator. It is intentionally not reset when a
    /// canonical id is retired, so a late RPC can never validate against a
    /// newly created mapping after the live entry is pruned.
    next_generation: u64,
    /// Canonical ids that were tracked and then removed or migrated away.
    /// These tombstones distinguish a stale snapshot from an id the adapter
    /// has never seen.
    stale_agents: HashMap<String, Instant>,
    /// Panes retired by migration/removal. Late status events for one of
    /// these panes must not resurrect a row through replay-order fallback.
    stale_panes: HashMap<String, Instant>,
    /// per-source monotonic ordering
    seqs: HashMap<String, u64>,
    /// herdr's monotonic per-agent state-change sequence. A stale status or
    /// pane.updated event below the last seen value must not overwrite a
    /// fresher `agent.list` state.
    status_seqs: HashMap<String, u64>,
    /// Consecutive successful catalog refreshes that listed each pane without
    /// an explicit session. Cleared by an explicit session, a demotion, or a
    /// pane disappearing from the trusted list.
    catalog_sessionless_refreshes: HashMap<String, usize>,
    /// panes with a dedicated event stream
    subscribed_panes: HashSet<String>,
    /// Cancellation handles for dedicated pane stream tasks. A pane can be
    /// removed and recreated with the same id; replacing the sender prevents
    /// an old retry loop from surviving into the new pane's generation.
    pane_streams: HashMap<String, watch::Sender<bool>>,
}

impl SessionState {
    fn prune_tombstones(&mut self) {
        let now = Instant::now();
        self.stale_agents
            .retain(|_, marked| now.duration_since(*marked) < STALE_TOMBSTONE_TTL);
        self.stale_panes
            .retain(|_, marked| now.duration_since(*marked) < STALE_TOMBSTONE_TTL);

        while self.stale_agents.len() > STALE_TOMBSTONE_CAP {
            let Some(oldest) = self
                .stale_agents
                .iter()
                .min_by_key(|(_, marked)| *marked)
                .map(|(agent_id, _)| agent_id.clone())
            else {
                break;
            };
            self.stale_agents.remove(&oldest);
        }
        while self.stale_panes.len() > STALE_TOMBSTONE_CAP {
            let Some(oldest) = self
                .stale_panes
                .iter()
                .min_by_key(|(_, marked)| *marked)
                .map(|(pane_id, _)| pane_id.clone())
            else {
                break;
            };
            self.stale_panes.remove(&oldest);
        }
    }

    fn mark_stale_agent(&mut self, agent_id: impl Into<String>) {
        let agent_id = agent_id.into();
        self.status_seqs.remove(&agent_id);
        self.stale_agents.insert(agent_id, Instant::now());
        self.prune_tombstones();
    }

    fn mark_stale_pane(&mut self, pane_id: impl Into<String>) {
        self.stale_panes.insert(pane_id.into(), Instant::now());
        self.prune_tombstones();
    }

    fn clear_stale_agent(&mut self, agent_id: &str) {
        self.stale_agents.remove(agent_id);
    }

    fn clear_stale_pane(&mut self, pane_id: &str) {
        self.stale_panes.remove(pane_id);
    }

    fn allocate_generation(&mut self, agent_id: &str) -> u64 {
        let generation = self
            .next_generation
            .checked_add(1)
            .expect("Herdr mapping generation exhausted");
        self.next_generation = generation;
        self.agent_generations
            .insert(agent_id.to_string(), generation);
        generation
    }

    fn clear_generation(&mut self, agent_id: &str) {
        self.agent_generations.remove(agent_id);
    }

    fn mapping_generation(&self, agent_id: &str, pane_id: &str) -> Option<u64> {
        if self.agent_panes.get(agent_id).map(String::as_str) != Some(pane_id)
            || self.pane_agents.get(pane_id).map(String::as_str) != Some(agent_id)
        {
            return None;
        }
        self.agent_generations.get(agent_id).copied()
    }

    fn mapping_matches(&self, agent_id: &str, pane_id: &str, generation: u64) -> bool {
        self.mapping_generation(agent_id, pane_id) == Some(generation)
    }

    fn is_stale_agent(&mut self, agent_id: &str) -> bool {
        self.prune_tombstones();
        self.stale_agents.contains_key(agent_id)
    }

    fn is_stale_pane(&mut self, pane_id: &str) -> bool {
        self.prune_tombstones();
        self.stale_panes.contains_key(pane_id)
    }

    /// Resolve or create the canonical agent_id for a pane. A herdr session
    /// id wins (stable across restarts); otherwise a pane-derived fallback
    /// is reused if one already exists so ids never churn.
    fn resolve_agent_id(&self, pane_id: &str, session_value: Option<&str>) -> String {
        if let Some(v) = session_value.filter(|v| !v.is_empty()) {
            return format!("herdr:{v}");
        }
        self.pane_agents
            .get(pane_id)
            .cloned()
            .unwrap_or_else(|| format!("herdr:pane:{pane_id}"))
    }

    /// Resolve the canonical identity for a trusted `agent.list` entry.
    ///
    /// An explicit session wins immediately and clears the debounce. A listed
    /// pane without a session keeps its previous explicit id on the first such
    /// refresh so one omitted optional field cannot re-identify or tombstone a
    /// live agent. Only after [`SESSIONLESS_DEMOTION_REFRESHES`] consecutive
    /// session-less catalog views is the pane-derived fallback authoritative,
    /// which still evicts a superseded id within a couple refresh cycles.
    fn resolve_catalog_agent_id(&mut self, pane_id: &str, session_value: Option<&str>) -> String {
        if let Some(v) = session_value.filter(|v| !v.is_empty()) {
            self.catalog_sessionless_refreshes.remove(pane_id);
            return format!("herdr:{v}");
        }
        let fallback = format!("herdr:pane:{pane_id}");
        let Some(previous) = self.pane_agents.get(pane_id).cloned() else {
            self.catalog_sessionless_refreshes.remove(pane_id);
            return fallback;
        };
        if previous == fallback {
            self.catalog_sessionless_refreshes.remove(pane_id);
            return fallback;
        }
        let count = self
            .catalog_sessionless_refreshes
            .entry(pane_id.to_string())
            .or_default();
        *count += 1;
        if *count >= SESSIONLESS_DEMOTION_REFRESHES {
            self.catalog_sessionless_refreshes.remove(pane_id);
            fallback
        } else {
            previous
        }
    }

    fn next_seq(&mut self, agent_id: &str) -> u64 {
        let seq = self.seqs.get_mut(agent_id);
        match seq {
            Some(n) => {
                *n += 1;
                *n
            }
            None => {
                self.seqs.insert(agent_id.to_string(), 1);
                1
            }
        }
    }

    /// Apply a status sequence while carrying ordering continuity across a
    /// pane id -> canonical id migration. The pane's previous canonical id
    /// may still hold the latest observed sequence; carry that value to the
    /// resolved id even when the incoming snapshot is stale, so a rebind can
    /// never reset the ordering clock. Missing source sequences stay
    /// compatible with older herdr payloads.
    fn apply_status_seq_transition(
        &mut self,
        pane_id: &str,
        agent_id: &str,
        incoming: Option<u64>,
    ) -> bool {
        let previous_id = self.pane_agents.get(pane_id).cloned();
        let previous_seq = previous_id
            .as_deref()
            .and_then(|previous| self.status_seqs.get(previous))
            .copied();
        let current_seq = self.status_seqs.get(agent_id).copied();
        let high = match (previous_seq, current_seq) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let accepted = incoming.is_none_or(|seq| high.is_none_or(|seen| seq >= seen));
        let effective = match (incoming, high) {
            (Some(seq), Some(seen)) => Some(seq.max(seen)),
            (Some(seq), None) => Some(seq),
            (None, Some(seen)) => Some(seen),
            (None, None) => None,
        };
        if let Some(seq) = effective {
            self.status_seqs.insert(agent_id.to_string(), seq);
        }
        accepted
    }

    /// Cancel a pane's dedicated stream and drop its per-pane membership.
    /// Used when a pane is retired or migrates to a new pane id, so the old
    /// id cannot spawn a second stream while the global stream remains live.
    fn cancel_pane_stream(&mut self, pane_id: &str) {
        self.subscribed_panes.remove(pane_id);
        if let Some(cancel) = self.pane_streams.remove(pane_id) {
            let _ = cancel.send(true);
        }
    }

    /// Cancel every dedicated pane stream before the global stream takes
    /// over their subscriptions on a re-bootstrap. Membership is preserved
    /// for all panes that survived reconciliation; the global subscription
    /// owns per-pane events from then on.
    fn cancel_all_pane_streams(&mut self) {
        for (_, cancel) in self.pane_streams.drain() {
            let _ = cancel.send(true);
        }
    }

    fn retire_pane(&mut self, pane_id: &str, tombstone_agent: bool) -> Option<String> {
        self.prune_tombstones();
        self.cancel_pane_stream(pane_id);
        self.mark_stale_pane(pane_id);
        let agent_id = self.pane_agents.remove(pane_id)?;
        // A pane can send a late close/status event after the same agent has
        // already migrated to a new pane. Do not let that late event remove
        // the new reverse mapping or delete the live store row.
        if self.agent_panes.get(&agent_id).map(String::as_str) != Some(pane_id) {
            return None;
        }
        self.clear_generation(&agent_id);
        self.agent_panes.remove(&agent_id);
        self.agent_names.remove(&agent_id);
        if tombstone_agent {
            self.mark_stale_agent(agent_id.clone());
            Some(agent_id)
        } else {
            None
        }
    }

    fn remove(&mut self, pane_id: &str) -> Option<String> {
        self.retire_pane(pane_id, true)
    }
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

/// Identifies one push-only event stream.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum StreamKey {
    /// Global lifecycle events + per-pane subs for all known panes.
    Global,
    /// Per-pane events for one pane.
    Pane(String),
}

/// Frames from all event streams, funnelled into one sink.
#[derive(Debug)]
enum SinkFrame {
    Event { event: Box<ParsedEvent> },
    Closed { key: StreamKey },
}

#[derive(Debug)]
enum EventStreamExit {
    SubscriptionFailed(RpcError),
    Subscribed(LiveEventStream),
}

/// Keep a stream forwarder tied to its connection attempt. If a pane is
/// removed while `run_event_stream` is waiting, canceling that future must not
/// leave a detached task holding the RPC client and socket open. Once the
/// subscription succeeds, ownership moves to [`LiveEventStream`].
struct AbortOnDrop(Option<tokio::task::JoinHandle<()>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

/// Owns a subscribed event forwarder. Dropping it aborts the forwarder; its
/// client clone then drops and deterministically tears down the RPC reader and
/// socket. Pane retry tasks keep this owner until the stream closes or the
/// pane is canceled.
#[derive(Debug)]
struct LiveEventStream {
    handle: tokio::task::JoinHandle<()>,
    started_at: Instant,
}

impl Drop for LiveEventStream {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub struct HerdrAdapter {
    socket_path: PathBuf,
    /// Canonical Corral/Herdr repo and git-fact view used while building a
    /// fresh agent record. It never derives attribution from pane labels.
    workspace_attribution: WorkspaceAttribution,
    state: Arc<Mutex<SessionState>>,
    /// The read model to retire after a source-level `agent_not_found`.
    /// `start` installs this for the production adapter; tests and direct
    /// hermetic adapter users can attach the same store explicitly.
    store: Arc<Mutex<Option<Store>>>,
    #[cfg(test)]
    store_remove_pause: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    #[cfg(test)]
    event_store_read_pause: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    #[cfg(test)]
    catalog_provider: Mutex<Option<Arc<dyn Fn() -> AgentListWire + Send + Sync>>>,
}

impl std::fmt::Debug for HerdrAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HerdrAdapter")
            .field("socket_path", &self.socket_path)
            .finish_non_exhaustive()
    }
}

impl HerdrAdapter {
    pub fn new(socket_path: PathBuf) -> Self {
        Self::new_with_attribution(
            socket_path,
            WorkspaceAttribution::from_roots(
                std::iter::empty::<crate::core::workspace::RepoRoot>(),
                PathBuf::from("/nonexistent-herdr-worktrees"),
            ),
        )
    }

    pub fn new_with_attribution(
        socket_path: PathBuf,
        workspace_attribution: WorkspaceAttribution,
    ) -> Self {
        Self {
            socket_path,
            workspace_attribution,
            state: Arc::new(Mutex::new(SessionState::default())),
            store: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            store_remove_pause: Mutex::new(None),
            #[cfg(test)]
            event_store_read_pause: Mutex::new(None),
            #[cfg(test)]
            catalog_provider: Mutex::new(None),
        }
    }

    fn attach_store(&self, store: Store) {
        *self.store.lock().unwrap() = Some(store);
    }

    #[cfg(test)]
    fn set_catalog_provider(&self, provider: Arc<dyn Fn() -> AgentListWire + Send + Sync>) {
        *self.catalog_provider.lock().unwrap() = Some(provider);
    }

    #[cfg(test)]
    fn pause_before_store_remove(
        &self,
        reached: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    ) {
        *self.store_remove_pause.lock().unwrap() = Some((reached, release));
    }

    #[cfg(test)]
    fn pause_after_event_store_read(
        &self,
        reached: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    ) {
        *self.event_store_read_pause.lock().unwrap() = Some((reached, release));
    }

    async fn run_forever(&self, store: Store) {
        let mut backoff = RECONNECT_BASE;
        loop {
            let started = Instant::now();
            match self.session(&store).await {
                Ok(()) => info!("herdr connection ended cleanly"),
                Err(e) => warn!(error = %e, "herdr adapter error"),
            }
            // A session that ran for a while (or bootstrapped successfully)
            // proves the server was reachable; reset the backoff so a herdr
            // restart after steady state is recovered at the base delay, not
            // after a 30s crawl.
            if started.elapsed() >= RECONNECT_RESET_AFTER {
                backoff = RECONNECT_BASE;
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(RECONNECT_MAX);
        }
    }

    /// One connect → bootstrap → event streams → sink loop cycle.
    async fn session(&self, store: &Store) -> Result<(), RpcError> {
        self.session_with_policy(store, StreamRetryPolicy::production())
            .await
    }

    async fn session_with_policy(
        &self,
        store: &Store,
        stream_policy: StreamRetryPolicy,
    ) -> Result<(), RpcError> {
        self.session_with_freshness(store, stream_policy, CatalogFreshnessPolicy::production())
            .await
    }

    async fn session_with_freshness(
        &self,
        store: &Store,
        stream_policy: StreamRetryPolicy,
        freshness: CatalogFreshnessPolicy,
    ) -> Result<(), RpcError> {
        info!(socket = %self.socket_path.display(), "connecting to herdr");

        // Bootstrap: trusted agent.list under the same reconcile path used by
        // the freshness watchdog.
        let agents = self.refresh_catalog(store).await?;
        info!(agents, "herdr bootstrap complete");

        // Event sink: one mpsc channel per session; every stream forwarder
        // sends into it, the session loop consumes it.
        let (sink_tx, mut sink_rx) = mpsc::channel(FRAME_CHANNEL_CAP);

        // Main event stream: global subs + per-pane subs for all known panes
        // in ONE events.subscribe request (the only one this connection gets).
        let global_retry = Arc::new(Mutex::new(GlobalStreamRetry::new(stream_policy)));
        self.spawn_event_stream(StreamKey::Global, sink_tx.clone(), global_retry.clone());
        let mut refresh_log = CatalogRefreshLog::default();
        let mut refresh = tokio::time::interval_at(
            tokio::time::Instant::now() + freshness.interval,
            freshness.interval,
        );
        refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                frame = sink_rx.recv() => match frame {
                    Some(SinkFrame::Event { event, .. }) => {
                        self.handle_event(*event, sink_tx.clone(), store).await;
                    }
                    Some(SinkFrame::Closed { key }) => match key {
                        StreamKey::Global => {
                            // Server restarted or stream dropped: re-bootstrap
                            // to reconcile (dropping ghost agents whose panes
                            // closed while the stream was down), then reopen.
                            // Cancel dedicated pane streams first: the global
                            // re-subscription carries their per-pane subs, so
                            // keeping them would double every event.
                            self.state.lock().unwrap().cancel_all_pane_streams();
                            info!("main event stream closed, re-bootstrapping");
                            self.refresh_catalog(store).await?;
                            self.spawn_event_stream(
                                StreamKey::Global,
                                sink_tx.clone(),
                                global_retry.clone(),
                            );
                        }
                        StreamKey::Pane(_) => {
                            // The pane retry task owns its live forwarder and
                            // reconnects after a short delay. The session
                            // must not schedule a second task here: a pane
                            // can disappear and be recreated during that
                            // delay, and a stale delayed task would race the
                            // new generation.
                        }
                    },
                    None => return Err(RpcError::Disconnected),
                },
                _ = refresh.tick() => {
                    // A global socket can remain open and subscribed while
                    // silently withholding every event. Trusted agent.list
                    // reconciliation is the watchdog that keeps membership,
                    // state, and session ids fresh.
                    if let Err(error) = self
                        .refresh_catalog_with_sink(store, Some(&sink_tx))
                        .await
                    {
                        refresh_log.failed(&error);
                    } else {
                        refresh_log.recovered();
                    }
                }
            }
        }
    }

    /// One trusted `agent.list` round trip followed by reconciliation.
    async fn refresh_catalog(&self, store: &Store) -> Result<usize, RpcError> {
        self.refresh_catalog_with_sink(store, None).await
    }

    async fn refresh_catalog_with_sink(
        &self,
        store: &Store,
        pane_stream_sink: Option<&mpsc::Sender<SinkFrame>>,
    ) -> Result<usize, RpcError> {
        #[cfg(test)]
        let provider = self.catalog_provider.lock().unwrap().clone();
        #[cfg(test)]
        if let Some(provider) = provider {
            let list = provider();
            let agents = list.agents.len();
            self.reconcile_against_list_with_streams(&list, store, pane_stream_sink)
                .await;
            debug!(agents, "herdr catalog refreshed from test provider");
            return Ok(agents);
        }
        let list = rpc_call(&self.socket_path, "agent.list", json!({})).await?;
        let list = Self::decode_agent_list(list)?;
        let agents = list.agents.len();
        self.reconcile_against_list_with_streams(&list, store, pane_stream_sink)
            .await;
        debug!(agents, "herdr catalog refreshed");
        Ok(agents)
    }

    fn decode_agent_list(value: Value) -> Result<AgentListWire, RpcError> {
        serde_json::from_value(value).map_err(|e| RpcError::Server {
            code: "decode".to_string(),
            message: e.to_string(),
        })
    }

    /// Reconcile tracked agents against a fresh `agent.list` in one state
    /// transition. The lock covers the pane diff and all new mappings before
    /// store I/O starts, so a stable session moving from pane A to pane B is
    /// never briefly tombstoned or removed from the read model. Panes closed
    /// while a stream was down never emit pane.closed on the new stream, so
    /// truly absent agents are removed after the atomic mapping update.
    #[cfg(test)]
    async fn reconcile_against_list(&self, list: &AgentListWire, store: &Store) {
        self.reconcile_against_list_with_streams(list, store, None)
            .await
    }

    async fn reconcile_against_list_with_streams(
        &self,
        list: &AgentListWire,
        store: &Store,
        pane_stream_sink: Option<&mpsc::Sender<SinkFrame>>,
    ) {
        let mut plan = {
            let mut state = self.state.lock().unwrap();
            state.prune_tombstones();
            let present: Vec<(String, String, Option<u64>)> = list
                .agents
                .iter()
                .map(|agent| {
                    let session = agent
                        .agent_session
                        .as_ref()
                        .and_then(|session| session.value.as_deref());
                    (
                        agent.pane_id.clone(),
                        state.resolve_catalog_agent_id(&agent.pane_id, session),
                        agent.state_change_seq,
                    )
                })
                .collect();
            let present_panes: HashSet<String> = present
                .iter()
                .map(|(pane_id, _, _)| pane_id.clone())
                .collect();
            state
                .catalog_sessionless_refreshes
                .retain(|pane_id, _| present_panes.contains(pane_id));
            let present_agents: HashSet<String> = present
                .iter()
                .map(|(_, agent_id, _)| agent_id.clone())
                .collect();
            let stale: Vec<String> = state
                .pane_agents
                .keys()
                .filter(|pane| !present_panes.contains(*pane))
                .cloned()
                .collect();
            let mut removals = Vec::new();
            for pane_id in stale {
                // A stable session may move between panes while the event
                // stream is down. Retire only the old pane edge; the agent
                // remains live through the present pane mapping.
                let remapped = state
                    .pane_agents
                    .get(&pane_id)
                    .is_some_and(|agent_id| present_agents.contains(agent_id));
                if let Some(agent_id) = state.retire_pane(&pane_id, !remapped) {
                    removals.push(agent_id);
                }
            }

            let mut migrations = Vec::new();
            let mut newly_subscribed = Vec::new();
            let mut migration_records = Vec::new();
            for (pane_id, agent_id, state_change_seq) in &present {
                // A fresh list is authoritative: it may revive a pane whose
                // old event arrived after a prior reconciliation, but only
                // this ordered snapshot path may clear its tombstone.
                state.clear_stale_pane(pane_id);
                let previous_agent_id = state.pane_agents.get(pane_id).cloned();
                state.apply_status_seq_transition(pane_id, agent_id, *state_change_seq);
                if let Some(old) = self.register_pane(
                    &mut state,
                    pane_id,
                    agent_id,
                    list.agents
                        .iter()
                        .find(|agent| agent.pane_id == *pane_id)
                        .and_then(|agent| agent.name.as_deref()),
                ) {
                    if previous_agent_id
                        .as_deref()
                        .is_some_and(|previous| previous != agent_id)
                    {
                        let generation = state
                            .agent_generations
                            .get(agent_id)
                            .copied()
                            .expect("registered Herdr migration has a generation");
                        migration_records.push(CatalogMigration {
                            old_agent_id: old.clone(),
                            agent_id: agent_id.clone(),
                            pane_id: pane_id.clone(),
                            generation,
                        });
                    }
                    migrations.push(old);
                }
                if state.subscribed_panes.insert(pane_id.clone()) {
                    newly_subscribed.push(pane_id.clone());
                }
            }
            ReconcilePlan {
                removals,
                migrations,
                newly_subscribed,
                catalog_migrations: migration_records,
                live_agent_ids: present_agents,
            }
        };
        if let Some(sink) = pane_stream_sink {
            for pane_id in &plan.newly_subscribed {
                spawn_pane_event_stream(
                    self.socket_path.clone(),
                    pane_id.clone(),
                    sink.clone(),
                    self.state.clone(),
                );
            }
        }
        for migration in &plan.catalog_migrations {
            self.migrate_record(
                store,
                &migration.old_agent_id,
                &migration.agent_id,
                &migration.pane_id,
                migration.generation,
            )
            .await;
        }
        plan.removals.extend(plan.migrations);
        for agent_id in plan.removals {
            info!(agent_id, "agent removed: pane absent from fresh agent.list");
            self.remove_if_unmapped(store, &agent_id).await;
        }
        // A superseded session can otherwise stay reachable through
        // `pane_agents` when the trusted list reports the old pane without its
        // session while the replacement appears on another pane. The catalog
        // resolver debounces that optional omission, then this compare against
        // the fresh catalog evicts and tombstones the old id after the
        // corroborating refresh. `remove_if_unmapped` remains the fail-closed
        // guard: a row with a live adapter mapping is never pruned by an
        // incomplete or racing catalog view.
        let catalog_evictions = store
            .matching(|agent| {
                agent.source == "herdr" && !plan.live_agent_ids.contains(&agent.agent_id)
            })
            .await;
        for agent in catalog_evictions {
            if self.remove_if_unmapped(store, &agent.agent_id).await {
                info!(
                    agent_id = %agent.agent_id,
                    "agent removed: session absent from live herdr catalog"
                );
            }
        }
        for agent in &list.agents {
            self.apply_agent_info_if_changed(agent, store).await;
            // `reconcile_against_list` already marked the pane subscribed
            // while holding the same state lock as the mapping transition.
        }
    }

    /// Spawn a push-only event stream. Subscription failures retry in the
    /// stream task with capped exponential backoff, so a herdr outage does not
    /// make the session re-bootstrap in a hot loop. A successfully subscribed
    /// global stream stays owned until its forwarder closes; the same global
    /// backoff delays its close notification so the session can reconcile without
    /// a hot loop. Pane streams keep ownership in their retry task and never need
    /// a separate session-level respawn.
    fn spawn_event_stream(
        &self,
        key: StreamKey,
        sink: mpsc::Sender<SinkFrame>,
        global_retry: Arc<Mutex<GlobalStreamRetry>>,
    ) {
        match key {
            StreamKey::Global => {
                let socket_path = self.socket_path.clone();
                let subs = self.global_subscriptions();
                tokio::spawn(async move {
                    let key = StreamKey::Global;
                    loop {
                        match run_event_stream(socket_path.clone(), subs.clone(), sink.clone())
                            .await
                        {
                            EventStreamExit::SubscriptionFailed(error) => {
                                let delay = global_retry
                                    .lock()
                                    .unwrap()
                                    .subscription_failed(&key, &error);
                                tokio::time::sleep(delay).await;
                            }
                            EventStreamExit::Subscribed(mut live) => {
                                let stable_for = {
                                    let result = (&mut live.handle).await;
                                    if let Err(error) = result {
                                        warn!(error = %error, ?key, "global event stream task ended");
                                    }
                                    live.started_at.elapsed()
                                };
                                let delay =
                                    global_retry.lock().unwrap().stream_closed(&key, stable_for);
                                tokio::time::sleep(delay).await;
                                // Delay the close notification itself. The
                                // session must not agent.list/reconcile until
                                // the same outage backoff has elapsed.
                                let _ = sink.send(SinkFrame::Closed { key: key.clone() }).await;
                                break;
                            }
                        }
                    }
                });
            }
            StreamKey::Pane(pane) => {
                spawn_pane_event_stream(self.socket_path.clone(), pane, sink, self.state.clone());
            }
        }
    }

    fn global_subscriptions(&self) -> Vec<Value> {
        let mut subs: Vec<Value> = vec![
            json!({"type": "pane.created"}),
            json!({"type": "pane.updated"}),
            json!({"type": "pane.closed"}),
            json!({"type": "pane.exited"}),
            json!({"type": "pane.agent_detected"}),
        ];
        let panes: Vec<String> = {
            let state = self.state.lock().unwrap();
            state.pane_agents.keys().cloned().collect()
        };
        for pane in panes {
            subs.extend(pane_subscriptions(&pane));
        }
        subs
    }

    #[cfg(test)]
    async fn apply_agent_info(&self, agent: &AgentInfoWire, store: &Store) {
        let _ = self.apply_agent_info_inner(agent, store, false).await;
    }

    async fn apply_agent_info_if_changed(&self, agent: &AgentInfoWire, store: &Store) -> bool {
        self.apply_agent_info_inner(agent, store, true).await
    }

    async fn apply_agent_info_inner(
        &self,
        agent: &AgentInfoWire,
        store: &Store,
        skip_unchanged: bool,
    ) -> bool {
        let session_value = agent
            .agent_session
            .as_ref()
            .and_then(|s| s.value.as_deref());
        let (agent_id, generation, previous_agent_id, migrated, stale_status, canonical) = {
            let mut state = self.state.lock().unwrap();
            let previous_agent_id = state.pane_agents.get(&agent.pane_id).cloned();
            let agent_id = state.resolve_agent_id(&agent.pane_id, session_value);
            // #210: the trusted catalog saw this agent — stamp the presence
            let stale_status = !state.apply_status_seq_transition(
                &agent.pane_id,
                &agent_id,
                agent.state_change_seq,
            );
            let migrated =
                self.register_pane(&mut state, &agent.pane_id, &agent_id, agent.name.as_deref());
            let canonical = if stale_status {
                None
            } else {
                Some(
                    self.build_agent(
                        &mut state,
                        &agent.pane_id,
                        &agent_id,
                        agent.agent.as_deref(),
                        AgentState::from_herdr_status(&agent.agent_status),
                        agent
                            .terminal_title_stripped
                            .clone()
                            .or_else(|| agent.terminal_title.clone())
                            .or_else(|| agent.title.clone()),
                        agent.foreground_cwd.clone().or_else(|| agent.cwd.clone()),
                        &agent.state_labels,
                        agent.name.clone(),
                    ),
                )
            };
            let generation = state
                .agent_generations
                .get(&agent_id)
                .copied()
                .expect("registered Herdr mapping has a generation");
            (
                agent_id,
                generation,
                previous_agent_id,
                migrated,
                stale_status,
                canonical,
            )
        };
        if let Some(previous) = previous_agent_id
            .as_deref()
            .filter(|previous| *previous != agent_id)
        {
            self.migrate_record(store, previous, &agent_id, &agent.pane_id, generation)
                .await;
        }
        if stale_status {
            if let Some(old) = migrated {
                self.remove_if_unmapped(store, &old).await;
            }
            return false;
        }
        if let Some(old) = migrated {
            self.remove_if_unmapped(store, &old).await;
        }
        let Some(canonical) = canonical else {
            return false;
        };
        let canonical = self.preserve_workspace(store, &agent_id, canonical).await;
        if skip_unchanged
            && store
                .get(&agent_id)
                .await
                .is_some_and(|existing| agent_content_matches(&existing, &canonical))
        {
            return false;
        }
        let persisted = self
            .upsert_if_current(store, canonical, &agent.pane_id, generation)
            .await;
        info!(
            agent_id = %agent_id,
            tool = %agent.agent.as_deref().unwrap_or("unknown"),
            state = ?AgentState::from_herdr_status(&agent.agent_status),
            persisted,
            "agent upsert"
        );
        persisted
    }

    /// Register pane -> agent_id; returns the previous agent_id if it changed
    /// (caller must emit a Remove for it).
    ///
    /// Migration case (F, live smoke): when a pane's id migrates from the
    /// pane-derived fallback to the session id (a pane.updated arrives with
    /// `agent_session` after `agent_detected`), the OLD mapping is removed
    /// AND the new agent_id is inserted — otherwise the drive target
    /// resolution (agent_id -> pane) loses the pane entirely and every drive
    /// on the migrated agent fails `UnknownAgent` until the next bootstrap.
    fn register_pane(
        &self,
        state: &mut SessionState,
        pane_id: &str,
        agent_id: &str,
        agent_name: Option<&str>,
    ) -> Option<String> {
        state.prune_tombstones();

        let previous_agent_for_pane = state.pane_agents.get(pane_id).cloned();
        let previous_pane = state.agent_panes.get(agent_id).cloned();
        let previous_name = state.agent_names.get(agent_id).cloned();
        let mapping_changed = previous_agent_for_pane.as_deref() != Some(agent_id)
            || previous_pane.as_deref() != Some(pane_id)
            || previous_name.as_deref() != agent_name;

        let previous_agent = state
            .pane_agents
            .insert(pane_id.to_string(), agent_id.to_string());
        let mut removed_agent = None;

        // A pane can be rebound from one canonical identity to another (the
        // fallback pane id becoming a stable session id). Remove the old
        // reverse/name entries while the same mutex is still held.
        if previous_agent.as_deref() != Some(agent_id)
            && let Some(old) = previous_agent
            && state.agent_panes.get(&old).map(String::as_str) == Some(pane_id)
        {
            state.agent_panes.remove(&old);
            state.agent_names.remove(&old);
            state.clear_generation(&old);
            state.mark_stale_agent(old.clone());
            removed_agent = Some(old);
        }

        // The same stable session can move from pane A to pane B. The old
        // implementation used `entry().or_insert_with`, which kept pane A as
        // the drive target forever. Evict that old forward edge atomically.
        if let Some(previous_pane) = state
            .agent_panes
            .insert(agent_id.to_string(), pane_id.to_string())
            && previous_pane != pane_id
        {
            if state.pane_agents.get(&previous_pane).map(String::as_str) == Some(agent_id) {
                state.pane_agents.remove(&previous_pane);
            }
            state.cancel_pane_stream(&previous_pane);
            state.mark_stale_pane(previous_pane);
        }

        state.clear_stale_agent(agent_id);
        match agent_name {
            Some(name) => {
                state
                    .agent_names
                    .insert(agent_id.to_string(), name.to_string());
            }
            None => {
                // A migration event without a wire name must not retain the
                // old name. Resolution then falls back to the current pane.
                state.agent_names.remove(agent_id);
            }
        }
        if mapping_changed {
            state.allocate_generation(agent_id);
        }
        removed_agent
    }

    #[allow(clippy::too_many_arguments)]
    fn build_agent(
        &self,
        state: &mut SessionState,
        pane_id: &str,
        agent_id: &str,
        tool: Option<&str>,
        agent_state: AgentState,
        title: Option<String>,
        worktree_path: Option<String>,
        state_labels: &HashMap<String, String>,
        display_name: Option<String>,
    ) -> Agent {
        let seq = state.next_seq(agent_id);
        let workspace_facts = worktree_path
            .as_deref()
            .and_then(|path| self.workspace_attribution.facts_for(Path::new(path)));
        Agent {
            agent_id: agent_id.to_string(),
            source: "herdr".to_string(),
            tool: tool.unwrap_or("unknown").to_string(),
            state: agent_state,
            reason: reason_from_labels(state_labels),
            seq,
            ts: now_millis(),
            capabilities: CAPABILITIES.iter().map(|s| s.to_string()).collect(),
            waiting_on: None,
            parent_id: None,
            host: None,
            workspace: Workspace {
                repo: workspace_facts
                    .as_ref()
                    .and_then(|facts| facts.repo.clone()),
                branch: workspace_facts
                    .as_ref()
                    .filter(|facts| facts.branch_known)
                    .and_then(|facts| facts.branch.clone()),
                worktree_path,
                pr_number: None,
                ..Default::default()
            },
            attachment: Some(Attachment {
                kind: "herdr-pane".to_string(),
                reference: pane_id.to_string(),
            }),
            display_name: display_name.map(|name| redact(&name).into_owned()),
            title: title.map(|t| redact(&t).into_owned()),
        }
    }

    /// Move a store row to a new canonical id while preserving its current
    /// state, `waiting_on`, and plane-merged workspace facts. The adapter
    /// must satisfy the live session-id precedence rule without losing a
    /// blocked agent's approval claim on migration.
    async fn migrate_record(
        &self,
        store: &Store,
        old_agent_id: &str,
        agent_id: &str,
        pane_id: &str,
        generation: u64,
    ) {
        let Some(mut agent) = store.get(old_agent_id).await else {
            return;
        };
        agent.agent_id = agent_id.to_string();
        agent.attachment = Some(Attachment {
            kind: "herdr-pane".to_string(),
            reference: pane_id.to_string(),
        });
        if let Some(waiting_on) = agent.waiting_on.as_mut() {
            waiting_on.approval_id =
                crate::approve::approval_id_for(agent_id, &waiting_on.prompt_hash);
        }
        let seq = self.state.lock().unwrap().next_seq(agent_id);
        agent.seq = seq;
        agent.ts = now_millis();
        info!(agent_id, previous = old_agent_id, "herdr agent id migrated");
        self.upsert_if_current(store, agent, pane_id, generation)
            .await;
    }

    async fn upsert_if_current(
        &self,
        store: &Store,
        agent: Agent,
        pane_id: &str,
        generation: u64,
    ) -> bool {
        let agent_id = agent.agent_id.clone();
        let pane_id = pane_id.to_string();
        // Store -> SessionState is the same lock order used by stale cleanup.
        // Every adapter path releases SessionState before awaiting this write,
        // and the predicate never awaits while either lock is held.
        store
            .upsert_if(agent, || {
                self.state
                    .lock()
                    .unwrap()
                    .mapping_matches(&agent_id, &pane_id, generation)
            })
            .await
    }

    async fn remove_if_unmapped(&self, store: &Store, agent_id: &str) -> bool {
        let agent_id = agent_id.to_string();
        store
            .remove_if(&agent_id, || {
                let mut state = self.state.lock().unwrap();
                if state.agent_panes.contains_key(&agent_id) {
                    return false;
                }
                // Match retire/register removal: an evicted target must keep
                // the refreshable stale-agent tombstone, not fall through to a
                // generic 404 unknown_agent on a later approve/drive.
                state.mark_stale_agent(agent_id.clone());
                true
            })
            .await
    }

    /// Mutate an existing record in the store, bump its seq, and re-apply it
    /// only if the pane mapping generation remains current across the whole
    /// read/modify/write window.
    async fn update_record(
        &self,
        store: &Store,
        agent_id: &str,
        pane_id: &str,
        f: impl FnOnce(&mut Agent),
    ) {
        let Some(generation) = self
            .state
            .lock()
            .unwrap()
            .mapping_generation(agent_id, pane_id)
        else {
            return;
        };
        let Some(mut agent) = store.get(agent_id).await else {
            return;
        };

        #[cfg(test)]
        let event_store_read_pause = self.event_store_read_pause.lock().unwrap().take();
        #[cfg(test)]
        if let Some((reached, release)) = event_store_read_pause {
            let _ = reached.send(());
            let _ = release.await;
        }

        f(&mut agent);
        let seq = self.state.lock().unwrap().next_seq(agent_id);
        agent.seq = seq;
        agent.ts = now_millis();
        self.upsert_if_current(store, agent, pane_id, generation)
            .await;
    }

    /// WS3 F1/#109: herdr owns `worktree_path` only. A full-record rebuild
    /// (agent info, pane.updated, agent_detected) must preserve the
    /// plane-merged workspace fields when the worktree is unchanged, while
    /// allowing the shared canonical resolver to provide a newer primary
    /// repo/branch fact. Canonical path matching keeps a symlink alias from
    /// looking like a worktree change. When the worktree really changes, the
    /// fresh workspace wins and the integrator re-derives facts for the new
    /// path on its next pass.
    async fn preserve_workspace(&self, store: &Store, agent_id: &str, mut agent: Agent) -> Agent {
        let Some(existing) = store.get(agent_id).await else {
            return agent;
        };
        // Herdr's catalog has no `waiting_on`: an unchanged blocked agent from
        // `agent.list` must not erase the derived approval prompt/claim set by
        // `pane.output_matched`. A transition out of Blocked still clears it
        // because the rebuilt row then has `waiting_on: None`.
        if agent.state == AgentState::Blocked && existing.state == AgentState::Blocked {
            agent.waiting_on = existing.waiting_on.clone();
        }
        if existing
            .workspace
            .worktree_path
            .as_deref()
            .zip(agent.workspace.worktree_path.as_deref())
            .is_some_and(|(existing, fresh)| paths_match(Path::new(existing), Path::new(fresh)))
        {
            let ws = &existing.workspace;
            let facts = agent
                .workspace
                .worktree_path
                .as_deref()
                .and_then(|path| self.workspace_attribution.facts_for(Path::new(path)));
            if facts
                .as_ref()
                .and_then(|facts| facts.repo.as_ref())
                .is_none()
            {
                agent.workspace.repo = ws.repo.clone();
            }
            match facts {
                Some(facts) if facts.branch_known => {
                    // A current git fact is authoritative for recognized
                    // paths, including a detached HEAD represented by None.
                    agent.workspace.branch = facts.branch;
                }
                Some(_) => {
                    // The path is recognized, but this generation has not
                    // observed its branch yet. Never restore a stale branch
                    // from the previous stored row during that gap.
                    agent.workspace.branch = None;
                }
                None => {
                    // Unknown paths retain the existing preservation
                    // behavior; they receive no guessed repo or branch.
                    agent.workspace.branch = ws.branch.clone();
                }
            }
            agent.workspace.dirty = ws.dirty;
            agent.workspace.ahead = ws.ahead;
            agent.workspace.behind = ws.behind;
            agent.workspace.pr_number = ws.pr_number;
            agent.workspace.ci_status = ws.ci_status;
            agent.workspace.head_sha = ws.head_sha.clone();
            agent.workspace.head_subject = ws.head_subject.clone();
            agent.workspace.pr_match_source = ws.pr_match_source.clone();
            agent.workspace.issues = ws.issues.clone();
        }
        agent
    }

    async fn handle_event(&self, event: ParsedEvent, sink: mpsc::Sender<SinkFrame>, store: &Store) {
        match event {
            ParsedEvent::PaneUpdated(pane) => {
                self.handle_pane_updated(&pane, sink, store).await;
            }
            ParsedEvent::StatusChanged(ev) => {
                self.handle_status_changed(&ev, store).await;
            }
            ParsedEvent::OutputMatched(ev) => {
                self.handle_output_matched(&ev, store).await;
            }
            ParsedEvent::AgentDetected(ev) => {
                if ev.released.unwrap_or(false) || ev.agent.is_none() {
                    let removed = self.state.lock().unwrap().remove(&ev.pane_id);
                    if let Some(agent_id) = removed {
                        self.remove_if_unmapped(store, &agent_id).await;
                    }
                } else if let Some(tool) = ev.agent {
                    let should_ignore = {
                        let mut state = self.state.lock().unwrap();
                        let known = state.pane_agents.contains_key(&ev.pane_id);
                        state.is_stale_pane(&ev.pane_id) && !known
                    };
                    if should_ignore {
                        // A late replay from a retired pane cannot recreate
                        // the old edge. Only an ordered agent.list
                        // reconciliation may clear this tombstone.
                        return;
                    }
                    let should_spawn = self
                        .state
                        .lock()
                        .unwrap()
                        .subscribed_panes
                        .insert(ev.pane_id.clone());
                    if should_spawn {
                        spawn_pane_event_stream(
                            self.socket_path.clone(),
                            ev.pane_id.clone(),
                            sink.clone(),
                            self.state.clone(),
                        );
                    }
                    self.register_agent_pane(&ev.pane_id, &tool, AgentState::Unknown, store)
                        .await;
                }
            }
            ParsedEvent::PaneClosed { pane_id } => {
                let removed = self.state.lock().unwrap().remove(&pane_id);
                if let Some(agent_id) = removed {
                    self.remove_if_unmapped(store, &agent_id).await;
                }
            }
            ParsedEvent::PaneCreated => {
                // Nothing to do: agent panes announce themselves via
                // pane.agent_detected / pane.updated.
            }
        }
    }

    async fn handle_pane_updated(
        &self,
        pane: &PaneInfoWire,
        sink: mpsc::Sender<SinkFrame>,
        store: &Store,
    ) {
        let session_value = pane.agent_session.as_ref().and_then(|s| s.value.as_deref());
        let (known, ignore_late) = {
            let mut state = self.state.lock().unwrap();
            let known = state.pane_agents.get(&pane.pane_id).cloned();
            let ignore_late = state.is_stale_pane(&pane.pane_id) && known.is_none();
            (known, ignore_late)
        };
        if ignore_late {
            // `pane.updated` replay can arrive after a move/removal. Do not
            // let it clear the retired-pane tombstone or re-create the old
            // edge through register_pane.
            return;
        }
        // Only track panes that have (or had) an agent.
        if pane.agent.is_none() && session_value.is_none() && known.is_none() {
            return;
        }
        let agent_state =
            AgentState::from_herdr_status(pane.agent_status.as_deref().unwrap_or("unknown"));
        let (agent_id, generation, previous_agent_id, migrated, stale_status, canonical) = {
            let mut state = self.state.lock().unwrap();
            let previous_agent_id = state.pane_agents.get(&pane.pane_id).cloned();
            let agent_id = state.resolve_agent_id(&pane.pane_id, session_value);
            let stale_status =
                !state.apply_status_seq_transition(&pane.pane_id, &agent_id, pane.state_change_seq);
            let migrated = self.register_pane(
                &mut state,
                &pane.pane_id,
                &agent_id,
                pane.display_agent.as_deref(),
            );
            let canonical = self.build_agent(
                &mut state,
                &pane.pane_id,
                &agent_id,
                pane.agent.as_deref(),
                agent_state,
                pane.terminal_title_stripped
                    .clone()
                    .or_else(|| pane.terminal_title.clone())
                    .or_else(|| pane.title.clone()),
                pane.foreground_cwd.clone().or_else(|| pane.cwd.clone()),
                &pane.state_labels,
                pane.display_agent.clone(),
            );
            let generation = state
                .agent_generations
                .get(&agent_id)
                .copied()
                .expect("registered Herdr mapping has a generation");
            (
                agent_id,
                generation,
                previous_agent_id,
                migrated,
                stale_status,
                canonical,
            )
        };
        if let Some(previous) = previous_agent_id
            .as_deref()
            .filter(|previous| *previous != agent_id)
        {
            self.migrate_record(store, previous, &agent_id, &pane.pane_id, generation)
                .await;
        }
        if stale_status {
            if let Some(old) = migrated {
                self.remove_if_unmapped(store, &old).await;
            }
            return;
        }
        if let Some(old) = migrated {
            self.remove_if_unmapped(store, &old).await;
        }
        let canonical = self.preserve_workspace(store, &agent_id, canonical).await;
        self.upsert_if_current(store, canonical, &pane.pane_id, generation)
            .await;
        if self
            .state
            .lock()
            .unwrap()
            .subscribed_panes
            .insert(pane.pane_id.clone())
        {
            spawn_pane_event_stream(
                self.socket_path.clone(),
                pane.pane_id.clone(),
                sink,
                self.state.clone(),
            );
        }
    }

    async fn handle_status_changed(&self, ev: &StatusChangedWire, store: &Store) {
        let (known_id, stale_status) = {
            let mut state = self.state.lock().unwrap();
            if state.is_stale_pane(&ev.pane_id) && !state.pane_agents.contains_key(&ev.pane_id) {
                return;
            }
            let known_id = state.pane_agents.get(&ev.pane_id).cloned();
            let stale_status = known_id.as_deref().is_some_and(|agent_id| {
                !state.apply_status_seq_transition(&ev.pane_id, agent_id, ev.state_change_seq)
            });
            (known_id, stale_status)
        };
        if stale_status {
            return;
        };
        let Some(agent_id) = known_id else {
            // Agent pane we never registered: create a record carrying the
            // event's actual status (not Unknown).
            {
                let mut state = self.state.lock().unwrap();
                let agent_id = state.resolve_agent_id(&ev.pane_id, None);
                state.apply_status_seq_transition(&ev.pane_id, &agent_id, ev.state_change_seq);
            };
            self.register_agent_pane(
                &ev.pane_id,
                ev.agent.as_deref().unwrap_or("unknown"),
                AgentState::from_herdr_status(ev.agent_status.as_deref().unwrap_or("unknown")),
                store,
            )
            .await;
            return;
        };
        let agent_state =
            AgentState::from_herdr_status(ev.agent_status.as_deref().unwrap_or("unknown"));
        let title = ev.title.clone().map(|t| redact(&t).into_owned());
        let labels = ev.state_labels.clone();
        self.update_record(store, &agent_id, &ev.pane_id, move |agent| {
            agent.state = agent_state;
            agent.reason = reason_from_labels(&labels);
            if let Some(t) = title {
                agent.title = Some(t);
            }
            if agent_state != AgentState::Blocked {
                agent.waiting_on = None;
            }
        })
        .await;
    }

    async fn handle_output_matched(&self, ev: &OutputMatchedWire, store: &Store) {
        let known_id = {
            let state = self.state.lock().unwrap();
            state.pane_agents.get(&ev.pane_id).cloned()
        };
        let Some(agent_id) = known_id else {
            return;
        };
        let Some(matched) = ev.matched_line.clone() else {
            return;
        };
        let text = ev
            .read
            .as_ref()
            .and_then(|r| r.text.clone())
            .unwrap_or_default();
        let waiting_on = classify_waiting_on(&matched, &text);
        let agent_id_for_claim = agent_id.clone();
        // #330: the structured exchange ledger homed on the shared store —
        // the agent's blocked question is recorded with its authoritative
        // role so the read_tail canonical stream can attribute it. Crash
        // questions are never conversation events.
        let exchange_role = match waiting_on.kind {
            crate::core::model::WaitingOnKind::ApproveTool => {
                Some(crate::core::provenance::ExchangeRole::Tool)
            }
            crate::core::model::WaitingOnKind::Menu
            | crate::core::model::WaitingOnKind::AnswerQuestion => {
                Some(crate::core::provenance::ExchangeRole::Assistant)
            }
            crate::core::model::WaitingOnKind::Crash => None,
        };
        let exchange = store.exchange();
        self.update_record(store, &agent_id, &ev.pane_id, move |agent| {
            if agent.state == AgentState::Blocked {
                let mut waiting_on = waiting_on.clone();
                // P3 D8: emit the live approval claim — the approval_id is
                // the stable identity (agent + exact prompt hash) clients
                // echo back in DrivePayload::Approve. The drive path
                // re-derives it and never trusts this stored copy.
                waiting_on.approval_id =
                    crate::approve::approval_id_for(&agent_id_for_claim, &waiting_on.prompt_hash);
                if let Some(role) = exchange_role {
                    exchange.record(crate::core::provenance::ExchangeEvent::new(
                        &waiting_on.approval_id,
                        &agent_id_for_claim,
                        role,
                        &waiting_on.prompt,
                        now_millis(),
                    ));
                }
                agent.waiting_on = Some(waiting_on);
            }
        })
        .await;
    }

    /// New agent detected in a pane (pane.agent_detected with a tool name).
    /// `agent_state` is the status from the triggering event when known —
    /// a status_changed for an unregistered pane must not read Unknown, or
    /// replay-ordering races make a blocked agent look unknown.
    async fn register_agent_pane(
        &self,
        pane_id: &str,
        tool: &str,
        agent_state: AgentState,
        store: &Store,
    ) {
        let (agent_id, generation, migrated, canonical) = {
            let mut state = self.state.lock().unwrap();
            if state.is_stale_pane(pane_id) && !state.pane_agents.contains_key(pane_id) {
                return;
            }
            let agent_id = state.resolve_agent_id(pane_id, None);
            let migrated = self.register_pane(&mut state, pane_id, &agent_id, None);
            let canonical = self.build_agent(
                &mut state,
                pane_id,
                &agent_id,
                Some(tool),
                agent_state,
                None,
                None,
                &HashMap::new(),
                None,
            );
            let generation = state
                .agent_generations
                .get(&agent_id)
                .copied()
                .expect("registered Herdr mapping has a generation");
            (agent_id, generation, migrated, canonical)
        };
        if let Some(old) = migrated {
            self.remove_if_unmapped(store, &old).await;
        }
        let canonical = self.preserve_workspace(store, &agent_id, canonical).await;
        self.upsert_if_current(store, canonical, pane_id, generation)
            .await;
        info!(pane = pane_id, tool, ?agent_state, "agent detected");
    }
}

/// Compare two canonical rows while ignoring the adapter-local `seq` and
/// display-only `ts`, which are intentionally new on every rebuild. This
/// lets periodic reconciliation stay a true read-model no-op instead of
/// publishing a new rev for refresh traffic.
fn agent_content_matches(existing: &Agent, fresh: &Agent) -> bool {
    let mut existing = existing.clone();
    existing.seq = fresh.seq;
    existing.ts = fresh.ts;
    existing == *fresh
}

/// Spawn a pane event stream with a persistent, capped retry loop. Returns
/// immediately; the retry loop runs in its own task. It remains alive until
/// the pane is removed or the stream succeeds and later closes. A pane that
/// returns after a herdr outage therefore recovers without a new event or a
/// daemon restart.
fn spawn_pane_event_stream(
    socket_path: PathBuf,
    pane_id: String,
    sink: mpsc::Sender<SinkFrame>,
    state: Arc<Mutex<SessionState>>,
) {
    // Reserve the generation before spawning so remove+recreate cannot race
    // an old task that has not reached its first poll yet. `remove` can
    // therefore always cancel the exact generation represented by this call.
    let cancel = {
        let mut state = state.lock().unwrap();
        if !state.subscribed_panes.contains(&pane_id) {
            return;
        }
        let (sender, receiver) = watch::channel(false);
        if let Some(previous) = state.pane_streams.insert(pane_id.clone(), sender) {
            let _ = previous.send(true);
        }
        receiver
    };
    tokio::spawn(async move {
        let mut cancel = cancel;
        let key = StreamKey::Pane(pane_id.clone());
        let subs = pane_subscriptions(&pane_id);
        let mut backoff = RetryBackoff::new(PANE_RETRY_BASE, RECONNECT_MAX);
        let mut failures = SubscriptionFailureLog::default();
        loop {
            if *cancel.borrow() || !pane_is_subscribed(&state, &pane_id) {
                return;
            }
            let outcome = tokio::select! {
                outcome = run_event_stream(
                    socket_path.clone(),
                    subs.clone(),
                    sink.clone(),
                ) => outcome,
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() {
                        return;
                    }
                    continue;
                }
            };
            match outcome {
                EventStreamExit::SubscriptionFailed(error) => {
                    failures.failed(&key, &error);
                    let delay = backoff.next_delay();
                    if sleep_or_cancel(delay, &mut cancel).await {
                        return;
                    }
                }
                EventStreamExit::Subscribed(mut live) => {
                    backoff.reset();
                    failures.recovered(&key);
                    tokio::select! {
                        _ = &mut live.handle => {}
                        changed = cancel.changed() => {
                            if changed.is_err() || *cancel.borrow() {
                                return;
                            }
                        }
                    }
                    if sleep_or_cancel(PANE_RESPAWN_DELAY, &mut cancel).await {
                        return;
                    }
                }
            }
        }
    });
}

fn pane_is_subscribed(state: &Arc<Mutex<SessionState>>, pane_id: &str) -> bool {
    state.lock().unwrap().subscribed_panes.contains(pane_id)
}

async fn sleep_or_cancel(delay: Duration, cancel: &mut watch::Receiver<bool>) -> bool {
    if *cancel.borrow() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        changed = cancel.changed() => changed.is_err() || *cancel.borrow(),
    }
}

/// One event-stream connection: subscribe once, forward pushed events into
/// `sink` until the socket dies. Subscription/connect failures are returned
/// to the caller, which owns the retry schedule and warning policy; the
/// returned live handle tells the caller when an accepted stream closes.
async fn run_event_stream(
    socket_path: PathBuf,
    subs: Vec<Value>,
    sink: mpsc::Sender<SinkFrame>,
) -> EventStreamExit {
    let stream = match UnixStream::connect(&socket_path).await {
        Ok(s) => s,
        Err(e) => {
            return EventStreamExit::SubscriptionFailed(RpcError::Server {
                code: "connect".to_string(),
                message: e.to_string(),
            });
        }
    };
    let (client, mut rx) = RpcClient::new(stream);
    let forwarder_sink = sink.clone();
    let client_for_forwarder = client.clone();
    let mut forwarder = AbortOnDrop(Some(tokio::spawn(async move {
        // Keep the client (and its write half) alive for the whole stream.
        let _client = client_for_forwarder;
        while let Some(EventFrame { event }) = rx.recv().await {
            if forwarder_sink
                .send(SinkFrame::Event {
                    event: Box::new(event),
                })
                .await
                .is_err()
            {
                break;
            }
        }
    })));
    if let Err(e) = client
        .call("events.subscribe", json!({ "subscriptions": subs }))
        .await
    {
        return EventStreamExit::SubscriptionFailed(e);
    }
    // The forwarder owns the subscribed stream after the response arrives.
    // Transfer that ownership to the caller so pane cancellation/replacement
    // can abort the live socket instead of leaving a detached task behind.
    let forwarder = forwarder
        .0
        .take()
        .expect("forwarder guard still owns a live task after subscribe");
    EventStreamExit::Subscribed(LiveEventStream {
        handle: forwarder,
        started_at: Instant::now(),
    })
}

fn pane_subscriptions(pane_id: &str) -> Vec<Value> {
    vec![
        json!({"type": "pane.agent_status_changed", "pane_id": pane_id}),
        json!({
            "type": "pane.output_matched",
            "pane_id": pane_id,
            "source": "recent_unwrapped",
            "lines": 40,
            "match": {"type": "regex", "value": PROMPT_REGEX}
        }),
    ]
}

/// Derive the human reason from herdr's state_labels. HashMap iteration
/// order is arbitrary, so keys are sorted first — a multi-label reason must
/// be deterministic for a given input. Pane-derived text is redacted before
/// it enters the canonical record (D9).
fn reason_from_labels(labels: &HashMap<String, String>) -> Option<String> {
    let mut keys: Vec<&String> = labels.keys().collect();
    keys.sort();
    keys.first().map(|k| {
        let v = &labels[*k];
        let reason = if v.is_empty() {
            (*k).clone()
        } else {
            format!("{k}: {v}")
        };
        redact(&reason).into_owned()
    })
}

/// Classify a matched output line into the canonical waiting_on record.
///
/// The prompt and the choice buffer are pane output: redacted at this
/// boundary (D9) so the stored prompt, its hash, and the serialized output
/// never carry secret-shaped text. The hash covers the redacted prompt —
/// host and client hash the same bytes the client sees.
///
/// P3 D8 (W2): the `prompt_hash` MUST cover the EXACT prompt text herdr
/// emitted — never trimmed — so the claim's hash is byte-identical to what
/// an approve reply must echo. `prompt` is therefore stored untrimmed too
/// (redacted only); the approval claim is derived from it by
/// `crate::approve`.
///
/// F4 (re-review): the kind is classified from the RAW matched line before
/// redaction, so a secret span swallowing the keyword cannot degrade
/// ApproveTool → AnswerQuestion.
///
/// TODO(F3, lands with W2's claim flow): pin the hash contract — add a
/// `REDACT_VERSION` constant and state in drive/mod.rs that the client MUST
/// hash the snapshot `prompt` string byte-for-byte (never the raw pane
/// line), so a future redaction rule change cannot silently break in-flight
/// approvals.
fn classify_waiting_on(matched_line: &str, read_text: &str) -> WaitingOn {
    let raw_prompt = matched_line;
    let lower = raw_prompt.to_lowercase();
    let kind = if ["approve", "approval", "permission", "allow"]
        .iter()
        .any(|k| lower.contains(k))
    {
        WaitingOnKind::ApproveTool
    } else if lower.contains("y/n") || lower.contains("yes/no") {
        WaitingOnKind::Menu
    } else {
        WaitingOnKind::AnswerQuestion
    };
    let prompt = redact(raw_prompt).into_owned();
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    let hash = format!("sha256:{}", hex(&hasher.finalize()));
    WaitingOn {
        kind,
        prompt,
        prompt_hash: hash,
        // The claim identity needs the agent_id, which the classifier does
        // not see; the adapter attaches it when persisting the record
        // (handle_output_matched).
        approval_id: String::new(),
        choices: extract_choices(redact(read_text).as_ref()),
    }
}

/// Lightweight menu detection: "[y/n]" or numbered options in the read
/// buffer. Bounded to 8 choices.
fn extract_choices(read_text: &str) -> Vec<String> {
    let mut choices = Vec::new();
    for line in read_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "[y/n]" || line == "(y/n)" {
            return vec!["y".to_string(), "n".to_string()];
        }
        if let Some(rest) = parse_numbered(line) {
            choices.push(rest);
            if choices.len() >= 8 {
                break;
            }
        }
    }
    choices
}

fn parse_numbered(line: &str) -> Option<String> {
    let mut digits = 0;
    for c in line.chars() {
        if c.is_ascii_digit() {
            digits += 1;
        } else if digits > 0 && (c == '.' || c == ')') {
            return Some(line[digits + 1..].trim().to_string());
        } else {
            return None;
        }
    }
    None
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Adapter trait impl (read path lives in core/api; drive path here)
// ---------------------------------------------------------------------------

impl Adapter for HerdrAdapter {
    fn source(&self) -> &'static str {
        "herdr"
    }

    fn start(self: Arc<Self>, store: Store) {
        self.attach_store(store.clone());
        tokio::spawn(async move { self.run_forever(store).await });
    }

    fn drive<'a>(
        &'a self,
        agent_id: &'a str,
        command: DriveCommand,
    ) -> futures::future::BoxFuture<'a, Result<(), DriveError>> {
        // read_tail is the one capability whose whole point is a response;
        // the API routes it through Adapter::read_tail. Refusing it here
        // keeps a discarded-response fallback impossible.
        if matches!(&command, DriveCommand::ReadTail { .. }) {
            return Box::pin(async { Err(DriveError::NotImplemented("read_tail")) });
        }
        let is_kill = matches!(&command, DriveCommand::Kill);
        let (target, pane_id, generation) = match self.drive_mapping_with_generation(agent_id) {
            Ok(mapping) => mapping,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let (method, params) = match command {
            DriveCommand::Prompt { text } => (
                "agent.prompt",
                json!({"target": target.clone(), "text": text}),
            ),
            DriveCommand::Interrupt => (
                "agent.send_keys",
                json!({"target": target.clone(), "keys": ["ctrl-c"]}),
            ),
            // read_tail is the one capability whose whole point is a
            // response — refused above, before target resolution.
            DriveCommand::ReadTail { .. } => unreachable!(),
            // Approve mechanism (P3 D8, verified live against herdr 0.7.5):
            // herdr exposes NO approve-specific RPC (`herdr api schema` lists
            // agent.prompt / agent.send_keys / pane.send_text / pane.send_input
            // — nothing approve-shaped), and the pane's approve IS an input
            // send. `agent.prompt` is herdr's input-send to the agent session
            // (the same call DriveCommand::Prompt uses); a blocked agent
            // receives the choice text, exactly as if the human had typed it.
            // Live-verified: an opencode agent blocked on a y/n menu executed
            // the choice submitted this way.
            DriveCommand::Approve { choice } => (
                "agent.prompt",
                json!({"target": target.clone(), "text": choice}),
            ),
            // Kill closes the mapped pane. Herdr identifies panes only by
            // pane_id, never by the user-facing agent target, so the RPC
            // carries the current reverse-mapped pane.
            DriveCommand::Kill => ("pane.close", json!({"pane_id": pane_id})),
            // Attach is response-bearing and must be routed through
            // Adapter::attach; this command handle has no result channel.
            DriveCommand::Attach => {
                return Box::pin(async { Err(DriveError::NotImplemented("attach")) });
            }
            // read_diff is likewise response-bearing: never dispatched
            // through the command path (the API routes it via
            // Adapter::read_diff).
            DriveCommand::ReadDiff { .. } => {
                return Box::pin(async { Err(DriveError::NotImplemented("read_diff")) });
            }
        };
        let agent_id = agent_id.to_string();
        let socket = self.socket_path.clone();
        let failed_target = target;
        Box::pin(async move {
            match rpc_call(&socket, method, params).await {
                Ok(_) if is_kill => {
                    if self
                        .retire_rpc_mapping(&agent_id, &failed_target, generation)
                        .await
                    {
                        Ok(())
                    } else {
                        Err(DriveError::StaleAgent(agent_id))
                    }
                }
                Ok(_) => Ok(()),
                Err(error) => {
                    let mapped = map_drive_rpc_error(&agent_id, method, error);
                    if matches!(mapped, DriveError::StaleAgent(_)) {
                        self.retire_rpc_mapping(&agent_id, &failed_target, generation)
                            .await;
                    }
                    Err(mapped)
                }
            }
        })
    }

    fn read_tail<'a>(
        &'a self,
        agent_id: &'a str,
        lines: u32,
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, DriveError>> {
        let (target, generation) = match self.drive_target_with_generation(agent_id) {
            Ok(t) => t,
            Err(e) => return Box::pin(async move { Err(e) }),
        };
        // Same source the output_matched subscription uses (D5: only the
        // requested window, never a prefetch).
        let params = json!({
            "target": target.clone(),
            "source": "recent_unwrapped",
            "lines": lines.clamp(1, READ_TAIL_MAX_LINES),
        });
        let socket = self.socket_path.clone();
        let agent_id = agent_id.to_string();
        let failed_target = target;
        Box::pin(async move {
            let response = match rpc_call(&socket, "agent.read", params).await {
                Ok(response) => response,
                Err(error) => {
                    let mapped = map_drive_rpc_error(&agent_id, "agent.read", error);
                    if matches!(mapped, DriveError::StaleAgent(_)) {
                        self.retire_rpc_mapping(&agent_id, &failed_target, generation)
                            .await;
                    }
                    return Err(mapped);
                }
            };
            let text = response
                .get("read")
                .and_then(|read| read.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or_default();
            Ok(bounded_redacted_tail(text, lines))
        })
    }

    fn read_tail_since<'a>(
        &'a self,
        agent_id: &'a str,
        lines: u32,
        since_rev: Option<u64>,
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, DriveError>> {
        let (target, generation) = match self.drive_target_with_generation(agent_id) {
            Ok(t) => t,
            Err(e) => return Box::pin(async move { Err(e) }),
        };
        let mut params = json!({
            "target": target.clone(),
            "source": "recent_unwrapped",
            "lines": lines.clamp(1, READ_TAIL_MAX_LINES),
        });
        if let Some(rev) = since_rev {
            params["rev"] = json!(rev);
        }
        let socket = self.socket_path.clone();
        let agent_id = agent_id.to_string();
        let failed_target = target;
        Box::pin(async move {
            let response = match rpc_call(&socket, "agent.read", params).await {
                Ok(response) => response,
                Err(error) => {
                    let mapped = map_drive_rpc_error(&agent_id, "agent.read", error);
                    if matches!(mapped, DriveError::StaleAgent(_)) {
                        self.retire_rpc_mapping(&agent_id, &failed_target, generation)
                            .await;
                    }
                    return Err(mapped);
                }
            };
            let text = response
                .get("read")
                .and_then(|read| read.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or_default();
            Ok(bounded_redacted_tail(text, lines))
        })
    }

    fn read_diff<'a>(
        &'a self,
        agent_id: &'a str,
        query: crate::drive::ReadDiffQuery,
    ) -> futures::future::BoxFuture<'a, Result<crate::drive::ReadDiffResult, DriveError>> {
        // #232: the worktree path comes from the SNAPSHOT state the herdr
        // adapter itself produced (agent.workspace.worktree_path) — the
        // client only ever supplies the agent_id; the path is NEVER
        // client-chosen. Diff is computed via libgit2; the page lines are
        // redacted (D9) before they leave the machine, like read_tail.
        let store = self.store.lock().unwrap().as_ref().cloned();
        let agent_id = agent_id.to_string();
        Box::pin(async move {
            let Some(store) = store else {
                return Err(DriveError::NotImplemented("read_diff"));
            };
            let Some(agent) = store.get(&agent_id).await else {
                return Err(if self.is_stale_agent(&agent_id) {
                    DriveError::StaleAgent(agent_id)
                } else {
                    DriveError::UnknownAgent(agent_id)
                });
            };
            let Some(worktree) = agent.workspace.worktree_path.as_deref() else {
                return Err(DriveError::NoWorktree(format!(
                    "agent {agent_id} has no herdr worktree path; only herdr-owned worktrees are readable"
                )));
            };
            let path = self.herdr_owned_worktree(worktree).ok_or_else(|| {
                DriveError::NoWorktree(format!(
                    "agent {agent_id} worktree {worktree} is not under the herdr worktrees root"
                ))
            })?;
            let result = crate::core::diff::read_worktree_diff(&path, &query)
                .map_err(|e| DriveError::NoWorktree(format!("agent {agent_id}: {e}")))?;
            let lines = result
                .lines
                .iter()
                .map(|line| redact(line).into_owned())
                .collect();
            Ok(crate::drive::ReadDiffResult { lines, ..result })
        })
    }

    fn attach<'a>(
        &'a self,
        agent_id: &'a str,
    ) -> futures::future::BoxFuture<'a, Result<Value, DriveError>> {
        let (target, pane_id, _) = match self.drive_mapping_with_generation(agent_id) {
            Ok(mapping) => mapping,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        Box::pin(async move {
            Ok(json!({
                "kind": "terminal_ref",
                "target": target,
                "pane_id": pane_id,
                "command": terminal_attach_command(&target),
                "args": ["herdr", "agent", "attach", "--takeover", target],
            }))
        })
    }

    fn knows_agent(&self, agent_id: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .agent_panes
            .contains_key(agent_id)
    }

    fn is_stale_agent(&self, agent_id: &str) -> bool {
        self.state.lock().unwrap().is_stale_agent(agent_id)
    }
}

/// Resolve the drive target for `agent_id`: the pane's current herdr agent
/// name when one is known, else the pane id. `None`-safe: an agent with no
/// current pane mapping is classified as stale when it was previously known,
/// otherwise it is the typed [`DriveError::UnknownAgent`].
impl HerdrAdapter {
    /// #232: canonicalize the agent-record worktree path and return it ONLY
    /// Resolve a herdr-attributed path to a diffable repo root. Herdr owns
    /// BOTH its linked worktrees (paths under the configured worktrees
    /// root) and the primary checkouts its agents run in (paths attributed
    /// by `repo_for` — configured/live primary roots). Anything else
    /// (arbitrary host paths, other repos) is refused: the path comes from
    /// snapshot state only, never from the client.
    fn herdr_owned_worktree(&self, path: &str) -> Option<PathBuf> {
        let canonical = canonicalize_existing_prefix(Path::new(path));
        if self.workspace_attribution.repo_for(&canonical).is_some() {
            return Some(canonical);
        }
        None
    }

    fn drive_mapping_with_generation(
        &self,
        agent_id: &str,
    ) -> Result<(String, String, u64), DriveError> {
        let mut state = self.state.lock().unwrap();
        let Some(pane) = state.agent_panes.get(agent_id).cloned() else {
            return if state.is_stale_agent(agent_id) {
                Err(DriveError::StaleAgent(agent_id.to_string()))
            } else {
                Err(DriveError::UnknownAgent(agent_id.to_string()))
            };
        };
        let target = state
            .agent_names
            .get(agent_id)
            .cloned()
            .unwrap_or_else(|| pane.clone());
        let generation = state.agent_generations.get(agent_id).copied().unwrap_or(0);
        Ok((target, pane, generation))
    }

    fn drive_target_with_generation(&self, agent_id: &str) -> Result<(String, u64), DriveError> {
        self.drive_mapping_with_generation(agent_id)
            .map(|(target, _, generation)| (target, generation))
    }

    #[cfg(test)]
    fn drive_target(&self, agent_id: &str) -> Result<String, DriveError> {
        self.drive_target_with_generation(agent_id)
            .map(|(target, _)| target)
    }

    async fn retire_rpc_mapping(
        &self,
        agent_id: &str,
        failed_target: &str,
        generation: u64,
    ) -> bool {
        // Clone the store handle before any await. State retirement happens
        // before the conditional Store mutation so a same-generation status
        // or integration update cannot make cleanup miss the row.
        let store = self.store.lock().unwrap().clone();
        let retired = {
            let mut state = self.state.lock().unwrap();
            state.prune_tombstones();
            let current_pane = state.agent_panes.get(agent_id).cloned();
            let current_target = current_pane.as_ref().map(|pane| {
                state
                    .agent_names
                    .get(agent_id)
                    .cloned()
                    .unwrap_or_else(|| pane.clone())
            });
            let current_generation = state.agent_generations.get(agent_id).copied();
            let reverse_matches = current_pane.as_ref().is_some_and(|pane| {
                state.pane_agents.get(pane).map(String::as_str) == Some(agent_id)
            });
            if current_generation != Some(generation)
                || current_target.as_deref() != Some(failed_target)
                || !reverse_matches
            {
                false
            } else {
                state
                    .agent_panes
                    .get(agent_id)
                    .cloned()
                    .and_then(|pane| state.retire_pane(&pane, true))
                    .is_some()
            }
        };

        #[cfg(test)]
        let store_remove_pause = self.store_remove_pause.lock().unwrap().take();
        #[cfg(test)]
        if let Some((reached, release)) = store_remove_pause {
            let _ = reached.send(());
            let _ = release.await;
        }

        if retired && let Some(store) = store {
            self.remove_if_unmapped(&store, agent_id).await;
        }
        if retired {
            // A close/reconcile race can register the same stable agent on a
            // fresh pane after the mapped target was retired but before the
            // conditional store removal ran. That is a newer live target, not
            // a successful kill of the agent the caller resolved.
            !self
                .state
                .lock()
                .unwrap()
                .agent_panes
                .contains_key(agent_id)
        } else {
            false
        }
    }
}

/// Human-ready shell command for the attach handle. The target is
/// single-quoted so a client that copies the line into a shell cannot split
/// it into extra arguments; the structured `args` array in the same handle is
/// the parser-safe form.
fn terminal_attach_command(target: &str) -> String {
    format!(
        "herdr agent attach --takeover {}",
        shell_single_quote(target)
    )
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Bound + redact + scrub the fetched tail at the adapter boundary, BEFORE
/// any byte leaves the machine (D9/D5/#253): at most `max_lines` lines
/// (clamped to [`READ_TAIL_MAX_LINES`]), the redacted text bounded to
/// [`READ_TAIL_MAX_BYTES`], every line through the shared redaction pass
/// and then the TUI-furniture scrub (box-drawing borders, progress bars).
fn bounded_redacted_tail(text: &str, max_lines: u32) -> Vec<String> {
    let max_lines = (max_lines as usize).clamp(1, READ_TAIL_MAX_LINES as usize);
    let mut lines: Vec<String> = Vec::new();
    let mut bytes = 0usize;
    for raw in text.lines().take(max_lines) {
        let line = scrub_unsupported_glyphs(&scrub_tui_furniture(&redact(raw)));
        // The wire carries one newline per line; count it so the serialized
        // payload stays under the byte bound too.
        bytes += line.len() + 1;
        if bytes > READ_TAIL_MAX_BYTES {
            break;
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Change;
    use serde_json::json;
    use serde_json::value::RawValue;

    /// Real `agent.list` entry captured from the live herdr socket: a claude
    /// agent with a session id, and an opencode agent without one.
    fn fixture_claude() -> serde_json::Value {
        json!({
            "agent": "claude",
            "agent_session": {
                "agent": "claude",
                "kind": "id",
                "source": "herdr:claude",
                "value": "2d5e5911-b103-4a92-adc3-a8bdc03fd784"
            },
            "agent_status": "idle",
            "cwd": "/Users/jirathip/.herdr/worktrees/project-hearthwild/feat-plush-visual-fidelity",
            "foreground_cwd": "/Users/jirathip/.herdr/worktrees/project-hearthwild/feat-plush-visual-fidelity",
            "focused": false,
            "interactive_ready": true,
            "name": "fix-plush-50",
            "pane_id": "wQ:p1",
            "revision": 59,
            "state_labels": {},
            "state_change_seq": 64,
            "tab_id": "wQ:t1",
            "terminal_id": "term_659133784428a1b",
            "terminal_title": "\u{2733} Fix Blender acceptance gate and run tests",
            "terminal_title_stripped": "Fix Blender acceptance gate and run tests",
            "workspace_id": "wQ"
        })
    }

    fn fixture_opencode_no_session() -> serde_json::Value {
        json!({
            "agent": "opencode",
            "agent_status": "working",
            "cwd": "/Users/jirathip/.herdr/worktrees/herdr-board/corral",
            "foreground_cwd": "/Users/jirathip/.herdr/worktrees/herdr-board/corral",
            "focused": true,
            "interactive_ready": true,
            "name": "corral-p1",
            "pane_id": "w1D:p1",
            "revision": 2,
            "state_labels": {},
            "tab_id": "w1D:t1",
            "terminal_id": "term_65914869fdc9d23",
            "terminal_title": "OC | Corral P1: Rust agent model + snapsho...",
            "terminal_title_stripped": "OC | Corral P1: Rust agent model + snapsho...",
            "workspace_id": "w1D"
        })
    }

    #[test]
    fn retry_backoff_is_exponential_capped_and_resettable() {
        let mut backoff = RetryBackoff::new(Duration::from_millis(10), Duration::from_millis(40));
        assert_eq!(backoff.next_delay(), Duration::from_millis(10));
        assert_eq!(backoff.next_delay(), Duration::from_millis(20));
        assert_eq!(backoff.next_delay(), Duration::from_millis(40));
        assert_eq!(backoff.next_delay(), Duration::from_millis(40));

        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_millis(10));
    }

    #[test]
    fn decode_event_parses_known_pane_payload_directly() {
        let raw: Box<RawValue> = serde_json::from_str(
            r#"{"pane":{"pane_id":"w1:p1","agent":"opencode","agent_status":"working","state_labels":{}}}"#,
        )
        .unwrap();
        let parsed = decode_event("pane_updated", Some(raw.as_ref())).expect("decode");
        let ParsedEvent::PaneUpdated(pane) = parsed.expect("known event") else {
            panic!("expected pane_updated");
        };
        assert_eq!(pane.pane_id, "w1:p1");
        assert_eq!(pane.agent.as_deref(), Some("opencode"));
    }

    #[test]
    fn decode_event_skips_unknown_kind_without_wire_typed_value() {
        let raw: Box<RawValue> =
            serde_json::from_str(r#"{"pane_id":"w1:p1","unused":"large payload"}"#).unwrap();
        assert!(
            decode_event("pane_unknown_kind", Some(raw.as_ref()))
                .expect("unknown kind is not a decode failure")
                .is_none()
        );
    }

    #[tokio::test]
    async fn fresh_agent_list_wire_decodes_and_reconciles_captured_shape() {
        let wire = serde_json::to_string(&json!({
            "agents": [fixture_claude(), fixture_opencode_no_session()]
        }))
        .unwrap();
        let decoded = HerdrAdapter::decode_agent_list(serde_json::from_str(&wire).unwrap())
            .expect("fresh agent.list wire shape decodes");
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        adapter.reconcile_against_list(&decoded, &store).await;

        let snapshot = store.snapshot().await;
        assert_eq!(snapshot.agents.len(), 2);
        assert!(
            snapshot
                .agents
                .contains_key("herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784")
        );
        assert!(snapshot.agents.contains_key("herdr:pane:w1D:p1"));
    }

    #[test]
    fn subscription_failures_warn_once_until_recovery() {
        let key = StreamKey::Pane("w1:p1".to_string());
        let error = RpcError::Timeout;
        let mut failures = SubscriptionFailureLog::default();

        failures.failed(&key, &error);
        assert_eq!(failures.attempts, 1);
        assert!(failures.warned);

        failures.failed(&key, &error);
        assert_eq!(failures.attempts, 2);
        assert!(failures.warned, "retries remain one logged outage");

        failures.recovered(&key);
        assert_eq!(failures.attempts, 0);
        assert!(!failures.warned);
    }

    #[test]
    fn global_closed_streams_share_backoff_and_reset_after_stability() {
        let key = StreamKey::Global;
        let mut retry = GlobalStreamRetry::new(StreamRetryPolicy {
            base: Duration::from_millis(10),
            max: Duration::from_millis(40),
            reset_after: Duration::from_millis(30),
        });

        assert_eq!(
            retry.stream_closed(&key, Duration::from_millis(1)),
            Duration::from_millis(10)
        );
        assert_eq!(
            retry.stream_closed(&key, Duration::from_millis(1)),
            Duration::from_millis(20)
        );
        assert_eq!(
            retry.stream_closed(&key, Duration::from_millis(1)),
            Duration::from_millis(40)
        );
        assert_eq!(retry.failures.attempts, 3);
        assert!(retry.failures.warned, "one outage warning remains active");

        assert_eq!(
            retry.stream_closed(&key, Duration::from_millis(30)),
            Duration::from_millis(10),
            "only a stable stream resets to the base delay"
        );
        assert_eq!(retry.failures.attempts, 1);
        assert!(
            retry.failures.warned,
            "the new closure starts one new outage"
        );
    }

    #[tokio::test]
    async fn normalizes_real_agent_list_entries() {
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));

        let claude: AgentInfoWire = serde_json::from_value(fixture_claude()).unwrap();
        adapter.apply_agent_info(&claude, &store).await;

        let opencode: AgentInfoWire =
            serde_json::from_value(fixture_opencode_no_session()).unwrap();
        adapter.apply_agent_info(&opencode, &store).await;

        let snap = store.snapshot().await;
        assert_eq!(snap.agents.len(), 2);

        // agent_id is the opaque session id — NEVER the pane_id.
        let c = snap
            .agents
            .get("herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784")
            .expect("session-based agent_id");
        assert_eq!(c.tool, "claude");
        assert_eq!(c.source, "herdr");
        assert_eq!(c.state, AgentState::Idle);
        assert_eq!(c.display_name.as_deref(), Some("fix-plush-50"));
        assert_eq!(
            c.workspace.worktree_path.as_deref(),
            Some("/Users/jirathip/.herdr/worktrees/project-hearthwild/feat-plush-visual-fidelity")
        );
        let att = c.attachment.as_ref().expect("attachment");
        assert_eq!(att.kind, "herdr-pane");
        assert_eq!(att.reference, "wQ:p1");
        assert_ne!(c.agent_id, att.reference, "agent_id must not be pane_id");
        assert_eq!(c.capabilities, CAPABILITIES);

        // No session: pane-derived fallback id, reused across events.
        let o = snap
            .agents
            .get("herdr:pane:w1D:p1")
            .expect("pane fallback id");
        assert_eq!(o.tool, "opencode");
        assert_eq!(o.state, AgentState::Working);
        assert!(o.attachment.is_some());

        // Per-source monotonic seqs.
        assert_eq!(c.seq, 1);
        assert_eq!(o.seq, 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn primary_linked_alias_and_unknown_paths_get_only_canonical_facts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let primary = temp.path().join("primary-repo");
        let worktrees = temp.path().join("worktrees");
        let linked = worktrees.join("linked-repo/feature");
        let alias = temp.path().join("primary-alias");
        let unknown = temp.path().join("unknown");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::create_dir_all(&unknown).unwrap();
        std::os::unix::fs::symlink(&primary, &alias).unwrap();

        let attribution = WorkspaceAttribution::from_roots(
            [crate::core::workspace::RepoRoot {
                path: primary.clone(),
                repo: "primary-repo".to_string(),
            }],
            worktrees,
        );
        attribution.record_branch(&primary, "main");
        attribution.record_branch(&linked, "feature/x");
        attribution.record_branch(&unknown, "must-not-infer");

        let store = Store::new();
        let adapter =
            HerdrAdapter::new_with_attribution(PathBuf::from("/nonexistent.sock"), attribution);
        let info = |id: &str, pane_id: &str, path: &Path| {
            serde_json::from_value::<AgentInfoWire>(json!({
                "agent": "opencode",
                "agent_session": {"agent": "opencode", "kind": "id", "value": id},
                "agent_status": "working",
                "foreground_cwd": path,
                "pane_id": pane_id,
                "state_labels": {}
            }))
            .expect("agent fixture")
        };

        adapter
            .apply_agent_info(&info("primary", "p-primary", &primary), &store)
            .await;
        adapter
            .apply_agent_info(&info("linked", "p-linked", &linked), &store)
            .await;
        adapter
            .apply_agent_info(&info("alias", "p-alias", &alias), &store)
            .await;
        adapter
            .apply_agent_info(&info("unknown", "p-unknown", &unknown), &store)
            .await;

        let snapshot = store.snapshot().await;
        assert_eq!(
            snapshot
                .agents
                .get("herdr:primary")
                .and_then(|agent| agent.workspace.repo.as_deref()),
            Some("primary-repo")
        );
        assert_eq!(
            snapshot
                .agents
                .get("herdr:primary")
                .and_then(|agent| agent.workspace.branch.as_deref()),
            Some("main")
        );
        assert_eq!(
            snapshot
                .agents
                .get("herdr:linked")
                .and_then(|agent| agent.workspace.repo.as_deref()),
            Some("linked-repo")
        );
        assert_eq!(
            snapshot
                .agents
                .get("herdr:linked")
                .and_then(|agent| agent.workspace.branch.as_deref()),
            Some("feature/x")
        );
        assert_eq!(
            snapshot
                .agents
                .get("herdr:alias")
                .and_then(|agent| agent.workspace.repo.as_deref()),
            Some("primary-repo")
        );
        assert_eq!(
            snapshot
                .agents
                .get("herdr:alias")
                .and_then(|agent| agent.workspace.branch.as_deref()),
            Some("main")
        );
        let unknown_agent = snapshot.agents.get("herdr:unknown").expect("unknown row");
        assert_eq!(unknown_agent.workspace.repo, None);
        assert_eq!(unknown_agent.workspace.branch, None);

        // A canonical alias is also the same worktree for preservation: the
        // plane-derived fields must survive a Herdr record rebuild.
        let mut existing = store.get("herdr:alias").await.expect("alias row");
        existing.workspace.dirty = true;
        existing.workspace.head_sha = Some("abc123".to_string());
        existing.workspace.head_subject = Some("subject".to_string());
        existing.workspace.pr_number = Some(7);
        existing.workspace.issues = vec![crate::core::events::GhIssueRef {
            repo: "primary-repo".to_string(),
            number: 109,
            state: "OPEN".to_string(),
            title: "primary attribution".to_string(),
            labels: vec![],
            url: String::new(),
            body: None,
            comments: vec![],
            comment_total: None,
        }];
        store.apply(Change::upsert(existing)).await;
        adapter
            .apply_agent_info(&info("alias", "p-alias", &primary), &store)
            .await;
        let preserved = store.get("herdr:alias").await.expect("preserved alias row");
        assert!(preserved.workspace.dirty);
        assert_eq!(preserved.workspace.head_sha.as_deref(), Some("abc123"));
        assert_eq!(preserved.workspace.pr_number, Some(7));
        assert_eq!(preserved.workspace.issues.len(), 1);
    }

    #[tokio::test]
    async fn blocked_status_then_matched_output_sets_waiting_on() {
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        let agent: AgentInfoWire = serde_json::from_value(fixture_claude()).unwrap();
        adapter.apply_agent_info(&agent, &store).await;

        let pane_id = "wQ:p1";

        // Status change to blocked.
        let status = serde_json::from_value::<StatusChangedWire>(json!({
            "pane_id": pane_id,
            "agent_status": "blocked",
            "agent": "claude",
            "title": "Fix Blender acceptance gate and run tests",
            "state_labels": {"waiting_for_input": ""}
        }))
        .unwrap();
        adapter.handle_status_changed(&status, &store).await;

        let blocked = store
            .get("herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784")
            .await
            .unwrap();
        assert_eq!(blocked.state, AgentState::Blocked);
        assert_eq!(blocked.reason.as_deref(), Some("waiting_for_input"));
        assert!(blocked.waiting_on.is_none());

        // Output matches the prompt regex while blocked -> waiting_on.
        let matched = serde_json::from_value::<OutputMatchedWire>(json!({
            "pane_id": pane_id,
            "matched_line": "  Do you want to proceed?",
            "read": {
                "pane_id": pane_id,
                "revision": 60,
                "source": "recent_unwrapped",
                "format": "text",
                "truncated": false,
                "text": "1. Continue\n2. Abort\n"
            }
        }))
        .unwrap();
        adapter.handle_output_matched(&matched, &store).await;

        let after = store
            .get("herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784")
            .await
            .unwrap();
        let w = after
            .waiting_on
            .as_ref()
            .expect("waiting_on set while blocked");
        assert_eq!(w.kind, WaitingOnKind::AnswerQuestion);
        // P3 D8: the prompt is the EXACT matched line — never trimmed — and
        // the hash covers those exact bytes (a trimmed re-hash would not
        // equal the claim's hash).
        assert_eq!(w.prompt, "  Do you want to proceed?");
        assert!(w.prompt_hash.starts_with("sha256:"));
        let mut hasher = Sha256::new();
        hasher.update(b"  Do you want to proceed?");
        assert_eq!(w.prompt_hash, format!("sha256:{}", hex(&hasher.finalize())));
        // Claim emission: the stored approval_id is the stable claim identity
        // clients echo in DrivePayload::Approve.
        assert_eq!(
            w.approval_id,
            crate::approve::approval_id_for(
                "herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784",
                &w.prompt_hash
            )
        );
        assert!(w.choices.iter().any(|c| c == "Continue"));
        assert!(after.seq > blocked.seq, "seq must be monotonic");

        // Leaving blocked clears waiting_on.
        let working = serde_json::from_value::<StatusChangedWire>(json!({
            "pane_id": pane_id,
            "agent_status": "working",
            "agent": "claude",
            "title": "Fix Blender acceptance gate and run tests",
            "state_labels": {}
        }))
        .unwrap();
        adapter.handle_status_changed(&working, &store).await;
        let cleared = store
            .get("herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784")
            .await
            .unwrap();
        assert_eq!(cleared.state, AgentState::Working);
        assert!(cleared.waiting_on.is_none());
    }

    #[tokio::test]
    async fn blocked_question_is_recorded_into_the_structured_exchange_ledger() {
        // #330: the agent's STRUCTURED blocked question (pane.output_matched
        // → waiting_on) is recorded into the store's exchange ledger with
        // its authoritative role, so the read_tail canonical stream can
        // attribute it. Leaving blocked clears waiting_on but never the
        // ledger (the question already happened).
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        let agent: AgentInfoWire = serde_json::from_value(fixture_claude()).unwrap();
        adapter.apply_agent_info(&agent, &store).await;
        let agent_id = "herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784";
        let pane_id = "wQ:p1";

        let status = serde_json::from_value::<StatusChangedWire>(json!({
            "pane_id": pane_id,
            "agent_status": "blocked",
            "agent": "claude",
            "title": "Fix Blender acceptance gate and run tests",
            "state_labels": {"waiting_for_input": ""}
        }))
        .unwrap();
        adapter.handle_status_changed(&status, &store).await;

        let matched = serde_json::from_value::<OutputMatchedWire>(json!({
            "pane_id": pane_id,
            "matched_line": "  Do you want to proceed?",
            "read": {
                "pane_id": pane_id,
                "revision": 60,
                "source": "recent_unwrapped",
                "format": "text",
                "truncated": false,
                "text": "1. Continue\n2. Abort\n"
            }
        }))
        .unwrap();
        adapter.handle_output_matched(&matched, &store).await;

        assert!(
            store.exchange().has_events_for(agent_id),
            "the blocked question must be recorded in the exchange ledger"
        );

        // The real read path trims the window candidate before binding. The
        // producer fixture deliberately carries the indented matched_line;
        // the canonical stream must still attribute that question as Agent.
        let blocks = crate::core::blocks::canonical_blocks_with_exchange(
            &["Do you want to proceed?".to_string()],
            &crate::core::provenance::PromptProvenance::new(),
            &store.exchange(),
            agent_id,
            None,
        );
        assert_eq!(
            blocks.iter().map(|block| block.kind).collect::<Vec<_>>(),
            vec![crate::core::blocks::TranscriptBlockKind::Agent],
            "the Herdr matched_line must bind through the canonical read path: {blocks:#?}"
        );

        let bound = store.exchange().bind_events(
            agent_id,
            &[Some("  Do you want to proceed?".to_string())],
            8,
        );
        assert_eq!(
            bound[0].as_ref().map(|e| e.role),
            Some(crate::core::provenance::ExchangeRole::Assistant),
            "an answer-question records the Assistant role"
        );

        // Leaving blocked clears waiting_on but the ledger entry survives.
        let working = serde_json::from_value::<StatusChangedWire>(json!({
            "pane_id": pane_id,
            "agent_status": "working",
            "agent": "claude",
            "title": "Fix Blender acceptance gate and run tests",
            "state_labels": {}
        }))
        .unwrap();
        adapter.handle_status_changed(&working, &store).await;
        assert!(
            store.exchange().has_events_for(agent_id),
            "the structured event outlives the transient waiting_on"
        );

        // An approve-tool question records the Tool role.
        let approve = serde_json::from_value::<OutputMatchedWire>(json!({
            "pane_id": pane_id,
            "matched_line": "Approve this change?",
            "read": {
                "pane_id": pane_id,
                "revision": 61,
                "source": "recent_unwrapped",
                "format": "text",
                "truncated": false,
                "text": ""
            }
        }))
        .unwrap();
        let status = serde_json::from_value::<StatusChangedWire>(json!({
            "pane_id": pane_id,
            "agent_status": "blocked",
            "agent": "claude",
            "title": "Fix Blender acceptance gate and run tests",
            "state_labels": {"waiting_for_input": ""}
        }))
        .unwrap();
        adapter.handle_status_changed(&status, &store).await;
        adapter.handle_output_matched(&approve, &store).await;
        let bound =
            store
                .exchange()
                .bind_events(agent_id, &[Some("Approve this change?".to_string())], 8);
        assert_eq!(
            bound[0].as_ref().map(|e| e.role),
            Some(crate::core::provenance::ExchangeRole::Tool),
            "an approve-tool question records the Tool role"
        );
    }

    #[test]
    fn classifies_prompt_kinds_and_hashes() {
        let w = classify_waiting_on("Approve this change?", "");
        assert_eq!(w.kind, WaitingOnKind::ApproveTool);
        assert!(w.prompt_hash.starts_with("sha256:"));
        assert_eq!(w.prompt_hash.len(), "sha256:".len() + 64);

        let w = classify_waiting_on("Proceed? [y/n]", "");
        assert_eq!(w.kind, WaitingOnKind::Menu);

        let w = classify_waiting_on("What should I name the branch?", "");
        assert_eq!(w.kind, WaitingOnKind::AnswerQuestion);

        // Deterministic hash.
        let a = classify_waiting_on("Approve this change?", "");
        let b = classify_waiting_on("Approve this change?", "");
        assert_eq!(a.prompt_hash, b.prompt_hash);

        // P3 D8: the hash covers the EXACT prompt text — leading/trailing
        // whitespace is part of the hashed bytes, never trimmed away.
        let spaced = classify_waiting_on("  Approve this change?  ", "");
        assert_eq!(spaced.prompt, "  Approve this change?  ");
        assert_ne!(spaced.prompt_hash, a.prompt_hash);
        let mut hasher = Sha256::new();
        hasher.update(b"  Approve this change?  ");
        assert_eq!(
            spaced.prompt_hash,
            format!("sha256:{}", hex(&hasher.finalize()))
        );
    }

    #[test]
    fn extract_choices_detects_menus() {
        assert_eq!(extract_choices("[y/n]"), vec!["y", "n"]);
        let text = "1. Approve\n2. Reject and comment\n3. Edit files";
        assert_eq!(
            extract_choices(text),
            vec!["Approve", "Reject and comment", "Edit files"]
        );
        assert!(extract_choices("nothing here").is_empty());
    }

    #[test]
    fn waiting_on_redacts_pane_text_at_the_boundary() {
        // The matched line carries a fake secret: the stored prompt and the
        // hash must cover the REDACTED form — the exact bytes a client sees.
        let w = classify_waiting_on("Approve deploy with token ghp_yyy?", "");
        assert_eq!(w.prompt, "Approve deploy with token [REDACTED]?");
        assert_eq!(
            w.prompt_hash,
            classify_waiting_on("Approve deploy with token [REDACTED]?", "").prompt_hash
        );

        // Choice buffer is pane output too.
        let w = classify_waiting_on("Which env?", "1. prod: API_KEY=abc\n2. staging\n");
        assert_eq!(w.choices, vec!["prod: API_KEY=[REDACTED]", "staging"]);

        // Ordinary prose prompts are untouched by redaction.
        let w = classify_waiting_on("Do you want to proceed?", "");
        assert_eq!(w.prompt, "Do you want to proceed?");

        // F4 (re-review): kind classifies from the RAW line, so a secret
        // span swallowing the keyword cannot degrade the kind.
        let w = classify_waiting_on("ghp_yyy approve?", "");
        assert_eq!(w.kind, WaitingOnKind::ApproveTool);
        assert_eq!(w.prompt, "[REDACTED] approve?");
        let w = classify_waiting_on("Proceed ghp_yyy? [y/n]", "");
        assert_eq!(w.kind, WaitingOnKind::Menu);
        let w = classify_waiting_on("sk-ant-xxx deploy? [y/n]", "");
        assert_eq!(w.kind, WaitingOnKind::Menu);
    }

    #[test]
    fn reason_and_title_redact_pane_derived_text() {
        let mut labels = HashMap::new();
        labels.insert(
            "waiting_for_approval".to_string(),
            "run ghp_zzz now".to_string(),
        );
        assert_eq!(
            reason_from_labels(&labels).as_deref(),
            Some("waiting_for_approval: run [REDACTED] now")
        );
        labels.insert("plain".to_string(), "nothing sensitive".to_string());
        assert_eq!(
            reason_from_labels(&labels).as_deref(),
            Some("plain: nothing sensitive"),
            "sorted first key wins; prose reasons pass through"
        );
    }

    #[tokio::test]
    async fn matched_output_with_secret_lands_redacted_in_the_store() {
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        let agent: AgentInfoWire = serde_json::from_value(fixture_claude()).unwrap();
        adapter.apply_agent_info(&agent, &store).await;

        let pane_id = "wQ:p1";
        let status = serde_json::from_value::<StatusChangedWire>(json!({
            "pane_id": pane_id,
            "agent_status": "blocked",
            "agent": "claude",
            "title": "Setup AWS key AKIA1234567890ABCDEF now",
            "state_labels": {"waiting_for_input": ""}
        }))
        .unwrap();
        adapter.handle_status_changed(&status, &store).await;

        let matched = serde_json::from_value::<OutputMatchedWire>(json!({
            "pane_id": pane_id,
            "matched_line": "  Approve with sk-ant-api03-AB12cdEF34ghIJ56klMN78op?",
            "read": {"text": "1. Approve\n2. Reject\n"}
        }))
        .unwrap();
        adapter.handle_output_matched(&matched, &store).await;

        let record = store
            .get("herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784")
            .await
            .unwrap();
        assert_eq!(
            record.title.as_deref(),
            Some("Setup AWS key [REDACTED] now"),
            "title redacted on ingest"
        );
        let w = record.waiting_on.expect("waiting_on set while blocked");
        // D8 (W2): the stored prompt is UNTRIMMED — the hash covers the exact
        // bytes a client echoes; leading whitespace is part of the claim.
        assert_eq!(w.prompt, "  Approve with [REDACTED]?");
        assert!(
            !w.prompt_hash.contains("sk-ant"),
            "hash covers the redacted prompt only"
        );
    }

    #[tokio::test]
    async fn drive_target_survives_session_id_migration() {
        // F (live smoke): a pane first detected without a session id is
        // tracked by the pane-derived fallback; once a pane.updated carries
        // `agent_session`, the id migrates. The drive target resolution must
        // follow the MIGRATED id (agent_id -> pane), or every drive on the
        // agent fails UnknownAgent until the next bootstrap.
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        adapter
            .register_agent_pane("wY:p1", "opencode", AgentState::Unknown, &store)
            .await;
        assert_eq!(
            adapter.drive_target("herdr:pane:wY:p1").unwrap(),
            "wY:p1",
            "pane-derived id maps before migration"
        );

        // The pane.updated with a session id migrates the canonical id.
        let updated = serde_json::from_value::<PaneInfoWire>(json!({
            "pane_id": "wY:p1",
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses_migrated"},
            "agent_status": "done",
            "title": "OC | Migrated",
            "state_labels": {}
        }))
        .unwrap();
        let (sink, _rx) = mpsc::channel(16);
        adapter.handle_pane_updated(&updated, sink, &store).await;

        assert!(
            adapter.drive_target("herdr:pane:wY:p1").is_err(),
            "old id is gone"
        );
        assert_eq!(
            adapter.drive_target("herdr:ses_migrated").unwrap(),
            "wY:p1",
            "migrated id still resolves its pane for drive"
        );
    }

    #[tokio::test]
    async fn drive_target_follows_stable_session_to_new_pane_and_name() {
        // A stable herdr session may be reported on a different pane after a
        // terminal/workspace migration. The target must follow the new pane
        // and name; the old entry must not survive through `or_insert`.
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        let first: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-moving"},
            "agent_status": "working",
            "name": "old-target",
            "pane_id": "w-old:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&first, &store).await;

        let moved: PaneInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-moving"},
            "agent_status": "working",
            "display_agent": "new-target",
            "pane_id": "w-new:p1",
            "state_labels": {}
        }))
        .unwrap();
        // Keep this hermetic control socket focused on request/response RPCs;
        // the production handler would open a pane subscription here.
        adapter
            .state
            .lock()
            .unwrap()
            .subscribed_panes
            .insert("w-new:p1".to_string());
        let (sink, _rx) = mpsc::channel(16);
        adapter.handle_pane_updated(&moved, sink, &store).await;

        assert_eq!(
            adapter.drive_target("herdr:ses-moving").unwrap(),
            "new-target",
            "name target follows the migrated pane"
        );
        let state = adapter.state.lock().unwrap();
        assert_eq!(
            state.agent_panes.get("herdr:ses-moving").unwrap(),
            "w-new:p1"
        );
        assert!(!state.pane_agents.contains_key("w-old:p1"));
    }

    #[tokio::test]
    async fn disappearance_is_typed_stale_for_read_prompt_and_approve() {
        // Once a known pane disappears, all three controls must refuse before
        // opening a transport. A stale selection is refreshable; it is not an
        // unknown id and must not become a generic socket failure.
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        adapter
            .register_agent_pane("w-stale:p1", "opencode", AgentState::Working, &store)
            .await;
        let (sink, _rx) = mpsc::channel(16);
        adapter
            .handle_event(
                ParsedEvent::PaneClosed {
                    pane_id: "w-stale:p1".to_string(),
                },
                sink,
                &store,
            )
            .await;
        let agent_id = "herdr:pane:w-stale:p1";

        assert!(matches!(
            adapter.read_tail(agent_id, 10).await,
            Err(DriveError::StaleAgent(id)) if id == agent_id
        ));
        assert!(matches!(
            adapter
                .drive(agent_id, DriveCommand::Prompt { text: "hi".into() })
                .await,
            Err(DriveError::StaleAgent(id)) if id == agent_id
        ));
        assert!(matches!(
            adapter
                .drive(agent_id, DriveCommand::Approve { choice: "y".into() })
                .await,
            Err(DriveError::StaleAgent(id)) if id == agent_id
        ));
    }

    #[tokio::test]
    async fn unknown_state_flows_through_read_path() {
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        // pane.agent_detected registers the pane with Unknown state.
        adapter
            .register_agent_pane("uX:p1", "opencode", AgentState::Unknown, &store)
            .await;
        let snap = store.snapshot().await;
        let agent = snap
            .agents
            .get("herdr:pane:uX:p1")
            .expect("detected agent record");
        assert_eq!(
            agent.state,
            AgentState::Unknown,
            "Unknown is a first-class state"
        );
        assert!(adapter.knows_agent(&agent.agent_id));
    }

    #[tokio::test]
    async fn drive_against_unknown_state_agent_is_typed_not_a_crash() {
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        adapter
            .register_agent_pane("uX:p1", "opencode", AgentState::Unknown, &store)
            .await;
        let snap = store.snapshot().await;
        let agent = snap
            .agents
            .get("herdr:pane:uX:p1")
            .expect("detected agent record");
        assert_eq!(agent.state, AgentState::Unknown);

        // A tracked pane in Unknown state is drivable (drive gates on the
        // pane mapping, not the state): the transport outcome is returned,
        // never hidden in a detached task.
        let result = adapter
            .drive(&agent.agent_id, DriveCommand::Prompt { text: "hi".into() })
            .await;
        assert!(
            matches!(result, Err(DriveError::Transport(_))),
            "drive on an unknown-state agent must expose transport failure: {result:?}"
        );

        // An agent with no pane mapping gets the typed error.
        let err = adapter
            .drive(
                "herdr:pane:absent",
                DriveCommand::Prompt { text: "hi".into() },
            )
            .await;
        assert!(matches!(err, Err(DriveError::UnknownAgent(id)) if id == "herdr:pane:absent"));
    }

    #[tokio::test]
    async fn drive_rejects_unknown_agents() {
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        assert!(!adapter.knows_agent("nope"));
        let err = adapter
            .drive("nope", DriveCommand::Prompt { text: "hi".into() })
            .await;
        assert!(matches!(err, Err(DriveError::UnknownAgent(id)) if id == "nope"));
    }

    #[tokio::test]
    async fn approve_dispatches_via_agent_prompt() {
        // The pane's approve is an input send; herdr exposes no
        // approve-shaped RPC, so the choice goes through agent.prompt (the
        // same input-send the human typing into the pane produces).
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        assert!(!adapter.knows_agent("nope"));
        let err = adapter
            .drive("nope", DriveCommand::Approve { choice: "y".into() })
            .await;
        assert!(matches!(err, Err(DriveError::UnknownAgent(id)) if id == "nope"));
    }

    // -----------------------------------------------------------------------
    // W2.1 read_tail: the adapter fetches agent.read SYNCHRONOUSLY, redacts
    // (D9) and bounds (D5) the tail before it leaves the machine.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn drive_refuses_read_tail_fallback() {
        // read_tail is the one capability whose whole point is a response:
        // the API layer routes it through Adapter::read_tail, and this path
        // refuses it rather than discarding the result.
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        let err = adapter
            .drive(
                "herdr:a",
                DriveCommand::ReadTail {
                    lines: Some(5),
                    since_rev: None,
                },
            )
            .await;
        assert!(matches!(err, Err(DriveError::NotImplemented("read_tail"))));
    }

    #[tokio::test]
    async fn read_tail_rejects_unknown_agents() {
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        let err = adapter.read_tail("nope", 10).await;
        assert!(matches!(err, Err(DriveError::UnknownAgent(id)) if id == "nope"));
    }

    #[tokio::test]
    async fn read_tail_transport_failure_is_typed() {
        // No socket: the RPC fails, mapped to DriveError::Transport (not a
        // panic, not a fire-and-forget).
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        adapter
            .register_agent_pane("p1", "opencode", AgentState::Working, &store)
            .await;
        let err = adapter.read_tail("herdr:pane:p1", 10).await;
        assert!(matches!(err, Err(DriveError::Transport(_))));
    }

    #[test]
    fn tail_is_redacted_and_bounded_at_the_boundary() {
        let secret = "sk-ant-api03-AB12cdEF34ghIJ56klMN78op";
        // 250 lines exceed the 200-line cap; each carries a seeded secret.
        let text = (0..250)
            .map(|i| format!("line {i:03} deploy {secret}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tail = bounded_redacted_tail(&text, 200);
        assert_eq!(tail.len(), READ_TAIL_MAX_LINES as usize, "line bound (D5)");
        assert_eq!(tail[0], "line 000 deploy [REDACTED]");
        let wire = tail.join("\n");
        assert!(
            !wire.contains("sk-ant"),
            "redaction (D9) before bytes leave"
        );
        assert!(wire.contains("[REDACTED]"));
    }

    #[test]
    fn tail_byte_bound_is_32_kib() {
        // 200 lines of ~1 KiB each would be 200 KiB unwired; the byte cap
        // must keep the returned payload under READ_TAIL_MAX_BYTES by
        // dropping whole trailing lines.
        let text = (0..200)
            .map(|i| format!("log line {i:03} {}", "x".repeat(1024)))
            .collect::<Vec<_>>()
            .join("\n");
        let tail = bounded_redacted_tail(&text, 200);
        assert!(tail.len() < 200, "byte bound cuts lines");
        let bytes = tail.join("\n").len() + 1;
        assert!(bytes <= READ_TAIL_MAX_BYTES, "byte bound (D5): {bytes}");
    }

    #[test]
    fn tail_passes_clean_prose_through_and_handles_empty_output() {
        // Ordinary prose survives byte-identical (redaction is display-safe).
        let text = "  1. Continue\n  2. Abort\n  → Waiting on your decision…\n";
        let tail = bounded_redacted_tail(text, 200);
        assert_eq!(
            tail,
            vec![
                "  1. Continue",
                "  2. Abort",
                "  → Waiting on your decision…"
            ]
        );
        // No output -> empty lines, never an error.
        assert!(bounded_redacted_tail("", 200).is_empty());
    }

    #[test]
    fn hundred_thousand_lines_stay_bounded_quickly() {
        let text = (0..100_000)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let started = std::time::Instant::now();
        let tail = bounded_redacted_tail(&text, 200);
        assert!(started.elapsed() < std::time::Duration::from_millis(300));
        assert!(tail.len() <= 200);
        assert!(tail.iter().map(|line| line.len() + 1).sum::<usize>() <= READ_TAIL_MAX_BYTES);
    }

    #[test]
    fn unsupported_private_use_glyphs_are_replaced_but_emoji_survive() {
        let line = "ok \u{e000}\u{e001} ✅ ⚠️";
        assert_eq!(scrub_unsupported_glyphs(line), "ok [icon] ✅ ⚠️");
    }

    #[test]
    fn tail_scrubs_tui_furniture_after_redaction() {
        let text = "╭────────────────────────────╮\n│ model: pilot │\n╰────────────────────────────╯\nlet sep = \"───────────────\";\npainter ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓░ 92%\n";
        let tail = bounded_redacted_tail(text, 200);
        assert_eq!(tail[0], "───", "top border collapses");
        assert_eq!(tail[1], "│ model: pilot │", "content line untouched");
        assert_eq!(tail[2], "───", "bottom border collapses");
        assert_eq!(
            tail[3], "let sep = \"───────────────\";",
            "dash run inside a string survives"
        );
        assert!(!tail[4].contains('▓'), "progress run compacted");
        assert!(tail[4].contains('▰'), "compact bar marker present");
    }

    /// One JSON-RPC exchange against a mock herdr socket: accept a
    /// connection, read the `agent.read` request, answer with a fixed tail.
    /// The caller binds the listener so the socket path exists before the
    /// client connects.
    async fn mock_socket_serve(
        listener: tokio::net::UnixListener,
        reply_text: Value,
    ) -> (Value, Value) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut line = String::new();
        let mut reader = BufReader::new(&mut stream);
        reader.read_line(&mut line).await.unwrap();
        let req: Value = serde_json::from_str(&line).unwrap();
        let resp = json!({
            "id": req["id"],
            "result": { "read": { "text": reply_text } }
        });
        let mut out = resp.to_string();
        out.push('\n');
        stream.write_all(out.as_bytes()).await.unwrap();
        (req, resp)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum PaneServerEvent {
        Connected(usize),
        Dropped(usize),
        Rejected(usize),
        Ready(usize),
        Closed(usize),
    }

    #[derive(Debug, Clone, Copy)]
    enum PaneServerReply {
        DropConnection,
        RejectSubscription,
        AcceptSubscription,
    }

    /// Serve a sequence of fake pane sockets. The listener stays alive across
    /// retries, so one task can prove recovery after a failed connection or
    /// subscribe without restarting the fake herdr daemon.
    async fn serve_pane_streams(
        listener: tokio::net::UnixListener,
        replies: Vec<PaneServerReply>,
        events: mpsc::Sender<PaneServerEvent>,
    ) {
        for (index, reply) in replies.into_iter().enumerate() {
            let (stream, _) = listener.accept().await.expect("pane accept");
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let request = lines
                .next_line()
                .await
                .expect("pane request read")
                .expect("pane request");
            let request: Value = serde_json::from_str(&request).expect("pane request json");
            events
                .send(PaneServerEvent::Connected(index))
                .await
                .expect("test receiver alive");

            match reply {
                PaneServerReply::DropConnection => {
                    // Drop both halves without answering the pending
                    // subscribe request. The client must classify this as a
                    // transport failure and retry.
                    events
                        .send(PaneServerEvent::Dropped(index))
                        .await
                        .expect("test receiver alive");
                    continue;
                }
                PaneServerReply::RejectSubscription => {
                    let response = json!({
                        "id": request["id"],
                        "error": {
                            "code": "synthetic_subscribe_failure",
                            "message": "fake pane subscription rejected"
                        }
                    });
                    let mut wire = response.to_string();
                    wire.push('\n');
                    write
                        .write_all(wire.as_bytes())
                        .await
                        .expect("write rejected subscribe");
                    write.flush().await.expect("flush rejected subscribe");
                    events
                        .send(PaneServerEvent::Rejected(index))
                        .await
                        .expect("test receiver alive");
                }
                PaneServerReply::AcceptSubscription => {
                    let response = json!({
                        "id": request["id"],
                        "result": null
                    });
                    let mut wire = response.to_string();
                    wire.push('\n');
                    write
                        .write_all(wire.as_bytes())
                        .await
                        .expect("write accepted subscribe");
                    write.flush().await.expect("flush accepted subscribe");
                    events
                        .send(PaneServerEvent::Ready(index))
                        .await
                        .expect("test receiver alive");
                }
            }

            // The adapter sends no request after subscribe. Drain until the
            // client closes so the test observes the exact socket teardown.
            while let Ok(Some(_)) = lines.next_line().await {}
            events
                .send(PaneServerEvent::Closed(index))
                .await
                .expect("test receiver alive");
        }
    }

    fn subscribed_pane_state(pane_id: &str) -> Arc<Mutex<SessionState>> {
        let mut state = SessionState::default();
        state.subscribed_panes.insert(pane_id.to_string());
        let agent_id = format!("herdr:pane:{pane_id}");
        state
            .pane_agents
            .insert(pane_id.to_string(), agent_id.clone());
        state.agent_panes.insert(agent_id, pane_id.to_string());
        Arc::new(Mutex::new(state))
    }

    async fn expect_pane_server_event(
        events: &mut mpsc::Receiver<PaneServerEvent>,
        expected: PaneServerEvent,
    ) {
        let observed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = events.recv().await.expect("pane server still running");
                if event == expected {
                    break event;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {expected:?}"));
        assert_eq!(observed, expected);
    }

    #[tokio::test]
    async fn removing_pane_cancels_live_forwarder_and_closes_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let (events_tx, mut events_rx) = mpsc::channel(8);
        let server = tokio::spawn(serve_pane_streams(
            listener,
            vec![PaneServerReply::AcceptSubscription],
            events_tx,
        ));
        let state = subscribed_pane_state("p1");
        let (sink, _sink_rx) = mpsc::channel(FRAME_CHANNEL_CAP);

        spawn_pane_event_stream(socket_path, "p1".to_string(), sink, state.clone());
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Connected(0)).await;
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Ready(0)).await;

        let removed = state.lock().unwrap().remove("p1");
        assert_eq!(removed.as_deref(), Some("herdr:pane:p1"));
        assert!(!state.lock().unwrap().pane_streams.contains_key("p1"));
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Closed(0)).await;

        server.await.expect("pane server task");
    }

    #[tokio::test]
    async fn cancel_all_pane_streams_preserves_membership_and_closes_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let (events_tx, mut events_rx) = mpsc::channel(8);
        let server = tokio::spawn(serve_pane_streams(
            listener,
            vec![PaneServerReply::AcceptSubscription],
            events_tx,
        ));
        let state = subscribed_pane_state("p1");
        let (sink, _sink_rx) = mpsc::channel(FRAME_CHANNEL_CAP);

        spawn_pane_event_stream(socket_path, "p1".to_string(), sink, state.clone());
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Connected(0)).await;
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Ready(0)).await;

        let (stream_cancelled, membership_kept) = {
            let mut state = state.lock().unwrap();
            state.cancel_all_pane_streams();
            (
                !state.pane_streams.contains_key("p1"),
                state.subscribed_panes.contains("p1"),
            )
        };
        assert!(stream_cancelled, "global re-bootstrap cancels pane task");
        assert!(
            membership_kept,
            "global re-subscription keeps the pane known so reconcile can reuse it"
        );
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Closed(0)).await;
        server.await.expect("pane server task");
    }

    #[tokio::test]
    async fn pane_migration_cancels_old_dedicated_stream_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let (events_tx, mut events_rx) = mpsc::channel(8);
        let server = tokio::spawn(serve_pane_streams(
            listener,
            vec![PaneServerReply::AcceptSubscription],
            events_tx,
        ));
        let mut initial = SessionState::default();
        initial.subscribed_panes.insert("p1".to_string());
        initial
            .pane_agents
            .insert("p1".to_string(), "herdr:session".to_string());
        initial
            .agent_panes
            .insert("herdr:session".to_string(), "p1".to_string());
        let state = Arc::new(Mutex::new(initial));
        let adapter = HerdrAdapter::new(socket_path.clone());
        let (sink, _sink_rx) = mpsc::channel(FRAME_CHANNEL_CAP);

        spawn_pane_event_stream(socket_path, "p1".to_string(), sink, state.clone());
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Connected(0)).await;
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Ready(0)).await;

        {
            let mut state = state.lock().unwrap();
            adapter.register_pane(&mut state, "p2", "herdr:session", None);
        }
        let (old_cancelled, old_membership, new_pane) = {
            let state = state.lock().unwrap();
            (
                !state.pane_streams.contains_key("p1"),
                !state.subscribed_panes.contains("p1"),
                state.agent_panes.get("herdr:session").map(String::as_str) == Some("p2"),
            )
        };
        assert!(old_cancelled, "migration cancels the old per-pane task");
        assert!(
            old_membership,
            "migration must not leave the old pane eligible for a second stream"
        );
        assert!(new_pane, "drive mapping follows the migrated pane");
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Closed(0)).await;
        server.await.expect("pane server task");
    }

    #[tokio::test]
    async fn pane_stream_recovers_after_connection_and_subscription_failures() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let (events_tx, mut events_rx) = mpsc::channel(16);
        let server = tokio::spawn(serve_pane_streams(
            listener,
            vec![
                PaneServerReply::DropConnection,
                PaneServerReply::RejectSubscription,
                PaneServerReply::AcceptSubscription,
            ],
            events_tx,
        ));
        let state = subscribed_pane_state("p1");
        let (sink, _sink_rx) = mpsc::channel(FRAME_CHANNEL_CAP);

        spawn_pane_event_stream(socket_path, "p1".to_string(), sink, state.clone());
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Connected(0)).await;
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Dropped(0)).await;
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Connected(1)).await;
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Rejected(1)).await;
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Closed(1)).await;
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Connected(2)).await;
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Ready(2)).await;
        assert!(state.lock().unwrap().pane_streams.contains_key("p1"));

        state.lock().unwrap().remove("p1");
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Closed(2)).await;
        server.await.expect("pane server task");
    }

    #[tokio::test]
    async fn pane_churn_aborts_old_generation_without_delayed_duplicate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let (events_tx, mut events_rx) = mpsc::channel(16);
        let server = tokio::spawn(serve_pane_streams(
            listener,
            vec![
                PaneServerReply::AcceptSubscription,
                PaneServerReply::AcceptSubscription,
                PaneServerReply::AcceptSubscription,
            ],
            events_tx,
        ));
        let state = subscribed_pane_state("p1");
        let (sink, _sink_rx) = mpsc::channel(FRAME_CHANNEL_CAP);

        spawn_pane_event_stream(
            socket_path.clone(),
            "p1".to_string(),
            sink.clone(),
            state.clone(),
        );
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Connected(0)).await;
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Ready(0)).await;

        state.lock().unwrap().remove("p1");
        state
            .lock()
            .unwrap()
            .subscribed_panes
            .insert("p1".to_string());
        spawn_pane_event_stream(socket_path, "p1".to_string(), sink, state.clone());

        expect_pane_server_event(&mut events_rx, PaneServerEvent::Closed(0)).await;
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Connected(1)).await;
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Ready(1)).await;

        state.lock().unwrap().remove("p1");
        expect_pane_server_event(&mut events_rx, PaneServerEvent::Closed(1)).await;

        // Keep the fake listener task waiting for a third accept. A stale
        // delayed respawn would make that connection during this window.
        let extra = tokio::time::timeout(PANE_RESPAWN_DELAY + Duration::from_millis(500), async {
            loop {
                if events_rx.recv().await == Some(PaneServerEvent::Connected(2)) {
                    break;
                }
            }
        })
        .await;
        assert!(extra.is_err(), "pane churn must not create a third stream");

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn read_tail_round_trips_over_the_socket_redacted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let seeded =
            "line one\ndeploy token ghp_AbCdEfGhIjKlMnOpQrStUvWxYz1234567890 now\ntail three\n";
        let server = tokio::spawn(mock_socket_serve(listener, seeded.into()));

        let store = Store::new();
        let adapter = HerdrAdapter::new(socket_path.clone());
        adapter
            .register_agent_pane("p1", "opencode", AgentState::Working, &store)
            .await;
        let tail = adapter
            .read_tail("herdr:pane:p1", 200)
            .await
            .expect("read_tail");
        let (req, _) = server.await.unwrap();

        assert_eq!(req["method"], "agent.read");
        assert_eq!(req["params"]["source"], "recent_unwrapped");
        assert_eq!(req["params"]["lines"], 200);
        assert_eq!(
            req["params"]["target"], "p1",
            "target resolves to the pane/name"
        );
        assert_eq!(
            tail,
            vec!["line one", "deploy token [REDACTED] now", "tail three"]
        );
        let wire = serde_json::to_string(&tail).unwrap();
        assert!(
            !wire.contains("ghp_"),
            "no secret-shaped span leaves the machine"
        );
    }

    #[tokio::test]
    async fn read_tail_empty_output_is_empty_lines_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(mock_socket_serve(listener, Value::Null));

        let store = Store::new();
        let adapter = HerdrAdapter::new(socket_path.clone());
        adapter
            .register_agent_pane("p1", "opencode", AgentState::Working, &store)
            .await;
        let tail = adapter
            .read_tail("herdr:pane:p1", 200)
            .await
            .expect("read_tail");
        server.await.unwrap();
        assert!(tail.is_empty(), "no output -> clean empty lines");
    }

    /// Three bounded control operations against one current migrated target.
    /// This is deliberately a socket-level assertion: resolving only the
    /// in-memory map would not prove that read_tail, prompt, and approve all
    /// send the same current target over their production RPC paths.
    #[tokio::test]
    async fn optional_name_migration_dispatches_all_controls_to_current_pane() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..3 {
                let (stream, _) = listener.accept().await.expect("accept");
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                let line = lines.next_line().await.expect("request").expect("line");
                let request: Value = serde_json::from_str(&line).expect("json request");
                let result = if request["method"] == "agent.read" {
                    json!({"read": {"text": "current tail\n"}})
                } else {
                    json!({"ok": true})
                };
                let response = json!({"id": request["id"], "result": result}).to_string() + "\n";
                write
                    .write_all(response.as_bytes())
                    .await
                    .expect("response");
                requests.push(request);
            }
            requests
        });

        let store = Store::new();
        let adapter = HerdrAdapter::new(socket_path);
        let first: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-controls"},
            "agent_status": "blocked",
            "name": "old-target",
            "pane_id": "w-old:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&first, &store).await;
        let moved: PaneInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-controls"},
            "agent_status": "blocked",
            "pane_id": "w-new:p1",
            "state_labels": {}
        }))
        .unwrap();
        // Keep this hermetic control socket focused on request/response RPCs;
        // the production handler would open a pane subscription here.
        adapter
            .state
            .lock()
            .unwrap()
            .subscribed_panes
            .insert("w-new:p1".to_string());
        let (sink, _rx) = mpsc::channel(16);
        adapter.handle_pane_updated(&moved, sink, &store).await;

        let agent_id = "herdr:ses-controls";
        let tail = adapter.read_tail(agent_id, 10).await.expect("read tail");
        assert_eq!(tail, vec!["current tail"]);
        adapter
            .drive(
                agent_id,
                DriveCommand::Prompt {
                    text: "hello".into(),
                },
            )
            .await
            .expect("prompt dispatch accepted");
        adapter
            .drive(agent_id, DriveCommand::Approve { choice: "y".into() })
            .await
            .expect("approve dispatch accepted");

        let requests = tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("three RPCs timeout")
            .expect("server task");
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0]["method"], "agent.read");
        assert_eq!(requests[0]["params"]["target"], "w-new:p1");
        assert_eq!(requests[1]["method"], "agent.prompt");
        assert_eq!(requests[1]["params"]["target"], "w-new:p1");
        assert_eq!(requests[1]["params"]["text"], "hello");
        assert_eq!(requests[2]["method"], "agent.prompt");
        assert_eq!(requests[2]["params"]["target"], "w-new:p1");
        assert_eq!(requests[2]["params"]["text"], "y");
    }

    #[tokio::test]
    async fn kill_closes_current_pane_and_retires_store_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let line = lines.next_line().await.expect("request").expect("line");
            let request: Value = serde_json::from_str(&line).expect("json request");
            let response = json!({"id": request["id"], "result": Value::Null}).to_string() + "\n";
            write
                .write_all(response.as_bytes())
                .await
                .expect("response");
            request
        });

        let store = Store::new();
        let adapter = HerdrAdapter::new(socket_path);
        adapter.attach_store(store.clone());
        let first: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-kill"},
            "agent_status": "working",
            "name": "agent-kill",
            "pane_id": "w-kill:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&first, &store).await;

        let agent_id = "herdr:ses-kill";
        adapter
            .drive(agent_id, DriveCommand::Kill)
            .await
            .expect("kill dispatched");
        let request = tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("kill RPC timeout")
            .expect("server task");

        assert_eq!(request["method"], "pane.close");
        assert_eq!(request["params"]["pane_id"], "w-kill:p1");
        assert!(
            request["params"].get("target").is_none(),
            "pane.close must use the mapped pane id, not the agent target"
        );
        assert!(
            store.snapshot().await.agents.is_empty(),
            "successful kill must retire the canonical store row"
        );
        assert!(adapter.is_stale_agent(agent_id));
        assert!(adapter.drive_target(agent_id).is_err());
    }

    #[tokio::test]
    async fn kill_pane_not_found_is_stale_and_retires_store_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let line = lines.next_line().await.expect("request").expect("line");
            let request: Value = serde_json::from_str(&line).expect("json request");
            let response = json!({
                "id": request["id"],
                "error": {"code": "pane_not_found", "message": "pane not found"}
            })
            .to_string()
                + "\n";
            write
                .write_all(response.as_bytes())
                .await
                .expect("response");
            request
        });

        let store = Store::new();
        let adapter = HerdrAdapter::new(socket_path);
        adapter.attach_store(store.clone());
        let first: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-kill-stale"},
            "agent_status": "working",
            "name": "agent-kill-stale",
            "pane_id": "w-kill-stale:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&first, &store).await;

        let agent_id = "herdr:ses-kill-stale";
        let result = adapter.drive(agent_id, DriveCommand::Kill).await;
        assert!(matches!(result, Err(DriveError::StaleAgent(id)) if id == agent_id));
        let request = tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("stale kill RPC timeout")
            .expect("server task");
        assert_eq!(request["method"], "pane.close");
        assert_eq!(request["params"]["pane_id"], "w-kill-stale:p1");
        assert!(store.snapshot().await.agents.is_empty());
        assert!(adapter.is_stale_agent(agent_id));
    }

    #[tokio::test]
    async fn kill_and_attach_unknown_agents_are_typed_without_connecting() {
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent-herdr.sock"));
        let kill = tokio::time::timeout(
            Duration::from_secs(1),
            adapter.drive("herdr:never", DriveCommand::Kill),
        )
        .await
        .expect("unknown kill must return immediately");
        assert!(matches!(
            kill,
            Err(DriveError::UnknownAgent(id)) if id == "herdr:never"
        ));

        let attach = tokio::time::timeout(Duration::from_secs(1), adapter.attach("herdr:never"))
            .await
            .expect("unknown attach must return immediately");
        assert!(matches!(
            attach,
            Err(DriveError::UnknownAgent(id)) if id == "herdr:never"
        ));
    }

    #[tokio::test]
    async fn attach_returns_stable_terminal_ref_for_current_target() {
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent-herdr.sock"));
        let first: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-attach"},
            "agent_status": "working",
            "name": "agent-attach",
            "pane_id": "w-attach:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&first, &store).await;

        let handle =
            tokio::time::timeout(Duration::from_secs(1), adapter.attach("herdr:ses-attach"))
                .await
                .expect("attach must not perform an RPC")
                .expect("attach handle");
        assert_eq!(handle["kind"], "terminal_ref");
        assert_eq!(handle["target"], "agent-attach");
        assert_eq!(handle["pane_id"], "w-attach:p1");
        assert_eq!(handle["command"], terminal_attach_command("agent-attach"));
        assert_eq!(
            handle["args"],
            json!(["herdr", "agent", "attach", "--takeover", "agent-attach"])
        );
    }

    #[tokio::test]
    async fn attach_dead_target_is_stale_not_unknown() {
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent-herdr.sock"));
        adapter.attach_store(store.clone());
        let first: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-attach-dead"},
            "agent_status": "working",
            "name": "agent-attach-dead",
            "pane_id": "w-attach-dead:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&first, &store).await;
        let agent_id = "herdr:ses-attach-dead";
        let generation = adapter
            .state
            .lock()
            .unwrap()
            .agent_generations
            .get(agent_id)
            .copied()
            .expect("generation");
        assert!(
            adapter
                .retire_rpc_mapping(agent_id, "agent-attach-dead", generation)
                .await
        );

        let result = adapter.attach(agent_id).await;
        assert!(matches!(result, Err(DriveError::StaleAgent(id)) if id == agent_id));
    }

    #[tokio::test]
    async fn late_kill_success_cannot_retire_newer_mapping() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let (request_tx, request_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let line = lines.next_line().await.expect("request").expect("line");
            let request: Value = serde_json::from_str(&line).expect("json request");
            request_tx.send(()).expect("request observer");
            release_rx.await.expect("release kill response");
            let response = json!({"id": request["id"], "result": {"ok": true}}).to_string() + "\n";
            write
                .write_all(response.as_bytes())
                .await
                .expect("response");
            request
        });

        let store = Store::new();
        let adapter = HerdrAdapter::new(socket_path);
        adapter.attach_store(store.clone());
        let first: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-kill-race"},
            "agent_status": "working",
            "name": "same-target",
            "pane_id": "w-kill-old:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&first, &store).await;

        let agent_id = "herdr:ses-kill-race";
        let kill = adapter.drive(agent_id, DriveCommand::Kill);
        tokio::pin!(kill);
        tokio::select! {
            _ = request_rx => {},
            result = &mut kill => panic!("kill completed before migration: {result:?}"),
        }

        let moved: PaneInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-kill-race"},
            "agent_status": "working",
            "display_agent": "same-target",
            "pane_id": "w-kill-new:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter
            .state
            .lock()
            .unwrap()
            .subscribed_panes
            .insert("w-kill-new:p1".to_string());
        let (sink, _rx) = mpsc::channel(16);
        adapter.handle_pane_updated(&moved, sink, &store).await;
        release_tx.send(()).expect("release kill response");

        assert!(matches!(
            kill.await,
            Err(DriveError::StaleAgent(id)) if id == agent_id
        ));
        server.await.expect("server task");
        assert_eq!(
            adapter.drive_target(agent_id).unwrap(),
            "same-target",
            "a successful kill of the old pane must not retire the migrated mapping"
        );
        assert!(!adapter.is_stale_agent(agent_id));
        assert!(store.snapshot().await.agents.contains_key(agent_id));
    }

    #[tokio::test]
    async fn server_agent_not_found_retires_store_row_for_read_prompt_and_approve() {
        // This is a local JSON-RPC mock, not a live Herdr proof. It exercises
        // the production response/error path for all three controls so an
        // asynchronous server rejection cannot be reported as success.
        for control in ["read_tail", "prompt", "approve"] {
            let dir = tempfile::tempdir().expect("tempdir");
            let socket_path = dir.path().join("herdr.sock");
            let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.expect("accept");
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                let line = lines.next_line().await.expect("request").expect("line");
                let request: Value = serde_json::from_str(&line).expect("json request");
                let response = json!({
                    "id": request["id"],
                    "error": {"code": "agent_not_found", "message": "agent not found"}
                })
                .to_string()
                    + "\n";
                write
                    .write_all(response.as_bytes())
                    .await
                    .expect("response");
                request
            });

            let store = Store::new();
            let adapter = HerdrAdapter::new(socket_path);
            adapter.attach_store(store.clone());
            adapter
                .register_agent_pane("p1", "opencode", AgentState::Working, &store)
                .await;
            let agent_id = "herdr:pane:p1";
            let result = match control {
                "read_tail" => adapter.read_tail(agent_id, 10).await.map(|_| ()),
                "prompt" => {
                    adapter
                        .drive(
                            agent_id,
                            DriveCommand::Prompt {
                                text: "hello".into(),
                            },
                        )
                        .await
                }
                "approve" => {
                    adapter
                        .drive(agent_id, DriveCommand::Approve { choice: "y".into() })
                        .await
                }
                _ => unreachable!(),
            };
            assert!(matches!(result, Err(DriveError::StaleAgent(id)) if id == agent_id));
            let request = tokio::time::timeout(Duration::from_secs(2), server)
                .await
                .expect("RPC timeout")
                .expect("server task");
            assert_eq!(
                request["method"].as_str(),
                Some(match control {
                    "read_tail" => "agent.read",
                    _ => "agent.prompt",
                })
            );
            assert!(
                store.snapshot().await.agents.is_empty(),
                "{control} RPC stale retires the canonical store row"
            );
            assert!(
                adapter.is_stale_agent(agent_id),
                "{control} RPC stale leaves a bounded refresh tombstone"
            );
            assert!(
                adapter.drive_target(agent_id).is_err(),
                "{control} RPC stale leaves no dispatchable control target"
            );
        }
    }

    #[tokio::test]
    async fn late_rpc_stale_does_not_retire_new_generation_or_store_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let (request_tx, request_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let line = lines.next_line().await.expect("request").expect("line");
            let request: Value = serde_json::from_str(&line).expect("json request");
            request_tx.send(()).expect("request observer");
            release_rx.await.expect("release stale response");
            let response = json!({
                "id": request["id"],
                "error": {"code": "agent_not_found", "message": "agent not found"}
            })
            .to_string()
                + "\n";
            write
                .write_all(response.as_bytes())
                .await
                .expect("response");
        });

        let store = Store::new();
        let adapter = HerdrAdapter::new(socket_path);
        adapter.attach_store(store.clone());
        let first: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-generation"},
            "agent_status": "working",
            "name": "same-target",
            "pane_id": "w-old:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&first, &store).await;

        let agent_id = "herdr:ses-generation";
        let stale_rpc = adapter.read_tail(agent_id, 10);
        tokio::pin!(stale_rpc);
        tokio::select! {
            _ = request_rx => {},
            result = &mut stale_rpc => panic!("stale RPC completed before migration: {result:?}"),
        }

        let moved: PaneInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-generation"},
            "agent_status": "working",
            "display_agent": "same-target",
            "pane_id": "w-new:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter
            .state
            .lock()
            .unwrap()
            .subscribed_panes
            .insert("w-new:p1".to_string());
        let (sink, _rx) = mpsc::channel(16);
        adapter.handle_pane_updated(&moved, sink, &store).await;
        release_tx.send(()).expect("release stale response");

        assert!(matches!(stale_rpc.await, Err(DriveError::StaleAgent(id)) if id == agent_id));
        server.await.expect("server task");
        assert_eq!(
            adapter.drive_target(agent_id).unwrap(),
            "same-target",
            "generation guard preserves a migrated mapping even when its wire target is unchanged"
        );
        assert!(!adapter.is_stale_agent(agent_id));
        assert!(store.snapshot().await.agents.contains_key(agent_id));
    }

    #[tokio::test]
    async fn conditional_rpc_removal_preserves_newer_same_target_generation() {
        let (release_tx, release_rx) = oneshot::channel();
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        adapter.attach_store(store.clone());
        let first: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-conditional"},
            "agent_status": "working",
            "name": "same-target",
            "pane_id": "w-old:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&first, &store).await;
        let agent_id = "herdr:ses-conditional";
        let first_seq = store.get(agent_id).await.expect("first row").seq;
        let generation = adapter
            .state
            .lock()
            .unwrap()
            .agent_generations
            .get(agent_id)
            .copied()
            .expect("first generation");

        let (retired_tx, retired_rx) = oneshot::channel();
        adapter.pause_before_store_remove(retired_tx, release_rx);
        let retirement = adapter.retire_rpc_mapping(agent_id, "same-target", generation);
        tokio::pin!(retirement);
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                result = retired_rx => result.expect("retirement reached"),
                _ = &mut retirement => panic!("retirement completed before store-removal pause"),
            }
        })
        .await
        .expect("retirement pause timeout");
        assert!(
            adapter.drive_target(agent_id).is_err(),
            "state is retired before the conditional store remove"
        );

        let newer: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-conditional"},
            "agent_status": "working",
            "name": "same-target",
            "pane_id": "w-new:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&newer, &store).await;
        assert_eq!(adapter.drive_target(agent_id).unwrap(), "same-target");
        let newer_seq = store.get(agent_id).await.expect("newer row").seq;
        assert!(newer_seq > first_seq, "new generation upserted a newer row");

        release_tx.send(()).expect("release conditional removal");
        assert!(
            !retirement.await,
            "a newer live mapping must not be reported as a successful retire"
        );
        assert_eq!(adapter.drive_target(agent_id).unwrap(), "same-target");
        assert!(store.snapshot().await.agents.contains_key(agent_id));
    }

    #[tokio::test]
    async fn conditional_rpc_removal_discards_same_generation_store_update() {
        let (release_tx, release_rx) = oneshot::channel();
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        adapter.attach_store(store.clone());
        let first: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-same-generation"},
            "agent_status": "working",
            "name": "same-target",
            "pane_id": "w-old:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&first, &store).await;
        let agent_id = "herdr:ses-same-generation";
        let generation = adapter
            .state
            .lock()
            .unwrap()
            .agent_generations
            .get(agent_id)
            .copied()
            .expect("first generation");

        let (retired_tx, retired_rx) = oneshot::channel();
        adapter.pause_before_store_remove(retired_tx, release_rx);
        let retirement = adapter.retire_rpc_mapping(agent_id, "same-target", generation);
        tokio::pin!(retirement);
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                result = retired_rx => result.expect("retirement reached"),
                _ = &mut retirement => panic!("retirement completed before store-removal pause"),
            }
        })
        .await
        .expect("retirement pause timeout");
        assert!(adapter.drive_target(agent_id).is_err());

        // A status/integration update for the retired generation can still
        // land before cleanup. It must not make Store cleanup miss the row.
        let mut derived_update = store.get(agent_id).await.expect("row before update");
        derived_update.state = AgentState::Blocked;
        derived_update.workspace.branch = Some("derived-update".to_string());
        derived_update.seq += 1;
        store.apply(Change::upsert(derived_update)).await;
        assert!(store.get(agent_id).await.is_some());

        release_tx.send(()).expect("release conditional removal");
        assert!(
            retirement.await,
            "retiring the only live mapping must be reported as retired"
        );
        assert!(store.get(agent_id).await.is_none());
        assert!(store.snapshot().await.agents.is_empty());
    }

    #[tokio::test]
    async fn stale_cleanup_rejects_inflight_event_upsert_after_store_read() {
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        adapter.attach_store(store.clone());
        let first: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-event-race"},
            "agent_status": "working",
            "name": "event-target",
            "pane_id": "w-event:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&first, &store).await;
        store.flush().await;
        let agent_id = "herdr:ses-event-race";
        let generation = adapter
            .state
            .lock()
            .unwrap()
            .agent_generations
            .get(agent_id)
            .copied()
            .expect("first generation");

        let status: StatusChangedWire = serde_json::from_value(json!({
            "pane_id": "w-event:p1",
            "agent_status": "blocked",
            "agent": "opencode",
            "state_labels": {"waiting_for_input": ""}
        }))
        .unwrap();
        let (read_tx, read_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        adapter.pause_after_event_store_read(read_tx, release_rx);
        let writer = adapter.handle_status_changed(&status, &store);
        tokio::pin!(writer);
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                result = read_rx => result.expect("event Store read reached"),
                _ = &mut writer => panic!("event writer completed before read barrier"),
            }
        })
        .await
        .expect("event read pause timeout");

        // Cleanup wins while the event still holds its cloned row.
        adapter
            .retire_rpc_mapping(agent_id, "event-target", generation)
            .await;
        assert!(store.get(agent_id).await.is_none());

        release_tx.send(()).expect("release event writer");
        writer.await;
        let delta = store.flush().await.expect("stale cleanup delta");
        assert!(
            delta.upd.is_empty(),
            "stale event must not resurrect the row"
        );
        assert_eq!(delta.del, vec![agent_id.to_string()]);
        assert!(store.snapshot().await.agents.is_empty());
    }

    #[tokio::test]
    async fn late_pane_events_cannot_resurrect_migrated_pane() {
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        let first: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-late"},
            "agent_status": "working",
            "name": "old-target",
            "pane_id": "w-old:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&first, &store).await;
        let moved: PaneInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-late"},
            "agent_status": "working",
            "display_agent": "new-target",
            "pane_id": "w-new:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter
            .state
            .lock()
            .unwrap()
            .subscribed_panes
            .insert("w-new:p1".to_string());
        let (sink, _rx) = mpsc::channel(16);
        adapter.handle_pane_updated(&moved, sink, &store).await;

        let late: PaneInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-late"},
            "agent_status": "blocked",
            "display_agent": "late-old-target",
            "pane_id": "w-old:p1",
            "state_labels": {}
        }))
        .unwrap();
        let (sink, _rx) = mpsc::channel(16);
        adapter.handle_pane_updated(&late, sink, &store).await;
        adapter
            .handle_event(
                ParsedEvent::AgentDetected(AgentDetectedWire {
                    pane_id: "w-old:p1".to_string(),
                    agent: Some("opencode".to_string()),
                    released: None,
                }),
                mpsc::channel(16).0,
                &store,
            )
            .await;

        assert_eq!(
            adapter.drive_target("herdr:ses-late").unwrap(),
            "new-target"
        );
        let (old_present, old_stale) = {
            let state = adapter.state.lock().unwrap();
            (
                state.pane_agents.contains_key("w-old:p1"),
                state.stale_panes.contains_key("w-old:p1"),
            )
        };
        assert!(!old_present);
        assert!(old_stale);
        assert_eq!(store.snapshot().await.agents.len(), 1);
    }

    #[tokio::test]
    async fn reconnect_list_remap_is_atomic_and_keeps_stable_agent_live() {
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        let first: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-remap"},
            "agent_status": "working",
            "name": "old-target",
            "pane_id": "w-old:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&first, &store).await;
        let list: AgentListWire = serde_json::from_value(json!({"agents": [{
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-remap"},
            "agent_status": "working",
            "name": "new-target",
            "pane_id": "w-new:p1",
            "state_labels": {}
        }]}))
        .unwrap();

        adapter.reconcile_against_list(&list, &store).await;

        assert_eq!(
            adapter.drive_target("herdr:ses-remap").unwrap(),
            "new-target"
        );
        let (agent_stale, old_present, old_stale) = {
            let state = adapter.state.lock().unwrap();
            (
                state.stale_agents.contains_key("herdr:ses-remap"),
                state.pane_agents.contains_key("w-old:p1"),
                state.stale_panes.contains_key("w-old:p1"),
            )
        };
        assert!(!agent_stale);
        assert!(!old_present);
        assert!(old_stale);
        let snapshot = store.snapshot().await;
        assert!(snapshot.agents.contains_key("herdr:ses-remap"));
        assert!(!snapshot.agents.contains_key("herdr:pane:w-old:p1"));
    }

    #[test]
    fn tombstones_are_bounded() {
        let mut state = SessionState::default();
        for i in 0..(STALE_TOMBSTONE_CAP + 17) {
            state.mark_stale_agent(format!("agent-{i}"));
            state.mark_stale_pane(format!("pane-{i}"));
        }
        assert!(state.stale_agents.len() <= STALE_TOMBSTONE_CAP);
        assert!(state.stale_panes.len() <= STALE_TOMBSTONE_CAP);
    }

    #[test]
    fn live_generation_map_is_bounded_and_never_reuses_tokens() {
        let mut state = SessionState::default();
        let mut previous = 0;
        for i in 0..(STALE_TOMBSTONE_CAP * 4) {
            let agent_id = format!("herdr:unique-{i}");
            let generation = state.allocate_generation(&agent_id);
            assert!(generation > previous, "generation allocation is monotonic");
            previous = generation;
            state.clear_generation(&agent_id);
        }
        assert!(
            state.agent_generations.is_empty(),
            "only live mappings retain generation entries"
        );

        let old = state.allocate_generation("herdr:reused");
        state.clear_generation("herdr:reused");
        let new = state.allocate_generation("herdr:reused");
        assert!(new > old, "a future mapping cannot reuse an old RPC token");
        assert_eq!(state.agent_generations.len(), 1);
    }

    // #105 regression: exercise the production reader -> forwarder -> sink
    // topology. Overflow must resolve the subscribe response, then close
    // the stream so the session can re-bootstrap instead of losing state.
    #[tokio::test]
    async fn subscribe_response_is_not_starved_by_replay_flood() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let server = tokio::spawn(async move {
            let (server, _) = listener.accept().await.expect("accept");
            let (read, mut write) = server.into_split();
            let mut lines = BufReader::new(read).lines();
            let _request = lines.next_line().await.expect("subscribe request");
            let mut flood = String::new();
            for i in 0..(FRAME_CHANNEL_CAP * 2 + 64) {
                flood.push_str(
                    &json!({
                        "event": "pane_updated",
                        "data": {"pane": {"pane_id": i.to_string(), "agent": "codex"}}
                    })
                    .to_string(),
                );
                flood.push('\n');
            }
            write
                .write_all(flood.as_bytes())
                .await
                .expect("write flood");
            write.flush().await.expect("flush flood");
            let response = json!({ "id": "corral:0", "result": { "ok": true } }).to_string() + "\n";
            write
                .write_all(response.as_bytes())
                .await
                .expect("write response");
            write.flush().await.expect("flush response");
            tokio::time::sleep(Duration::from_secs(3)).await;
        });

        let (sink, mut sink_rx) = mpsc::channel(FRAME_CHANNEL_CAP);
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            run_event_stream(socket_path, vec![], sink),
        )
        .await
        .expect("subscribe response timeout");
        let EventStreamExit::Subscribed(mut live) = result else {
            panic!("subscribe must resolve before overflow reconnect");
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                tokio::select! {
                    result = &mut live.handle => {
                        result.expect("forwarder task must not panic");
                        break;
                    }
                    frame = sink_rx.recv() => {
                        match frame {
                            Some(SinkFrame::Event { .. }) => {}
                            None => {
                                (&mut live.handle)
                                    .await
                                    .expect("forwarder task must not panic");
                                break;
                            }
                            Some(SinkFrame::Closed { .. }) => {
                                panic!("run_event_stream must not close the session sink")
                            }
                        }
                    }
                }
            }
        })
        .await
        .expect("overflow must retire the forwarder");
        server.abort();
    }

    // #117 regression: a fake herdr that accepts every global subscription and
    // then closes it must not make the session re-bootstrap and resubscribe at
    // full speed. The policy is intentionally short here so the test proves
    // the production supervisor's timing and cap without taking minutes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum GlobalServerEvent {
        AgentList,
        Subscription(usize),
        ConnectionClosed(usize),
    }

    async fn wait_for_client_close(lines: &mut tokio::io::Lines<BufReader<OwnedReadHalf>>) {
        let result = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("fake herdr client close timeout")
            .expect("fake herdr client close read");
        assert!(result.is_none(), "fake herdr client must close its socket");
    }

    async fn serve_repeatedly_closed_global_stream(
        listener: tokio::net::UnixListener,
        close_count: usize,
        stable_for: Duration,
        events: mpsc::Sender<(GlobalServerEvent, Instant)>,
        stop: oneshot::Receiver<()>,
    ) {
        let mut stop = std::pin::pin!(stop);
        let mut subscriptions = 0;
        loop {
            let (stream, _) = tokio::select! {
                accepted = listener.accept() => accepted.expect("fake herdr accept"),
                _ = &mut stop => return,
            };
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let request = lines
                .next_line()
                .await
                .expect("fake herdr request read")
                .expect("fake herdr request");
            let request: Value = serde_json::from_str(&request).expect("fake herdr request json");
            let id = request["id"].clone();
            match request["method"].as_str() {
                Some("agent.list") => {
                    events
                        .send((GlobalServerEvent::AgentList, Instant::now()))
                        .await
                        .expect("regression receiver alive");
                    let response = json!({ "id": id, "result": { "agents": [] } });
                    write_json_line(&mut write, response).await;
                    drop(write);
                    wait_for_client_close(&mut lines).await;
                }
                Some("events.subscribe") => {
                    let index = subscriptions;
                    subscriptions += 1;
                    events
                        .send((GlobalServerEvent::Subscription(index), Instant::now()))
                        .await
                        .expect("regression receiver alive");
                    let response = json!({ "id": id, "result": null });
                    write_json_line(&mut write, response).await;
                    if index == close_count {
                        // One genuinely stable stream must reset the outage
                        // ladder before it closes, then the next stream is
                        // kept alive for the recovery assertion.
                        tokio::time::sleep(stable_for).await;
                        drop(write);
                        wait_for_client_close(&mut lines).await;
                        events
                            .send((GlobalServerEvent::ConnectionClosed(index), Instant::now()))
                            .await
                            .expect("regression receiver alive");
                    } else if index > close_count {
                        // Keep the recovered stream alive until the test has
                        // observed a full stable interval. This is recovery
                        // on the same fake daemon, not a restart.
                        let _ = stop.await;
                        drop(write);
                        wait_for_client_close(&mut lines).await;
                        events
                            .send((GlobalServerEvent::ConnectionClosed(index), Instant::now()))
                            .await
                            .expect("regression receiver alive");
                        return;
                    } else {
                        drop(write);
                        wait_for_client_close(&mut lines).await;
                        events
                            .send((GlobalServerEvent::ConnectionClosed(index), Instant::now()))
                            .await
                            .expect("regression receiver alive");
                    }
                }
                method => panic!("unexpected fake herdr method: {method:?}"),
            }
        }
    }

    async fn write_json_line(write: &mut tokio::net::unix::OwnedWriteHalf, value: Value) {
        let mut wire = value.to_string();
        wire.push('\n');
        write
            .write_all(wire.as_bytes())
            .await
            .expect("fake herdr response write");
        write.flush().await.expect("fake herdr response flush");
    }

    #[tokio::test]
    async fn accepted_then_closed_global_streams_back_off_and_recover() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let (events_tx, mut events_rx) = mpsc::channel(32);
        let (stop_tx, stop_rx) = oneshot::channel();
        let close_count = 4;
        let stable_for = Duration::from_millis(40);
        let server = tokio::spawn(serve_repeatedly_closed_global_stream(
            listener,
            close_count,
            stable_for,
            events_tx,
            stop_rx,
        ));
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from(&socket_path));
        let session = tokio::spawn(async move {
            adapter
                .session_with_policy(
                    &store,
                    StreamRetryPolicy {
                        base: Duration::from_millis(10),
                        max: Duration::from_millis(40),
                        reset_after: Duration::from_millis(30),
                    },
                )
                .await
        });

        let mut agent_lists = 0;
        let mut subscriptions = Vec::new();
        let mut closed_connections = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while subscriptions.len() <= close_count + 1 {
            let remaining = deadline
                .checked_duration_since(tokio::time::Instant::now())
                .expect("fake herdr regression timed out");
            let (event, at) = tokio::time::timeout(remaining, events_rx.recv())
                .await
                .expect("fake herdr event timeout")
                .expect("fake herdr server still running");
            match event {
                GlobalServerEvent::AgentList => agent_lists += 1,
                GlobalServerEvent::Subscription(index) => {
                    assert_eq!(index, subscriptions.len(), "subscription order");
                    subscriptions.push(at);
                }
                GlobalServerEvent::ConnectionClosed(index) => {
                    assert_eq!(index, closed_connections, "closed socket order");
                    closed_connections += 1;
                }
            }
        }

        assert_eq!(
            agent_lists,
            close_count + 2,
            "one delayed reconcile per closed stream"
        );
        assert_eq!(subscriptions.len(), close_count + 2);
        let gaps: Vec<Duration> = subscriptions
            .windows(2)
            .map(|window| window[1].duration_since(window[0]))
            .collect();
        assert!(
            gaps[0] >= Duration::from_millis(8),
            "base retry gap: {gaps:?}"
        );
        assert!(
            gaps[1] >= Duration::from_millis(18),
            "doubled retry gap: {gaps:?}"
        );
        assert!(
            gaps[2] >= Duration::from_millis(38),
            "capped retry gap: {gaps:?}"
        );
        assert!(
            gaps[3] >= Duration::from_millis(38),
            "capped retry gap: {gaps:?}"
        );
        assert!(
            gaps[4] >= stable_for + Duration::from_millis(8),
            "stable stream still needs the reset base delay: {gaps:?}"
        );
        assert!(
            gaps[4] < stable_for + Duration::from_millis(25),
            "stable stream resets instead of using the capped delay: {gaps:?}"
        );

        // The final subscription remains live on the same fake socket. Wait
        // beyond the stable interval and prove the session does not churn a
        // new task or re-bootstrap while the stream is healthy.
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(
            events_rx.try_recv().is_err(),
            "stable stream must stay quiet"
        );

        let _ = stop_tx.send(());
        session.abort();
        let _ = session.await;
        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("final stream socket teardown timeout")
            .expect("fake herdr server task");
        while let Ok(event) = events_rx.try_recv() {
            if let GlobalServerEvent::ConnectionClosed(index) = event.0 {
                assert_eq!(index, closed_connections, "closed socket order");
                closed_connections += 1;
            }
        }
        assert_eq!(closed_connections, close_count + 2);
    }

    #[tokio::test]
    async fn session_refreshes_catalog_without_global_stream_close() {
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        let initial: AgentListWire = serde_json::from_value(json!({ "agents": [
            {
                "agent": "opencode",
                "agent_session": {"agent": "opencode", "kind": "id",
                    "source": "herdr:opencode", "value": "watchdog-a"},
                "agent_status": "idle",
                "state_change_seq": 10,
                "name": "agent-a",
                "pane_id": "watchdog-a:p1",
                "state_labels": {}
            },
            {
                "agent": "claude",
                "agent_status": "working",
                "state_change_seq": 7,
                "name": "agent-b",
                "pane_id": "watchdog-b:p1",
                "state_labels": {}
            }
        ] }))
        .unwrap();
        let refreshed: AgentListWire = serde_json::from_value(json!({ "agents": [
            {
                "agent": "opencode",
                "agent_session": {"agent": "opencode", "kind": "id",
                    "source": "herdr:opencode", "value": "watchdog-a"},
                "agent_status": "working",
                "state_change_seq": 11,
                "name": "agent-a",
                "pane_id": "watchdog-a:p1",
                "state_labels": {}
            },
            {
                "agent": "codex",
                "agent_session": {"agent": "codex", "kind": "id",
                    "source": "herdr:codex", "value": "watchdog-c"},
                "agent_status": "blocked",
                "state_change_seq": 1,
                "name": "agent-c",
                "pane_id": "watchdog-c:p1",
                "state_labels": {}
            }
        ] }))
        .unwrap();
        let calls = Arc::new(AtomicU64::new(0));
        let (call_tx, mut call_rx) = mpsc::channel::<u64>(8);
        let provider_calls = calls.clone();
        let call_sender = call_tx.clone();
        adapter.set_catalog_provider(Arc::new(move || {
            let version = provider_calls.fetch_add(1, Ordering::SeqCst);
            let _ = call_sender.try_send(version);
            if version == 0 {
                initial.clone()
            } else {
                refreshed.clone()
            }
        }));

        let store = Store::new();
        let session_store = store.clone();
        let session = tokio::spawn(async move {
            adapter
                .session_with_freshness(
                    &session_store,
                    StreamRetryPolicy {
                        base: Duration::from_millis(10),
                        max: Duration::from_millis(40),
                        reset_after: Duration::from_millis(30),
                    },
                    CatalogFreshnessPolicy {
                        interval: Duration::from_millis(20),
                    },
                )
                .await
        });

        let converged = tokio::time::timeout(Duration::from_secs(3), async {
            // Version 2 means the refreshed reconcile started after the
            // previous refresh completed; awaiting the provider call itself
            // makes convergence deterministic instead of racing wall-clock
            // sleeps.
            loop {
                let version = call_rx.recv().await.expect("watchdog call");
                if version >= 2 {
                    break;
                }
            }
            let snapshot = store.snapshot().await;
            assert!(
                snapshot
                    .agents
                    .get("herdr:watchdog-a")
                    .is_some_and(|a| a.state == AgentState::Working)
                    && snapshot
                        .agents
                        .get("herdr:watchdog-c")
                        .is_some_and(|c| c.state == AgentState::Blocked)
                    && !snapshot.agents.contains_key("herdr:pane:watchdog-b:p1"),
                "watchdog catalog did not converge: {snapshot:?}"
            );
            snapshot
        })
        .await
        .expect("watchdog catalog did not converge");
        assert_eq!(converged.agents.len(), 2);
        assert!(converged.agents.contains_key("herdr:watchdog-a"));
        assert!(converged.agents.contains_key("herdr:watchdog-c"));

        let stable_rev = converged.rev;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let version = call_rx.recv().await.expect("watchdog call");
                if version >= 3 {
                    break;
                }
            }
        })
        .await
        .expect("watchdog stopped refreshing");
        let after_noop = store.snapshot().await;
        assert_eq!(after_noop.rev, stable_rev);
        assert!(
            calls.load(Ordering::SeqCst) >= 4,
            "watchdog must keep refreshing while no stream close arrives"
        );

        session.abort();
        let _ = session.await;
    }

    // #105 fd teardown: a dropped client must close its socket promptly.
    // The reader task owns the read half; without deterministic teardown it
    // lingers (blocked on next_line) until herdr closes the idle connection,
    // so every failed subscribe and one-shot rpc_call leaks a descriptor
    // while the timeout storm rages.
    //
    // NOTE: dropping the write half alone must NOT count as teardown —
    // tokio's OwnedWriteHalf::drop only shutdown(SHUT_WR)s, and a live
    // reader keeps the fd (and its read half) open. The observable that
    // discriminates is whether the reader still forwards frames after the
    // client is dropped: an unfixed reader does (RED), a torn-down one
    // cannot (GREEN).
    #[tokio::test]
    async fn dropped_client_stops_the_reader() {
        let (server, client) = UnixStream::pair().expect("socketpair");
        let (write_now, write_later) = oneshot::channel();
        let server = tokio::spawn(async move {
            let mut server = server;
            write_later.await.expect("drop synchronization");
            let frame = json!({ "event": "pane_updated", "data": {} }).to_string() + "\n";
            let _ = server.write_all(frame.as_bytes()).await;
        });

        let (client, mut events) = RpcClient::new(client);
        drop(client); // last Arc: must abort the reader (read half teardown)
        write_now.send(()).expect("server still waiting");
        let closed = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("receiver closure timeout");
        server.abort();
        assert!(closed.is_none(), "dropped client must close event receiver");
    }

    #[tokio::test]
    async fn reader_exit_cancels_pending_call() {
        let (server, client) = UnixStream::pair().expect("socketpair");
        let server = tokio::spawn(async move {
            let (read, _) = server.into_split();
            let mut lines = BufReader::new(read).lines();
            let _request = lines.next_line().await.expect("request");
            // Closing the read half makes the client reader exit without a
            // response; the pending call must fail immediately, not timeout.
        });
        let (client, _events) = RpcClient::new(client);
        let call = tokio::spawn({
            let client = client.clone();
            async move { client.call("agent.list", json!({})).await }
        });
        let result = tokio::time::timeout(Duration::from_secs(1), call)
            .await
            .expect("pending call cancellation timeout")
            .expect("call task panicked");
        server.await.expect("server task panicked");
        assert!(matches!(result, Err(RpcError::Disconnected)));
    }

    #[tokio::test]
    async fn call_after_reader_exit_is_rejected_without_timeout() {
        let (server, client) = UnixStream::pair().expect("socketpair");
        let (client, mut events) = RpcClient::new(client);
        drop(server);

        let closed = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("reader exit timeout");
        assert!(closed.is_none(), "reader exit must close event receiver");
        let result =
            tokio::time::timeout(Duration::from_secs(1), client.call("agent.list", json!({})))
                .await
                .expect("closed-call registration timeout");
        assert!(matches!(result, Err(RpcError::Disconnected)));
    }
}

// ---------------------------------------------------------------------------
// AC2 (P3 verdict gate): the LIVE claim flow against a real blocked herdr
// agent — the wrong-question race simulation.
//
// Gated by env so the normal suite stays hermetic:
//   CORRAL_AC2=1              enable the live test
//   CORRAL_AC2_PANE=<pane_id> the pane whose agent must be BLOCKED on a prompt
//   CORRAL_SOCKET=<path>      optional herdr socket override
//
// Exercises the production paths end to end over the real herdr socket:
// agent.list bootstrap -> claim emission from the pane's REAL output ->
// claim check (stale hash / stale id refused, correct hash + choice
// executes) -> approve dispatch -> agent unblocks.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod ac2_live_tests {
    use super::*;
    use crate::approve::{ApprovalError, claim_for};
    use serde_json::json;

    /// Emulate the pane.output_matched match (PROMPT_REGEX over the
    /// recent_unwrapped window) to obtain the exact matched line the real
    /// subscription would deliver. herdr unwraps full terminal lines, so a
    /// question line arrives as one WIDE line whose `?` may sit mid-line
    /// (right-hand column merged in). The prompt is the last line containing
    /// both a question mark and a prompt phrase (e.g. the footer "…select…"
    /// matches phrases but has no `?` and must not win). The chosen line is
    /// hashed EXACTLY as delivered — untrimmed — which is the D8 contract.
    fn ac2_matched_line(read_text: &str) -> Option<&str> {
        let phrases = [
            "approve",
            "approval",
            "permission",
            "allow this",
            "confirm",
            "proceed?",
            "continue?",
            "do you want",
            "should i",
            "are you sure",
            "is that",
            "is this",
            "waiting for",
            "select",
            "choose",
            "[y/n]",
            "(y/n)",
            "yes/no",
            "please review",
            "need your input",
            "your decision",
        ];
        let lines: Vec<&str> = read_text.lines().collect();
        lines
            .iter()
            .rev()
            .find(|line| {
                let lower = line.trim().to_lowercase();
                lower.contains('?') && phrases.iter().any(|k| lower.contains(k))
            })
            .or_else(|| {
                lines.iter().rev().find(|line| {
                    let lower = line.trim().to_lowercase();
                    phrases.iter().any(|k| lower.contains(k))
                })
            })
            .copied()
    }
    fn ac2_env() -> Option<(PathBuf, String)> {
        if std::env::var("CORRAL_AC2").as_deref() != Ok("1") {
            return None;
        }
        let pane = std::env::var("CORRAL_AC2_PANE").ok()?;
        let socket = std::env::var("CORRAL_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                PathBuf::from(home).join(".config/herdr/herdr.sock")
            });
        Some((socket, pane))
    }

    fn ac2_evidence(name: &str, value: &impl serde::Serialize) {
        println!(
            "AC2_EVIDENCE {} {}",
            name,
            serde_json::to_string(value).unwrap()
        );
    }

    #[tokio::test]
    async fn ac2_live_claim_flow() {
        let Some((socket, pane)) = ac2_env() else {
            return;
        };

        // 1. Bootstrap over the real socket: agent.list.
        let list = rpc_call(&socket, "agent.list", json!({}))
            .await
            .expect("agent.list");
        let list: AgentListWire = serde_json::from_value(list).expect("agent list decode");
        let info = list
            .agents
            .iter()
            .find(|a| a.pane_id == pane)
            .expect("CORRAL_AC2_PANE must be in agent.list");
        assert_eq!(
            info.agent_status.as_str(),
            "blocked",
            "the AC2 agent must be blocked on a prompt before the test runs"
        );
        let store = Store::new();
        let adapter = HerdrAdapter::new(socket.clone());
        adapter.apply_agent_info(info, &store).await;
        let agent_id = {
            let state = adapter.state.lock().unwrap();
            state
                .pane_agents
                .get(&pane)
                .cloned()
                .expect("pane registered by bootstrap")
        };
        ac2_evidence(
            "bootstrap-agent",
            &json!({ "agent_id": agent_id, "pane_id": pane }),
        );

        // 2. Status change to blocked (mirrors the real event stream).
        let status = StatusChangedWire {
            pane_id: pane.clone(),
            agent_status: Some("blocked".to_string()),
            agent: info.agent.clone(),
            title: info.title.clone(),
            state_labels: info.state_labels.clone(),
            state_change_seq: None,
        };
        adapter.handle_status_changed(&status, &store).await;

        // 3. Real read: the pane's current output window, with the same
        //    source/lines the output_matched subscription uses.
        let read: Value = rpc_call(
            &socket,
            "agent.read",
            json!({ "target": pane, "source": "recent_unwrapped", "lines": 40 }),
        )
        .await
        .expect("agent.read");
        let read_text = read
            .get("read")
            .and_then(|r| r.get("text"))
            .and_then(|t| t.as_str())
            .expect("agent.read text")
            .to_string();
        let matched_line = ac2_matched_line(&read_text).expect("prompt line in read window");
        let mut hasher = Sha256::new();
        hasher.update(matched_line.as_bytes());
        ac2_evidence(
            "matched-line",
            &json!({ "line": matched_line, "hash": format!("sha256:{}", hex(&hasher.finalize())) }),
        );
        ac2_evidence("read-window", &read_text);
        let matched = OutputMatchedWire {
            pane_id: pane.clone(),
            matched_line: Some(matched_line.to_string()),
            read: Some(OutputReadWire {
                text: Some(read_text.clone()),
            }),
        };
        adapter.handle_output_matched(&matched, &store).await;

        // 4. The live claim, emitted by the production path.
        let agent = store.get(&agent_id).await.expect("agent record");
        let w = agent
            .waiting_on
            .as_ref()
            .expect("waiting_on set while blocked");
        let claim = claim_for(&agent_id, w);
        assert_eq!(
            claim.approval_id, w.approval_id,
            "derived claim == stored claim"
        );
        assert_eq!(claim.prompt_hash, w.prompt_hash);
        ac2_evidence("approval-claim", &claim);

        // 5a. Wrong-question race: SAME approval_id, STALE prompt_hash ->
        //     typed refusal, nothing dispatched.
        let stale_hash = format!("sha256:{}", "0".repeat(64));
        let refusal = crate::approve::check_approval_claim(
            &agent_id,
            Some(w),
            &claim.approval_id,
            &stale_hash,
            "1",
        );
        assert_eq!(refusal, Err(ApprovalError::HashMismatch));
        ac2_evidence("stale-hash-approve", &format!("{refusal:?}"));

        // 5b. Stale approval identity -> typed refusal.
        let stale_id = format!("herdr:stale-agent:sha256:{}", "0".repeat(64));
        let refusal = crate::approve::check_approval_claim(
            &agent_id,
            Some(w),
            &stale_id,
            &claim.prompt_hash,
            "1",
        );
        assert_eq!(refusal, Err(ApprovalError::StaleApproval));
        ac2_evidence("stale-id-approve", &format!("{refusal:?}"));

        // 6. Correct hash + choice -> the claim executes and is dispatched
        //    over the real socket (agent.prompt = the pane's input send).
        let approved = crate::approve::check_approval_claim(
            &agent_id,
            Some(w),
            &claim.approval_id,
            &claim.prompt_hash,
            "1",
        )
        .expect("matching claim executes");
        ac2_evidence("approved", &approved);
        adapter
            .drive(
                &agent_id,
                DriveCommand::Approve {
                    choice: approved.choice.clone(),
                },
            )
            .await
            .expect("approve dispatch accepted");

        // 7. The agent receives the input and leaves blocked (agent.wait is
        //    herdr's event-driven wait, not polling).
        let waited: Value = rpc_call(
            &socket,
            "agent.wait",
            json!({ "target": pane, "until": ["working", "done", "idle"], "timeout_ms": 30000 }),
        )
        .await
        .expect("agent.wait");
        ac2_evidence("agent-after-approve", &waited);

        // 8. Mirror the real status_changed(working) the event stream would
        //    deliver after the approve: the consumed approval must not stay
        //    live in the record.
        let working = StatusChangedWire {
            pane_id: pane.clone(),
            agent_status: Some("working".to_string()),
            agent: info.agent.clone(),
            title: info.title.clone(),
            state_labels: HashMap::new(),
            state_change_seq: None,
        };
        adapter.handle_status_changed(&working, &store).await;
        let after = store
            .get(&agent_id)
            .await
            .expect("agent record after approve");
        assert!(
            after.waiting_on.is_none(),
            "a consumed approval must not stay live in the record"
        );

        // 9. Evidence: the pane's own output now shows the answered prompt.
        let read_after: Value = rpc_call(
            &socket,
            "agent.read",
            json!({ "target": pane, "source": "visible", "lines": 30 }),
        )
        .await
        .expect("agent.read after approve");
        ac2_evidence(
            "pane-after-approve",
            &read_after
                .get("read")
                .and_then(|r| r.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or_default(),
        );
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use crate::core::model::AgentState as S;
    use crate::core::workspace::RepoRoot;
    use serde_json::json;

    fn adapter() -> HerdrAdapter {
        HerdrAdapter::new(PathBuf::from("/nonexistent.sock"))
    }

    #[tokio::test]
    async fn unregistered_pane_status_changed_keeps_actual_status() {
        // m4: a status_changed for a pane we never registered must create the
        // record with the event's status (blocked), not Unknown.
        let store = Store::new();
        let adapter = adapter();
        let ev = serde_json::from_value::<StatusChangedWire>(json!({
            "pane_id": "wX:p1",
            "agent_status": "blocked",
            "agent": "claude",
            "title": "Waiting on approval",
            "state_labels": {}
        }))
        .unwrap();
        adapter.handle_status_changed(&ev, &store).await;

        let snap = store.snapshot().await;
        let agent = snap.agents.get("herdr:pane:wX:p1").expect("record created");
        assert_eq!(agent.state, S::Blocked, "must not read Unknown");
        assert_eq!(agent.tool, "claude");
        assert!(adapter.knows_agent(&agent.agent_id));
    }

    #[tokio::test]
    async fn reconcile_removes_ghost_agents() {
        // M2: panes closed while a stream was down never emit pane.closed on
        // the new stream; the agent.list diff must drop the ghosts.
        let store = Store::new();
        let adapter = adapter();
        let claude: AgentInfoWire = serde_json::from_value(json!({
            "agent": "claude",
            "agent_session": {"agent": "claude", "kind": "id",
                "source": "herdr:claude", "value": "ses-live"},
            "agent_status": "idle",
            "pane_id": "wQ:p1",
            "cwd": "/Users/jirathip/worktrees/a",
            "name": "live-one",
            "state_labels": {}
        }))
        .unwrap();
        let ghost: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_status": "working",
            "pane_id": "wG:p1",
            "cwd": "/Users/jirathip/worktrees/ghost",
            "name": "ghost",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&claude, &store).await;
        adapter.apply_agent_info(&ghost, &store).await;
        assert_eq!(store.snapshot().await.agents.len(), 2);

        // Fresh agent.list: wG:p1 (and its pane) are gone.
        let list: AgentListWire = serde_json::from_value(json!({ "agents": [{
            "agent": "claude",
            "agent_session": {"agent": "claude", "kind": "id",
                "source": "herdr:claude", "value": "ses-live"},
            "agent_status": "idle",
            "pane_id": "wQ:p1",
            "cwd": "/Users/jirathip/worktrees/a",
            "name": "live-one",
            "state_labels": {}
        }] }))
        .unwrap();
        adapter.reconcile_against_list(&list, &store).await;

        let snap = store.snapshot().await;
        assert_eq!(snap.agents.len(), 1, "ghost agent must be removed");
        assert!(snap.agents.contains_key("herdr:ses-live"));
        assert!(!snap.agents.contains_key("herdr:pane:wG:p1"));
        assert!(
            !adapter.knows_agent("herdr:pane:wG:p1"),
            "state must forget the ghost too"
        );
    }

    #[tokio::test]
    async fn catalog_reconcile_evicts_superseded_session_from_sessionless_pane() {
        // #178 reachable single-adapter shape: a re-arm leaves the old pane in
        // agent.list without its explicit session while the replacement runs on
        // another pane. One session-less view is debounced so a transient
        // omission cannot hit a live pane; the second consecutive view
        // corroborates the old id is gone and migrates to the pane fallback,
        // letting the same refresh evict and tombstone it.
        let temp = tempfile::tempdir().expect("tempdir");
        let primary = temp.path().join("corral");
        let worktrees = temp.path().join("worktrees");
        let old_worktree = worktrees.join("corral/g178-superseded");
        let rearmed_worktree = worktrees.join("corral/g178-rearmed");
        for path in [&primary, &worktrees, &old_worktree, &rearmed_worktree] {
            std::fs::create_dir_all(path).unwrap();
        }
        let attribution = WorkspaceAttribution::from_roots(
            [RepoRoot {
                path: primary,
                repo: "corral".to_string(),
            }],
            worktrees.clone(),
        );
        attribution.record_branch(&old_worktree, "g178/superseded");
        attribution.record_branch(&rearmed_worktree, "g178/rearmed");

        let store = Store::new();
        let adapter =
            HerdrAdapter::new_with_attribution(PathBuf::from("/nonexistent.sock"), attribution);
        let old: AgentListWire = serde_json::from_value(json!({ "agents": [{
            "agent": "codex",
            "agent_session": {"agent": "codex", "kind": "id",
                "source": "herdr:codex", "value": "ses-superseded"},
            "agent_status": "working",
            "state_change_seq": 10,
            "name": "impl-g178-old",
            "pane_id": "w-g178-old:p1",
            "foreground_cwd": old_worktree.clone(),
            "state_labels": {}
        }] }))
        .unwrap();
        adapter.reconcile_against_list(&old, &store).await;
        let old_id = "herdr:ses-superseded";
        assert!(
            store.snapshot().await.agents.contains_key(old_id),
            "seed refresh must store the superseded session"
        );

        let refreshed: AgentListWire = serde_json::from_value(json!({ "agents": [
            {
                "agent": "codex",
                "agent_status": "idle",
                "state_change_seq": 11,
                "name": "impl-g178-old-unclaimed",
                "pane_id": "w-g178-old:p1",
                "foreground_cwd": old_worktree,
                "state_labels": {}
            },
            {
                "agent": "codex",
                "agent_session": {"agent": "codex", "kind": "id",
                    "source": "herdr:codex", "value": "ses-rearmed"},
                "agent_status": "blocked",
                "state_change_seq": 21,
                "name": "impl-g178",
                "pane_id": "w-g178-rearmed:p1",
                "foreground_cwd": rearmed_worktree,
                "state_labels": {"waiting_for_input": ""}
            }
        ] }))
        .unwrap();
        let fallback = "herdr:pane:w-g178-old:p1";
        adapter.reconcile_against_list(&refreshed, &store).await;

        let debounced = store.snapshot().await;
        assert!(
            debounced.agents.contains_key(old_id),
            "one session-less refresh must not demote an explicit session"
        );
        assert!(
            !debounced.agents.contains_key(fallback),
            "debounce must not create a duplicate fallback row"
        );
        assert!(
            !adapter.is_stale_agent(old_id),
            "debounce must not tombstone the still-live id"
        );
        assert!(
            adapter.drive_target(old_id).is_ok(),
            "debounce must keep the drive plane dispatchable"
        );
        assert_eq!(debounced.agents.len(), 2);

        adapter.reconcile_against_list(&refreshed, &store).await;
        let snapshot = store.snapshot().await;
        assert!(
            !snapshot.agents.contains_key(old_id),
            "a superseded session must be evicted after corroborating refetches"
        );
        let old_catalog_row = snapshot
            .agents
            .get(fallback)
            .expect("the still-listed session-less pane keeps its fallback row");
        assert_eq!(old_catalog_row.workspace.repo.as_deref(), Some("corral"));
        assert_eq!(
            old_catalog_row.workspace.branch.as_deref(),
            Some("g178/superseded")
        );
        let rearmed = snapshot
            .agents
            .get("herdr:ses-rearmed")
            .expect("live replacement must be inserted");
        assert_eq!(rearmed.state, S::Blocked);
        assert_eq!(rearmed.workspace.repo.as_deref(), Some("corral"));
        assert_eq!(rearmed.workspace.branch.as_deref(), Some("g178/rearmed"));
        assert_eq!(
            rearmed.attachment.as_ref().map(|a| a.reference.as_str()),
            Some("w-g178-rearmed:p1")
        );
        assert_eq!(
            snapshot.agents.len(),
            2,
            "the session-less pane and replacement must not leave an orphan row"
        );
        assert!(
            adapter.is_stale_agent(old_id),
            "eviction must leave the refreshable stale tombstone"
        );
        assert!(
            !adapter
                .remove_if_unmapped(&store, "herdr:ses-rearmed")
                .await,
            "a still-mapped live replacement must never be evicted"
        );
        assert!(
            matches!(adapter.drive_target(old_id), Err(DriveError::StaleAgent(id)) if id == old_id),
            "a late drive on the evicted id must be a refreshable 409, not 404"
        );
        // The migration itself tombstones the old id here; the sweep's own
        // orphan-row tombstone is pinned by the next test.
    }

    #[tokio::test]
    async fn catalog_sessionless_refresh_keeps_live_session_identity() {
        // NR1: a listed live pane may omit agent_session on one refresh. The
        // debounce must keep its explicit id, mapping, drive target and store
        // rev stable instead of demoting to `herdr:pane:<pane>` and returning
        // 409 for a working agent.
        let store = Store::new();
        let adapter = adapter();
        let initial: AgentListWire = serde_json::from_value(json!({ "agents": [{
            "agent": "codex",
            "agent_session": {"agent": "codex", "kind": "id",
                "source": "herdr:codex", "value": "ses-live"},
            "agent_status": "working",
            "state_change_seq": 40,
            "name": "impl-g178-live",
            "pane_id": "w-live:p1",
            "foreground_cwd": "/tmp/corral-g178-live",
            "state_labels": {}
        }] }))
        .unwrap();
        adapter.reconcile_against_list(&initial, &store).await;

        let before = store.snapshot().await;
        assert!(before.agents.contains_key("herdr:ses-live"));
        assert!(!before.agents.contains_key("herdr:pane:w-live:p1"));
        let rev_before = before.rev;

        let sessionless: AgentListWire = serde_json::from_value(json!({ "agents": [{
            "agent": "codex",
            "agent_status": "working",
            "state_change_seq": 40,
            "name": "impl-g178-live",
            "pane_id": "w-live:p1",
            "foreground_cwd": "/tmp/corral-g178-live",
            "state_labels": {}
        }] }))
        .unwrap();
        adapter.reconcile_against_list(&sessionless, &store).await;

        let mid = store.snapshot().await;
        assert!(
            mid.agents.contains_key("herdr:ses-live"),
            "one omitted session field must not demote a live agent"
        );
        assert!(
            !mid.agents.contains_key("herdr:pane:w-live:p1"),
            "a live agent must not be duplicated under the pane fallback"
        );
        assert!(
            !adapter.is_stale_agent("herdr:ses-live"),
            "a live agent must not be tombstoned by an omitted field"
        );
        assert!(
            matches!(
                adapter.drive_target("herdr:ses-live"),
                Ok(target) if target == "impl-g178-live"
            ),
            "the live drive plane must stay dispatchable"
        );
        assert!(
            !adapter.remove_if_unmapped(&store, "herdr:ses-live").await,
            "the still-live mapping must never be evicted"
        );
        assert_eq!(
            mid.rev, rev_before,
            "a session-less no-op refresh must not republish the live row"
        );
    }

    #[tokio::test]
    async fn catalog_sweep_tombstones_unmapped_row_without_pruning_live_agent() {
        // Defense-in-depth for the sweep itself: an orphan store row with no
        // live adapter mapping is a unit seam (corrald has no row persistence,
        // so a fresh adapter is not a production trigger). The production
        // single-adapter trigger is covered above; this pins that the sweep
        // tombstones rather than turning a late drive into a generic 404, and
        // that it never prunes a still-mapped agent sharing the same repo.
        let temp = tempfile::tempdir().expect("tempdir");
        let primary = temp.path().join("corral");
        let worktrees = temp.path().join("worktrees");
        let live_worktree = worktrees.join("corral/g178-live");
        let dead_worktree = worktrees.join("corral/g178-superseded");
        for path in [&primary, &worktrees, &live_worktree, &dead_worktree] {
            std::fs::create_dir_all(path).unwrap();
        }
        let attribution = WorkspaceAttribution::from_roots(
            [RepoRoot {
                path: primary,
                repo: "corral".to_string(),
            }],
            worktrees.clone(),
        );
        attribution.record_branch(&live_worktree, "g178/live");
        attribution.record_branch(&dead_worktree, "g178/old");

        let store = Store::new();
        let seed_adapter = HerdrAdapter::new_with_attribution(
            PathBuf::from("/nonexistent.sock"),
            attribution.clone(),
        );
        let initial: AgentListWire = serde_json::from_value(json!({ "agents": [
            {
                "agent": "codex",
                "agent_session": {"agent": "codex", "kind": "id",
                    "source": "herdr:codex", "value": "ses-still-live"},
                "agent_status": "working",
                "state_change_seq": 30,
                "name": "impl-g178-live",
                "pane_id": "w-live:p1",
                "foreground_cwd": live_worktree.clone(),
                "state_labels": {}
            },
            {
                "agent": "codex",
                "agent_session": {"agent": "codex", "kind": "id",
                    "source": "herdr:codex", "value": "ses-dead-sibling"},
                "agent_status": "idle",
                "state_change_seq": 5,
                "name": "impl-g178-old",
                "pane_id": "w-dead:p1",
                "foreground_cwd": dead_worktree,
                "state_labels": {}
            }
        ] }))
        .unwrap();
        seed_adapter.reconcile_against_list(&initial, &store).await;

        let refreshed: AgentListWire = serde_json::from_value(json!({ "agents": [{
            "agent": "codex",
            "agent_session": {"agent": "codex", "kind": "id",
                "source": "herdr:codex", "value": "ses-still-live"},
            "agent_status": "working",
            "state_change_seq": 31,
            "name": "impl-g178-live",
            "pane_id": "w-live:p1",
            "foreground_cwd": live_worktree,
            "state_labels": {}
        }] }))
        .unwrap();
        let sweeping_adapter =
            HerdrAdapter::new_with_attribution(PathBuf::from("/nonexistent.sock"), attribution);
        sweeping_adapter
            .reconcile_against_list(&refreshed, &store)
            .await;

        let snapshot = store.snapshot().await;
        let live = snapshot
            .agents
            .get("herdr:ses-still-live")
            .expect("live agent must survive eviction");
        assert_eq!(live.state, S::Working);
        assert_eq!(live.workspace.repo.as_deref(), Some("corral"));
        assert_eq!(live.workspace.branch.as_deref(), Some("g178/live"));
        assert!(!snapshot.agents.contains_key("herdr:ses-dead-sibling"));
        assert!(
            sweeping_adapter.is_stale_agent("herdr:ses-dead-sibling"),
            "the catalog sweep must leave the refreshable stale tombstone"
        );
        assert!(
            matches!(
                sweeping_adapter.drive_target("herdr:ses-dead-sibling"),
                Err(DriveError::StaleAgent(id)) if id == "herdr:ses-dead-sibling"
            ),
            "a late drive on an evicted id must be a refreshable 409, not 404"
        );
        assert!(
            !sweeping_adapter
                .remove_if_unmapped(&store, "herdr:ses-still-live")
                .await,
            "a live mapped agent must never be evicted by the sweep"
        );
        assert!(
            store
                .snapshot()
                .await
                .agents
                .contains_key("herdr:ses-still-live")
        );
        assert_eq!(snapshot.agents.len(), 1);
    }

    #[tokio::test]
    async fn reconcile_tracks_add_remove_and_state_change_without_noop_rev() {
        let store = Store::new();
        let adapter = adapter();
        let initial: AgentListWire = serde_json::from_value(json!({ "agents": [
            {
                "agent": "opencode",
                "agent_session": {"agent": "opencode", "kind": "id",
                    "source": "herdr:opencode", "value": "ses-a"},
                "agent_status": "idle",
                "state_change_seq": 10,
                "name": "agent-a",
                "pane_id": "wa:p1",
                "state_labels": {}
            },
            {
                "agent": "claude",
                "agent_status": "working",
                "state_change_seq": 7,
                "name": "ghost",
                "pane_id": "wb:p1",
                "state_labels": {}
            }
        ] }))
        .unwrap();
        adapter.reconcile_against_list(&initial, &store).await;
        let before_rev = store.snapshot().await.rev;
        assert!(store.snapshot().await.agents.contains_key("herdr:ses-a"));
        assert!(
            store
                .snapshot()
                .await
                .agents
                .contains_key("herdr:pane:wb:p1")
        );

        let refreshed: AgentListWire = serde_json::from_value(json!({ "agents": [
            {
                "agent": "opencode",
                "agent_session": {"agent": "opencode", "kind": "id",
                    "source": "herdr:opencode", "value": "ses-a"},
                "agent_status": "working",
                "state_change_seq": 11,
                "name": "agent-a",
                "pane_id": "wa:p1",
                "state_labels": {}
            },
            {
                "agent": "codex",
                "agent_session": {"agent": "codex", "kind": "id",
                    "source": "herdr:codex", "value": "ses-c"},
                "agent_status": "blocked",
                "state_change_seq": 1,
                "name": "agent-c",
                "pane_id": "wc:p1",
                "state_labels": {}
            }
        ] }))
        .unwrap();
        adapter.reconcile_against_list(&refreshed, &store).await;

        let snapshot = store.snapshot().await;
        let agent_a = snapshot
            .agents
            .get("herdr:ses-a")
            .expect("agent a remains live");
        assert_eq!(agent_a.state, S::Working);
        let agent_c = snapshot
            .agents
            .get("herdr:ses-c")
            .expect("agent c is added");
        assert_eq!(agent_c.state, S::Blocked);
        assert_eq!(
            agent_c.attachment.as_ref().map(|a| a.reference.as_str()),
            Some("wc:p1")
        );
        assert!(!snapshot.agents.contains_key("herdr:pane:wb:p1"));
        assert_eq!(snapshot.agents.len(), 2);
        assert!(snapshot.rev > before_rev);

        adapter.reconcile_against_list(&refreshed, &store).await;
        let noop = store.snapshot().await;
        assert_eq!(
            noop.rev, snapshot.rev,
            "a no-op reconcile must not publish a new rev"
        );
        assert_eq!(noop.agents.len(), 2);
    }

    #[tokio::test]
    async fn stale_status_event_cannot_clobber_fresh_catalog_state() {
        let store = Store::new();
        let adapter = adapter();
        let agent: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-stale-guard"},
            "agent_status": "working",
            "state_change_seq": 11,
            "name": "guard",
            "pane_id": "wg:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&agent, &store).await;

        let stale: StatusChangedWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_status": "idle",
            "state_change_seq": 10,
            "pane_id": "wg:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.handle_status_changed(&stale, &store).await;
        let row = store.get("herdr:ses-stale-guard").await.expect("row");
        assert_eq!(
            row.state,
            S::Working,
            "older event must not overwrite fresh state"
        );

        let newer: StatusChangedWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_status": "blocked",
            "state_change_seq": 12,
            "pane_id": "wg:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.handle_status_changed(&newer, &store).await;
        assert_eq!(
            store.get("herdr:ses-stale-guard").await.unwrap().state,
            S::Blocked,
            "newer event must still update state"
        );
    }

    #[tokio::test]
    async fn stale_catalog_snapshot_cannot_clobber_newer_event() {
        let store = Store::new();
        let adapter = adapter();
        let event: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-catalog-guard"},
            "agent_status": "working",
            "state_change_seq": 12,
            "name": "guard",
            "pane_id": "wg2:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&event, &store).await;

        let stale_list: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-catalog-guard"},
            "agent_status": "idle",
            "state_change_seq": 11,
            "name": "guard",
            "pane_id": "wg2:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&stale_list, &store).await;
        assert_eq!(
            store.get("herdr:ses-catalog-guard").await.unwrap().state,
            S::Working,
            "stale catalog snapshot must not overwrite a newer event"
        );

        let fresh_list: AgentInfoWire = serde_json::from_value(json!({
            "agent": "opencode",
            "agent_session": {"agent": "opencode", "kind": "id",
                "source": "herdr:opencode", "value": "ses-catalog-guard"},
            "agent_status": "blocked",
            "state_change_seq": 13,
            "name": "guard",
            "pane_id": "wg2:p1",
            "state_labels": {}
        }))
        .unwrap();
        adapter.apply_agent_info(&fresh_list, &store).await;
        assert_eq!(
            store.get("herdr:ses-catalog-guard").await.unwrap().state,
            S::Blocked,
            "a newer catalog snapshot must still update state"
        );
    }

    #[tokio::test]
    async fn periodic_catalog_refresh_preserves_blocked_waiting_on_without_rev() {
        let store = Store::new();
        let adapter = adapter();
        let agent: AgentInfoWire = serde_json::from_value(json!({
            "agent": "claude",
            "agent_session": {"agent": "claude", "kind": "id",
                "source": "herdr:claude", "value": "ses-wait"},
            "agent_status": "blocked",
            "state_change_seq": 11,
            "name": "wait-one",
            "pane_id": "ww:p1",
            "state_labels": {"waiting_for_approval": ""}
        }))
        .unwrap();
        adapter.apply_agent_info(&agent, &store).await;

        let matched = serde_json::from_value::<OutputMatchedWire>(json!({
            "pane_id": "ww:p1",
            "matched_line": "  Do you want to proceed?",
            "read": {
                "pane_id": "ww:p1",
                "revision": 60,
                "source": "recent_unwrapped",
                "format": "text",
                "truncated": false,
                "text": "1. Continue\n2. Abort\n"
            }
        }))
        .unwrap();
        adapter.handle_output_matched(&matched, &store).await;
        let before = store.get("herdr:ses-wait").await.expect("blocked agent");
        let waiting = before.waiting_on.clone().expect("approval state");
        let rev_before = store.snapshot().await.rev;

        let list: AgentListWire = serde_json::from_value(json!({ "agents": [{
            "agent": "claude",
            "agent_session": {"agent": "claude", "kind": "id",
                "source": "herdr:claude", "value": "ses-wait"},
            "agent_status": "blocked",
            "state_change_seq": 11,
            "name": "wait-one",
            "pane_id": "ww:p1",
            "state_labels": {"waiting_for_approval": ""}
        }] }))
        .unwrap();
        adapter.reconcile_against_list(&list, &store).await;

        let after = store.get("herdr:ses-wait").await.expect("blocked agent");
        assert_eq!(after.state, S::Blocked);
        assert_eq!(
            after.waiting_on.as_ref(),
            Some(&waiting),
            "an unchanged catalog refresh must preserve the approval claim"
        );
        let snapshot = store.snapshot().await;
        assert_eq!(
            snapshot.rev, rev_before,
            "preserving waiting_on must remain a true no-op reconcile"
        );
    }

    #[tokio::test]
    async fn session_id_migration_preserves_waiting_on_and_approval_claim() {
        let store = Store::new();
        let adapter = adapter();
        let initial: AgentListWire = serde_json::from_value(json!({ "agents": [{
            "agent": "claude",
            "agent_status": "blocked",
            "state_change_seq": 20,
            "name": "migrating-agent",
            "pane_id": "ww3:p1",
            "state_labels": {"waiting_for_approval": ""}
        }] }))
        .unwrap();
        adapter.reconcile_against_list(&initial, &store).await;

        let fallback = "herdr:pane:ww3:p1";
        let matched = serde_json::from_value::<OutputMatchedWire>(json!({
            "pane_id": "ww3:p1",
            "matched_line": "Approve this change?",
            "read": {
                "pane_id": "ww3:p1",
                "revision": 62,
                "source": "recent_unwrapped",
                "format": "text",
                "truncated": false,
                "text": "[y/n]\n"
            }
        }))
        .unwrap();
        adapter.handle_output_matched(&matched, &store).await;
        let waiting = store
            .get(fallback)
            .await
            .expect("fallback waiting row")
            .waiting_on
            .expect("approval state");
        let rev_before = store.snapshot().await.rev;

        let migrated_id = "herdr:ses-fresh-migration";
        let fresh: AgentListWire = serde_json::from_value(json!({ "agents": [{
            "agent": "claude",
            "agent_session": {"agent": "claude", "kind": "id",
                "source": "herdr:claude", "value": "ses-fresh-migration"},
            "agent_status": "blocked",
            "state_change_seq": 21,
            "name": "migrating-agent",
            "pane_id": "ww3:p1",
            "state_labels": {"waiting_for_approval": ""}
        }] }))
        .unwrap();
        adapter.reconcile_against_list(&fresh, &store).await;

        let snapshot = store.snapshot().await;
        assert!(!snapshot.agents.contains_key(fallback));
        let migrated = snapshot
            .agents
            .get(migrated_id)
            .expect("fresh migration must create the session-id row");
        assert_eq!(migrated.state, S::Blocked);
        assert_eq!(
            migrated.waiting_on.as_ref().map(|w| &w.prompt),
            Some(&waiting.prompt)
        );
        assert_eq!(
            migrated.waiting_on.as_ref().map(|w| w.approval_id.as_str()),
            Some(crate::approve::approval_id_for(migrated_id, &waiting.prompt_hash).as_str())
        );
        assert!(snapshot.rev > rev_before, "migration must publish a rev");

        adapter.reconcile_against_list(&fresh, &store).await;
        let noop = store.snapshot().await;
        assert_eq!(
            noop.rev, snapshot.rev,
            "a no-op list after migration must not publish another rev"
        );
    }

    #[tokio::test]
    async fn stale_pane_to_session_migration_preserves_state_and_sequence() {
        let store = Store::new();
        let adapter = adapter();
        let initial: AgentListWire = serde_json::from_value(json!({ "agents": [{
            "agent": "claude",
            "agent_status": "blocked",
            "state_change_seq": 20,
            "name": "migrating-agent",
            "pane_id": "ww2:p1",
            "state_labels": {"waiting_for_approval": ""}
        }] }))
        .unwrap();
        adapter.reconcile_against_list(&initial, &store).await;
        let fallback = "herdr:pane:ww2:p1";
        assert_eq!(
            store.get(fallback).await.expect("fallback row").state,
            S::Blocked
        );

        let matched = serde_json::from_value::<OutputMatchedWire>(json!({
            "pane_id": "ww2:p1",
            "matched_line": "Approve this change?",
            "read": {
                "pane_id": "ww2:p1",
                "revision": 61,
                "source": "recent_unwrapped",
                "format": "text",
                "truncated": false,
                "text": "[y/n]\n"
            }
        }))
        .unwrap();
        adapter.handle_output_matched(&matched, &store).await;
        let waiting = store
            .get(fallback)
            .await
            .expect("fallback waiting row")
            .waiting_on
            .expect("approval state");
        let rev_before = store.snapshot().await.rev;

        let migrated_id = "herdr:ses-migrated";
        let stale: AgentListWire = serde_json::from_value(json!({ "agents": [{
            "agent": "claude",
            "agent_session": {"agent": "claude", "kind": "id",
                "source": "herdr:claude", "value": "ses-migrated"},
            "agent_status": "working",
            "state_change_seq": 10,
            "name": "migrating-agent",
            "pane_id": "ww2:p1",
            "state_labels": {}
        }] }))
        .unwrap();
        adapter.reconcile_against_list(&stale, &store).await;

        let snapshot = store.snapshot().await;
        assert!(
            !snapshot.agents.contains_key(fallback),
            "the stale fallback identity must be pruned"
        );
        let migrated = snapshot
            .agents
            .get(migrated_id)
            .expect("migrated row must survive a stale snapshot");
        assert_eq!(
            migrated.state,
            S::Blocked,
            "stale migration must not overwrite fresher blocked state"
        );
        assert_eq!(
            migrated.waiting_on.as_ref().map(|w| &w.prompt),
            Some(&waiting.prompt)
        );
        assert_eq!(
            migrated.waiting_on.as_ref().map(|w| w.approval_id.as_str()),
            Some(crate::approve::approval_id_for(migrated_id, &waiting.prompt_hash).as_str())
        );
        assert!(
            snapshot.rev > rev_before,
            "migration must publish a new rev"
        );

        let fresh: AgentListWire = serde_json::from_value(json!({ "agents": [{
            "agent": "claude",
            "agent_session": {"agent": "claude", "kind": "id",
                "source": "herdr:claude", "value": "ses-migrated"},
            "agent_status": "working",
            "state_change_seq": 21,
            "name": "migrating-agent",
            "pane_id": "ww2:p1",
            "state_labels": {}
        }] }))
        .unwrap();
        adapter.reconcile_against_list(&fresh, &store).await;
        assert_eq!(
            store.get(migrated_id).await.unwrap().state,
            S::Working,
            "a newer sequence must still update the migrated row"
        );

        let rev_after_fresh = store.snapshot().await.rev;
        let stale_after_fresh: AgentListWire = serde_json::from_value(json!({ "agents": [{
            "agent": "claude",
            "agent_session": {"agent": "claude", "kind": "id",
                "source": "herdr:claude", "value": "ses-migrated"},
            "agent_status": "idle",
            "state_change_seq": 19,
            "name": "migrating-agent",
            "pane_id": "ww2:p1",
            "state_labels": {}
        }] }))
        .unwrap();
        adapter
            .reconcile_against_list(&stale_after_fresh, &store)
            .await;
        assert_eq!(
            store.get(migrated_id).await.unwrap().state,
            S::Working,
            "the migrated ordering clock must reject older sequences"
        );
        assert_eq!(
            store.snapshot().await.rev,
            rev_after_fresh,
            "rejected stale migration must not publish a rev"
        );
    }

    #[test]
    fn reason_from_labels_is_deterministic() {
        // m8: HashMap order is arbitrary; the derived reason must not depend
        // on it, so identical label sets produce identical reasons.
        let mut a = HashMap::new();
        a.insert("waiting_for_approval".to_string(), "".to_string());
        a.insert("focus_lost".to_string(), "user switched pane".to_string());
        let mut b = HashMap::new();
        b.insert("focus_lost".to_string(), "user switched pane".to_string());
        b.insert("waiting_for_approval".to_string(), "".to_string());
        assert_eq!(reason_from_labels(&a), reason_from_labels(&b));
        assert_eq!(
            reason_from_labels(&a).as_deref(),
            Some("focus_lost: user switched pane")
        );
    }

    // -- #232 read_diff (adapter boundary) --------------------------------

    fn diff_init_repo(path: &Path) {
        let repo = git2::Repository::init(path).expect("init");
        let mut cfg = repo.config().expect("config");
        cfg.set_str("user.name", "corral-test").expect("name");
        cfg.set_str("user.email", "t@corral.test").expect("email");
        std::fs::write(path.join("a.txt"), "one\ntwo\nthree\n").expect("file");
        let mut index = repo.index().expect("index");
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .expect("add all");
        index.write().expect("index write");
        let tree = repo.index().expect("index").write_tree().expect("tree");
        let tree = repo.find_tree(tree).expect("tree");
        let sig = repo.signature().expect("sig");
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .expect("commit");
    }

    /// Realistic GitHub PAT canary: `ghp_` + 40 alphanumerics, the shape the
    /// redactor's prefix rule fires on (a short `ghp_ab` fragment never
    /// matches — that was the weak-canary trap this test used to have).
    const DIFF_FIXTURE_SECRET: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789abcd";

    /// Fixture: a repo with one committed file and one local modification on
    /// disk (unstaged, dirty worktree) + worktrees-root attribution.
    struct DiffFixture {
        _dir: tempfile::TempDir,
        worktree: PathBuf,
        worktrees_root: PathBuf,
        adapter: HerdrAdapter,
        store: Store,
    }

    fn diff_fixture() -> DiffFixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let worktrees_root = dir.path().join("worktrees");
        let worktree = worktrees_root.join("corral/feature-x");
        std::fs::create_dir_all(&worktree).expect("worktree dir");
        diff_init_repo(&worktree);
        // Long line that PUSHES the secret across the 4096-char truncation
        // cut (the '-' before "ghp_" gives the redactor a word boundary —
        // an alnum glue would intentionally pass per F1).
        let dirty = format!(
            "one\ntwo!!\nthree\n{}-{DIFF_FIXTURE_SECRET}-\n",
            "x".repeat(4080)
        );
        std::fs::write(worktree.join("a.txt"), dirty).expect("dirty");

        let attribution = WorkspaceAttribution::from_roots(
            std::iter::empty::<crate::core::workspace::RepoRoot>(),
            worktrees_root.clone(),
        );
        let adapter =
            HerdrAdapter::new_with_attribution(PathBuf::from("/nonexistent.sock"), attribution);
        let store = Store::new();
        adapter.attach_store(store.clone());
        DiffFixture {
            _dir: dir,
            worktree,
            worktrees_root,
            adapter,
            store,
        }
    }

    async fn seed_diff_agent(fix: &DiffFixture) {
        let agent = Agent {
            agent_id: "herdr:feature".to_string(),
            source: "herdr".to_string(),
            tool: "claude".to_string(),
            state: AgentState::Working,
            reason: None,
            seq: 1,
            ts: 1,
            capabilities: Vec::new(),
            waiting_on: None,
            parent_id: None,
            host: None,
            workspace: Workspace {
                worktree_path: Some(fix.worktree.to_string_lossy().into_owned()),
                ..Default::default()
            },
            attachment: None,
            display_name: None,
            title: None,
        };
        fix.store
            .apply(crate::core::model::Change::upsert(agent))
            .await;
    }

    #[tokio::test]
    async fn read_diff_serves_real_worktree_diff_and_redacts() {
        let fix = diff_fixture();
        seed_diff_agent(&fix).await;
        let query = crate::drive::ReadDiffQuery::clamped(Some(10), Some(0), Some(50));

        let result = fix
            .adapter
            .read_diff("herdr:feature", query)
            .await
            .expect("diff");

        assert_eq!(result.stats.files, 1);
        assert_eq!(result.stats.adds, 2);
        assert_eq!(result.stats.dels, 1);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "a.txt");
        assert_eq!(result.files[0].adds, 2);
        assert_eq!(result.files[0].dels, 1);
        assert!(
            result.lines.iter().any(|l| l.contains("+two!!")),
            "staged+unstaged change must appear: {:?}",
            result.lines
        );
        assert!(
            result.branch.as_deref() == Some("master") || result.branch.as_deref() == Some("main"),
            "branch: {:?}",
            result.branch
        );
        // D9 redaction: the realistic `ghp_`+40 canary is scrubber BEFORE
        // the 4096-char truncation (core::diff), so no prefix of the secret
        // survives the cut in ANY served line (the old assertion checked a
        // marker that never exists in output — this checks the secret).
        for line in &result.lines {
            assert!(
                !line.contains("ghp_"),
                "raw PAT canary must be redacted: {line}"
            );
        }
        assert!(
            result.lines.iter().any(|l| l.contains("[REDACTED]")),
            "redacted marker must appear: {:?}",
            result.lines
        );
    }

    #[tokio::test]
    async fn read_diff_refuses_paths_outside_the_worktrees_root() {
        let fix = diff_fixture();
        let outside = fix.worktrees_root.join("..").join("elsewhere");
        let mut agent = fixture_agent("herdr:outside");
        agent.workspace = Workspace {
            worktree_path: Some(outside.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let store = fix.store.clone();
        store.apply(crate::core::model::Change::upsert(agent)).await;

        let err = fix
            .adapter
            .read_diff(
                "herdr:outside",
                crate::drive::ReadDiffQuery::clamped(None, None, None),
            )
            .await
            .expect_err("non-herdr path must be refused");
        assert!(
            matches!(err, DriveError::NoWorktree(_)),
            "typed refusal expected: {err:?}"
        );
    }

    fn fixture_agent(id: &str) -> Agent {
        Agent {
            agent_id: id.to_string(),
            source: "herdr".to_string(),
            tool: "claude".to_string(),
            state: AgentState::Working,
            reason: None,
            seq: 1,
            ts: 1,
            capabilities: Vec::new(),
            waiting_on: None,
            parent_id: None,
            host: None,
            workspace: Workspace::default(),
            attachment: None,
            display_name: None,
            title: None,
        }
    }

    #[tokio::test]
    async fn read_diff_agent_without_worktree_path_is_no_worktree() {
        let fix = diff_fixture();
        let store = fix.store.clone();
        store
            .apply(crate::core::model::Change::upsert(fixture_agent(
                "herdr:nopath",
            )))
            .await;
        let err = fix
            .adapter
            .read_diff(
                "herdr:nopath",
                crate::drive::ReadDiffQuery::clamped(None, None, None),
            )
            .await
            .expect_err("no path -> refusal");
        assert!(matches!(err, DriveError::NoWorktree(_)));
    }

    #[tokio::test]
    async fn read_diff_unknown_agent_is_typed() {
        let fix = diff_fixture();
        let err = fix
            .adapter
            .read_diff(
                "herdr:ghost",
                crate::drive::ReadDiffQuery::clamped(None, None, None),
            )
            .await
            .expect_err("unknown");
        assert!(
            matches!(err, DriveError::UnknownAgent(_) | DriveError::StaleAgent(_)),
            "{err:?}"
        );
    }
}
