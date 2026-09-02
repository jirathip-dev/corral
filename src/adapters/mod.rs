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

pub type ReadTailResult = (Vec<String>, Option<u64>);

/// Command targeting a single agent. Drive-path module boundary: cut to the
/// read-only command vocabulary (#354) — no mutating commands remain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveCommand {
    ReadTail {
        lines: Option<u32>,
        since_rev: Option<u64>,
    },
    /// #232: read the agent's worktree diff (changed-files list + unified
    /// diff page + diffstat). Routed through [`Adapter::read_diff`] like
    /// read_tail so the response can carry the paged result.
    ReadDiff { query: crate::drive::ReadDiffQuery },
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
    /// #232: the target agent has no diff the daemon may serve (no herdr
    /// worktree path, path not herdr-owned, or the git read itself failed).
    NoWorktree(String),
}

impl std::fmt::Display for DriveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotImplemented(cmd) => write!(f, "command not implemented: {cmd}"),
            Self::UnknownAgent(id) => write!(f, "unknown agent: {id}"),
            Self::StaleAgent(id) => write!(f, "stale agent: {id}"),
            Self::Transport(msg) => write!(f, "transport error: {msg}"),
            Self::NoWorktree(msg) => write!(f, "worktree diff unavailable: {msg}"),
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
            Self::NoWorktree(_) => "no_worktree",
        }
    }
}

/// A source of canonical agent records.
pub trait Adapter: Debug + Send + Sync {
    /// Canonical source name, e.g. "herdr", "claude", "codex".
    fn source(&self) -> &'static str;

    /// Begin streaming normalized records into `store`. Must not block;
    /// spawns background work. Event push is the primary signal; a bounded
    /// freshness/reconciliation pass is allowed (herdr re-lists the catalog
    /// on a fixed cadence) but adapters must never busy-poll.
    fn start(self: Arc<Self>, store: Store);

    /// Drive path: issue a command to `agent_id` and await the source's
    /// response. The adapter owns target resolution and maps source-level
    /// agent disappearance to [`DriveError::StaleAgent`]; callers must not
    /// report success until this future completes. `ReadTail` and `Attach`
    /// are the response-bearing exceptions and are dispatched through
    /// [`Adapter::read_tail`] and [`Adapter::attach`] instead.
    fn drive<'a>(
        &'a self,
        agent_id: &'a str,
        command: DriveCommand,
    ) -> futures::future::BoxFuture<'a, Result<(), DriveError>>;

    /// `read_tail`: fetch `agent_id`'s recent output and return
    /// it to the caller. The returned lines are redacted at the adapter
    /// boundary (D9) and bounded (D5: `READ_TAIL_MAX_LINES` /
    /// `READ_TAIL_MAX_BYTES`) BEFORE they leave the machine — the caller
    /// serializes them verbatim. An empty vec means "no output". The API
    /// layer routes `DriveCommand::ReadTail` here and never through
    /// [`Adapter::drive`] (the drive future also awaits the source outcome).
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

    /// Incremental read_tail seam. Adapters that do not expose source
    /// revisions retain the safe full-tail behavior.
    fn read_tail_since<'a>(
        &'a self,
        agent_id: &'a str,
        lines: u32,
        since_rev: Option<u64>,
    ) -> futures::future::BoxFuture<'a, Result<Vec<String>, DriveError>> {
        let _ = since_rev;
        self.read_tail(agent_id, lines)
    }

    /// Incremental read_tail plus source revision returned by the provider.
    fn read_tail_since_with_rev<'a>(
        &'a self,
        agent_id: &'a str,
        lines: u32,
        since_rev: Option<u64>,
    ) -> futures::future::BoxFuture<'a, Result<ReadTailResult, DriveError>> {
        Box::pin(async move {
            self.read_tail_since(agent_id, lines, since_rev)
                .await
                .map(|lines| (lines, None))
        })
    }

    /// #232: `read_diff`: fetch `agent_id`'s worktree diff (changed-files
    /// list + unified diff page + diffstat). The adapter resolves the
    /// worktree path from SNAPSHOT/HERDR STATE (the agent record's
    /// `workspace.worktree_path`), never from the client, verifies it is a
    /// herdr-owned worktree path, and computes the bounded diff via git2.
    /// Redaction (D9) happens here before any line leaves the machine, the
    /// same as [`Adapter::read_tail`]. Default: this adapter does not
    /// implement the command.
    fn read_diff<'a>(
        &'a self,
        agent_id: &'a str,
        query: crate::drive::ReadDiffQuery,
    ) -> futures::future::BoxFuture<'a, Result<crate::drive::ReadDiffResult, DriveError>> {
        let _ = (agent_id, query);
        Box::pin(async move { Err(DriveError::NotImplemented("read_diff")) })
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
