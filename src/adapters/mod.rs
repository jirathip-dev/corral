//! Source adapters. Each adapter normalizes one agent tool's (or host's)
//! stream into the canonical [`crate::core::model::Agent`] records.
//!
//! The [`Adapter`] trait is the seam P2/P3 add adapters behind (claude/codex/
//! opencode/gemini direct APIs, git watcher, gh poller) without touching core.

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
    Prompt { text: String },
    Interrupt,
    Approve,
    ReadTail { lines: Option<u32> },
    Kill,
    Attach,
}

#[derive(Debug)]
pub enum DriveError {
    /// This adapter/source does not implement the command.
    NotImplemented(&'static str),
    /// The given agent_id is not known to this adapter.
    UnknownAgent(String),
    /// Transport-level failure reaching the source.
    Transport(String),
}

impl std::fmt::Display for DriveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotImplemented(cmd) => write!(f, "command not implemented: {cmd}"),
            Self::UnknownAgent(id) => write!(f, "unknown agent: {id}"),
            Self::Transport(msg) => write!(f, "transport error: {msg}"),
        }
    }
}

impl std::error::Error for DriveError {}

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
    /// adapter.
    fn drive(&self, agent_id: &str, command: DriveCommand) -> Result<(), DriveError>;

    /// True if `agent_id` is currently tracked by this adapter.
    fn knows_agent(&self, agent_id: &str) -> bool;
}

/// Convenience: canonical record with its agent_id, as adapters hand off to
/// the store.
pub type NormalizedAgent = Agent;
