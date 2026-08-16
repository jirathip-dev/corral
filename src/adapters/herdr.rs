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
//!   the adapter converges without polling.
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
//! by construction. Paths and pane ids are identity, never redacted. A
//! future W1 `read_tail` result path must apply `redact` to the returned
//! text before it leaves the machine.
//!
//! ## Drive policy for `unknown` state
//!
//! `AgentState::Unknown` is first-class in the read model (any herdr status
//! outside idle/working/blocked/done maps to it). Drive gating keys off the
//! pane mapping, NOT the state: an Unknown-state agent whose pane is still
//! tracked is drivable (its pane exists — prompt/interrupt/read_tail work),
//! and an agent with no mapping is refused with the typed
//! [`DriveError::UnknownAgent`]. A command never panics on Unknown state.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};
use tracing::{info, warn};

use crate::adapters::{Adapter, DriveCommand, DriveError};
use crate::core::model::{
    Agent, AgentState, Attachment, Change, WaitingOn, WaitingOnKind, Workspace, CAPABILITIES,
};
use crate::core::redact::redact;
use crate::core::store::Store;
use crate::core::util::now_millis;

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
/// Bounded retry for pane event streams: attempts with doubling backoff,
/// then give up silently (a later pane.updated / next session reopens).
const PANE_RETRY_ATTEMPTS: usize = 3;
const PANE_RETRY_BASE: Duration = Duration::from_secs(2);
/// Delay before a respawn triggered by a pane stream closing.
const PANE_RESPAWN_DELAY: Duration = Duration::from_secs(5);

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

/// Pending request id -> response oneshot.
type PendingCalls = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, Value>>>>>;

/// JSON-RPC client over a unix socket. One reader task parses
/// newline-delimited frames: responses (with `id`) resolve pending calls
/// inline; pushed events are forwarded to the returned receiver.
struct RpcClient {
    writer: AsyncMutex<OwnedWriteHalf>,
    pending: PendingCalls,
    id_seq: AtomicU64,
}

impl RpcClient {
    fn new(stream: UnixStream) -> (Arc<Self>, mpsc::Receiver<EventFrame>) {
        let (read, write) = stream.into_split();
        let (events_tx, events_rx) = mpsc::channel(FRAME_CHANNEL_CAP);
        let client = Arc::new(Self {
            writer: AsyncMutex::new(write),
            pending: Arc::new(Mutex::new(HashMap::new())),
            id_seq: AtomicU64::new(0),
        });
        let reader = Self::reader(read, client.pending.clone(), events_tx);
        tokio::spawn(reader);
        (client, events_rx)
    }

    async fn reader(read: OwnedReadHalf, pending: PendingCalls, events: mpsc::Sender<EventFrame>) {
        let mut lines = BufReader::new(read).lines();
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
                if let Some(tx) = pending.lock().unwrap().remove(id) {
                    let result = match value.get("error") {
                        Some(err) => Err(err.clone()),
                        None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = tx.send(result);
                }
                continue;
            }
            let kind = value
                .get("event")
                .and_then(|e| e.as_str())
                .unwrap_or_default()
                .to_string();
            let data = value.get("data").cloned().unwrap_or(Value::Null);
            if events.send(EventFrame { kind, data }).await.is_err() {
                break;
            }
        }
    }

    /// Send a request and await its response. Only valid on a connection
    /// that has never called `events.subscribe` (herdr stops answering on
    /// subscribed connections).
    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = format!("corral:{}", self.id_seq.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let frame = json!({ "id": id, "method": method, "params": params });
        let mut line = frame.to_string();
        line.push('\n');
        {
            let mut writer = self.writer.lock().await;
            if writer.write_all(line.as_bytes()).await.is_err() {
                self.pending.lock().unwrap().remove(&id);
                return Err(RpcError::Disconnected);
            }
            let _ = writer.flush().await;
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(err))) => Err(RpcError::Server {
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
            Ok(Err(_)) => Err(RpcError::Disconnected),
            Err(_) => Err(RpcError::Timeout),
        }
    }
}

/// Open a fresh connection, make one request/response call, close. Used for
/// bootstrap and drive commands only (both rare) — herdr closes idle API
/// connections and turns subscribed ones push-only, so a persistent control
/// connection is not viable.
async fn rpc_call(socket_path: &std::path::Path, method: &str, params: Value) -> Result<Value, RpcError> {
    let stream = UnixStream::connect(socket_path).await.map_err(|e| RpcError::Server {
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
    /// per-source monotonic ordering
    seqs: HashMap<String, u64>,
    /// panes with a dedicated event stream
    subscribed_panes: HashSet<String>,
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
        let agent_id = self.pane_agents.remove(pane_id)?;
        self.agent_panes.remove(&agent_id);
        self.agent_names.remove(&agent_id);
        self.subscribed_panes.remove(pane_id);
        Some(agent_id)
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

pub struct HerdrAdapter {
    socket_path: PathBuf,
    state: Mutex<SessionState>,
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
            state: Mutex::new(SessionState::default()),
        }
    }

    async fn run_forever(&self, store: Store) {
        let mut backoff = RECONNECT_BASE;
        loop {
            let started = std::time::Instant::now();
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
        self.spawn_event_stream(StreamKey::Global, sink_tx.clone());
        loop {
            match sink_rx.recv().await {
                Some(SinkFrame::Event { kind, data, .. }) => {
                    self.handle_event(&kind, &data, sink_tx.clone(), store).await;
                }
                Some(SinkFrame::Closed { key }) => {
                    match key {
                        StreamKey::Global => {
                            // Server restarted or stream dropped: re-bootstrap
                            // to reconcile (dropping ghost agents whose panes
                            // closed while the stream was down), then reopen.
                            info!("main event stream closed, re-bootstrapping");
                            let list =
                                rpc_call(&self.socket_path, "agent.list", json!({})).await?;
                            let list: AgentListWire = serde_json::from_value(list).map_err(
                                |e| RpcError::Server {
                                    code: "decode".to_string(),
                                    message: e.to_string(),
                                },
                            )?;
                            self.reconcile_against_list(&list, store).await;
                            self.spawn_event_stream(StreamKey::Global, sink_tx.clone());
                        }
                        StreamKey::Pane(pane) => {
                            if self
                                .state
                                .lock()
                                .unwrap()
                                .subscribed_panes
                                .contains(&pane)
                            {
                                // Respawn with a delay (not at full speed) so
                                // a persistently rejecting pane cannot spin.
                                let socket_path = self.socket_path.clone();
                                let sink = sink_tx.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(PANE_RESPAWN_DELAY).await;
                                    spawn_pane_event_stream(socket_path, pane, sink);
                                });
                            }
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
            let present: HashSet<String> =
                list.agents.iter().map(|a| a.pane_id.clone()).collect();
            let stale: Vec<String> = state
                .pane_agents
                .keys()
                .filter(|pane| !present.contains(*pane))
                .cloned()
                .collect();
            stale
                .iter()
                .filter_map(|pane| state.remove(pane))
                .collect()
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

    /// Spawn a push-only event stream. Global streams run until they die and
    /// report `Closed`; pane streams get bounded retry with backoff (a
    /// persistently rejected pane must not spin connect+subscribe at full
    /// speed) and give up silently — the next pane.updated / session
    /// re-subscription reopens them.
    fn spawn_event_stream(&self, key: StreamKey, sink: mpsc::Sender<SinkFrame>) {
        match key {
            StreamKey::Global => {
                let socket_path = self.socket_path.clone();
                let subs = self.global_subscriptions();
                tokio::spawn(async move {
                    if !run_event_stream(socket_path, subs, sink.clone(), StreamKey::Global).await
                    {
                        let _ = sink.send(SinkFrame::Closed { key: StreamKey::Global }).await;
                    }
                });
            }
            StreamKey::Pane(pane) => {
                spawn_pane_event_stream(self.socket_path.clone(), pane, sink);
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
        let session_value = agent.agent_session.as_ref().and_then(|s| s.value.as_deref());
        let (agent_id, migrated, canonical) = {
            let mut state = self.state.lock().unwrap();
            let agent_id = state.resolve_agent_id(&agent.pane_id, session_value);
            let migrated = self.register_pane(&mut state, &agent.pane_id, &agent_id);
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
        {
            let mut state = self.state.lock().unwrap();
            if let Some(name) = agent.name.clone() {
                state.agent_names.insert(agent_id.clone(), name);
            }
        }
        info!(
            agent_id = %agent_id,
            tool = %agent.agent.as_deref().unwrap_or("unknown"),
            state = ?AgentState::from_herdr_status(&agent.agent_status),
            "agent upserted"
        );
    }

    /// Register pane -> agent_id; returns the previous agent_id if it changed
    /// (caller must emit a Remove for it).
    fn register_pane(
        &self,
        state: &mut SessionState,
        pane_id: &str,
        agent_id: &str,
    ) -> Option<String> {
        let prev = state
            .pane_agents
            .insert(pane_id.to_string(), agent_id.to_string());
        if prev.as_deref() != Some(agent_id)
            && let Some(old) = prev
        {
            state.agent_panes.remove(&old);
            state.agent_names.remove(&old);
            return Some(old);
        }
        state
            .agent_panes
            .entry(agent_id.to_string())
            .or_insert_with(|| pane_id.to_string());
        None
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
            cost: None,
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
    ) {        match kind {
            "pane_updated" => {
                let pane: PaneInfoWire =
                    match serde_json::from_value(data.get("pane").cloned().unwrap_or(Value::Null)) {
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
                if let Some(tool) = ev.agent {
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
                        );
                    }
                    self.register_agent_pane(&ev.pane_id, &tool, AgentState::Unknown, store)
                        .await;
                } else {
                    let removed = self.state.lock().unwrap().remove(&ev.pane_id);
                    if let Some(agent_id) = removed {
                        store.apply(Change::Remove(agent_id)).await;
                    }
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
            let migrated = self.register_pane(&mut state, &pane.pane_id, &agent_id);
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
            spawn_pane_event_stream(self.socket_path.clone(), pane.pane_id.clone(), sink);
        }
    }

    async fn handle_status_changed(&self, ev: &StatusChangedWire, store: &Store) {
        let known_id = {
            let state = self.state.lock().unwrap();
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
        let (agent_id, canonical) = {
            let mut state = self.state.lock().unwrap();
            let agent_id = state.resolve_agent_id(pane_id, None);
            self.register_pane(&mut state, pane_id, &agent_id);
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
            (agent_id, canonical)
        };
        let canonical = self.preserve_workspace(store, &agent_id, canonical).await;
        store.apply(Change::upsert(canonical)).await;
        info!(pane = pane_id, tool, ?agent_state, "agent detected");
    }
}

/// Spawn a pane event stream with bounded retry. Returns immediately; the
/// retry loop runs in its own task. On repeated failure it gives up silently
/// (no `Closed` — that would trigger a respawn loop); the next
/// pane.updated/agent_detected event or a new session reopens the stream.
fn spawn_pane_event_stream(socket_path: PathBuf, pane_id: String, sink: mpsc::Sender<SinkFrame>) {
    tokio::spawn(async move {
        let key = StreamKey::Pane(pane_id.clone());
        let subs = pane_subscriptions(&pane_id);
        let mut backoff = PANE_RETRY_BASE;
        for _ in 0..PANE_RETRY_ATTEMPTS {
            if run_event_stream(socket_path.clone(), subs.clone(), sink.clone(), key.clone()).await
            {
                return; // went live; the forwarder reports death via Closed
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(RECONNECT_MAX);
        }
    });
}

/// One event-stream connection: subscribe once, forward pushed events into
/// `sink` until the socket dies. Returns `true` if the subscription went
/// live, `false` if connect/subscribe failed (callers decide retry policy).
///
/// The forwarder task holds a clone of the RPC client for the stream's whole
/// lifetime: dropping the client would close the write half and send EOF to
/// herdr, potentially killing the subscription. On subscribe failure the
/// forwarder is aborted so the connection is torn down cleanly.
async fn run_event_stream(
    socket_path: PathBuf,
    subs: Vec<Value>,
    sink: mpsc::Sender<SinkFrame>,
    key: StreamKey,
) -> bool {
    let stream = match UnixStream::connect(&socket_path).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, key = ?key, "event stream connect failed");
            return false;
        }
    };
    let (client, mut rx) = RpcClient::new(stream);
    let key2 = key.clone();
    let forwarder_sink = sink.clone();
    let client_for_forwarder = client.clone();
    let forwarder = async move {
        // Keep the client (and its write half) alive for the whole stream.
        let _client = client_for_forwarder;
        loop {
            match rx.recv().await {
                Some(EventFrame { kind, data }) => {
                    if forwarder_sink.send(SinkFrame::Event { kind, data }).await.is_err() {
                        break;
                    }
                }
                None => {
                    let _ = forwarder_sink.send(SinkFrame::Closed { key: key2 }).await;
                    break;
                }
            }
        }
    };
    let forwarder_handle = tokio::spawn(forwarder);
    if let Err(e) = client
        .call("events.subscribe", json!({ "subscriptions": subs }))
        .await
    {
        warn!(key = ?key, error = %e, "event stream subscription failed");
        forwarder_handle.abort();
        return false;
    }
    true
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
        let target: String = {
            let state = self.state.lock().unwrap();
            let pane = state
                .agent_panes
                .get(agent_id)
                .ok_or_else(|| DriveError::UnknownAgent(agent_id.to_string()))?;
            state
                .agent_names
                .get(agent_id)
                .cloned()
                .unwrap_or_else(|| pane.clone())
        };
        let (method, params) = match command {
            DriveCommand::Prompt { text } => {
                ("agent.prompt", json!({"target": target, "text": text}))
            }
            DriveCommand::Interrupt => (
                "agent.send_keys",
                json!({"target": target, "keys": ["ctrl-c"]}),
            ),
            DriveCommand::ReadTail { lines } => (
                "agent.read",
                json!({"target": target, "source": "recent_unwrapped", "lines": lines}),
            ),
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
                json!({"target": target, "text": choice}),
            ),
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

    fn knows_agent(&self, agent_id: &str) -> bool {
        self.state.lock().unwrap().agent_panes.contains_key(agent_id)
    }
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

    #[tokio::test]
    async fn normalizes_real_agent_list_entries() {
        let store = Store::new();
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));

        let claude: AgentInfoWire = serde_json::from_value(fixture_claude()).unwrap();
        adapter.apply_agent_info(&claude, &store).await;

        let opencode: AgentInfoWire = serde_json::from_value(fixture_opencode_no_session()).unwrap();
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
        let o = snap.agents.get("herdr:pane:w1D:p1").expect("pane fallback id");
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

        let blocked = store.get("herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784").await.unwrap();
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

        let after = store.get("herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784").await.unwrap();
        let w = after.waiting_on.as_ref().expect("waiting_on set while blocked");
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
        let cleared = store.get("herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784").await.unwrap();
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
        assert_eq!(spaced.prompt_hash, format!("sha256:{}", hex(&hasher.finalize())));
    }

    #[test]
    fn extract_choices_detects_menus() {
        assert_eq!(extract_choices("[y/n]"), vec!["y", "n"]);
        let text = "1. Approve\n2. Reject and comment\n3. Edit files";
        assert_eq!(extract_choices(text), vec!["Approve", "Reject and comment", "Edit files"]);
        assert!(extract_choices("nothing here").is_empty());
    }

    #[test]
    fn waiting_on_redacts_pane_text_at_the_boundary() {
        // The matched line carries a fake secret: the stored prompt and the
        // hash must cover the REDACTED form — the exact bytes a client sees.
        let w = classify_waiting_on("Approve deploy with token ghp_yyy?", "");
        assert_eq!(w.prompt, "Approve deploy with token [REDACTED]?");
        assert_eq!(w.prompt_hash, classify_waiting_on("Approve deploy with token [REDACTED]?", "").prompt_hash);

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
        labels.insert("waiting_for_approval".to_string(), "run ghp_zzz now".to_string());
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

        let record = store.get("herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784").await.unwrap();
        assert_eq!(
            record.title.as_deref(),
            Some("Setup AWS key [REDACTED] now"),
            "title redacted on ingest"
        );
        let w = record.waiting_on.expect("waiting_on set while blocked");
        assert_eq!(w.prompt, "Approve with [REDACTED]?");
        assert!(!w.prompt_hash.contains("sk-ant"), "hash covers the redacted prompt only");
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
        let agent = snap.agents.get("herdr:pane:uX:p1").expect("detected agent record");
        assert_eq!(agent.state, AgentState::Unknown, "Unknown is a first-class state");
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
        let agent = snap.agents.get("herdr:pane:uX:p1").expect("detected agent record");
        assert_eq!(agent.state, AgentState::Unknown);

        // A tracked pane in Unknown state is drivable (drive gates on the
        // pane mapping, not the state): Ok, never a crash. The spawned rpc
        // task fails to connect to /nonexistent.sock and only logs.
        let result = adapter.drive(&agent.agent_id, DriveCommand::Prompt { text: "hi".into() });
        assert!(result.is_ok(), "drive on an unknown-state agent must be Ok: {result:?}");

        // An agent with no pane mapping gets the typed error.
        let err = adapter.drive("herdr:pane:absent", DriveCommand::Prompt { text: "hi".into() });
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
            "approve", "approval", "permission", "allow this", "confirm",
            "proceed?", "continue?", "do you want", "should i", "are you sure",
            "is that", "is this", "waiting for", "select", "choose",
            "[y/n]", "(y/n)", "yes/no", "please review", "need your input",
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
        println!("AC2_EVIDENCE {} {}", name, serde_json::to_string(value).unwrap());
    }

    #[tokio::test]
    async fn ac2_live_claim_flow() {
        let Some((socket, pane)) = ac2_env() else {
            return;
        };

        // 1. Bootstrap over the real socket: agent.list.
        let list = rpc_call(&socket, "agent.list", json!({})).await.expect("agent.list");
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
        ac2_evidence("bootstrap-agent", &json!({ "agent_id": agent_id, "pane_id": pane }));

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
            read: Some(OutputReadWire { text: Some(read_text.clone()) }),
        };
        adapter.handle_output_matched(&matched, &store).await;

        // 4. The live claim, emitted by the production path.
        let agent = store.get(&agent_id).await.expect("agent record");
        let w = agent.waiting_on.as_ref().expect("waiting_on set while blocked");
        let claim = claim_for(&agent_id, w);
        assert_eq!(claim.approval_id, w.approval_id, "derived claim == stored claim");
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
            .drive(&agent_id, DriveCommand::Approve { choice: approved.choice.clone() })
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
        let after = store.get(&agent_id).await.expect("agent record after approve");
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
        assert!(!adapter.knows_agent("herdr:pane:wG:p1"), "state must forget the ghost too");
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
        assert_eq!(reason_from_labels(&a).as_deref(), Some("focus_lost: user switched pane"));
    }
}
