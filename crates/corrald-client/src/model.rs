//! Typed read model, mirroring the daemon's canonical schema
//! (`src/core/model.rs`) field-for-field. Decoding is tolerant of
//! defaulted/missing fields so older clients can read snapshots that add
//! defaulted fields (unknown extra fields are ignored by serde by default);
//! breaking removals bump the schema version.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Snapshot/delta schema version served by corrald.
pub const SCHEMA_VERSION: u32 = 5;
pub const SUPPORTED_PROTOCOL_VERSION: u32 = 1;
pub const MINIMUM_SCHEMA_VERSION: u32 = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildIdentity {
    pub build_id: String,
    pub version: String,
    pub protocol_version: u32,
    pub schema_version: u32,
}

impl BuildIdentity {
    pub fn compatibility_warning(&self) -> Option<String> {
        if self.protocol_version != SUPPORTED_PROTOCOL_VERSION {
            return Some(format!(
                "Host protocol {} is incompatible with client protocol {}. Update corrald.",
                self.protocol_version, SUPPORTED_PROTOCOL_VERSION
            ));
        }
        if self.schema_version < MINIMUM_SCHEMA_VERSION {
            return Some(format!(
                "Host snapshot schema {} is older than client schema {}. Update corrald.",
                self.schema_version, MINIMUM_SCHEMA_VERSION
            ));
        }
        None
    }
}

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

/// Issue reference joined into the agent's workspace (G23): the issues the
/// agent's PR closes, per GitHub's authoritative `closingIssuesReferences`.
/// `state` is `"UNKNOWN"` when the closing ref's issue is not among the
/// repo's recently-fetched issues (the daemon enriches it from the same
/// poll's repo-level issues fetch when available).
/// One label attached to a GitHub issue (mirrors corrald's `GhIssueLabel`;
/// #113 wires `labels` + `url` onto the issue ref).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhIssueLabel {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub color: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhIssueRef {
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub number: u64,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<GhIssueLabel>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
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
    /// Debug-only: how the agent's PR was resolved — `"head_sha"`,
    /// `"branch"` (committed-but-unpushed fallback), or `"bound_pr"`.
    /// `None` when no PR is bound. Not a render-driver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_match_source: Option<String>,
    /// Issues the agent's PR closes (GitHub's authoritative
    /// `closingIssuesReferences`). Empty when no PR is bound or the PR
    /// links none.
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

/// The canonical drive capabilities (D7). Never hardcoded per tool —
/// clients render buttons from `Agent.capabilities`; mirror of the
/// daemon's `core::model::CAPABILITIES` (must stay in sync, #232 adds
/// `read_diff`).
pub const CAPABILITIES: [&str; 7] = [
    "prompt",
    "interrupt",
    "approve",
    "read_tail",
    "read_diff",
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
    #[serde(default)]
    pub build_identity: Option<BuildIdentity>,
    pub rev: u64,
    pub generated_at: u64,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_identity_classifies_contract_boundaries() {
        let compatible = BuildIdentity {
            build_id: "release-sha".into(),
            version: "0.1.0".into(),
            protocol_version: SUPPORTED_PROTOCOL_VERSION,
            schema_version: MINIMUM_SCHEMA_VERSION,
        };
        assert_eq!(compatible.compatibility_warning(), None);
        assert!(
            BuildIdentity {
                schema_version: MINIMUM_SCHEMA_VERSION - 1,
                ..compatible.clone()
            }
            .compatibility_warning()
            .is_some()
        );
        assert!(
            BuildIdentity {
                protocol_version: SUPPORTED_PROTOCOL_VERSION + 1,
                ..compatible
            }
            .compatibility_warning()
            .is_some()
        );
    }

    #[test]
    fn workspace_mirrors_the_g23_daemon_fields_tolerantly() {
        // A G23-shaped workspace (pr_match_source + issues) decodes.
        let ws: Workspace = serde_json::from_value(serde_json::json!({
            "repo": "herdr-board",
            "branch": "ws2/gh-plane",
            "worktree_path": "/wt/a",
            "pr_number": 42,
            "ci_status": "success",
            "dirty": false,
            "ahead": 0,
            "behind": 0,
            "pr_match_source": "branch",
            "issues": [{"repo": "herdr-board", "number": 22, "state": "OPEN",
                        "title": "PR badges: add headRefName to gh fragment"}]
        }))
        .expect("G23 workspace decodes");
        assert_eq!(ws.pr_match_source.as_deref(), Some("branch"));
        assert_eq!(ws.issues.len(), 1);
        assert_eq!(ws.issues[0].number, 22);
        assert_eq!(
            ws.issues[0].title,
            "PR badges: add headRefName to gh fragment"
        );

        // A pre-G23 workspace (fields absent) decodes with defaults —
        // additive-only alignment with the daemon's schema.
        let old: Workspace = serde_json::from_value(serde_json::json!({
            "repo": null, "branch": null, "worktree_path": "/wt/a", "pr_number": null,
            "ci_status": null, "dirty": false, "ahead": 0, "behind": 0
        }))
        .expect("pre-G23 workspace decodes");
        assert_eq!(old.pr_match_source, None);
        assert!(old.issues.is_empty());
    }

    #[test]
    fn workspace_serializes_issues_always_and_match_source_only_when_bound() {
        let ws = Workspace {
            issues: vec![GhIssueRef {
                repo: "herdr-board".to_string(),
                number: 22,
                state: "OPEN".to_string(),
                title: "t".to_string(),
                labels: vec![],
                url: String::new(),
            }],
            ..Default::default()
        };
        let v = serde_json::to_value(&ws).unwrap();
        assert_eq!(
            v["issues"],
            serde_json::json!([{
                "repo": "herdr-board", "number": 22, "state": "OPEN", "title": "t"
            }])
        );
        assert!(
            !v.as_object().unwrap().contains_key("pr_match_source"),
            "unbound -> the debug match source is omitted"
        );

        let bound = Workspace {
            pr_match_source: Some("head_sha".to_string()),
            ..Default::default()
        };
        let v = serde_json::to_value(&bound).unwrap();
        assert_eq!(v["pr_match_source"], "head_sha");
    }
}
