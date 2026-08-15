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
            display_name,
            title,
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
        let title = ev.title.clone();
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
        self.update_record(store, &agent_id, move |agent| {
            if agent.state == AgentState::Blocked {
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
/// be deterministic for a given input.
fn reason_from_labels(labels: &HashMap<String, String>) -> Option<String> {
    let mut keys: Vec<&String> = labels.keys().collect();
    keys.sort();
    keys.first().map(|k| {
        let v = &labels[*k];
        if v.is_empty() {
            (*k).clone()
        } else {
            format!("{k}: {v}")
        }
    })
}

/// Classify a matched output line into the canonical waiting_on record.
fn classify_waiting_on(matched_line: &str, read_text: &str) -> WaitingOn {
    let prompt = matched_line.trim().to_string();
    let lower = prompt.to_lowercase();
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
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    let hash = format!("sha256:{}", hex(&hasher.finalize()));
    WaitingOn {
        kind,
        prompt,
        prompt_hash: hash,
        choices: extract_choices(read_text),
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
            DriveCommand::Approve => return Err(DriveError::NotImplemented("approve")),
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
        assert_eq!(w.prompt, "Do you want to proceed?");
        assert!(w.prompt_hash.starts_with("sha256:"));
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
    }

    #[test]
    fn extract_choices_detects_menus() {
        assert_eq!(extract_choices("[y/n]"), vec!["y", "n"]);
        let text = "1. Approve\n2. Reject and comment\n3. Edit files";
        assert_eq!(extract_choices(text), vec!["Approve", "Reject and comment", "Edit files"]);
        assert!(extract_choices("nothing here").is_empty());
    }

    #[test]
    fn drive_rejects_unknown_agents() {
        let adapter = HerdrAdapter::new(PathBuf::from("/nonexistent.sock"));
        assert!(!adapter.knows_agent("nope"));
        let err = adapter.drive("nope", DriveCommand::Prompt { text: "hi".into() });
        assert!(matches!(err, Err(DriveError::UnknownAgent(id)) if id == "nope"));
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
