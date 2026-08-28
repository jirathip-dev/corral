//! UI-visible state: the fleet cache (applied from snapshot/delta SSE),
//! connection status, banners/toasts, device registration records, and
//! per-agent drive bookkeeping. All plain data — the UI thread owns it;
//! background tasks hand over messages through channels.

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::drive::{DriveFailure, DriveOutcome};
use crate::model::{Agent, FleetHealthEntry, FleetIdentities, GhIssueRef};

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
    #[serde(default)]
    pub auto_reconnect: Option<bool>,
    #[serde(default)]
    pub group_by_repo: Option<bool>,
    #[serde(default)]
    pub show_idle_collapsed: Option<bool>,
    #[serde(default)]
    pub stick_to_bottom: Option<bool>,
    #[serde(default)]
    pub theme: Option<String>,
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

/// #232: one agent's cached read_diff surface (paged accumulation of the
/// daemon's bounded pages — the cache itself is never re-bounded, only
/// presented; full-diff content never lives here, only fetched pages).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiffCache {
    /// Repo identity from the daemon's snapshot-driven attribution (never
    /// client-supplied).
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub head: Option<String>,
    /// Whole-diffstat (all tracked changes vs HEAD).
    pub stats: crate::drive::DiffStats,
    /// Changed-files list (first N files, daemon-capped at 128).
    pub files: Vec<crate::drive::DiffFileStat>,
    pub files_truncated: bool,
    /// Accumulated unified diff lines (each fetched page appended).
    pub lines: Vec<String>,
    /// Full stream size in lines (aggregate offset space).
    pub total: u32,
    pub has_more: bool,
    /// Next page cursor; `None` when fully loaded.
    pub next_offset: Option<u32>,
}

/// The fleet view model.
#[derive(Debug, Default)]
pub struct Fleet {
    pub agents: BTreeMap<String, Agent>,
    pub rev: Option<u64>,
    pub generated_at: Option<u64>,
    /// Drive outcome bookkeeping per agent, newest first.
    pub recent_drives: HashMap<String, VecDeque<DriveState>>,
    /// read_tail content per agent (bounded; initially hydrated once for the
    /// visible Cards selection when capability/grant-gated, and still
    /// reloadable from its explicit control).
    pub tails: HashMap<String, Vec<String>>,
    /// #232: per-agent read_diff cache (changed-files list + paged unified
    /// diff + diffstat). Paged: each fetch appends at `next_offset`.
    pub diffs: HashMap<String, DiffCache>,
    /// Expanded agent ids (row detail open).
    pub expanded: Vec<String>,
    /// Master/detail selection. Kept in the view model so a frame can
    /// resolve a still-valid default without an extra egui temp key.
    pub selected_agent: Option<String>,
    /// #113: repo-level issue set from the daemon's read-only `GET /issues`
    /// view, keyed by repo/fleet name. Empty until the first fetch — the
    /// Issues tab renders from this (never from branch inference).
    pub issues: BTreeMap<String, Vec<GhIssueRef>>,
    /// Whether the repo-level issues have been fetched at least once.
    pub issues_loaded: bool,
    /// Whether a repo-level issue request is currently in flight.
    pub issues_loading: bool,
    /// Last issue-fetch failure. A previous successful snapshot remains
    /// visible while this is set so a transient refresh cannot blank the tab.
    pub issues_error: Option<String>,
    /// #237: fleet-ops CLI validated identity catalog (configless). `Ok`
    /// includes daemon-side `status="error"` responses so an unavailable
    /// identity path is visible; `Err` is a transport/endpoint failure.
    pub fleets: Option<Result<FleetIdentities, String>>,
    pub fleets_loading: bool,
    /// Whether the identity catalog has been successfully fetched at least
    /// once.
    pub fleets_loaded: bool,
    /// #210: per-fleet health strip from the latest snapshot (orch alive,
    /// live worker count, presence-heartbeat anchor, warnings). Empty until
    /// the daemon sends a snapshot carrying `fleet_health`.
    pub fleet_health: Vec<FleetHealthEntry>,
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
        self.fleet_health = snap.fleet_health.clone();
        // #64 review R8: a reconnect snapshot that dropped an agent must
        // not leave an orphan tail cache (a stale read_tail fetch against
        // it would burn an audited unknown_agent fetch). The bounded
        // read_tail cache follows the same removal rule.
        let agents = &self.agents;
        self.tails.retain(|id, _| agents.contains_key(id));
        self.expanded.retain(|id| agents.contains_key(id));
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

    /// Remove a target immediately when a drive reports that its snapshot
    /// identity is stale. The next snapshot/SSE delta may re-add a genuinely
    /// current identity, but no controls remain usable during the refresh.
    pub fn remove_agent(&mut self, agent_id: &str) {
        self.agents.remove(agent_id);
        self.tails.remove(agent_id);
        self.diffs.remove(agent_id);
        self.recent_drives.remove(agent_id);
        self.expanded.retain(|id| id != agent_id);
        if self.selected_agent.as_deref() == Some(agent_id) {
            self.selected_agent = None;
        }
    }

    pub fn is_expanded(&self, agent_id: &str) -> bool {
        self.expanded.iter().any(|e| e == agent_id)
    }

    /// #113: fold one repo-level issue fetch outcome into the view model.
    /// The browser sorts/renders the successful snapshot; the daemon remains
    /// the authority on which issue is startable. Errors do not discard the
    /// last successful snapshot, which keeps a refresh failure visible and
    /// retryable instead of turning it into a misleading empty state.
    pub fn set_issues(&mut self, result: Result<BTreeMap<String, Vec<GhIssueRef>>, String>) {
        self.issues_loading = false;
        match result {
            Ok(issues) => {
                self.issues = issues;
                self.issues_loaded = true;
                self.issues_error = None;
            }
            Err(error) => {
                self.issues_error = Some(error);
            }
        }
    }

    /// Whether the selected agent needs its first Recent-output hydration.
    /// A cached empty tail is still a successful response and must not cause
    /// a request loop; an existing drive for read_tail likewise proves that
    /// the fetch path is already in flight or has completed.
    pub fn needs_recent_output(&self, agent_id: &str) -> bool {
        if self.tails.contains_key(agent_id) {
            return false;
        }
        if self.recent_drives.get(agent_id).is_some_and(|drives| {
            drives.iter().any(|state| {
                matches!(
                    state,
                    DriveState::Sending { capability, .. }
                        | DriveState::Ok { capability, .. }
                        | DriveState::Failed { capability, .. }
                        if capability == "read_tail"
                )
            })
        }) {
            return false;
        }
        true
    }

    /// #237: fold one fleet-identities fetch outcome into the tab's view
    /// model.
    pub fn set_fleets(&mut self, result: Result<FleetIdentities, String>) {
        self.fleets_loading = false;
        if result.is_ok() {
            self.fleets_loaded = true;
        }
        self.fleets = Some(result);
    }

    /// Whether the catalog contains the successful daemon-side identity list
    /// needed to turn an Issues category into an exact fleet-name drive
    /// target. Transport failures and daemon `status="error"` responses
    /// remain in the view model for diagnostics, but neither is actionable.
    pub fn fleets_ready(&self) -> bool {
        matches!(
            self.fleets.as_ref(),
            Some(Ok(fleets)) if fleets.status.trim() == "ok"
        )
    }

    pub fn toggle_expanded(&mut self, agent_id: &str) {
        if self.is_expanded(agent_id) {
            self.expanded.retain(|e| e != agent_id);
        } else {
            self.expanded.push(agent_id.to_string());
        }
    }

    pub fn select_agent(&mut self, agent_id: &str) {
        self.selected_agent = Some(agent_id.to_string());
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

    /// #232: fold one read_diff page into the per-agent cache. The first
    /// page (offset 0) seeds the cache; later pages append at the page's
    /// offset (the daemon's offsets are aggregate-line offsets, so
    /// out-of-order/missing pages never silently garble the stream).
    pub fn remember_diff_page(&mut self, agent_id: &str, page: crate::drive::DiffPage) {
        let cache = self.diffs.entry(agent_id.to_string()).or_default();
        cache.repo = page.repo;
        cache.branch = page.branch;
        cache.head = page.head;
        cache.stats = page.stats;
        cache.files = page.files;
        cache.files_truncated = page.files_truncated;
        cache.total = page.total;
        cache.has_more = page.has_more;
        cache.next_offset = page.next_offset;
        if page.offset == 0 && cache.lines.is_empty() {
            cache.lines = page.lines;
        } else if page.offset as usize <= cache.lines.len() {
            // Append the new page's lines; pages are served in order by the
            // UI, and a re-fetch of an already-present window only replaces
            // that window (idempotent append).
            let start = page.offset as usize;
            let end = start + page.lines.len();
            if end > cache.lines.len() {
                cache
                    .lines
                    .extend(page.lines[cache.lines.len().saturating_sub(start)..].to_vec());
            }
        } else {
            // Gap: drop the stale accumulated lines and reseed from this
            // page (a worktree change implicitly renumbers offsets).
            cache.lines = page.lines;
        }
    }

    pub fn remember_drive(&mut self, agent_id: &str, state: DriveState) {
        let deque = self.recent_drives.entry(agent_id.to_string()).or_default();
        deque.push_front(state);
        while deque.len() > 8 {
            deque.pop_back();
        }
    }

    /// The newest in-flight drive state for a target (agent id or, for the
    /// fleet-level worktree start, the fleet/repo name). Used by the issue
    /// browser to render a pending indicator while the daemon creates the
    /// worktree.
    pub fn latest_drive(&self, target: &str) -> Option<&DriveState> {
        self.recent_drives.get(target).and_then(|d| d.front())
    }
}

/// The single source of truth for device capability gating. Controls are
/// rendered for the canonical capability set; a control is ready when the
/// agent advertises the capability and the grant record (registration
/// grants, minus observed `not_granted` refusals, plus observed successes)
/// allows it. Missing capability and missing grant are reported separately.
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
            schema_version: 5,
            rev: 10,
            generated_at: 0,
            agents: BTreeMap::new(),
            fleet_health: Vec::new(),
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
            schema_version: 5,
            rev: 20,
            generated_at: 0,
            agents: BTreeMap::new(),
            fleet_health: Vec::new(),
        };
        snap.agents.insert("a".into(), agent("a"));
        fleet.apply_snapshot(&snap);
        fleet.apply_delta(&Delta {
            rev: 19,
            upd: vec![agent("c")],
            del: vec![],
        });
        assert_eq!(fleet.rev, Some(20), "rev is monotonic");
        assert!(!fleet.agents.contains_key("c"), "late payload is ignored");
    }

    #[test]
    fn stale_snapshot_cannot_overwrite_newer_sse_state() {
        let mut fleet = Fleet::default();
        let mut current = agent("a");
        current.title = Some("newer SSE".into());
        let mut current_snapshot = crate::model::Snapshot {
            schema_version: 3,
            rev: 20,
            generated_at: 0,
            agents: BTreeMap::new(),
            fleet_health: Vec::new(),
        };
        current_snapshot.agents.insert("a".into(), agent("a"));
        fleet.apply_snapshot(&current_snapshot);
        fleet.apply_delta(&Delta {
            rev: 21,
            upd: vec![current.clone()],
            del: vec![],
        });

        let mut stale_fetch = current_snapshot;
        stale_fetch.rev = 20;
        stale_fetch.agents.insert("a".into(), agent("old fetch"));
        fleet.apply_snapshot(&stale_fetch);
        fleet.apply_delta(&Delta {
            rev: 20,
            upd: vec![agent("late")],
            del: vec![],
        });

        assert_eq!(fleet.rev, Some(21));
        assert_eq!(fleet.agents["a"].title.as_deref(), Some("newer SSE"));
        assert!(!fleet.agents.contains_key("late"));
    }

    #[test]
    fn fleets_fetch_marks_loaded_only_on_success_and_clears_loading() {
        let mut fleet = Fleet {
            fleets_loading: true,
            ..Default::default()
        };
        fleet.set_fleets(Err("connect refused".into()));
        assert!(!fleet.fleets_loading);
        assert!(
            !fleet.fleets_loaded,
            "a failed fetch must not mark the catalog loaded"
        );
        assert!(matches!(fleet.fleets, Some(Err(_))));

        let identities: FleetIdentities = serde_json::from_value(serde_json::json!({
            "status": "ok",
            "error": null,
            "fleets": [{"name": "corral", "gh_repo": "jirathip-dev/corral", "orch": "orch-corral", "workers": 0, "paused": false}]
        }))
        .unwrap();
        fleet.set_fleets(Ok(identities));
        assert!(fleet.fleets_loaded);
        assert!(!fleet.fleets_loading);
        assert!(fleet.fleets_ready());

        fleet.set_fleets(Ok(FleetIdentities {
            status: "error".into(),
            error: Some("no herdr-fleet".into()),
            fleets: Vec::new(),
        }));
        assert!(fleet.fleets_loaded);
        assert!(!fleet.fleets_ready());
    }

    fn issue(repo: &str, number: u64, title: &str) -> GhIssueRef {
        GhIssueRef {
            repo: repo.to_string(),
            number,
            state: "OPEN".to_string(),
            title: title.to_string(),
            labels: Vec::new(),
            url: format!("https://github.com/example/{repo}/issues/{number}"),
        }
    }

    #[test]
    fn issues_apply_keeps_the_full_grouped_snapshot_and_retry_error() {
        let mut fleet = Fleet {
            issues_loading: true,
            ..Default::default()
        };
        let issues = BTreeMap::from([
            (
                "corral".into(),
                vec![
                    issue("corral", 207, "fetch path"),
                    issue("corral", 208, "other"),
                ],
            ),
            ("fleet-ops".into(), vec![issue("fleet-ops", 15, "ops")]),
        ]);

        // This is the ApplyMsg::Issues consumer contract: no repo or issue
        // may be dropped while the result crosses into UI-owned state.
        fleet.set_issues(Ok(issues.clone()));
        assert!(fleet.issues_loaded);
        assert!(!fleet.issues_loading);
        assert_eq!(fleet.issues, issues);
        assert_eq!(fleet.issues.values().map(Vec::len).sum::<usize>(), 3);
        assert!(fleet.issues_error.is_none());

        // A later transport failure must not replace a known-good full view
        // with the misleading "no repo-level issues fetched" state.
        fleet.issues_loading = true;
        fleet.set_issues(Err("GET /issues unavailable".into()));
        assert!(!fleet.issues_loading);
        assert_eq!(fleet.issues, issues);
        assert_eq!(
            fleet.issues_error.as_deref(),
            Some("GET /issues unavailable")
        );
    }

    #[test]
    fn recent_output_hydration_is_one_shot_until_a_payload_or_result_arrives() {
        let mut fleet = Fleet::default();
        assert!(fleet.needs_recent_output("a"));

        fleet.remember_drive(
            "a",
            DriveState::Sending {
                request_id: "r1".into(),
                capability: "read_tail".into(),
            },
        );
        assert!(
            !fleet.needs_recent_output("a"),
            "in-flight fetch is not duplicated"
        );

        fleet.recent_drives.clear();
        fleet.remember_tail("a", Vec::new());
        assert!(
            !fleet.needs_recent_output("a"),
            "an empty daemon result is still loaded"
        );
    }

    #[test]
    fn deletion_cleans_derived_state() {
        let mut fleet = Fleet::default();
        let mut snap = crate::model::Snapshot {
            schema_version: 5,
            rev: 1,
            generated_at: 0,
            agents: BTreeMap::new(),
            fleet_health: Vec::new(),
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

    /// #232 test helper: build a daemon-shaped DiffPage.
    fn page(
        offset: u32,
        lines: Vec<&str>,
        total: u32,
        has_more: bool,
        next: Option<u32>,
    ) -> crate::drive::DiffPage {
        crate::drive::DiffPage {
            repo: Some("corral".into()),
            branch: Some("g232/read-diff".into()),
            head: Some("abc".into()),
            stats: crate::drive::DiffStats {
                files: 2,
                adds: 4,
                dels: 1,
            },
            files: vec![crate::drive::DiffFileStat {
                path: "src/core/diff.rs".into(),
                adds: 4,
                dels: 1,
            }],
            files_truncated: false,
            offset,
            lines: lines.into_iter().map(str::to_string).collect(),
            total,
            has_more,
            next_offset: next,
        }
    }

    #[test]
    fn diff_pages_append_and_remove_agent_discards_cache() {
        let mut fleet = Fleet::default();

        // First page (offset 0) seeds the cache with metadata + lines.
        fleet.remember_diff_page("a", page(0, vec!["+one", "+two"], 6, true, Some(2)));
        let cache = fleet.diffs.get("a").expect("cached");
        assert_eq!(cache.lines, vec!["+one", "+two"]);
        assert_eq!(cache.total, 6);
        assert!(cache.has_more);
        assert_eq!(cache.next_offset, Some(2));
        assert_eq!(cache.stats.adds, 4);

        // Next page (offset 2) appends; the row diffstat reads real numbers.
        fleet.remember_diff_page("a", page(2, vec!["+three", "-four"], 6, false, None));
        let cache = fleet.diffs.get("a").expect("cached");
        assert_eq!(cache.lines, vec!["+one", "+two", "+three", "-four"]);
        assert!(!cache.has_more);
        assert_eq!(cache.next_offset, None);

        // A re-fetch of the same window is idempotent (no duplicates).
        fleet.remember_diff_page("a", page(0, vec!["+one", "+two"], 6, true, Some(2)));
        assert_eq!(
            fleet.diffs["a"].lines,
            vec!["+one", "+two", "+three", "-four"]
        );

        // Cache dies with the agent (stale-drive removal path).
        fleet.remove_agent("a");
        assert!(!fleet.diffs.contains_key("a"));
    }
}
