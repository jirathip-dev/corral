//! herdr adapter: JSON-RPC over the herdr unix socket, push-only.
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
//!   the adapter converges without polling. If bounded event delivery
//!   overflows, the stream is retired after its pending subscribe response;
//!   a successfully subscribed global stream re-bootstraps the session after
//!   the same capped outage backoff as connect/subscribe failures, while a
//!   pane stream reconnects and re-subscribes with its capped retry delay.
//!   Connection and subscribe failures do not trigger a global re-bootstrap
//!   until their owning retry delay has elapsed.
//!
//! Bootstrap is one `agent.list` call on connect (initial state — never a
//! poll loop; AC5: no sleep-loops calling `herdr agent list`).
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
//! by construction. Paths and pane ids are identity, never redacted. The
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
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot, watch};
use tracing::{info, warn};

use crate::adapters::{Adapter, DriveCommand, DriveError};
use crate::core::model::{
    Agent, AgentState, Attachment, CAPABILITIES, Change, WaitingOn, WaitingOnKind, Workspace,
};
use crate::core::redact::redact;
use crate::core::store::Store;
use crate::core::util::now_millis;
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
/// Pane event streams use a longer initial delay so a single unhealthy pane
/// cannot compete with the global stream for the herdr socket.
const PANE_RETRY_BASE: Duration = Duration::from_secs(2);
/// Delay before a pane task retries after a live stream closes.
const PANE_RESPAWN_DELAY: Duration = Duration::from_secs(5);

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
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct StatusChangedWire {
    pane_id: String,
    agent_status: Option<String>,
    agent: Option<String>,
    title: Option<String>,
    state_labels: HashMap<String, String>,
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

// ---------------------------------------------------------------------------
// RPC framing
// ---------------------------------------------------------------------------

/// A pushed event (responses are resolved inline by the reader).
#[derive(Debug, Clone)]
struct EventFrame {
    kind: String,
    data: Value,
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
            let value: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, "herdr frame parse error");
                    continue;
                }
            };
            if let Some(id) = value.get("id").and_then(|i| i.as_str()) {
                if let Some(tx) = pending.lock().unwrap().calls.remove(id) {
                    let result = match value.get("error") {
                        Some(err) => Err(RpcError::Server {
                            code: err
                                .get("code")
                                .and_then(|c| c.as_str())
                                .unwrap_or("error")
                                .to_string(),
                            message: err
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("unknown error")
                                .to_string(),
                        }),
                        None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
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
            let kind = value
                .get("event")
                .and_then(|e| e.as_str())
                .unwrap_or_default()
                .to_string();
            let data = value.get("data").cloned().unwrap_or(Value::Null);
            match events.try_send(EventFrame { kind, data }) {
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
    /// Canonical ids that were tracked and then removed or migrated away.
    /// These tombstones distinguish a stale snapshot from an id the adapter
    /// has never seen.
    stale_agents: HashSet<String>,
    /// Panes retired by migration/removal. Late status events for one of
    /// these panes must not resurrect a row through replay-order fallback.
    stale_panes: HashSet<String>,
    /// per-source monotonic ordering
    seqs: HashMap<String, u64>,
    /// panes with a dedicated event stream
    subscribed_panes: HashSet<String>,
    /// Cancellation handles for dedicated pane stream tasks. A pane can be
    /// removed and recreated with the same id; replacing the sender prevents
    /// an old retry loop from surviving into the new pane's generation.
    pane_streams: HashMap<String, watch::Sender<bool>>,
}

impl SessionState {
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

    fn remove(&mut self, pane_id: &str) -> Option<String> {
        let agent_id = self.pane_agents.remove(pane_id);
        self.subscribed_panes.remove(pane_id);
        self.stale_panes.insert(pane_id.to_string());
        // Always cancel a live pane stream, even when the pane had no agent
        // mapping left: a removed-and-recreated pane must not leak a task.
        if let Some(cancel) = self.pane_streams.remove(pane_id) {
            let _ = cancel.send(true);
        }
        let agent_id = agent_id?;
        // A pane can send a late close/status event after the same agent has
        // already migrated to a new pane. Do not let that late event remove
        // the new reverse mapping or delete the live store row.
        if self.agent_panes.get(&agent_id).map(String::as_str) != Some(pane_id) {
            return None;
        }
        self.agent_panes.remove(&agent_id);
        self.agent_names.remove(&agent_id);
        self.stale_agents.insert(agent_id.clone());
        Some(agent_id)
    }
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
    Event { kind: String, data: Value },
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
    state: Arc<Mutex<SessionState>>,
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
        Self {
            socket_path,
            state: Arc::new(Mutex::new(SessionState::default())),
        }
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
        info!(socket = %self.socket_path.display(), "connecting to herdr");

        // Bootstrap: one agent.list (initial state, never a poll loop).
        let list = rpc_call(&self.socket_path, "agent.list", json!({})).await?;
        let list: AgentListWire = serde_json::from_value(list).map_err(|e| RpcError::Server {
            code: "decode".to_string(),
            message: e.to_string(),
        })?;
        self.reconcile_against_list(&list, store).await;
        info!(agents = list.agents.len(), "herdr bootstrap complete");

        // Event sink: one mpsc channel per session; every stream forwarder
        // sends into it, the session loop consumes it.
        let (sink_tx, mut sink_rx) = mpsc::channel(FRAME_CHANNEL_CAP);

        // Main event stream: global subs + per-pane subs for all known panes
        // in ONE events.subscribe request (the only one this connection gets).
        let global_retry = Arc::new(Mutex::new(GlobalStreamRetry::new(stream_policy)));
        self.spawn_event_stream(StreamKey::Global, sink_tx.clone(), global_retry.clone());
        loop {
            match sink_rx.recv().await {
                Some(SinkFrame::Event { kind, data, .. }) => {
                    self.handle_event(&kind, &data, sink_tx.clone(), store)
                        .await;
                }
                Some(SinkFrame::Closed { key }) => {
                    match key {
                        StreamKey::Global => {
                            // Server restarted or stream dropped: re-bootstrap
                            // to reconcile (dropping ghost agents whose panes
                            // closed while the stream was down), then reopen.
                            info!("main event stream closed, re-bootstrapping");
                            let list = rpc_call(&self.socket_path, "agent.list", json!({})).await?;
                            let list: AgentListWire =
                                serde_json::from_value(list).map_err(|e| RpcError::Server {
                                    code: "decode".to_string(),
                                    message: e.to_string(),
                                })?;
                            self.reconcile_against_list(&list, store).await;
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
                    }
                }
                None => return Err(RpcError::Disconnected),
            }
        }
    }

    /// Reconcile tracked agents against a fresh `agent.list`. Upserts present
    /// panes; removes every tracked agent whose pane is absent from the list.
    /// Panes closed while a stream was down never emit pane.closed on the new
    /// stream (subscription only replays current pane state), so without this
    /// diff their agents would linger as ghosts forever.
    async fn reconcile_against_list(&self, list: &AgentListWire, store: &Store) {
        let removals: Vec<String> = {
            let mut state = self.state.lock().unwrap();
            let present: HashSet<String> = list.agents.iter().map(|a| a.pane_id.clone()).collect();
            let stale: Vec<String> = state
                .pane_agents
                .keys()
                .filter(|pane| !present.contains(*pane))
                .cloned()
                .collect();
            stale.iter().filter_map(|pane| state.remove(pane)).collect()
        };
        for agent_id in removals {
            info!(agent_id, "agent removed: pane absent from fresh agent.list");
            store.apply(Change::Remove(agent_id)).await;
        }
        for agent in &list.agents {
            self.apply_agent_info(agent, store).await;
            self.state
                .lock()
                .unwrap()
                .subscribed_panes
                .insert(agent.pane_id.clone());
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

    async fn apply_agent_info(&self, agent: &AgentInfoWire, store: &Store) {
        let session_value = agent
            .agent_session
            .as_ref()
            .and_then(|s| s.value.as_deref());
        let (agent_id, migrated, canonical) = {
            let mut state = self.state.lock().unwrap();
            let agent_id = state.resolve_agent_id(&agent.pane_id, session_value);
            let migrated =
                self.register_pane(&mut state, &agent.pane_id, &agent_id, agent.name.as_deref());
            let canonical = self.build_agent(
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
            );
            (agent_id, migrated, canonical)
        };
        if let Some(old) = migrated {
            store.apply(Change::Remove(old)).await;
        }
        let canonical = self.preserve_workspace(store, &agent_id, canonical).await;
        store.apply(Change::upsert(canonical)).await;
        info!(
            agent_id = %agent_id,
            tool = %agent.agent.as_deref().unwrap_or("unknown"),
            state = ?AgentState::from_herdr_status(&agent.agent_status),
            "agent upserted"
        );
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
        state.stale_panes.remove(pane_id);

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
            state.stale_agents.insert(old.clone());
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
            state.subscribed_panes.remove(&previous_pane);
            state.stale_panes.insert(previous_pane);
        }

        state.stale_agents.remove(agent_id);
        if let Some(name) = agent_name {
            state
                .agent_names
                .insert(agent_id.to_string(), name.to_string());
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
        // G34: best-effort cumulative spend from the D30 background cache,
        // keyed by (tool, worktree_path). `None` until the refresh loop
        // has populated a match for this pair — same as the pre-G34
        // hardcoded None, never an error path.
        let cost = tool.and_then(|t| {
            worktree_path
                .as_deref()
                .and_then(|w| crate::cost::agent_cache::cumulative_cost_for(t, w))
        });
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
            cost,
            parent_id: None,
            host: None,
            workspace: Workspace {
                repo: None,
                branch: None,
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

    /// Mutate an existing record in the store, bump its seq, re-apply.
    async fn update_record(&self, store: &Store, agent_id: &str, f: impl FnOnce(&mut Agent)) {
        let Some(mut agent) = store.get(agent_id).await else {
            return;
        };
        f(&mut agent);
        let seq = self.state.lock().unwrap().next_seq(agent_id);
        agent.seq = seq;
        agent.ts = now_millis();
        store.apply(Change::upsert(agent)).await;
    }

    /// WS3 F1: herdr owns `worktree_path` only. A full-record rebuild (agent
    /// info, pane.updated, agent_detected) must PRESERVE the plane-merged
    /// workspace read-model fields (repo/branch/dirty/ahead/behind/pr_number/
    /// ci_status) when the worktree is unchanged, or every herdr upsert
    /// clobbers the integrator's merged view. When the worktree changed, the
    /// fresh workspace wins (the integrator re-derives facts for the new
    /// path on its next pass).
    async fn preserve_workspace(&self, store: &Store, agent_id: &str, mut agent: Agent) -> Agent {
        let Some(existing) = store.get(agent_id).await else {
            return agent;
        };
        if existing.workspace.worktree_path == agent.workspace.worktree_path {
            let ws = &existing.workspace;
            agent.workspace.repo = ws.repo.clone();
            agent.workspace.branch = ws.branch.clone();
            agent.workspace.dirty = ws.dirty;
            agent.workspace.ahead = ws.ahead;
            agent.workspace.behind = ws.behind;
            agent.workspace.pr_number = ws.pr_number;
            agent.workspace.ci_status = ws.ci_status;
        }
        agent
    }

    async fn handle_event(
        &self,
        kind: &str,
        data: &Value,
        sink: mpsc::Sender<SinkFrame>,
        store: &Store,
    ) {
        match kind {
            "pane_updated" => {
                let pane: PaneInfoWire = match serde_json::from_value(
                    data.get("pane").cloned().unwrap_or(Value::Null),
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(error = %e, "pane.updated decode failed");
                        return;
                    }
                };
                self.handle_pane_updated(&pane, sink.clone(), store).await;
            }
            "pane_agent_status_changed" => {
                let ev: StatusChangedWire = match serde_json::from_value(data.clone()) {
                    Ok(e) => e,
                    Err(e) => {
                        warn!(error = %e, "pane.agent_status_changed decode failed");
                        return;
                    }
                };
                self.handle_status_changed(&ev, store).await;
            }
            "pane_output_matched" => {
                let ev: OutputMatchedWire = match serde_json::from_value(data.clone()) {
                    Ok(e) => e,
                    Err(e) => {
                        warn!(error = %e, "pane.output_matched decode failed");
                        return;
                    }
                };
                self.handle_output_matched(&ev, store).await;
            }
            "pane_agent_detected" => {
                let ev: AgentDetectedWire = match serde_json::from_value(data.clone()) {
                    Ok(e) => e,
                    Err(e) => {
                        warn!(error = %e, "pane.agent_detected decode failed");
                        return;
                    }
                };
                if ev.released.unwrap_or(false) || ev.agent.is_none() {
                    let removed = self.state.lock().unwrap().remove(&ev.pane_id);
                    if let Some(agent_id) = removed {
                        store.apply(Change::Remove(agent_id)).await;
                    }
                } else if let Some(tool) = ev.agent {
                    if self
                        .state
                        .lock()
                        .unwrap()
                        .subscribed_panes
                        .insert(ev.pane_id.clone())
                    {
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
            "pane_exited" | "pane_closed" => {
                if let Some(pane_id) = data.get("pane_id").and_then(|p| p.as_str()) {
                    let removed = self.state.lock().unwrap().remove(pane_id);
                    if let Some(agent_id) = removed {
                        store.apply(Change::Remove(agent_id)).await;
                    }
                }
            }
            "pane_created" => {
                // Nothing to do: agent panes announce themselves via
                // pane.agent_detected / pane.updated.
            }
            _ => {}
        }
    }

    async fn handle_pane_updated(
        &self,
        pane: &PaneInfoWire,
        sink: mpsc::Sender<SinkFrame>,
        store: &Store,
    ) {
        let session_value = pane.agent_session.as_ref().and_then(|s| s.value.as_deref());
        let known = {
            let state = self.state.lock().unwrap();
            state.pane_agents.get(&pane.pane_id).cloned()
        };
        // Only track panes that have (or had) an agent.
        if pane.agent.is_none() && session_value.is_none() && known.is_none() {
            return;
        }
        let agent_state =
            AgentState::from_herdr_status(pane.agent_status.as_deref().unwrap_or("unknown"));
        let (agent_id, migrated, canonical) = {
            let mut state = self.state.lock().unwrap();
            let agent_id = state.resolve_agent_id(&pane.pane_id, session_value);
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
            (agent_id, migrated, canonical)
        };
        if let Some(old) = migrated {
            store.apply(Change::Remove(old)).await;
        }
        let canonical = self.preserve_workspace(store, &agent_id, canonical).await;
        store.apply(Change::upsert(canonical)).await;
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
        let known_id = {
            let state = self.state.lock().unwrap();
            if state.stale_panes.contains(&ev.pane_id) {
                return;
            }
            state.pane_agents.get(&ev.pane_id).cloned()
        };
        let Some(agent_id) = known_id else {
            // Agent pane we never registered: create a record carrying the
            // event's actual status (not Unknown).
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
        self.update_record(store, &agent_id, move |agent| {
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
        self.update_record(store, &agent_id, move |agent| {
            if agent.state == AgentState::Blocked {
                let mut waiting_on = waiting_on.clone();
                // P3 D8: emit the live approval claim — the approval_id is
                // the stable identity (agent + exact prompt hash) clients
                // echo back in DrivePayload::Approve. The drive path
                // re-derives it and never trusts this stored copy.
                waiting_on.approval_id =
                    crate::approve::approval_id_for(&agent_id_for_claim, &waiting_on.prompt_hash);
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
        let (agent_id, migrated, canonical) = {
            let mut state = self.state.lock().unwrap();
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
            (agent_id, migrated, canonical)
        };
        if let Some(old) = migrated {
            store.apply(Change::Remove(old)).await;
        }
        let canonical = self.preserve_workspace(store, &agent_id, canonical).await;
        store.apply(Change::upsert(canonical)).await;
        info!(pane = pane_id, tool, ?agent_state, "agent detected");
    }
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
        while let Some(EventFrame { kind, data }) = rx.recv().await {
            if forwarder_sink
                .send(SinkFrame::Event { kind, data })
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
        tokio::spawn(async move { self.run_forever(store).await });
    }

    fn drive(&self, agent_id: &str, command: DriveCommand) -> Result<(), DriveError> {
        // read_tail is the one capability whose whole point is a response —
        // it never dispatches fire-and-forget. The API layer routes it
        // through Adapter::read_tail (synchronous, redacted, bounded);
        // drive() refusing it here keeps a silent fallback to the
        // discarded-response path impossible.
        if matches!(command, DriveCommand::ReadTail { .. }) {
            return Err(DriveError::NotImplemented("read_tail"));
        }
        let target = self.drive_target(agent_id)?;
        let (method, params) = match command {
            DriveCommand::Prompt { text } => {
                ("agent.prompt", json!({"target": target, "text": text}))
            }
            DriveCommand::Interrupt => (
                "agent.send_keys",
                json!({"target": target, "keys": ["ctrl-c"]}),
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
            DriveCommand::Approve { choice } => {
                ("agent.prompt", json!({"target": target, "text": choice}))
            }
            DriveCommand::Kill => return Err(DriveError::NotImplemented("kill")),
            DriveCommand::Attach => return Err(DriveError::NotImplemented("attach")),
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let agent_id = agent_id.to_string();
                let socket = self.socket_path.clone();
                handle.spawn(async move {
                    if let Err(e) = rpc_call(&socket, method, params).await {
                        warn!(agent_id, error = %e, "drive command failed");
                    }
                });
                Ok(())
            }
            Err(_) => Err(DriveError::Transport(
                "no tokio runtime available for drive".to_string(),
            )),
        }
    }

    fn read_tail<'a>(
        &'a self,
        agent_id: &'a str,
        lines: u32,
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, DriveError>> {
        let target = match self.drive_target(agent_id) {
            Ok(t) => t,
            Err(e) => return Box::pin(async move { Err(e) }),
        };
        // Same source the output_matched subscription uses (D5: only the
        // requested window, never a prefetch).
        let params = json!({
            "target": target,
            "source": "recent_unwrapped",
            "lines": lines.clamp(1, READ_TAIL_MAX_LINES),
        });
        let socket = self.socket_path.clone();
        Box::pin(async move {
            let response = rpc_call(&socket, "agent.read", params)
                .await
                .map_err(|e| DriveError::Transport(format!("agent.read failed: {e}")))?;
            let text = response
                .get("read")
                .and_then(|read| read.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or_default();
            Ok(bounded_redacted_tail(text, lines))
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
        self.state.lock().unwrap().stale_agents.contains(agent_id)
    }
}

/// Resolve the drive target for `agent_id`: the pane's current herdr agent
/// name when one is known, else the pane id. `None`-safe: an agent with no
/// current pane mapping is classified as stale when it was previously known,
/// otherwise it is the typed [`DriveError::UnknownAgent`].
impl HerdrAdapter {
    fn drive_target(&self, agent_id: &str) -> Result<String, DriveError> {
        let state = self.state.lock().unwrap();
        let Some(pane) = state.agent_panes.get(agent_id) else {
            return if state.stale_agents.contains(agent_id) {
                Err(DriveError::StaleAgent(agent_id.to_string()))
            } else {
                Err(DriveError::UnknownAgent(agent_id.to_string()))
            };
        };
        Ok(state
            .agent_names
            .get(agent_id)
            .cloned()
            .unwrap_or_else(|| pane.clone()))
    }
}

/// Bound + redact the fetched tail at the adapter boundary, BEFORE any byte
/// leaves the machine (D9/D5): at most `max_lines` lines (clamped to
/// [`READ_TAIL_MAX_LINES`]), the redacted text bounded to
/// [`READ_TAIL_MAX_BYTES`], every line through the shared redaction pass.
fn bounded_redacted_tail(text: &str, max_lines: u32) -> Vec<String> {
    let max_lines = (max_lines as usize).clamp(1, READ_TAIL_MAX_LINES as usize);
    let mut lines: Vec<String> = Vec::new();
    let mut bytes = 0usize;
    for raw in text.lines().take(max_lines) {
        let line = redact(raw).into_owned();
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
    use serde_json::json;

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
                "pane_closed",
                &json!({"pane_id": "w-stale:p1"}),
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
            adapter.drive(agent_id, DriveCommand::Prompt { text: "hi".into() }),
            Err(DriveError::StaleAgent(id)) if id == agent_id
        ));
        assert!(matches!(
            adapter.drive(agent_id, DriveCommand::Approve { choice: "y".into() }),
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
        // pane mapping, not the state): Ok, never a crash. The spawned rpc
        // task fails to connect to /nonexistent.sock and only logs.
        let result = adapter.drive(&agent.agent_id, DriveCommand::Prompt { text: "hi".into() });
        assert!(
            result.is_ok(),
            "drive on an unknown-state agent must be Ok: {result:?}"
        );

        // An agent with no pane mapping gets the typed error.
        let err = adapter.drive(
            "herdr:pane:absent",
            DriveCommand::Prompt { text: "hi".into() },
        );
        assert!(matches!(err, Err(DriveError::UnknownAgent(id)) if id == "herdr:pane:absent"));
    }

    #[test]
    fn drive_rejects_unknown_agents() {
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        assert!(!adapter.knows_agent("nope"));
        let err = adapter.drive("nope", DriveCommand::Prompt { text: "hi".into() });
        assert!(matches!(err, Err(DriveError::UnknownAgent(id)) if id == "nope"));
    }

    #[test]
    fn approve_dispatches_via_agent_prompt() {
        // The pane's approve is an input send; herdr exposes no
        // approve-shaped RPC, so the choice goes through agent.prompt (the
        // same input-send the human typing into the pane produces).
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        assert!(!adapter.knows_agent("nope"));
        let err = adapter.drive("nope", DriveCommand::Approve { choice: "y".into() });
        assert!(matches!(err, Err(DriveError::UnknownAgent(id)) if id == "nope"));
    }

    // -----------------------------------------------------------------------
    // W2.1 read_tail: the adapter fetches agent.read SYNCHRONOUSLY, redacts
    // (D9) and bounds (D5) the tail before it leaves the machine.
    // -----------------------------------------------------------------------

    #[test]
    fn drive_refuses_read_tail_fire_and_forget() {
        // read_tail is the one capability whose whole point is a response:
        // drive() (fire-and-forget) must refuse it so a silent fallback to
        // the discarded-response path is impossible; the API layer routes it
        // through Adapter::read_tail.
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        let err = adapter.drive("herdr:a", DriveCommand::ReadTail { lines: Some(5) });
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
    async fn current_snapshot_target_dispatches_read_prompt_and_approve() {
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
            .expect("prompt dispatch accepted");
        adapter
            .drive(agent_id, DriveCommand::Approve { choice: "y".into() })
            .expect("approve dispatch accepted");

        let requests = tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("three RPCs timeout")
            .expect("server task");
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0]["method"], "agent.read");
        assert_eq!(requests[0]["params"]["target"], "new-target");
        assert_eq!(requests[1]["method"], "agent.prompt");
        assert_eq!(requests[1]["params"]["target"], "new-target");
        assert_eq!(requests[1]["params"]["text"], "hello");
        assert_eq!(requests[2]["method"], "agent.prompt");
        assert_eq!(requests[2]["params"]["target"], "new-target");
        assert_eq!(requests[2]["params"]["text"], "y");
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
}
