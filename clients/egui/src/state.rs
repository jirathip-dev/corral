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
    /// #64: transcript pane per agent (fetched on demand; each pane is a
    /// bounded sliding window, and at most 64 agents are cached here —
    /// LRU-evicted by `transcript_clock`).
    pub transcripts: HashMap<String, crate::transcript::TranscriptPane>,
    /// Monotonic clock stamping pane generations (response correlation,
    /// review F1) and last-touch order (real LRU eviction, review F14).
    pub transcript_clock: u64,
    /// Expanded agent ids (row detail open).
    pub expanded: Vec<String>,
}

impl Fleet {
    pub fn apply_snapshot(&mut self, snap: &crate::model::Snapshot) {
        self.agents = snap.agents.clone();
        self.rev = Some(snap.rev);
        self.generated_at = Some(snap.generated_at);
        // #64 review R8: a reconnect snapshot that dropped an agent must
        // not leave an orphan transcript pane (a stale-cursor auto-reload
        // against it would burn an audited unknown_agent fetch). `tails`
        // has the same pre-existing gap; pruning it is out of #64's scope.
        let agents = &self.agents;
        self.transcripts.retain(|id, _| agents.contains_key(id));
    }

    pub fn apply_delta(&mut self, delta: &crate::model::Delta) {
        for agent in &delta.upd {
            self.agents.insert(agent.agent_id.clone(), agent.clone());
        }
        for id in &delta.del {
            self.remove_agent(id);
        }
        if delta.rev > self.rev.unwrap_or(0) {
            self.rev = Some(delta.rev);
        }
    }

    /// Remove a target immediately when a drive reports that its snapshot
    /// identity is stale. The next snapshot/SSE delta may re-add a genuinely
    /// current identity, but no controls remain usable during the refresh.
    pub fn remove_agent(&mut self, agent_id: &str) {
        self.agents.remove(agent_id);
        self.tails.remove(agent_id);
        self.transcripts.remove(agent_id);
        self.recent_drives.remove(agent_id);
        self.expanded.retain(|id| id != agent_id);
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

    /// #64: the transcript pane for an agent, creating it under a fresh
    /// generation if absent. Bounded at 64 cached agents with REAL LRU
    /// eviction (least-recently-touched — review F14: for transcripts a
    /// wrong eviction throws away pages the user paged to by hand, each
    /// an audited read, so "arbitrary map order" is not good enough).
    pub fn transcript_pane_mut(
        &mut self,
        agent_id: &str,
    ) -> &mut crate::transcript::TranscriptPane {
        self.transcript_clock += 1;
        let clock = self.transcript_clock;
        if self.transcripts.len() >= 64
            && !self.transcripts.contains_key(agent_id)
            && let Some(lru) = self
                .transcripts
                .iter()
                .min_by_key(|(_, pane)| pane.touched)
                .map(|(id, _)| id.clone())
        {
            self.transcripts.remove(&lru);
        }
        let pane = self
            .transcripts
            .entry(agent_id.to_string())
            .or_insert_with(|| crate::transcript::TranscriptPane {
                generation: clock,
                ..Default::default()
            });
        pane.touched = clock;
        pane
    }

    /// #64 review F1: fold one fetch outcome into the pane it belongs
    /// to — or refuse. Pure and unit-tested: a response whose
    /// generation does not match the CURRENT pane (reset, evicted, or
    /// recreated since the request left) is DROPPED, and a response for
    /// a pane that no longer exists (agent deleted) never resurrects it
    /// (`get_mut`, not `transcript_pane_mut`). A first-strike stale
    /// cursor resets the pane under a new generation and asks the
    /// caller to refetch.
    pub fn fold_transcript(
        &mut self,
        msg: crate::transcript::TranscriptMsg,
    ) -> crate::transcript::FoldOutcome {
        use crate::transcript::FoldOutcome;
        self.transcript_clock += 1;
        let clock = self.transcript_clock;
        let Some(pane) = self.transcripts.get_mut(&msg.agent_id) else {
            return FoldOutcome::Dropped;
        };
        if msg.generation != pane.generation {
            return FoldOutcome::Dropped;
        }
        match msg.outcome {
            Ok(page) => {
                pane.apply_page(page);
                FoldOutcome::AppliedOk
            }
            Err(failure) if failure.is_stale_cursor() && !pane.auto_reloaded => {
                pane.auto_reloaded = true;
                pane.reset(clock);
                FoldOutcome::NeedsReload
            }
            Err(failure) => {
                let not_granted = failure.is_not_granted();
                pane.apply_failure(failure);
                if not_granted {
                    FoldOutcome::NotGranted
                } else {
                    FoldOutcome::Applied
                }
            }
        }
    }

    pub fn remember_drive(&mut self, agent_id: &str, state: DriveState) {
        let deque = self.recent_drives.entry(agent_id.to_string()).or_default();
        deque.push_front(state);
        while deque.len() > 8 {
            deque.pop_back();
        }
    }
}

/// G34 cost-meter view state: the last `GET /cost` result, delivered over
/// the same channel snapshots arrive on. `None` before the first poll;
/// `Err` (daemon down, non-2xx, malformed body) degrades the tiles to
/// "unknown" — never a panic.
#[derive(Debug, Clone, Default)]
pub struct CostState {
    pub report: Option<Result<crate::model::CostReport, String>>,
}

impl CostState {
    pub fn apply(&mut self, result: Result<crate::model::CostReport, String>) {
        self.report = Some(result);
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

    /// #64 review F5: a typed `not_granted` from a non-drive surface
    /// (the transcript GET) demotes the capability the same way a drive
    /// refusal does — the board must not keep claiming read_tail works
    /// after the daemon just refused it.
    pub fn note_denied(&mut self, capability: &str) {
        if !self.is_denied(capability) {
            self.denied.push(capability.to_string());
        }
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
            issues: vec![],
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
        fleet
            .transcripts
            .insert("a".into(), crate::transcript::TranscriptPane::default());
        fleet.apply_delta(&Delta {
            rev: 2,
            upd: vec![],
            del: vec!["a".into()],
        });
        assert!(fleet.agents.is_empty());
        assert!(fleet.tails.is_empty());
        assert!(fleet.transcripts.is_empty(), "#64 pane cleaned up too");
        assert!(fleet.expanded.is_empty());
    }

    /// #64: the transcript pane cache is bounded — a 65th agent evicts
    /// the LEAST-RECENTLY-TOUCHED pane (review F14: real LRU, never the
    /// one most recently fetched for — R5: pure reading does not stamp
    /// the clock), and an existing agent's pane
    /// never evicts.
    #[test]
    fn transcript_pane_cache_is_bounded_lru() {
        let mut fleet = Fleet::default();
        for i in 0..64 {
            let _ = fleet.transcript_pane_mut(&format!("agent-{i:02}"));
        }
        assert_eq!(fleet.transcripts.len(), 64);
        let _ = fleet.transcript_pane_mut("agent-00");
        assert_eq!(fleet.transcripts.len(), 64, "existing agent: no evict");
        // agent-01 is now the least-recently-touched — it must be the
        // one to go, and the just-touched agent-00 must survive.
        let _ = fleet.transcript_pane_mut("agent-new");
        assert_eq!(fleet.transcripts.len(), 64, "new agent evicts one");
        assert!(fleet.transcripts.contains_key("agent-new"));
        assert!(
            fleet.transcripts.contains_key("agent-00"),
            "LRU spares the most recently fetched-for pane"
        );
        assert!(
            !fleet.transcripts.contains_key("agent-01"),
            "LRU evicts the stalest"
        );
    }

    /// #64 review R8/round-3: a reconnect snapshot that dropped an
    /// agent prunes its transcript pane (delta deletion already did).
    #[test]
    fn snapshot_prunes_orphan_transcript_panes() {
        let mut fleet = Fleet::default();
        let mut snap = crate::model::Snapshot {
            schema_version: 3,
            rev: 1,
            generated_at: 0,
            agents: BTreeMap::new(),
        };
        snap.agents.insert("a".into(), agent("a"));
        fleet.apply_snapshot(&snap);
        let _ = fleet.transcript_pane_mut("a");
        assert!(fleet.transcripts.contains_key("a"));

        // Reconnect: a fresh snapshot without the agent.
        let empty = crate::model::Snapshot {
            schema_version: 3,
            rev: 2,
            generated_at: 0,
            agents: BTreeMap::new(),
        };
        fleet.apply_snapshot(&empty);
        assert!(
            fleet.transcripts.is_empty(),
            "orphan pane pruned on reconnect snapshot"
        );
    }

    fn msg(
        agent: &str,
        generation: u64,
        outcome: Result<crate::transcript::TranscriptPage, crate::transcript::TranscriptFailure>,
    ) -> crate::transcript::TranscriptMsg {
        crate::transcript::TranscriptMsg {
            agent_id: agent.into(),
            generation,
            outcome,
        }
    }

    fn a_page() -> crate::transcript::TranscriptPage {
        serde_json::from_value(serde_json::json!({
            "agent": "a", "store": "claude", "session": "claude:s.jsonl",
            "bind": "worktree", "stores_unavailable": [],
            "entries": [{"role": "user", "text": "hi", "ts": null}],
            "next_cursor": "b.5.aa", "skipped": 0,
        }))
        .expect("parses")
    }

    fn stale_cursor() -> crate::transcript::TranscriptFailure {
        crate::transcript::TranscriptFailure {
            kind: "bad_cursor".into(),
            message: "stale".into(),
            candidates: vec![],
        }
    }

    /// #64 review F1: response correlation. A late response from a
    /// superseded generation is dropped; a response for a pane that no
    /// longer exists (deleted agent) never resurrects it.
    #[test]
    fn fold_transcript_drops_stale_generations_and_never_resurrects() {
        use crate::transcript::FoldOutcome;
        let mut fleet = Fleet::default();
        let generation = fleet.transcript_pane_mut("a").generation;

        // In-flight response, then the user reloads (new generation):
        // the late response must be DROPPED, not appended to the fresh
        // pane.
        let tick = fleet.transcript_clock + 1;
        fleet.transcript_clock = tick;
        fleet
            .transcripts
            .get_mut("a")
            .expect("pane")
            .user_reset(tick);
        assert_eq!(
            fleet.fold_transcript(msg("a", generation, Ok(a_page()))),
            FoldOutcome::Dropped,
            "late page from the old generation is refused"
        );
        assert!(
            fleet.transcripts.get("a").expect("pane").entries.is_empty(),
            "the fresh pane stays clean"
        );

        // Current generation folds fine.
        let current = fleet.transcripts.get("a").expect("pane").generation;
        assert_eq!(
            fleet.fold_transcript(msg("a", current, Ok(a_page()))),
            FoldOutcome::AppliedOk
        );

        // Delete the agent: the in-flight response must NOT resurrect
        // the pane.
        fleet.transcripts.remove("a");
        assert_eq!(
            fleet.fold_transcript(msg("a", current, Ok(a_page()))),
            FoldOutcome::Dropped
        );
        assert!(!fleet.transcripts.contains_key("a"), "no resurrection");
    }

    /// #64 review F1/F7: a first stale cursor resets under a NEW
    /// generation and asks for a reload; a second surfaces as the error;
    /// a not_granted failure is routed to the ledger.
    #[test]
    fn fold_transcript_stale_cursor_once_and_not_granted_routing() {
        use crate::transcript::FoldOutcome;
        let mut fleet = Fleet::default();
        let g0 = fleet.transcript_pane_mut("a").generation;
        assert_eq!(
            fleet.fold_transcript(msg("a", g0, Err(stale_cursor()))),
            FoldOutcome::NeedsReload,
            "first strike reloads"
        );
        let pane = fleet.transcripts.get("a").expect("pane");
        assert_ne!(pane.generation, g0, "reset minted a new generation");
        assert!(pane.loading);

        let g1 = pane.generation;
        assert_eq!(
            fleet.fold_transcript(msg("a", g1, Err(stale_cursor()))),
            FoldOutcome::Applied,
            "second strike surfaces the error"
        );

        let g2 = fleet.transcripts.get("a").expect("pane").generation;
        let denied = crate::transcript::TranscriptFailure {
            kind: "not_granted".into(),
            message: "read_tail not granted".into(),
            candidates: vec![],
        };
        assert_eq!(
            fleet.fold_transcript(msg("a", g2, Err(denied))),
            FoldOutcome::NotGranted,
            "grant refusals reach the ledger"
        );
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
            &vec![
                "  computing…",
                "deploy token [REDACTED]",
                "",
                "  done rev 42"
            ]
        );
        // The detail view renders each line as a monospace label.
        assert_eq!(tail.len(), 4);
        assert!(
            tail[1].contains("[REDACTED]"),
            "daemon-redacted line survives"
        );
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

    #[test]
    fn cost_state_holds_the_last_poll_outcome() {
        // G34: the cost poll delivers over the same channel snapshots use;
        // the state holds the last outcome so the tiles never refetch per
        // frame, and an error degrades to "unknown" instead of a panic.
        let mut state = CostState::default();
        assert!(state.report.is_none(), "no poll yet → unknown");
        let report = crate::model::CostReport {
            generated_at: 0,
            providers: vec![],
        };
        state.apply(Ok(report.clone()));
        assert_eq!(state.report, Some(Ok(report)));
        state.apply(Err("daemon down".into()));
        assert!(matches!(state.report, Some(Err(_))));
    }
}
