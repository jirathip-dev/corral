//! Wire model mirrors of corrald's canonical agent model (src/core/model.rs)
//! plus pure rendering helpers. `docs/corral/P4-conformance.md` is the
//! normative contract; the daemon's serde shapes are mirrored 1:1 here so
//! snapshot/delta payloads decode verbatim.

use std::collections::{BTreeMap, BTreeSet};

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

/// G34 cost meter wire mirrors of corrald's `src/cost/mod.rs` types — the
/// serde shapes (snake_case enums, same field names) are mirrored 1:1 so
/// `GET /cost` decodes verbatim, never as `serde_json::Value` in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostProvider {
    Opencode,
    Claude,
    Codex,
}

impl CostProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Opencode => "opencode",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

/// Rolling windows: `FiveHour` serializes as `five_hour` (mirroring the
/// daemon's `Window` serde rename), displayed as "5h".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostWindow {
    FiveHour,
    Weekly,
    Monthly,
}

impl CostWindow {
    pub fn label(self) -> &'static str {
        match self {
            Self::FiveHour => "5h",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }
}

/// fleet-watch-style severity: `Problem` is the before-exhaustion signal
/// (a window at/above the alert threshold), not "already exhausted".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostStatus {
    Ok,
    Warning,
    Problem,
}

impl CostStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Problem => "problem",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostWindowUsage {
    pub window: CostWindow,
    pub usd: f64,
    pub cap_usd: f64,
    /// `true` when `cap_usd` is the daemon's built-in placeholder, not an
    /// operator-configured `CORRAL_COST_CAP_*` value — the percentage must
    /// be flagged as provisional, never shown as authoritative.
    pub cap_is_placeholder: bool,
    pub pct_of_cap: f64,
    pub status: CostStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostProviderUsage {
    pub provider: CostProvider,
    /// Whether the provider's session store was found at all (a fresh
    /// install, or a provider Guy doesn't use, is "no data", not an error).
    pub store_found: bool,
    pub windows: Vec<CostWindowUsage>,
}

/// `GET /cost` response body (mirrors corrald's cost handler).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostReport {
    pub generated_at: u64,
    pub providers: Vec<CostProviderUsage>,
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
            "schema_version": 3,
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
                    "cost": null,
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
    fn cost_report_decodes_from_daemon_shape() {
        // G34 acceptance: the exact `GET /cost` shape (mirrors
        // src/cost/mod.rs `WindowUsage` / `ProviderUsage` / the handler's
        // `{generated_at, providers}` wrapper) decodes into the typed
        // model — not `serde_json::Value`.
        let wire = serde_json::json!({
            "generated_at": 1784210400000u64,
            "providers": [
                {
                    "provider": "opencode",
                    "store_found": true,
                    "windows": [
                        {
                            "window": "five_hour",
                            "usd": 12.34,
                            "cap_usd": 100.0,
                            "cap_is_placeholder": true,
                            "pct_of_cap": 12.34,
                            "status": "ok"
                        }
                    ]
                }
            ]
        });
        let report: CostReport = serde_json::from_value(wire).unwrap();
        assert_eq!(report.generated_at, 1784210400000u64);
        assert_eq!(report.providers.len(), 1);
        let provider = &report.providers[0];
        assert_eq!(provider.provider, CostProvider::Opencode);
        assert!(provider.store_found);
        let window = &provider.windows[0];
        assert_eq!(window.window, CostWindow::FiveHour);
        assert!((window.usd - 12.34).abs() < 1e-9);
        assert!((window.cap_usd - 100.0).abs() < 1e-9);
        assert!(window.cap_is_placeholder, "placeholder caps are surfaced");
        assert!((window.pct_of_cap - 12.34).abs() < 1e-9);
        assert_eq!(window.status, CostStatus::Ok);
    }

    #[test]
    fn cost_report_decodes_all_windows_statuses_and_missing_stores() {
        let wire = serde_json::json!({
            "generated_at": 0,
            "providers": [
                {
                    "provider": "claude",
                    "store_found": false,
                    "windows": [
                        { "window": "five_hour", "usd": 0.0, "cap_usd": 5.0, "cap_is_placeholder": true, "pct_of_cap": 0.0, "status": "ok" },
                        { "window": "weekly", "usd": 25.0, "cap_usd": 35.0, "cap_is_placeholder": true, "pct_of_cap": 71.4, "status": "warning" },
                        { "window": "monthly", "usd": 130.0, "cap_usd": 140.0, "cap_is_placeholder": true, "pct_of_cap": 92.9, "status": "problem" }
                    ]
                },
                {
                    "provider": "codex",
                    "store_found": true,
                    "windows": []
                }
            ]
        });
        let report: CostReport = serde_json::from_value(wire).unwrap();
        assert_eq!(report.providers.len(), 2);
        let claude = &report.providers[0];
        assert_eq!(claude.provider, CostProvider::Claude);
        assert!(
            !claude.store_found,
            "absent store decodes to false, not an error"
        );
        let pairs: Vec<(CostWindow, CostStatus)> = claude
            .windows
            .iter()
            .map(|w| (w.window, w.status))
            .collect();
        assert_eq!(
            pairs,
            vec![
                (CostWindow::FiveHour, CostStatus::Ok),
                (CostWindow::Weekly, CostStatus::Warning),
                (CostWindow::Monthly, CostStatus::Problem),
            ]
        );
        assert!(report.providers[1].windows.is_empty());
        assert_eq!(report.providers[1].provider, CostProvider::Codex);
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
            issues: vec![],
        };
        assert_eq!(a.display(), "w2/egui-desktop");
        a.display_name = Some(String::new());
        assert_eq!(a.display(), "herdr:x");
        a.display_name = None;
        assert_eq!(a.display(), "herdr:x");
    }
}
