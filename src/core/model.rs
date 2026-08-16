//! Canonical agent model — the schema every source adapter normalizes into.
//!
//! Versioned (see [`SCHEMA_VERSION`]), additive-only. `agent_id` is opaque and
//! source-stable; source-specific identifiers (like herdr's pane_id) live in
//! [`Attachment`], never in `agent_id`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Issue reference joined into the agent model (G23): the events contract's
/// [`GhIssueRef`](crate::core::events::GhIssueRef) is the single definition —
/// re-exported here so the canonical wire model and the contract cannot
/// drift.
pub use crate::core::events::GhIssueRef;

/// Snapshot/delta schema version. Bump on breaking changes; keep additive.
///
/// v1: P1 canonical record.
/// v2 (P2): `Workspace` gained the task-centric read-model fields `ci_status`,
/// `dirty`, `ahead`, `behind` — all serde-defaulted, so P1-shaped payloads
/// (and P1 clients reading v2 snapshots) still decode.
/// v3 (P3 D8): `WaitingOn` gained `approval_id` (the claim identity clients
/// echo in `DrivePayload::Approve`) — serde-defaulted, additive-only.
/// G23: `Workspace` gained `pr_match_source` (debug-only match-source note)
/// and `issues` (the bound PR's authoritative closing-issue refs) — both
/// serde-defaulted, additive-only; no version bump.
pub const SCHEMA_VERSION: u32 = 3;

/// Coarse agent lifecycle state. Deliberately small: per-tool nuance lives in
/// `reason` / `waiting_on`, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

impl AgentState {
    pub fn from_herdr_status(s: &str) -> Self {
        match s {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            _ => Self::Unknown,
        }
    }
}

/// Why an agent is blocked. "Blocked" is not one UI: an approve-tool prompt,
/// a free-form question, a menu, and a crash each render differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitingOnKind {
    ApproveTool,
    AnswerQuestion,
    Menu,
    /// Reserved: crash detection needs exit-code signals (P2+, pane exit
    /// with an active agent), so no P1 source produces it yet.
    Crash,
}

/// Structured "what is this agent waiting for". `prompt_hash` lets clients
/// dedupe across polls/sources; `choices` are populated when the prompt
/// exposes a menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitingOn {
    pub kind: WaitingOnKind,
    pub prompt: String,
    pub prompt_hash: String,
    /// Claim identity for the live approval (P3 D8): the client echoes it in
    /// `DrivePayload::Approve`. Stable for one waiting approval
    /// (`agent_id:prompt_hash`); the source adapter sets it when it emits
    /// the record. The drive path re-derives it from `agent_id` +
    /// `prompt_hash` and never trusts this stored copy for validation.
    #[serde(default)]
    pub approval_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

/// CI verdict for the agent's current PR (P2), normalized from the gh plane.
/// `Option::None` means "no PR/CI data for this branch yet"; `Unknown` is the
/// gh plane's explicit "cannot tell" verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiStatus {
    Success,
    Failure,
    Pending,
    Unknown,
}

/// Git topology + task-centric read-model fields (P2). P1 fills only
/// `worktree_path` (from the source's cwd); the git watcher fills
/// `branch`/`repo` (derived from the worktree path pattern), `dirty`/
/// `ahead`/`behind`; the gh poller fills `pr_number`/`ci_status`. Every field
/// defaults so P1-shaped payloads and P1 sources still decode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub pr_number: Option<u64>,
    /// CI verdict for the branch's PR. `None` until the gh plane reports one.
    #[serde(default)]
    pub ci_status: Option<CiStatus>,
    /// Uncommitted changes in the worktree (staged or unstaged).
    #[serde(default)]
    pub dirty: bool,
    /// Commits ahead of the upstream branch.
    #[serde(default)]
    pub ahead: u64,
    /// Commits behind the upstream branch.
    #[serde(default)]
    pub behind: u64,
    /// Debug-only: how the agent's PR was resolved against the gh facts —
    /// `"head_sha"` (the primary match), `"branch"` (the #22 fallback for
    /// committed-but-unpushed work), or `"bound_pr"` (the still-open bound
    /// PR surviving head-SHA lag). `None` when no PR is bound. Not a
    /// render-driver: clients must not branch UI on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_match_source: Option<String>,
    /// Issues the agent's bound PR closes, from GitHub's authoritative
    /// `closingIssuesReferences` (#23) — no heuristic, no branch-name
    /// inference. Empty when no PR is bound or the PR links none.
    #[serde(default)]
    pub issues: Vec<GhIssueRef>,
}

/// Link back to the source's own identity for this agent (e.g. herdr pane).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

/// Capabilities the client renders buttons from — never hardcoded per tool.
pub const CAPABILITIES: [&str; 6] = [
    "prompt",
    "interrupt",
    "approve",
    "read_tail",
    "kill",
    "attach",
];

/// Canonical agent record. Flat keyed record in snapshot/delta payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    /// Opaque, source-stable identity. NOT a pane id.
    pub agent_id: String,
    /// Adapter/source name, e.g. "herdr", "claude", "codex", "opencode", "gemini".
    pub source: String,
    /// Underlying tool binary, e.g. "claude", "codex", "opencode", "gemini".
    pub tool: String,
    pub state: AgentState,
    /// Free-form human reason for the current state.
    pub reason: Option<String>,
    /// Per-source monotonic ordering. `ts` is display-only.
    pub seq: u64,
    /// Wall-clock when this record was last changed (epoch millis).
    pub ts: u64,
    pub capabilities: Vec<String>,
    pub waiting_on: Option<WaitingOn>,
    /// Cumulative spend in USD, nullable (P1: always null from herdr).
    pub cost: Option<f64>,
    /// Topology: reviewer belongs to its implementation agent (P2+).
    pub parent_id: Option<String>,
    /// Host public-key identity (D10). P1: null until P3 device keys.
    pub host: Option<String>,
    pub workspace: Workspace,
    pub attachment: Option<Attachment>,
    /// Human-friendly name for the agent session.
    pub display_name: Option<String>,
    /// One-line task title.
    pub title: Option<String>,
}

/// Full point-in-time state, served by `GET /snapshot` and by SSE when a
/// client's cursor is too old.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub schema_version: u32,
    /// Monotonic cursor; a client's `Last-Event-ID` is compared against this.
    pub rev: u64,
    /// Epoch millis when this snapshot was assembled.
    pub generated_at: u64,
    /// Flat keyed records (NOT JSON Patch; JSON, not CBOR).
    pub agents: BTreeMap<String, Agent>,
}

/// Incremental change batch, the unit of SSE delivery.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Delta {
    pub rev: u64,
    /// Full records to upsert.
    pub upd: Vec<Agent>,
    /// agent_ids to delete.
    pub del: Vec<String>,
}

/// Change applied to the store by an adapter: upsert a record or remove one.
#[derive(Debug, Clone)]
pub enum Change {
    Upsert(Box<Agent>),
    Remove(String),
}

impl Change {
    pub fn upsert(agent: Agent) -> Self {
        Self::Upsert(Box::new(agent))
    }
}

/// Result of a client resuming from `Last-Event-ID`.
#[derive(Debug)]
pub enum Resume {
    /// Cursor missing/too old: client needs the full snapshot.
    Snapshot(Snapshot),
    /// Cursor fresh: replay these deltas, then go live.
    Deltas {
        deltas: Vec<Delta>,
        live_from_rev: u64,
    },
    /// Cursor already current: no replay, straight to live.
    Live { rev: u64 },
}
