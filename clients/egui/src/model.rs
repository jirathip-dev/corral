//! Wire model mirrors of corrald's canonical agent model (src/core/model.rs)
//! plus pure rendering helpers. `docs/corral/P4-conformance.md` is the
//! normative contract; the daemon's serde shapes are mirrored 1:1 here so
//! snapshot/delta payloads decode verbatim.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Snapshot/delta schema version (corrald `SCHEMA_VERSION`).
pub const SCHEMA_VERSION: u32 = 5;

/// Coarse agent lifecycle state.
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
    /// Stable, distinct badge text per state (P4: four states, never one).
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Unknown => "unknown",
        }
    }
}

/// Why an agent is blocked — each kind renders as a DISTINCT badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitingOnKind {
    ApproveTool,
    AnswerQuestion,
    Menu,
    Crash,
}

impl WaitingOnKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::ApproveTool => "approve-tool",
            Self::AnswerQuestion => "question",
            Self::Menu => "menu",
            Self::Crash => "crash",
        }
    }

    /// Which of the four kind badges a waiting prompt maps to. This is the
    /// only place the mapping lives, so the board and tests share it.
    pub fn from_waiting(w: Option<&WaitingOn>) -> Option<Self> {
        w.map(|w| w.kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitingOn {
    pub kind: WaitingOnKind,
    pub prompt: String,
    pub prompt_hash: String,
    /// Claim identity echoed verbatim in approve envelopes.
    #[serde(default)]
    pub approval_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

impl WaitingOn {
    /// `sha256:` + lowercase hex of the SHA-256 of the EXACT untrimmed
    /// prompt string (conformance: clients hash the snapshot string
    /// byte-for-byte, never raw pane text).
    pub fn prompt_hash_of(prompt: &str) -> String {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(prompt.as_bytes());
        let mut hex = String::with_capacity(2 + 64);
        hex.push_str("sha256:");
        for b in digest {
            use std::fmt::Write;
            write!(hex, "{b:02x}").expect("hex write to String cannot fail");
        }
        hex
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiStatus {
    Success,
    Failure,
    Pending,
    Unknown,
}

impl CiStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Pending => "pending",
            Self::Unknown => "n/a",
        }
    }
}

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

/// One label attached to a GitHub issue (mirrors corrald's `GhIssueLabel`;
/// #113 wires `labels` + `url` onto the issue ref).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhIssueLabel {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub color: String,
}

/// Authoritative GitHub issue ref joined into the snapshot from the PR's
/// `closingIssuesReferences` (mirrors corrald's `GhIssueRef`; G23 wires the
/// daemon join — this client mirror decodes it via `#[serde(default)]` on
/// [`Agent::issues`], so daemons without the join stay empty).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhIssueRef {
    pub repo: String,
    pub number: u64,
    pub state: String,
    pub title: String,
    /// Labels (name + color). Empty on older daemons — never a guess.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<GhIssueLabel>,
    /// Canonical HTML URL. Empty on older daemons — the row renders without
    /// a clickable link.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
}

/// Canonical agent record (mirrors corrald's `Agent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    pub agent_id: String,
    pub source: String,
    pub tool: String,
    pub state: AgentState,
    pub reason: Option<String>,
    pub seq: u64,
    pub ts: u64,
    pub capabilities: Vec<String>,
    pub waiting_on: Option<WaitingOn>,
    pub parent_id: Option<String>,
    pub host: Option<String>,
    pub workspace: Workspace,
    pub attachment: Option<Attachment>,
    pub display_name: Option<String>,
    pub title: Option<String>,
    /// Authoritative issues this work closes (daemon-joined
    /// `closingIssuesReferences`, G23). Empty on older daemons — the
    /// branch-name inference (infer.rs) then validates against an empty
    /// set and stays flagged (`~#N?`), never asserted.
    #[serde(default)]
    pub issues: Vec<GhIssueRef>,
}

impl Agent {
    pub fn display(&self) -> String {
        self.display_name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.agent_id.clone())
    }

    /// The fetched authoritative issue numbers for this agent — the set
    /// branch-name inference validates against (D21: display-only).
    pub fn known_issue_numbers(&self) -> BTreeSet<u64> {
        self.issues.iter().map(|i| i.number).collect()
    }
}

/// #135: read-only `GET /fleet-registry` response mirror. A daemon parse/IO
/// failure is still a successful HTTP response with `status="error"` and an
/// empty registry, so the board renders the failure instead of an empty
/// fleet list.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FleetRegistry {
    pub status: String,
    pub path: String,
    #[serde(default)]
    pub error: Option<String>,
    pub fleets: Vec<FleetRegistryEntry>,
}

impl FleetRegistry {
    pub fn failed(&self) -> bool {
        self.status == "error"
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FleetRegistryEntry {
    pub name: String,
    pub gh_repo: String,
    pub local: String,
    pub worktree_dir: String,
    pub orch: String,
    pub workers: Vec<String>,
    pub paused: bool,
    pub models: FleetModels,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FleetModels {
    pub orch: String,
    #[serde(rename = "impl")]
    pub impl_: String,
    pub review: String,
    #[serde(default)]
    pub impl_alt: Option<String>,
    #[serde(default)]
    pub impl_alt2: Option<String>,
    /// Forward-compatible fleet-operations effort map, preserved by the
    /// daemon as opaque JSON so a new effort key never breaks decoding.
    #[serde(default)]
    pub reasoning_effort: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub rev: u64,
    pub generated_at: u64,
    pub agents: BTreeMap<String, Agent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    pub rev: u64,
    pub upd: Vec<Agent>,
    pub del: Vec<String>,
}

/// Epoch millis -> local "HH:MM:SS" clock time (display only).
pub fn clock_of(epoch_millis: u64) -> String {
    let secs = epoch_millis.saturating_div(1000) as i64;
    match chrono::DateTime::from_timestamp(secs, 0) {
        Some(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string(),
        None => "--:--:--".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_hash_is_sha256_prefixed_lowercase_hex() {
        let h = WaitingOn::prompt_hash_of("continue?");
        assert!(h.starts_with("sha256:"));
        assert_eq!(h.len(), 7 + 64);
        // Cross-check against sha2 directly (independent of the helper).
        let digest: [u8; 32] = {
            use sha2::Digest;
            sha2::Sha256::digest(b"continue?").into()
        };
        assert_eq!(h, format!("sha256:{}", hex_of(&digest)));
        // Empty and newline-containing strings are valid (never trimmed).
        assert_eq!(
            WaitingOn::prompt_hash_of(""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert!(WaitingOn::prompt_hash_of("\n").starts_with("sha256:"));
    }

    fn hex_of(bytes: &[u8]) -> String {
        use std::fmt::Write;
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            write!(s, "{b:02x}").unwrap();
        }
        s
    }

    #[test]
    fn prompt_hash_is_stable_and_distinct() {
        assert_eq!(
            WaitingOn::prompt_hash_of("approve the plan?"),
            WaitingOn::prompt_hash_of("approve the plan?")
        );
        assert_ne!(
            WaitingOn::prompt_hash_of("approve the plan?"),
            WaitingOn::prompt_hash_of("approve the plan? ")
        );
        // Untrimmed strings hash differently (byte-for-byte rule).
        assert_ne!(
            WaitingOn::prompt_hash_of("continue"),
            WaitingOn::prompt_hash_of("continue\n")
        );
    }

    #[test]
    fn state_and_kind_labels_are_distinct() {
        for (i, s) in [
            AgentState::Idle,
            AgentState::Working,
            AgentState::Blocked,
            AgentState::Done,
            AgentState::Unknown,
        ]
        .iter()
        .enumerate()
        {
            for (j, t) in [
                AgentState::Idle,
                AgentState::Working,
                AgentState::Blocked,
                AgentState::Done,
                AgentState::Unknown,
            ]
            .iter()
            .enumerate()
            {
                if i != j {
                    assert_ne!(s.label(), t.label());
                }
            }
        }
        for kind in [
            WaitingOnKind::ApproveTool,
            WaitingOnKind::AnswerQuestion,
            WaitingOnKind::Menu,
            WaitingOnKind::Crash,
        ] {
            assert!(!kind.label().is_empty());
        }
        assert_eq!(
            WaitingOnKind::from_waiting(Some(&WaitingOn {
                kind: WaitingOnKind::Crash,
                prompt: "boom".into(),
                prompt_hash: "sha256:0".into(),
                approval_id: String::new(),
                choices: vec![],
            })),
            Some(WaitingOnKind::Crash)
        );
        assert_eq!(WaitingOnKind::from_waiting(None), None);
    }

    #[test]
    fn snapshot_and_delta_decode_from_daemon_shapes() {
        let wire = serde_json::json!({
            "schema_version": 5,
            "rev": 42,
            "generated_at": 1700000000000u64,
            "agents": {
                "herdr:agent-a": {
                    "agent_id": "herdr:agent-a",
                    "source": "herdr",
                    "tool": "claude",
                    "state": "blocked",
                    "reason": "waiting for approval",
                    "seq": 7,
                    "ts": 1700000000000u64,
                    "capabilities": ["prompt", "interrupt", "approve", "read_tail"],
                    "waiting_on": {
                        "kind": "approve_tool",
                        "prompt": "Approve the plan?",
                        "prompt_hash": "sha256:abc",
                        "approval_id": "herdr:agent-a:sha256:abc",
                        "choices": ["Yes", "No"]
                    },
                    "parent_id": null,
                    "host": null,
                    "workspace": {
                        "repo": "herdr-board",
                        "branch": "w2/egui-desktop",
                        "worktree_path": "/wts/herdr-board/w2",
                        "pr_number": 12,
                        "ci_status": "pending",
                        "dirty": true,
                        "ahead": 3,
                        "behind": 1
                    },
                    "attachment": {"kind": "pane", "ref": "pane-1"},
                    "display_name": "w2/egui-desktop",
                    "title": "egui desktop client"
                }
            }
        });
        let snap: Snapshot = serde_json::from_value(wire).unwrap();
        assert_eq!(snap.rev, 42);
        assert_eq!(snap.schema_version, SCHEMA_VERSION);
        let agent = snap.agents.get("herdr:agent-a").unwrap();
        assert_eq!(agent.state, AgentState::Blocked);
        assert_eq!(
            agent.waiting_on.as_ref().unwrap().kind,
            WaitingOnKind::ApproveTool
        );
        assert_eq!(agent.workspace.repo.as_deref(), Some("herdr-board"));
        assert_eq!(agent.workspace.ahead, 3);
        assert_eq!(agent.workspace.ci_status, Some(CiStatus::Pending));
        assert_eq!(agent.workspace.pr_number, Some(12));
        assert!(agent.workspace.dirty);
        assert!(
            agent.issues.is_empty(),
            "absent `issues` field decodes empty (backward compat, pre-G23 daemon)"
        );
        assert!(agent.known_issue_numbers().is_empty());
    }

    #[test]
    fn snapshot_decodes_authoritative_closing_issue_refs() {
        // G23 join: `issues: [GhIssueRef]` on the agent record. The client
        // mirrors corrald's GhIssueRef (repo/number/state/title) and the
        // known-issue set feeds D21 validation (display-only).
        let wire = serde_json::json!({
            "schema_version": 5,
            "rev": 1,
            "generated_at": 0,
            "agents": {
                "herdr:a": {
                    "agent_id": "herdr:a",
                    "source": "herdr",
                    "tool": "claude",
                    "state": "working",
                    "reason": null,
                    "seq": 2,
                    "ts": 0,
                    "capabilities": [],
                    "waiting_on": null,
                    "parent_id": null,
                    "host": null,
                    "workspace": {
                        "repo": "corral",
                        "branch": "issue-24-issue-inference"
                    },
                    "attachment": null,
                    "display_name": null,
                    "title": null,
                    "issues": [
                        {
                            "repo": "jirathip-k/corral",
                            "number": 24,
                            "state": "open",
                            "title": "Branch-name issue inference"
                        }
                    ]
                }
            }
        });
        let snap: Snapshot = serde_json::from_value(wire).unwrap();
        let agent = snap.agents.get("herdr:a").unwrap();
        assert_eq!(agent.issues.len(), 1);
        assert_eq!(agent.issues[0].number, 24);
        assert_eq!(agent.issues[0].repo, "jirathip-k/corral");
        assert_eq!(agent.issues[0].title, "Branch-name issue inference");
        assert_eq!(agent.known_issue_numbers(), BTreeSet::from([24]));
    }

    #[test]
    fn fleet_registry_ok_fixture_decodes_models_and_reasoning_effort() {
        let wire = serde_json::json!({
            "status": "ok",
            "path": "/tmp/fleets.json",
            "error": null,
            "fleets": [
                {
                    "name": "corral",
                    "gh_repo": "jirathip-dev/corral",
                    "local": "~/Projects/corral",
                    "worktree_dir": "corral",
                    "orch": "orch-corral",
                    "workers": ["w1", "w2"],
                    "paused": true,
                    "models": {
                        "orch": "codex/deepseek-v4-flash-vision-exp",
                        "impl": "codex/deepseek-v4-flash-vision-exp",
                        "review": "codex/deepseek-v4-flash-vision-exp",
                        "impl_alt": "opencode-go/deepseek-v4-flash",
                        "impl_alt2": "codex/deepseek-v4-flash",
                        "reasoning_effort": {
                            "orch": "medium",
                            "impl": "max",
                            "review": "xhigh",
                            "future_effort": "high"
                        }
                    }
                }
            ]
        });
        let registry: FleetRegistry = serde_json::from_value(wire).unwrap();
        assert_eq!(registry.status, "ok");
        assert_eq!(registry.path, "/tmp/fleets.json");
        assert_eq!(registry.error, None);
        assert!(!registry.failed());
        assert_eq!(registry.fleets.len(), 1);

        let fleet = &registry.fleets[0];
        assert_eq!(fleet.name, "corral");
        assert_eq!(fleet.gh_repo, "jirathip-dev/corral");
        assert_eq!(fleet.workers, vec!["w1", "w2"]);
        assert!(fleet.paused);
        assert_eq!(fleet.models.impl_, "codex/deepseek-v4-flash-vision-exp");
        assert_eq!(
            fleet.models.impl_alt.as_deref(),
            Some("opencode-go/deepseek-v4-flash")
        );
        let effort = fleet.models.reasoning_effort.as_ref().unwrap();
        assert_eq!(effort["orch"], "medium");
        assert_eq!(effort["future_effort"], "high");
    }

    #[test]
    fn fleet_registry_error_fixture_decodes_with_empty_fleet_list() {
        let wire = serde_json::json!({
            "status": "error",
            "path": "/tmp/broken.json",
            "error": "parse: expected value at line 1",
            "fleets": []
        });
        let registry: FleetRegistry = serde_json::from_value(wire).unwrap();
        assert!(registry.failed());
        assert_eq!(
            registry.error.as_deref(),
            Some("parse: expected value at line 1")
        );
        assert!(registry.fleets.is_empty());
    }

    #[test]
    fn fleet_registry_optional_model_slots_default_to_none() {
        let wire = serde_json::json!({
            "status": "ok",
            "path": "/tmp/fleets.json",
            "error": null,
            "fleets": [{
                "name": "board",
                "gh_repo": "jirathip-dev/herdr-board",
                "local": "/opt/board",
                "worktree_dir": "board",
                "orch": "orch-board",
                "workers": [],
                "paused": false,
                "models": {"orch": "a", "impl": "b", "review": "c"}
            }]
        });
        let registry: FleetRegistry = serde_json::from_value(wire).unwrap();
        let models = &registry.fleets[0].models;
        assert_eq!(models.impl_alt, None);
        assert_eq!(models.impl_alt2, None);
        assert_eq!(models.reasoning_effort, None);
    }

    #[test]
    fn delta_decodes_with_empty_upd_del() {
        let wire = serde_json::json!({ "rev": 43, "upd": [], "del": [] });
        let delta: Delta = serde_json::from_value(wire).unwrap();
        assert_eq!(delta.rev, 43);
        assert!(delta.upd.is_empty());
        assert!(delta.del.is_empty());
    }

    #[test]
    fn clock_of_renders_local_hhmmss() {
        let s = clock_of(0);
        assert_eq!(s.len(), 8);
        assert_eq!(s.as_bytes()[2], b':');
        assert_eq!(s.as_bytes()[5], b':');
    }

    #[test]
    fn agent_display_falls_back_to_agent_id() {
        let mut a = Agent {
            agent_id: "herdr:x".into(),
            source: "herdr".into(),
            tool: "claude".into(),
            state: AgentState::Idle,
            reason: None,
            seq: 0,
            ts: 0,
            capabilities: vec![],
            waiting_on: None,
            parent_id: None,
            host: None,
            workspace: Workspace::default(),
            attachment: None,
            display_name: Some("w2/egui-desktop".into()),
            title: None,
            issues: vec![],
        };
        assert_eq!(a.display(), "w2/egui-desktop");
        a.display_name = Some(String::new());
        assert_eq!(a.display(), "herdr:x");
        a.display_name = None;
        assert_eq!(a.display(), "herdr:x");
    }
}
