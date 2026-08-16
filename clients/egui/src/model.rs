//! Wire model mirrors of corrald's canonical agent model (src/core/model.rs)
//! plus pure rendering helpers. `docs/corral/P4-conformance.md` is the
//! normative contract; the daemon's serde shapes are mirrored 1:1 here so
//! snapshot/delta payloads decode verbatim.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Snapshot/delta schema version (corrald `SCHEMA_VERSION`).
pub const SCHEMA_VERSION: u32 = 3;

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
    pub cost: Option<f64>,
    pub parent_id: Option<String>,
    pub host: Option<String>,
    pub workspace: Workspace,
    pub attachment: Option<Attachment>,
    pub display_name: Option<String>,
    pub title: Option<String>,
}

impl Agent {
    pub fn display(&self) -> String {
        self.display_name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.agent_id.clone())
    }
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
            "schema_version": 3,
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
                    "cost": null,
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
            cost: None,
            parent_id: None,
            host: None,
            workspace: Workspace::default(),
            attachment: None,
            display_name: Some("w2/egui-desktop".into()),
            title: None,
        };
        assert_eq!(a.display(), "w2/egui-desktop");
        a.display_name = Some(String::new());
        assert_eq!(a.display(), "herdr:x");
        a.display_name = None;
        assert_eq!(a.display(), "herdr:x");
    }
}
