//! Typed read model, mirroring the daemon's canonical schema
//! (`src/core/model.rs` on main) field-for-field. Additive-only alignment:
//! decoding is tolerant of defaulted/missing fields so a client on schema
//! v4 still reads snapshots from a daemon that has since added defaulted
//! fields (unknown extra fields are ignored by serde by default).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Snapshot/delta schema version served by corrald on main.
pub const SCHEMA_VERSION: u32 = 4;

/// Coarse agent lifecycle state. Deliberately small: per-tool nuance lives
/// in `reason` / `waiting_on`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

/// Why an agent is blocked: an approve-tool prompt, a free-form question, a
/// menu, and a crash each render differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitingOnKind {
    ApproveTool,
    AnswerQuestion,
    Menu,
    Crash,
}

/// Structured "what is this agent waiting for". `prompt_hash` lets clients
/// dedupe across polls/sources; `choices` are populated when the prompt
/// exposes a menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitingOn {
    pub kind: WaitingOnKind,
    /// EXACT (untrimmed), redacted prompt as served by the snapshot. The
    /// approval claim's `prompt_hash` covers these bytes verbatim — never
    /// re-normalize, never trim.
    pub prompt: String,
    pub prompt_hash: String,
    /// Claim identity for the live approval (P3 D8): `agent_id:prompt_hash`.
    /// Clients echo it in the approve payload.
    #[serde(default)]
    pub approval_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

/// CI verdict for the agent's current PR (P2). `None` means "no PR/CI data
/// for this branch yet"; `Unknown` is the gh plane's "cannot tell".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiStatus {
    Success,
    Failure,
    Pending,
    Unknown,
}

/// Git topology + task-centric read-model fields (P2). Every field defaults
/// so P1-shaped payloads still decode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub pr_number: Option<u64>,
    #[serde(default)]
    pub ci_status: Option<CiStatus>,
    #[serde(default)]
    pub dirty: bool,
    #[serde(default)]
    pub ahead: u64,
    #[serde(default)]
    pub behind: u64,
    /// Current HEAD commit (full SHA) of the worktree (G21), from the git
    /// plane's probe — `null` for unborn/empty checkouts.
    #[serde(default)]
    pub head_sha: Option<String>,
    /// First line of the HEAD commit message (G21) — line-2 identity
    /// (`a1b3f9c "subject"`). `null` when there is no head commit.
    #[serde(default)]
    pub head_subject: Option<String>,
}

/// Link back to the source's own identity for this agent (e.g. herdr pane).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

/// The six canonical drive capabilities (D7). Never hardcoded per tool —
/// clients render buttons from `Agent.capabilities`.
pub const CAPABILITIES: [&str; 6] = [
    "prompt",
    "interrupt",
    "approve",
    "read_tail",
    "kill",
    "attach",
];

/// Canonical agent record. Flat keyed record in snapshot/delta payloads.
/// `agent_id` is opaque and source-stable (never a pane id).
///
/// Every optional field carries `#[serde(default)]` so a future daemon that
/// omits one degrades to `None` instead of hard-failing the whole
/// snapshot/SSE decode (additive-only alignment).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    pub agent_id: String,
    pub source: String,
    pub tool: String,
    pub state: AgentState,
    #[serde(default)]
    pub reason: Option<String>,
    /// Per-source monotonic ordering. `ts` is display-only.
    pub seq: u64,
    /// Wall-clock when this record was last changed (epoch millis).
    pub ts: u64,
    pub capabilities: Vec<String>,
    pub waiting_on: Option<WaitingOn>,
    /// Cumulative spend in USD, nullable.
    #[serde(default)]
    pub cost: Option<f64>,
    /// Topology: reviewer belongs to its implementation agent (P2+).
    #[serde(default)]
    pub parent_id: Option<String>,
    /// Host public-key identity (D10). null until P3 device keys.
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub workspace: Workspace,
    #[serde(default)]
    pub attachment: Option<Attachment>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

/// Full point-in-time state, served by `GET /snapshot` and by SSE when a
/// client's cursor is too old.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    /// Monotonic cursor; a client's `Last-Event-ID` is compared against this.
    pub rev: u64,
    /// Epoch millis when this snapshot was assembled.
    pub generated_at: u64,
    /// Flat keyed records (NOT JSON Patch).
    pub agents: BTreeMap<String, Agent>,
}

/// Incremental change batch, the unit of SSE delivery.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    pub rev: u64,
    /// Full records to upsert.
    pub upd: Vec<Agent>,
    /// agent_ids to delete.
    pub del: Vec<String>,
}

/// Apply a delta to an in-memory agent map (upserts + removals). The
/// client-side mirror of the daemon's store semantics; W2's board applies
/// this per SSE delta.
pub fn apply_delta(agents: &mut BTreeMap<String, Agent>, delta: &Delta) {
    for agent_id in &delta.del {
        agents.remove(agent_id);
    }
    for agent in &delta.upd {
        agents.insert(agent.agent_id.clone(), agent.clone());
    }
}
