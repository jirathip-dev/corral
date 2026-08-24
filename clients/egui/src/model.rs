//! Wire model mirrors of corrald's canonical agent model (src/core/model.rs)
//! plus pure rendering helpers. `docs/corral/P4-conformance.md` is the
//! normative contract; the daemon's serde shapes are mirrored 1:1 here so
//! snapshot/delta payloads decode verbatim.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Snapshot/delta schema version (corrald `SCHEMA_VERSION`).
pub const SCHEMA_VERSION: u32 = 5;

/// Maximum characters kept when an agent id is the final row-label fallback.
/// Long enough to distinguish common UUID prefixes, short enough to keep
/// issue/branch tokens visible beside it in the fixed-width agent column.
const SHORT_ID_MAX_CHARS: usize = 8;

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
    /// Human-readable primary board row label.
    ///
    /// Priority: non-empty `display_name`, non-empty worktree `title`,
    /// non-empty `workspace.branch`, then a bounded form of `agent_id`.
    /// The stable full id is intentionally reserved for the detail view.
    pub fn row_label(&self) -> String {
        self.display_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .or_else(|| self.title.as_deref().filter(|title| !title.is_empty()))
            .or_else(|| {
                self.workspace
                    .branch
                    .as_deref()
                    .filter(|branch| !branch.is_empty())
            })
            .map(str::to_string)
            .unwrap_or_else(|| shortened_agent_id(&self.agent_id))
    }

    /// Compatibility alias for callers of the previous display helper.
    pub fn display(&self) -> String {
        self.row_label()
    }

    /// The fetched authoritative issue numbers for this agent — the set
    /// branch-name inference validates against (D21: display-only).
    pub fn known_issue_numbers(&self) -> BTreeSet<u64> {
        self.issues.iter().map(|i| i.number).collect()
    }
}

/// Bounded human-readable agent-id fallback.
///
/// Strips the opaque `herdr:` transport prefix, keeps the last two
/// colon-separated components (so `herdr:pane:wGE:p1` becomes `wGE:p1`), and
/// truncates any remaining text to [`SHORT_ID_MAX_CHARS`] characters. It
/// never returns the full raw id.
fn shortened_agent_id(agent_id: &str) -> String {
    let opaque = agent_id.strip_prefix("herdr:").unwrap_or(agent_id);
    let parts: Vec<&str> = opaque.split(':').collect();
    let tail = if parts.len() > 1 {
        parts[parts.len() - 2..].join(":")
    } else {
        opaque.to_string()
    };
    tail.chars().take(SHORT_ID_MAX_CHARS).collect()
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

/// Epoch millis -> prototype-style relative age: `<1m`, `42m`, `3h 02m`,
/// `3d 04h`. A zero timestamp is unknown and renders as `—`.
pub fn relative_age(epoch_millis: u64, now_millis: u64) -> String {
    if epoch_millis == 0 {
        return "—".to_string();
    }
    let elapsed = now_millis.saturating_sub(epoch_millis);
    let minutes = elapsed / 60_000;
    if minutes == 0 {
        return "<1m".to_string();
    }
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h {:02}m", minutes % 60);
    }
    format!("{}d {:02}h", hours / 24, hours % 24)
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
    fn relative_age_renders_prototype_durations() {
        let now = 1_700_000_000_000;
        assert_eq!(relative_age(now, now), "<1m");
        assert_eq!(relative_age(now, now + 59_000), "<1m");
        assert_eq!(relative_age(now, now + 42 * 60_000), "42m");
        assert_eq!(relative_age(now, now + 70 * 60_000), "1h 10m");
        assert_eq!(relative_age(now, now + 182 * 60_000), "3h 02m");
        assert_eq!(relative_age(now, now + (23 * 60 + 59) * 60_000), "23h 59m");
        assert_eq!(relative_age(now, now + 24 * 60 * 60_000), "1d 00h");
        assert_eq!(relative_age(now, now + 76 * 60 * 60_000), "3d 04h");
        assert_eq!(relative_age(now, now + 100 * 24 * 60 * 60_000), "100d 00h");
        assert_eq!(relative_age(0, now), "—");
        assert_eq!(relative_age(0, 0), "—");
        assert_eq!(relative_age(now + 60_000, now), "<1m");
        assert_eq!(relative_age(u64::MAX, now), "<1m");
    }

    fn base_agent(agent_id: &str) -> Agent {
        Agent {
            agent_id: agent_id.into(),
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
            display_name: None,
            title: None,
            issues: vec![],
        }
    }

    #[test]
    fn row_label_prefers_display_name_over_title_and_branch() {
        let mut agent = base_agent("herdr:01a029d1-0000");
        agent.workspace.branch = Some("g128".into());
        agent.title = Some("board agent labels".into());
        agent.display_name = Some("Ada".into());
        assert_eq!(agent.row_label(), "Ada");
    }

    #[test]
    fn row_label_uses_title_when_display_name_is_missing_or_empty() {
        let mut agent = base_agent("herdr:01a029d1-0000");
        agent.workspace.branch = Some("review-g128".into());
        agent.title = Some("review board labels".into());
        agent.display_name = Some(String::new());
        assert_eq!(agent.row_label(), "review board labels");

        agent.display_name = None;
        assert_eq!(agent.row_label(), "review board labels");
    }

    #[test]
    fn row_label_uses_branch_when_title_is_missing() {
        let mut agent = base_agent("herdr:01a029d1-0000");
        agent.workspace.branch = Some("g92".into());
        assert_eq!(agent.row_label(), "g92");

        agent.title = Some(String::new());
        assert_eq!(agent.row_label(), "g92");
    }

    #[test]
    fn row_label_shortens_uuid_fallback_and_never_shows_full_raw_id() {
        let id = "herdr:01a029d1-1234-5678-9abc-def012345678";
        let agent = base_agent(id);
        assert_eq!(agent.row_label(), "01a029d1");
        assert!(!agent.row_label().contains(id));
    }

    #[test]
    fn row_label_shortens_pane_fallback_to_bounded_human_form() {
        let id = "herdr:pane:wGE:p1";
        let agent = base_agent(id);
        assert_eq!(agent.row_label(), "wGE:p1");
        assert!(!agent.row_label().contains(id));
    }

    #[test]
    fn display_alias_matches_row_label() {
        let mut agent = base_agent("herdr:pane:wGE:p1");
        assert_eq!(agent.display(), agent.row_label());
        agent.title = Some("worktree title".into());
        assert_eq!(agent.display(), agent.row_label());
    }
}
