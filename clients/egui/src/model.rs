//! Wire model mirrors of corrald's canonical agent model (src/core/model.rs)
//! plus pure rendering helpers. `docs/corral/P4-conformance.md` is the
//! normative contract; the daemon's serde shapes are mirrored 1:1 here so
//! snapshot/delta payloads decode verbatim.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

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

pub fn host_compatibility_warning(identity: Option<&BuildIdentity>) -> Option<String> {
    match identity {
        None => Some(
            "Host compatibility is unknown — update corrald before using this client.".to_string(),
        ),
        Some(identity) => identity.compatibility_warning(),
    }
}

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
    /// #270: body text from the daemon's repo-level issue read. Issue refs
    /// joined into agent snapshots intentionally omit it on older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
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

    /// Structured role from the source identity, never from transcript text.
    pub fn role(&self) -> AgentRole {
        [Some(self.agent_id.as_str()), self.display_name.as_deref()]
            .into_iter()
            .flatten()
            .flat_map(|value| value.split(|ch: char| !ch.is_ascii_alphanumeric()))
            .find_map(AgentRole::from_token)
            .unwrap_or(AgentRole::Unknown)
    }

    /// Active supervision evidence projected from the adapter's structured
    /// state-label reason. The raw command is intentionally never returned.
    pub fn supervision_activity(&self) -> Option<SupervisionActivity> {
        if self.state != AgentState::Done || self.role() != AgentRole::Orchestrator {
            return None;
        }
        parse_supervision_activity(self.reason.as_deref()?)
    }

    pub fn presentation_group(&self) -> PresentationGroup {
        match self.state {
            AgentState::Blocked => PresentationGroup::NeedsYou,
            AgentState::Working => PresentationGroup::Working,
            AgentState::Done => self
                .supervision_activity()
                .map_or(PresentationGroup::Finished, |_| {
                    PresentationGroup::Supervising
                }),
            AgentState::Idle | AgentState::Unknown => PresentationGroup::Idle,
        }
    }
}

/// Stable source-identity role used only for derived presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    Orchestrator,
    Implementer,
    Reviewer,
    Unknown,
}

impl AgentRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::Orchestrator => "Orchestrator",
            Self::Implementer => "Implementer",
            Self::Reviewer => "Reviewer",
            Self::Unknown => "Unknown",
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "orch" | "orchestrator" => Some(Self::Orchestrator),
            "impl" | "implementer" => Some(Self::Implementer),
            "review" | "reviewer" | "rev" => Some(Self::Reviewer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisionKind {
    Polling,
    Watcher,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisionActivity {
    pub kind: SupervisionKind,
    pub interval_seconds: Option<u64>,
    pub queued_work: Option<u64>,
}

impl SupervisionActivity {
    pub fn summary(self) -> String {
        let label = match self.kind {
            SupervisionKind::Polling => "Polling",
            SupervisionKind::Watcher => "Watcher",
        };
        match self.interval_seconds {
            Some(seconds) => format!("↻ {label} · every {seconds}s"),
            None => format!("↻ {label}"),
        }
    }

    /// Accessible text contains only the derived label and safe counters.
    pub fn accessibility_label(self) -> String {
        let mut label = match self.kind {
            SupervisionKind::Polling => "Activity: Supervising, polling".to_string(),
            SupervisionKind::Watcher => "Activity: Supervising, watcher".to_string(),
        };
        if let Some(seconds) = self.interval_seconds {
            label.push_str(&format!(", every {seconds} seconds"));
        }
        if let Some(queued) = self.queued_work {
            label.push_str(&format!(", queued {queued}"));
        }
        label.push_str(", current command redacted");
        label
    }
}

/// Ordered derived presentation groups. These do not alter `AgentState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationGroup {
    NeedsYou,
    Working,
    Supervising,
    Finished,
    Idle,
}

impl PresentationGroup {
    pub const ORDER: [Self; 5] = [
        Self::NeedsYou,
        Self::Working,
        Self::Supervising,
        Self::Finished,
        Self::Idle,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::NeedsYou => "Needs you",
            Self::Working => "Working",
            Self::Supervising => "Supervising",
            Self::Finished => "Finished",
            Self::Idle => "Idle",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationSection {
    pub group: PresentationGroup,
    pub agent_ids: Vec<String>,
}

impl PresentationSection {
    pub fn header(&self) -> String {
        format!("{} ({})", self.group.label(), self.agent_ids.len())
    }
}

pub fn presentation_sections(agents: &[Agent]) -> Vec<PresentationSection> {
    PresentationGroup::ORDER
        .into_iter()
        .filter_map(|group| {
            let agent_ids = agents
                .iter()
                .filter(|agent| agent.presentation_group() == group)
                .map(|agent| agent.agent_id.clone())
                .collect::<Vec<_>>();
            (!agent_ids.is_empty()).then_some(PresentationSection { group, agent_ids })
        })
        .collect()
}

fn parse_supervision_activity(reason: &str) -> Option<SupervisionActivity> {
    let mut payload = reason.trim().to_ascii_lowercase();
    for prefix in [
        "done:",
        "foreground_command:",
        "foreground-command:",
        "current_command:",
        "current-command:",
        "pane_label:",
        "pane-label:",
        "activity:",
        "poll:",
        "polling:",
        "watch:",
        "watcher:",
        "sleep:",
        "while:",
    ] {
        if let Some(rest) = payload.strip_prefix(prefix) {
            payload = rest.trim().to_string();
            break;
        }
    }
    let lower = payload.as_str();
    if lower.is_empty() || lower.starts_with("not ") || lower.contains("inactive") {
        return None;
    }

    let command = lower
        .split([';', '|', '·'])
        .next()
        .map(str::trim)
        .unwrap_or_default();
    let first = command.split_whitespace().next().unwrap_or_default();
    let command_name = first.rsplit('/').next().unwrap_or(first);
    let remainder = command.strip_prefix(first).unwrap_or_default().trim();
    let kind = if matches!(command_name, "watch" | "watcher") {
        SupervisionKind::Watcher
    } else if matches!(command_name, "poll" | "polling" | "sleep" | "while") {
        SupervisionKind::Polling
    } else {
        return None;
    };
    let structured = match command_name {
        "sleep" | "while" => first_number_after(lower, &["sleep"]).is_some(),
        "poll" | "polling" | "watch" | "watcher" => {
            remainder.is_empty()
                || remainder.starts_with("every")
                || remainder.starts_with("-n")
                || remainder.starts_with("interval")
                || remainder.starts_with("active")
                || remainder.starts_with("queued")
                || remainder.starts_with("current_command")
                || remainder.starts_with("current-command")
        }
        _ => false,
    };
    if !structured {
        return None;
    }

    let interval_seconds = first_number_after(lower, &["every", "sleep", "-n"]);
    let queued_work = first_number_after(lower, &["queued_work", "queued"]);
    Some(SupervisionActivity {
        kind,
        interval_seconds,
        queued_work,
    })
}

fn first_number_after(text: &str, markers: &[&str]) -> Option<u64> {
    markers.iter().find_map(|marker| {
        let start = text.find(marker)? + marker.len();
        let digits = text[start..].trim_start_matches(|ch: char| {
            ch == ':' || ch == '=' || ch == '-' || ch.is_whitespace()
        });
        let end = digits
            .char_indices()
            .find(|(_, ch)| !ch.is_ascii_digit())
            .map_or(digits.len(), |(index, _)| index);
        (end > 0).then(|| digits[..end].parse().ok()).flatten()
    })
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    #[serde(default)]
    pub build_identity: Option<BuildIdentity>,
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
    fn host_identity_warns_on_unknown_protocol_or_old_schema() {
        assert!(
            host_compatibility_warning(None)
                .expect("unknown host warning")
                .contains("unknown")
        );
        let compatible = BuildIdentity {
            build_id: "release-sha".into(),
            version: "0.1.0".into(),
            protocol_version: SUPPORTED_PROTOCOL_VERSION,
            schema_version: MINIMUM_SCHEMA_VERSION,
        };
        assert_eq!(compatible.compatibility_warning(), None);
        let old_schema = BuildIdentity {
            schema_version: MINIMUM_SCHEMA_VERSION - 1,
            ..compatible.clone()
        };
        assert!(
            old_schema
                .compatibility_warning()
                .expect("old schema warning")
                .contains("Update corrald")
        );
        assert!(
            host_compatibility_warning(Some(&old_schema))
                .expect("old schema helper warning")
                .contains("Update corrald")
        );
        let new_protocol = BuildIdentity {
            protocol_version: SUPPORTED_PROTOCOL_VERSION + 1,
            ..compatible
        };
        assert!(
            new_protocol
                .compatibility_warning()
                .expect("protocol warning")
                .contains("incompatible")
        );
        assert!(
            host_compatibility_warning(Some(&new_protocol))
                .expect("protocol helper warning")
                .contains("incompatible")
        );
    }

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

    fn presentation_agent(id: &str, state: AgentState, reason: Option<&str>) -> Agent {
        let mut agent = base_agent(id);
        agent.state = state;
        agent.reason = reason.map(str::to_owned);
        agent.display_name = Some(id.to_owned());
        agent
    }

    #[test]
    fn active_done_orchestrator_poll_is_supervising_with_safe_activity_text() {
        let agent = presentation_agent(
            "herdr:demo:orch",
            AgentState::Done,
            Some("done: poll every 60s; queued_work=1; current_command=/private/token"),
        );

        assert_eq!(agent.role(), AgentRole::Orchestrator);
        assert_eq!(agent.presentation_group(), PresentationGroup::Supervising);
        let activity = agent
            .supervision_activity()
            .expect("structured poll evidence");
        assert_eq!(activity.kind, SupervisionKind::Polling);
        assert_eq!(activity.interval_seconds, Some(60));
        assert_eq!(activity.queued_work, Some(1));
        assert_eq!(activity.summary(), "↻ Polling · every 60s");
        assert!(!activity.summary().contains("/private/token"));
        assert!(
            activity
                .accessibility_label()
                .contains("Activity: Supervising")
        );
        assert!(
            activity
                .accessibility_label()
                .contains("current command redacted")
        );
        assert!(!activity.accessibility_label().contains("/private/token"));
    }

    #[test]
    fn supervision_requires_done_orchestrator_and_active_structured_evidence() {
        let inactive = presentation_agent("herdr:demo:orch", AgentState::Done, None);
        assert_eq!(inactive.presentation_group(), PresentationGroup::Finished);

        let implementer = presentation_agent(
            "herdr:demo:impl",
            AgentState::Done,
            Some("done: poll every 60s"),
        );
        assert_eq!(implementer.role(), AgentRole::Implementer);
        assert_eq!(
            implementer.presentation_group(),
            PresentationGroup::Finished
        );

        let reviewer = presentation_agent(
            "herdr:demo:review",
            AgentState::Done,
            Some("done: poll every 60s"),
        );
        assert_eq!(reviewer.role(), AgentRole::Reviewer);
        assert_eq!(reviewer.presentation_group(), PresentationGroup::Finished);

        let not_done = presentation_agent(
            "herdr:demo:orch",
            AgentState::Working,
            Some("working: poll every 60s"),
        );
        assert_eq!(not_done.presentation_group(), PresentationGroup::Working);

        let role_only =
            presentation_agent("herdr:demo:orch", AgentState::Done, Some("task complete"));
        assert_eq!(role_only.presentation_group(), PresentationGroup::Finished);
        assert!(role_only.supervision_activity().is_none());

        let arbitrary_prose = presentation_agent(
            "herdr:demo:orch",
            AgentState::Done,
            Some("polling complete"),
        );
        assert_eq!(
            arbitrary_prose.presentation_group(),
            PresentationGroup::Finished
        );
        assert!(arbitrary_prose.supervision_activity().is_none());
    }

    #[test]
    fn presentation_sections_are_ordered_and_counted_without_empty_groups() {
        let agents = vec![
            presentation_agent("herdr:idle", AgentState::Idle, None),
            presentation_agent("herdr:done:review", AgentState::Done, None),
            presentation_agent(
                "herdr:orch",
                AgentState::Done,
                Some("done: watcher every 30s"),
            ),
            presentation_agent("herdr:working", AgentState::Working, None),
            presentation_agent("herdr:blocked", AgentState::Blocked, None),
        ];

        let sections = presentation_sections(&agents);
        assert_eq!(
            sections
                .iter()
                .map(PresentationSection::header)
                .collect::<Vec<_>>(),
            vec![
                "Needs you (1)",
                "Working (1)",
                "Supervising (1)",
                "Finished (1)",
                "Idle (1)"
            ],
        );
        assert_eq!(sections[2].group, PresentationGroup::Supervising);
        assert_eq!(sections[2].agent_ids, vec!["herdr:orch"]);
    }

    #[test]
    fn done_state_uses_finished_wording_without_changing_mark_or_rank() {
        let state = crate::theme::AgentStateLike::Done;
        assert_eq!(state.label(), "Finished");
        assert_eq!(state.mark(), "check");
        assert_eq!(state.rank(), 1);
    }
}
