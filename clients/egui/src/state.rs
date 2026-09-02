//! UI-visible state: the fleet cache (applied from snapshot/delta SSE),
//! connection status, banners/toasts, device registration records, and
//! per-agent read-drive bookkeeping. All plain data — the UI thread owns
//! it; background tasks hand over messages through channels.
//!
//! #354 read-only cut: only the read surface remains (snapshot/SSE +
//! bounded read_tail); the mutating drive bookkeeping, issues cache, diff
//! cache and completed-mode machinery were removed with their surfaces.

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::drive::{DriveFailure, DriveOutcome};
use crate::model::Agent;

/// Registration record persisted per host fingerprint.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RegistrationRecord {
    pub host_fingerprint: String,
    pub key_id: String,
    /// Grants the daemon returned at registration (read-only default:
    /// empty). The drive outcome feed keeps this honest (not_granted
    /// refusals demote).
    pub grants: Vec<String>,
    /// Capabilities observed refused with `not_granted` (persisted so a
    /// restart keeps the demotion; cleared when a successful drive or a
    /// grants refresh re-enables them).
    #[serde(default)]
    pub denied: Vec<String>,
}

/// Persisted client config (`~/.config/corral/ui/config.json`). The wire
/// shape is tolerant: every field is optional so an older config file (with
/// the pre-cut view toggles) still decodes — the extra keys are ignored.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Default)]
pub struct PersistedConfig {
    pub host_url: Option<String>,
    pub registration: Option<RegistrationRecord>,
    #[serde(default)]
    pub auto_reconnect: Option<bool>,
}

/// One drive action in flight (UI-side bookkeeping only — the actual
/// HTTP runs on the tokio runtime). Each variant carries the capability
/// so the board can answer "what did this drive do" (e.g. whether a
/// read_tail Ok dispatched) without extra state.
#[derive(Debug, Clone, PartialEq)]
pub enum DriveState {
    Sending {
        request_id: String,
        capability: String,
    },
    Ok {
        rev: u64,
        capability: String,
    },
    Failed {
        failure: DriveFailure,
        capability: String,
    },
}

impl DriveState {
    /// Whether this bookkeeping entry is for the retained read drive.
    pub fn is_read_tail(&self) -> bool {
        matches!(
            self,
            DriveState::Sending { capability, .. }
                | DriveState::Ok { capability, .. }
                | DriveState::Failed { capability, .. }
                if capability == "read_tail"
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Connected,
    Reconnecting { backoff_ms: u64 },
    Down,
}

impl ConnState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Connected => "live",
            Self::Reconnecting { .. } => "reconnecting",
            Self::Down => "down",
        }
    }
}

/// A transient UI message (error banner / success toast).
#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub level: Level,
    pub at: std::time::Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// The fleet read model: agents + rev from snapshot/delta SSE, and the
/// bounded per-agent read_tail caches behind the recents drill-in.
#[derive(Debug, Clone, Default)]
pub struct Fleet {
    pub agents: BTreeMap<String, Agent>,
    pub rev: Option<u64>,
    pub generated_at: Option<u64>,
    /// Drive outcome bookkeeping per agent, newest first (read_tail only
    /// after the cut).
    pub recent_drives: HashMap<String, VecDeque<DriveState>>,
    /// read_tail content per agent (bounded; initially hydrated once for the
    /// visible selection when capability/grant-gated).
    pub tails: HashMap<String, Vec<String>>,
    pub tail_source_revs: HashMap<String, u64>,
    /// #315: the daemon's CANONICAL semantic blocks for the read window,
    /// cached alongside the legacy `tails` lines. When present the Recent
    /// output view renders these verbatim — the client never re-derives
    /// block kinds from raw lines.
    pub tail_blocks: HashMap<String, Vec<crate::drive::CanonicalBlock>>,
    /// Master/detail selection. Kept in the view model so a frame can
    /// resolve a still-valid default without an extra egui temp key.
    pub selected_agent: Option<String>,
}

impl Fleet {
    pub fn apply_snapshot(&mut self, snap: &crate::model::Snapshot) {
        if snap.rev < self.rev.unwrap_or(0) {
            // A stale-agent recovery fetch races the SSE stream. A response
            // from before the current cursor must not roll the board back.
            return;
        }
        self.agents = snap.agents.clone();
        self.rev = Some(snap.rev);
        self.generated_at = Some(snap.generated_at);
        // A reconnect snapshot that dropped an agent must not leave an
        // orphan tail cache (a stale read_tail fetch against it would burn
        // an audited unknown_agent fetch). The bounded read_tail cache
        // follows the same removal rule.
        let agents = &self.agents;
        self.tails.retain(|id, _| agents.contains_key(id));
        self.tail_blocks.retain(|id, _| agents.contains_key(id));
        if self
            .selected_agent
            .as_deref()
            .is_some_and(|id| !agents.contains_key(id))
        {
            self.selected_agent = None;
        }
    }

    pub fn apply_delta(&mut self, delta: &crate::model::Delta) {
        if delta.rev <= self.rev.unwrap_or(0) {
            // Duplicate and late SSE frames are already represented by the
            // current read model; applying their payload could regress an
            // agent even though the cursor stays monotonic.
            return;
        }
        for agent in &delta.upd {
            self.agents.insert(agent.agent_id.clone(), agent.clone());
        }
        for id in &delta.del {
            self.remove_agent(id);
        }
        self.rev = Some(delta.rev);
    }

    /// Remove an agent (delta delete / stale identity). The next
    /// snapshot/SSE delta may re-add a genuinely current identity.
    pub fn remove_agent(&mut self, agent_id: &str) {
        self.agents.remove(agent_id);
        self.tails.remove(agent_id);
        self.tail_blocks.remove(agent_id);
        self.tail_source_revs.remove(agent_id);
        self.recent_drives.remove(agent_id);
        if self.selected_agent.as_deref() == Some(agent_id) {
            self.selected_agent = None;
        }
    }

    pub fn select_agent(&mut self, agent_id: &str) {
        self.selected_agent = Some(agent_id.to_string());
    }

    /// Bounded tail cache: newest N agents only (the daemon bounds the
    /// content at 200 lines / 32 KiB; the client bounds the number of
    /// cached tails).
    pub fn remember_tail(&mut self, agent_id: &str, tail: Vec<String>) {
        self.remember_tail_full(agent_id, tail, Vec::new(), None);
    }

    /// #315: fold a full read_tail result (lines + canonical blocks) into
    /// the caches. `blocks` empty = an old daemon without the canonical
    /// stream; the Recent output view falls back to the legacy lines.
    pub fn remember_tail_full(
        &mut self,
        agent_id: &str,
        tail: Vec<String>,
        blocks: Vec<crate::drive::CanonicalBlock>,
        source_rev: Option<u64>,
    ) {
        if self.tails.len() >= 64
            && !self.tails.contains_key(agent_id)
            && let Some(oldest) = self.tails.keys().next().cloned()
        {
            self.tails.remove(&oldest);
            self.tail_blocks.remove(&oldest);
        }
        self.tails.insert(agent_id.to_string(), tail);
        self.tail_blocks.insert(agent_id.to_string(), blocks);
        if let Some(source_rev) = source_rev {
            self.tail_source_revs
                .insert(agent_id.to_string(), source_rev);
        }
    }

    pub fn remember_drive(&mut self, agent_id: &str, state: DriveState) {
        let deque = self.recent_drives.entry(agent_id.to_string()).or_default();
        deque.push_front(state);
        while deque.len() > 8 {
            deque.pop_back();
        }
    }

    /// The newest in-flight drive state for an agent.
    pub fn latest_drive(&self, agent_id: &str) -> Option<&DriveState> {
        self.recent_drives.get(agent_id).and_then(|d| d.front())
    }

    /// Whether the selected agent needs its first Recent-output hydration.
    /// A cached empty tail is still a successful response and must not cause
    /// a request loop; an existing drive for read_tail likewise proves that
    /// the fetch path is already in flight or has completed.
    pub fn needs_recent_output(&self, agent_id: &str) -> bool {
        if self.tails.contains_key(agent_id) {
            return false;
        }
        if self
            .recent_drives
            .get(agent_id)
            .is_some_and(|drives| drives.iter().any(DriveState::is_read_tail))
        {
            return false;
        }
        true
    }

    /// The cached `source_rev` a visible agent's automatic Recent-output
    /// refresh must carry, if a refresh is eligible right now. Eligible = a
    /// cached tail exists (so this is a refresh, not the first hydration),
    /// its cached source revision is known, and no read_tail request for the
    /// agent is currently in flight. Hidden agents never reach this: the
    /// caller passes only the resolved visible selection.
    pub fn recent_output_refresh_candidate(&self, agent_id: &str) -> Option<u64> {
        let source_rev = *self.tail_source_revs.get(agent_id)?;
        if !self.tails.contains_key(agent_id) {
            return None;
        }
        let newest_read_tail_in_flight = self.recent_drives.get(agent_id).is_some_and(|drives| {
            drives
                .iter()
                .find(|state| state.is_read_tail())
                .is_some_and(|state| matches!(state, DriveState::Sending { .. }))
        });
        if newest_read_tail_in_flight {
            return None;
        }
        Some(source_rev)
    }
}

/// The single source of truth for device capability gating (read-only after
/// the #354 cut): a read control is ready when the registration grant record
/// allows it. Missing grant is reported separately from capability absence.
#[derive(Debug, Clone, Default)]
pub struct GrantLedger {
    /// Grants known from the last registration response.
    pub base: Vec<String>,
    /// Capabilities observed refused with `not_granted`.
    pub denied: Vec<String>,
}

impl GrantLedger {
    pub fn allowed(&self, capability: &str) -> bool {
        self.base.iter().any(|c| c == capability) && !self.is_denied(capability)
    }

    pub fn is_denied(&self, capability: &str) -> bool {
        self.denied.iter().any(|c| c == capability)
    }

    pub fn note_refusal(&mut self, failure: &DriveFailure) {
        if let DriveFailure::NotGranted(message) = failure {
            // The message carries the capability name ("capability not
            // granted: read_tail") — parse defensively.
            let capability = message
                .rsplit(": ")
                .next()
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            if !capability.is_empty() && !self.is_denied(&capability) {
                self.denied.push(capability);
            }
        }
    }

    pub fn note_success(&mut self, capability: &str) {
        self.denied.retain(|c| c != capability);
    }
}

/// Outcome of a drive round-trip delivered to the UI.
#[derive(Debug, Clone)]
pub struct DriveMsg {
    pub agent_id: String,
    pub capability: String,
    pub outcome: DriveOutcome,
    /// Identity epoch at dispatch time (#310 r3): a drive initiated before
    /// the current key/registration generation must never set or clear
    /// current recovery state when its result arrives late.
    pub identity_generation: u64,
}

impl DriveMsg {
    pub fn is_ok(&self) -> bool {
        matches!(self.outcome, DriveOutcome::Ok { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentState, Delta};

    fn agent(id: &str) -> Agent {
        Agent {
            agent_id: id.into(),
            source: "herdr".into(),
            tool: "claude".into(),
            state: AgentState::Idle,
            reason: None,
            seq: 1,
            ts: 0,
            capabilities: vec!["read_tail".into()],
            workspace: Default::default(),
            attachment: None,
            display_name: None,
            title: None,
        }
    }

    #[test]
    fn conn_state_labels_cover_all_states() {
        for s in [
            ConnState::Connecting,
            ConnState::Connected,
            ConnState::Reconnecting { backoff_ms: 100 },
            ConnState::Down,
        ] {
            assert!(!s.label().is_empty());
        }
        assert_eq!(ConnState::Connected.label(), "live");
        assert_eq!(ConnState::Down.label(), "down");
    }

    #[test]
    fn snapshot_delta_and_tail_cache_follow_the_read_model() {
        let mut fleet = Fleet::default();
        let snap = crate::model::Snapshot {
            schema_version: 5,
            rev: 10,
            generated_at: 1,
            agents: BTreeMap::from([("herdr:a".to_string(), agent("herdr:a"))]),
        };
        fleet.apply_snapshot(&snap);
        assert_eq!(fleet.rev, Some(10));
        assert!(fleet.needs_recent_output("herdr:a"));

        fleet.remember_tail("herdr:a", vec!["line".to_string()]);
        assert!(!fleet.needs_recent_output("herdr:a"));

        // Delta delete prunes the tail cache with the agent.
        let delta = Delta {
            rev: 11,
            upd: vec![],
            del: vec!["herdr:a".to_string()],
        };
        fleet.apply_delta(&delta);
        assert!(!fleet.agents.contains_key("herdr:a"));
        assert!(!fleet.tails.contains_key("herdr:a"));
    }

    #[test]
    fn remember_tail_full_bounds_cached_agents_and_records_source_rev() {
        let mut fleet = Fleet::default();
        for index in 0..70 {
            let id = format!("herdr:agent-{index:02}");
            let mut a = agent(&id);
            a.state = AgentState::Working;
            fleet.agents.insert(id.clone(), a);
            fleet.remember_tail_full(&id, vec![], Vec::new(), Some(index as u64));
        }
        assert!(fleet.tails.len() <= 64, "tail cache stays bounded");
        assert_eq!(
            fleet.tail_source_revs.get("herdr:agent-69"),
            Some(&69),
            "newest agent's revision is retained"
        );
    }

    #[test]
    fn stale_snapshot_and_duplicate_delta_are_ignored() {
        let mut fleet = Fleet::default();
        let snap = crate::model::Snapshot {
            schema_version: 5,
            rev: 5,
            generated_at: 1,
            agents: BTreeMap::from([("herdr:a".to_string(), agent("herdr:a"))]),
        };
        fleet.apply_snapshot(&snap);
        let stale = crate::model::Snapshot {
            schema_version: 5,
            rev: 3,
            generated_at: 2,
            agents: BTreeMap::new(),
        };
        fleet.apply_snapshot(&stale);
        assert_eq!(
            fleet.rev,
            Some(5),
            "stale snapshot cannot roll the board back"
        );

        let duplicate = Delta {
            rev: 5,
            upd: vec![agent("herdr:b")],
            del: vec![],
        };
        fleet.apply_delta(&duplicate);
        assert!(!fleet.agents.contains_key("herdr:b"));
    }

    #[test]
    fn refresh_candidate_respects_cache_and_single_flight() {
        let mut fleet = Fleet::default();
        assert_eq!(fleet.recent_output_refresh_candidate("herdr:a"), None);

        fleet.remember_tail_full("herdr:a", vec!["x".into()], Vec::new(), Some(7));
        assert_eq!(fleet.recent_output_refresh_candidate("herdr:a"), Some(7));

        fleet.remember_drive(
            "herdr:a",
            DriveState::Sending {
                request_id: "r".into(),
                capability: "read_tail".into(),
            },
        );
        assert_eq!(
            fleet.recent_output_refresh_candidate("herdr:a"),
            None,
            "a read_tail in flight suppresses the refresh"
        );
    }
}
