//! Source adapters. Each adapter normalizes one agent tool's (or host's)
//! stream into the canonical [`crate::core::model::Agent`] records.
//!
//! The [`Adapter`] trait is the seam P2/P3 add adapters behind (claude/codex/
//! opencode/gemini direct APIs, git watcher, gh poller) without touching core.

pub mod gh_plane;
pub mod git_plane;
pub mod herdr;

use std::fmt::Debug;
use std::sync::Arc;

use crate::core::model::Agent;
use crate::core::store::Store;

/// Command targeting a single agent. Drive-path module boundary: the HTTP
/// drive endpoints arrive in P3; the command vocabulary lives here from day
/// one so the read path never reaches into adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveCommand {
    Prompt {
        text: String,
    },
    Interrupt,
    /// Claim-checked approval reply (P3 D8): `choice` is the validated
    /// choice text to send to the agent (menu member, approve-tool answer,
    /// or free-form answer).
    Approve {
        choice: String,
    },
    ReadTail {
        lines: Option<u32>,
    },
    Kill,
    Attach,
}

#[derive(Debug)]
pub enum DriveError {
    /// This adapter/source does not implement the command.
    NotImplemented(&'static str),
    /// The given agent_id is not known to this adapter.
    UnknownAgent(String),
    /// The adapter knew this agent, but its live target disappeared or moved
    /// before the command could be dispatched. This is actionable client
    /// state, not a generic transport failure: refresh the snapshot before
    /// offering the row's controls again.
    StaleAgent(String),
    /// Transport-level failure reaching the source.
    Transport(String),
}

impl std::fmt::Display for DriveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotImplemented(cmd) => write!(f, "command not implemented: {cmd}"),
            Self::UnknownAgent(id) => write!(f, "unknown agent: {id}"),
            Self::StaleAgent(id) => write!(f, "stale agent: {id}"),
            Self::Transport(msg) => write!(f, "transport error: {msg}"),
        }
    }
}

impl std::error::Error for DriveError {}

impl DriveError {
    /// Stable wire kind for dispatch-level outcomes. Clients use this field
    /// instead of parsing human-facing error text.
    pub fn wire_kind(&self) -> &'static str {
        match self {
            Self::NotImplemented(_) => "not_implemented",
            Self::UnknownAgent(_) => "unknown_agent",
            Self::StaleAgent(_) => "stale_agent",
            Self::Transport(_) => "transport",
        }
    }
}

/// A source of canonical agent records.
pub trait Adapter: Debug + Send + Sync {
    /// Canonical source name, e.g. "herdr", "claude", "codex".
    fn source(&self) -> &'static str;

    /// Begin streaming normalized records into `store`. Must not block;
    /// spawns background work. Zero polling: the adapter is push-driven from
    /// this point on (one bootstrap call for initial state is allowed, never
    /// a poll loop).
    fn start(self: Arc<Self>, store: Store);

    /// Drive path: issue a command to `agent_id`. Synchronous validation,
    /// transport happens in the background; failures are logged by the
    /// adapter. Fire-and-forget by contract — every capability except
    /// `read_tail` (whose whole point is a response) dispatches this way.
    fn drive(&self, agent_id: &str, command: DriveCommand) -> Result<(), DriveError>;

    /// Synchronous `read_tail`: fetch `agent_id`'s recent output and return
    /// it to the caller. The returned lines are redacted at the adapter
    /// boundary (D9) and bounded (D5: `READ_TAIL_MAX_LINES` /
    /// `READ_TAIL_MAX_BYTES`) BEFORE they leave the machine — the caller
    /// serializes them verbatim. An empty vec means "no output". The API
    /// layer routes `DriveCommand::ReadTail` here and never through
    /// [`Adapter::drive`] (the `drive` path stays fire-and-forget).
    /// Default: this adapter does not implement the command.
    ///
    /// A boxed future keeps the trait dyn-compatible (callers hold
    /// `Arc<dyn Adapter>`); `Send` by construction (`BoxFuture`).
    fn read_tail<'a>(
        &'a self,
        agent_id: &'a str,
        lines: u32,
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, DriveError>> {
        let _ = (agent_id, lines);
        Box::pin(async move { Err(DriveError::NotImplemented("read_tail")) })
    }

    /// True if `agent_id` is currently tracked by this adapter.
    fn knows_agent(&self, agent_id: &str) -> bool;

    /// True if this adapter previously tracked `agent_id` but has since
    /// removed it. The drive API uses this distinction to return a typed
    /// refreshable stale-target result while preserving `unknown_agent` for
    /// ids that were never present.
    fn is_stale_agent(&self, _agent_id: &str) -> bool {
        false
    }
}

/// Convenience: canonical record with its agent_id, as adapters hand off to
/// the store.
pub type NormalizedAgent = Agent;
