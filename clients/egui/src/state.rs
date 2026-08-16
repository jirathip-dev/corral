//! UI-visible state: the fleet cache (applied from snapshot/delta SSE),
//! connection status, banners/toasts, device registration records, and
//! per-agent drive bookkeeping. All plain data — the UI thread owns it;
//! background tasks hand over messages through channels.

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

/// Persisted client config (`~/.config/corral/ui/config.json`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Default)]
pub struct PersistedConfig {
    pub host_url: Option<String>,
    pub registration: Option<RegistrationRecord>,
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

/// The fleet view model.
#[derive(Debug, Default)]
pub struct Fleet {
    pub agents: BTreeMap<String, Agent>,
    pub rev: Option<u64>,
    pub generated_at: Option<u64>,
    /// Drive outcome bookkeeping per agent, newest first.
    pub recent_drives: HashMap<String, VecDeque<DriveState>>,
    /// read_tail content per agent (only ever fetched on tap; bounded).
    pub tails: HashMap<String, Vec<String>>,
    /// Expanded agent ids (row detail open).
    pub expanded: Vec<String>,
}

impl Fleet {
    pub fn apply_snapshot(&mut self, snap: &crate::model::Snapshot) {
        self.agents = snap.agents.clone();
        self.rev = Some(snap.rev);
        self.generated_at = Some(snap.generated_at);
    }

    pub fn apply_delta(&mut self, delta: &crate::model::Delta) {
        for agent in &delta.upd {
            self.agents.insert(agent.agent_id.clone(), agent.clone());
        }
        for id in &delta.del {
            self.agents.remove(id);
            self.tails.remove(id);
            self.recent_drives.remove(id);
            self.expanded.retain(|e| e != id);
        }
        if delta.rev > self.rev.unwrap_or(0) {
            self.rev = Some(delta.rev);
        }
    }

    pub fn is_expanded(&self, agent_id: &str) -> bool {
        self.expanded.iter().any(|e| e == agent_id)
    }

    pub fn toggle_expanded(&mut self, agent_id: &str) {
        if self.is_expanded(agent_id) {
            self.expanded.retain(|e| e != agent_id);
        } else {
            self.expanded.push(agent_id.to_string());
        }
    }

    /// Bounded tail cache: newest N agents only (the daemon bounds the
    /// content at 200 lines / 32 KiB; the client bounds the number of
    /// cached tails).
    pub fn remember_tail(&mut self, agent_id: &str, tail: Vec<String>) {
        if self.tails.len() >= 64
            && !self.tails.contains_key(agent_id)
            && let Some(oldest) = self.tails.keys().next().cloned()
        {
            self.tails.remove(&oldest);
        }
        self.tails.insert(agent_id.to_string(), tail);
    }

    pub fn remember_drive(&mut self, agent_id: &str, state: DriveState) {
        let deque = self.recent_drives.entry(agent_id.to_string()).or_default();
        deque.push_front(state);
        while deque.len() > 8 {
            deque.pop_back();
        }
    }
}

/// The single source of truth for device capability gating: an agent's
/// capability button renders only if the agent advertises it AND the
/// device's grant record (registration grants, minus observed
/// `not_granted` refusals, plus observed successes) allows it.
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
            // granted: prompt") — parse defensively.
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
}

impl DriveMsg {
    pub fn is_ok(&self) -> bool {
        matches!(self.outcome, DriveOutcome::Ok { .. })
    }
}

/// One audit view refresh (host-admin).
#[derive(Debug, Clone)]
pub struct AuditMsg {
    pub view: Result<crate::protocol::AuditView, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Delta;

    fn agent(id: &str) -> Agent {
        Agent {
            agent_id: id.into(),
            source: "herdr".into(),
            tool: "claude".into(),
            state: crate::model::AgentState::Idle,
            reason: None,
            seq: 1,
            ts: 0,
            capabilities: vec!["prompt".into()],
            waiting_on: None,
            cost: None,
            parent_id: None,
            host: None,
            workspace: Default::default(),
            attachment: None,
            display_name: None,
            title: None,
        }
    }

    #[test]
    fn delta_upserts_deletes_and_tracks_rev() {
        let mut fleet = Fleet::default();
        let mut snap = crate::model::Snapshot {
            schema_version: 3,
            rev: 10,
            generated_at: 0,
            agents: BTreeMap::new(),
        };
        snap.agents.insert("a".into(), agent("a"));
        snap.agents.insert("b".into(), agent("b"));
        fleet.apply_snapshot(&snap);
        assert_eq!(fleet.rev, Some(10));
        assert_eq!(fleet.agents.len(), 2);

        let mut b2 = agent("b");
        b2.title = Some("new".into());
        fleet.apply_delta(&Delta {
            rev: 11,
            upd: vec![b2.clone()],
            del: vec!["a".into()],
        });
        assert_eq!(fleet.rev, Some(11));
        assert_eq!(fleet.agents.len(), 1);
        assert_eq!(fleet.agents["b"].title.as_deref(), Some("new"));
    }

    #[test]
    fn older_deltas_do_not_regress_rev() {
        let mut fleet = Fleet::default();
        let mut snap = crate::model::Snapshot {
            schema_version: 3,
            rev: 20,
            generated_at: 0,
            agents: BTreeMap::new(),
        };
        snap.agents.insert("a".into(), agent("a"));
        fleet.apply_snapshot(&snap);
        fleet.apply_delta(&Delta {
            rev: 19,
            upd: vec![agent("c")],
            del: vec![],
        });
        assert_eq!(fleet.rev, Some(20), "rev is monotonic");
        assert!(fleet.agents.contains_key("c"));
    }

    #[test]
    fn deletion_cleans_derived_state() {
        let mut fleet = Fleet::default();
        let mut snap = crate::model::Snapshot {
            schema_version: 3,
            rev: 1,
            generated_at: 0,
            agents: BTreeMap::new(),
        };
        snap.agents.insert("a".into(), agent("a"));
        fleet.apply_snapshot(&snap);
        fleet.expanded.push("a".into());
        fleet.tails.insert("a".into(), vec!["line".into()]);
        fleet.apply_delta(&Delta {
            rev: 2,
            upd: vec![],
            del: vec!["a".into()],
        });
        assert!(fleet.agents.is_empty());
        assert!(fleet.tails.is_empty());
        assert!(fleet.expanded.is_empty());
    }

    #[test]
    fn grant_ledger_gates_and_demotes() {
        let mut ledger = GrantLedger {
            base: vec!["prompt".into(), "interrupt".into()],
            denied: vec![],
        };
        assert!(ledger.allowed("prompt"));
        assert!(!ledger.allowed("kill"));
        ledger.note_refusal(&DriveFailure::NotGranted(
            "capability not granted: interrupt".into(),
        ));
        assert!(!ledger.allowed("interrupt"));
        assert!(ledger.allowed("prompt"));
        ledger.note_success("interrupt");
        assert!(ledger.allowed("interrupt"), "observed success re-allows");
    }

    #[test]
    fn tail_cache_is_bounded() {
        let mut fleet = Fleet::default();
        for i in 0..80 {
            fleet.remember_tail(&format!("a{i}"), vec![format!("line {i}")]);
        }
        assert!(fleet.tails.len() <= 64);
        assert!(fleet.tails.contains_key("a79"));
    }

    #[test]
    fn read_tail_result_flows_into_the_tail_cache_the_detail_view_renders() {
        // P4 W2.1 round trip: the daemon's DriveResponse.result for
        // read_tail → parse → remember_tail → the exact lines board.rs's
        // detail view iterates over.
        let result = serde_json::json!({
            "lines": ["  computing…", "deploy token [REDACTED]", "", "  done rev 42"]
        });
        let mut fleet = Fleet::default();
        fleet.remember_tail("herdr:a", crate::drive::parse_tail_lines(&result));

        let tail = fleet.tails.get("herdr:a").expect("tail cached");
        assert_eq!(
            tail,
            &vec!["  computing…", "deploy token [REDACTED]", "", "  done rev 42"]
        );
        // The detail view renders each line as a monospace label.
        assert_eq!(tail.len(), 4);
        assert!(tail[1].contains("[REDACTED]"), "daemon-redacted line survives");
    }

    #[test]
    fn empty_read_tail_result_is_a_clean_empty_state() {
        // No output: the daemon returns `{"lines": []}`; the cache holds an
        // empty tail and the detail view renders the empty-state copy —
        // never an error.
        let result = serde_json::json!({ "lines": [] });
        let mut fleet = Fleet::default();
        fleet.remember_tail("herdr:a", crate::drive::parse_tail_lines(&result));
        let tail = fleet.tails.get("herdr:a").expect("empty tail stored");
        assert!(tail.is_empty());
    }

    #[test]
    fn recent_drives_are_bounded() {
        let mut fleet = Fleet::default();
        for i in 0..20 {
            fleet.remember_drive(
                "a",
                DriveState::Sending {
                    request_id: format!("r{i}"),
                    capability: "read_tail".into(),
                },
            );
        }
        assert_eq!(fleet.recent_drives["a"].len(), 8);
    }
}
