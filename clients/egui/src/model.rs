//! Wire model mirrors of corrald's canonical agent model (src/core/model.rs)
//! plus pure rendering helpers. `docs/corral/P4-conformance.md` is the
//! normative contract; the daemon's serde shapes are mirrored 1:1 here so
//! snapshot/delta payloads decode verbatim.
//!
//! #354 read-only cut: the read model is pruned to what the board renders —
//! the Issues join, waiting-on claims, and supervision/presentation grouping
//! were removed with their surfaces. Unknown wire keys (a transitional
//! daemon still emitting them) decode as ignored, exactly like the iOS read
//! model after L2.

use std::collections::BTreeMap;

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
    /// Raw herdr state token, verbatim (#354 amendment 09-02: herdr 0.8.2
    /// has NO `done`; finished Hermes panes fall back to idle and the board
    /// never invents Needs-you / Supervising / Finished wording).
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

/// CI verdict colors (decoded from the workspace payload for wire safety;
/// the post-cut board no longer renders a CI column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiStatus {
    Success,
    Failure,
    Pending,
    Unknown,
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

/// Link back to the source's own identity for this agent (e.g. herdr pane).
/// The small pane reference is the board row's debug aid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

/// Canonical agent record (mirrors corrald's `Agent`). Fields removed by the
/// cut (`waiting_on`, `parent_id`, `host`, `issues`) are simply ignored when
/// a transitional daemon still sends them.
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
    pub workspace: Workspace,
    pub attachment: Option<Attachment>,
    pub display_name: Option<String>,
    pub title: Option<String>,
}

impl Agent {
    /// Human-readable primary board row label.
    ///
    /// Priority: non-empty `display_name`, non-empty worktree `title`,
    /// non-empty `workspace.branch`, then a bounded form of `agent_id`.
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

    /// The small pane reference (debug aid): the attachment's reference when
    /// it is a pane, shortened to its final path segment (the same rule the
    /// iOS row uses: `herdr:pane:w21:p1` → `w21:p1`). Unlike the row-label
    /// fallback there is no character cap — real pane ids are already
    /// bounded (`w21:p1`), and the demo identities' role tails (`p04:fleet`)
    /// must never render half-truncated.
    pub fn pane_reference(&self) -> Option<String> {
        let reference = self.attachment.as_ref()?.reference.as_str();
        let opaque = reference.strip_prefix("herdr:").unwrap_or(reference);
        let parts: Vec<&str> = opaque.split(':').collect();
        let tail = if parts.len() > 1 {
            parts[parts.len() - 2..].join(":")
        } else {
            opaque.to_string()
        };
        (!tail.is_empty()).then_some(tail)
    }

    /// The workspace repo name for board grouping, if any (orphans bucket).
    pub fn repo(&self) -> Option<&str> {
        self.workspace
            .repo
            .as_deref()
            .filter(|repo| !repo.is_empty())
    }

    /// Whether the retained read capability is advertised for this agent.
    /// The recents drill-in only ever reads an agent that advertises it.
    pub fn can_read_tail(&self) -> bool {
        self.capabilities.iter().any(|cap| cap == "read_tail")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_labels_are_raw_herdr_tokens() {
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
        assert_eq!(AgentState::Blocked.label(), "blocked");
        assert_eq!(AgentState::Working.label(), "working");
        assert_eq!(AgentState::Idle.label(), "idle");
        assert_eq!(AgentState::Done.label(), "done");
        assert_eq!(AgentState::Unknown.label(), "unknown");
    }

    #[test]
    fn snapshot_and_delta_decode_from_daemon_shapes_and_tolerate_legacy_keys() {
        // Full transitional-daemon payload (still carrying the pre-cut
        // waiting_on/issues/parent_id/host keys). Unknown keys are ignored;
        // every retained key decodes verbatim.
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
                    "capabilities": ["read_tail"],
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
                    "attachment": {"kind": "pane", "ref": "herdr:pane:w21:p1"},
                    "display_name": "w2/egui-desktop",
                    "title": "egui desktop client",
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
        assert_eq!(snap.rev, 42);
        assert_eq!(snap.schema_version, SCHEMA_VERSION);
        let agent = snap.agents.get("herdr:agent-a").unwrap();
        assert_eq!(agent.state, AgentState::Blocked);
        assert_eq!(agent.workspace.repo.as_deref(), Some("herdr-board"));
        assert_eq!(agent.workspace.ahead, 3);
        assert_eq!(agent.workspace.ci_status, Some(CiStatus::Pending));
        assert_eq!(agent.workspace.pr_number, Some(12));
        assert!(agent.workspace.dirty);
        assert!(agent.can_read_tail());
        assert_eq!(
            agent.pane_reference().as_deref(),
            Some("w21:p1"),
            "pane ref shortens to its final path segments"
        );
    }

    #[test]
    fn snapshot_decodes_without_legacy_keys_at_all() {
        // A current daemon may omit the pre-cut keys entirely.
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
                    "workspace": {
                        "repo": "corral",
                        "branch": "g354-l3-egui-cut"
                    },
                    "attachment": null,
                    "display_name": null,
                    "title": null
                }
            }
        });
        let snap: Snapshot = serde_json::from_value(wire).unwrap();
        let agent = snap.agents.get("herdr:a").unwrap();
        assert_eq!(agent.state, AgentState::Working);
        assert_eq!(agent.repo(), Some("corral"));
        assert!(!agent.can_read_tail());
        assert_eq!(agent.pane_reference(), None);
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
            workspace: Workspace::default(),
            attachment: None,
            display_name: None,
            title: None,
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
    fn pane_reference_shortens_to_final_segments() {
        let mut agent = base_agent("herdr:pane:wGE:p1");
        assert_eq!(agent.pane_reference(), None, "no attachment = no pane ref");
        agent.attachment = Some(Attachment {
            kind: "pane".into(),
            reference: "herdr:pane:w21:p1".into(),
        });
        assert_eq!(agent.pane_reference().as_deref(), Some("w21:p1"));
        // Role tails longer than the row-label cap must stay whole
        // (demo identities like demo:p04:fleet).
        agent.attachment = Some(Attachment {
            kind: "pane".into(),
            reference: "demo:p04:fleet".into(),
        });
        assert_eq!(agent.pane_reference().as_deref(), Some("p04:fleet"));
    }
}
