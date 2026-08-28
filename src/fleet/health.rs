//! #210: read-only per-fleet health aggregation (HEALTH ONLY — no spend).
//!
//! The status strip shows, per fleet from the fleet-ops CLI validated
//! identity catalog: is the orch agent alive?, live worker count, and the
//! last heartbeat age. It is a pure aggregation of signals the daemon
//! already has:
//!
//! - fleet identities (name, gh_repo, orch, declared workers, paused) from
//!   the fleet-ops CLI (`herdr-fleet list`) — see [`crate::fleet::cli`];
//! - the live agent set (corrald's canonical records built from herdr's
//!   `agent.list` + pane events);
//! - the per-agent presence heartbeat ([`Adapter::last_seen_millis`]) —
//!   when corrald last observed the agent in herdr's trusted catalog.
//!
//! Nothing here reads wallet/provider state, no new backend state is
//! invented, and nothing is written anywhere: the aggregation is computed
//! on demand at snapshot-assembly time and carried on the snapshot
//! (`Snapshot.fleet_health`) with an epoch-millis anchor so clients render
//! a ticking age locally.
//!
//! Membership rule: an agent belongs to a fleet when its `workspace.repo`
//! equals the fleet's orch agent's repo group (the strongest signal, exact
//! even when the fleet's worktree/checkout directory differs from its
//! name, e.g. `synergy` -> `synergy-costing`). When the orch is absent,
//! the fallback repo set is the fleet name and the `gh_repo` basename —
//! the only repo spellings the CLI identity carries; a fleet whose
//! orchestrator is missing falls back conservatively (live workers may
//! count as zero until the orchestrator returns), which is the honest
//! direction for a health signal.
//!
//! Warning semantics (reads as HEALTH, never a stall accusation):
//! - `orch_missing`      — no live agent carries the registered orch name;
//! - `heartbeat_stale`   — the orch agent exists but has not been observed
//!   in the trusted herdr catalog for [`STALE_HEARTBEAT_SECS`] (herdr is
//!   down, the pane/process is gone, or the lane is truly dead);
//! - `workers_missing`   — the registry declares workers but none are live.
//!
//! Paused fleets are never degraded: they are intentionally parked.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::core::model::{Agent, AgentState};

use super::cli::FleetIdentity;

/// Presence-heartbeat deadline. Herdr's trusted catalog refresh runs every
/// two seconds in production, so an agent not observed for this long is
/// really absent (herdr reconnect backoff tops out at 30 s and would
/// otherwise re-stamp every agent during a restart; 60 s stays far below
/// the "suspicious lane" timescales fleet tooling uses).
pub const STALE_HEARTBEAT_SECS: u64 = 60;

/// One fleet's health row, as carried on the snapshot and rendered by the
/// board + iOS strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetHealthEntry {
    /// Fleet-ops validated fleet name.
    pub name: String,
    pub gh_repo: String,
    /// Fleet is paused in the registry (intentionally parked).
    pub paused: bool,
    /// Registered orchestrator agent name.
    pub orch: String,
    /// True when the orch agent is present in the live herdr catalog.
    pub orch_alive: bool,
    /// Orch agent state when alive (`idle`/`working`/`blocked`/`done`/
    /// `unknown`); `None` when the orch is absent.
    pub orch_state: Option<String>,
    /// Live worker count (agents in the fleet's repo group, excluding the
    /// orch agent itself).
    pub workers: usize,
    /// Epoch millis of the last observation of the orch agent (the
    /// presence heartbeat anchor; clients render `now - last_heartbeat`).
    /// `None` when the orch has never been observed.
    pub last_heartbeat: Option<u64>,
    /// True when the fleet is degraded/stale; always false for paused
    /// fleets. Reads as HEALTH, never as a stall accusation.
    pub degraded: bool,
    /// Machine-readable warning tokens: `orch_missing`, `heartbeat_stale`,
    /// `workers_missing`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl FleetHealthEntry {
    /// True when the entry carries the given warning token.
    pub fn warned(&self, token: &str) -> bool {
        self.warnings.iter().any(|w| w == token)
    }
}

/// The fleet repo group of an agent (its attributed `workspace.repo`).
fn agent_repo(agent: &Agent) -> Option<&str> {
    agent
        .workspace
        .repo
        .as_deref()
        .map(str::trim)
        .filter(|repo| !repo.is_empty())
}

/// Fallback repo spellings when the orch agent is absent (the CLI identity
/// carries the fleet name and the `gh_repo` basename only).
fn fallback_repos(fleet: &FleetIdentity) -> Vec<String> {
    let mut repos = vec![fleet.name.clone()];
    if let Some((_, basename)) = fleet.gh_repo.rsplit_once('/') {
        let basename = basename.trim();
        if !basename.is_empty() && basename != fleet.name {
            repos.push(basename.to_string());
        }
    }
    repos
}

/// Computed per-fleet health. Pure: no I/O, no mutation.
///
/// `now_millis` is the wall clock at assembly time; `last_seen` maps
/// canonical agent ids to the last time the adapter observed them (the
/// heartbeat map, empty for adapters that do not track presence).
pub fn aggregate(
    identities: &[FleetIdentity],
    agents: &BTreeMap<String, Agent>,
    last_seen: &HashMap<String, u64>,
    now_millis: u64,
) -> Vec<FleetHealthEntry> {
    let mut entries = Vec::with_capacity(identities.len());
    for fleet in identities {
        entries.push(aggregate_one(fleet, agents, last_seen, now_millis));
    }
    entries
}

fn aggregate_one(
    fleet: &FleetIdentity,
    agents: &BTreeMap<String, Agent>,
    last_seen: &HashMap<String, u64>,
    now_millis: u64,
) -> FleetHealthEntry {
    let orch_agent = agents
        .values()
        .find(|agent| agent.display_name.as_deref() == Some(fleet.orch.as_str()));

    // Live workers: every agent in the fleet's repo group, excluding the
    // orch agent itself. With a live orch the group is the orch's exact
    // repo; otherwise fall back to the spellings the CLI identity carries.
    let group: Vec<String> = match orch_agent {
        Some(orch) => agent_repo(orch)
            .map(|repo| vec![repo.to_string()])
            .unwrap_or_default(),
        None => fallback_repos(fleet),
    };
    let workers = agents
        .values()
        .filter(|agent| {
            let Some(repo) = agent_repo(agent) else {
                return false;
            };
            if group.iter().any(|candidate| candidate == repo) {
                if let Some(orch) = orch_agent
                    && orch.agent_id == agent.agent_id
                {
                    return false;
                }
                return true;
            }
            false
        })
        .count();

    let mut warnings = Vec::new();
    let mut last_heartbeat = None;
    let mut orch_state = None;
    let mut orch_alive = false;
    if let Some(orch) = orch_agent {
        orch_alive = true;
        orch_state = Some(state_token(orch.state));
        let seen = last_seen.get(&orch.agent_id).copied();
        match seen {
            Some(seen_at) => {
                last_heartbeat = Some(seen_at);
                if now_millis.saturating_sub(seen_at) > STALE_HEARTBEAT_SECS * 1000 {
                    warnings.push("heartbeat_stale".to_string());
                }
            }
            // The orch is tracked but has no observation record (adapter
            // without presence tracking, or the record predates the map):
            // refuse to guess a fresh heartbeat.
            None => warnings.push("heartbeat_stale".to_string()),
        }
    } else {
        warnings.push("orch_missing".to_string());
    }
    if fleet.workers > 0 && workers == 0 {
        warnings.push("workers_missing".to_string());
    }
    let mut degraded = !warnings.is_empty();
    if fleet.paused {
        // Parked fleets are not degraded: they are intentionally stopped.
        // The strip reads "paused" off the registry value.
        degraded = false;
        warnings.clear();
    }

    FleetHealthEntry {
        name: fleet.name.clone(),
        gh_repo: fleet.gh_repo.clone(),
        paused: fleet.paused,
        orch: fleet.orch.clone(),
        orch_alive,
        orch_state,
        workers,
        last_heartbeat,
        degraded,
        warnings,
    }
}

fn state_token(state: AgentState) -> String {
    match state {
        AgentState::Idle => "idle".to_string(),
        AgentState::Working => "working".to_string(),
        AgentState::Blocked => "blocked".to_string(),
        AgentState::Done => "done".to_string(),
        AgentState::Unknown => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{AgentState, Attachment, Workspace};

    fn agent(id: &str, name: Option<&str>, state: AgentState, repo: Option<&str>) -> Agent {
        Agent {
            agent_id: id.to_string(),
            source: "herdr".to_string(),
            tool: "hermes".to_string(),
            state,
            reason: None,
            seq: 1,
            ts: 1,
            capabilities: Vec::new(),
            waiting_on: None,
            parent_id: None,
            host: None,
            workspace: Workspace {
                repo: repo.map(str::to_string),
                ..Default::default()
            },
            attachment: Some(Attachment {
                kind: "herdr-pane".to_string(),
                reference: "w1:p1".to_string(),
            }),
            display_name: name.map(str::to_string),
            title: None,
        }
    }

    fn fleet(name: &str, gh_repo: &str, orch: &str, workers: usize, paused: bool) -> FleetIdentity {
        FleetIdentity {
            name: name.to_string(),
            gh_repo: gh_repo.to_string(),
            local: std::path::PathBuf::from(format!("~/Projects/{name}")),
            worktree_dir: name.to_string(),
            orch: orch.to_string(),
            workers,
            paused,
        }
    }

    #[test]
    fn live_fleet_reports_orch_workers_and_fresh_heartbeat() {
        let fleets = vec![fleet(
            "corral",
            "jirathip-dev/corral",
            "orch-corral",
            0,
            false,
        )];
        let agents = BTreeMap::from([
            (
                "a1".to_string(),
                agent(
                    "a1",
                    Some("orch-corral"),
                    AgentState::Working,
                    Some("corral"),
                ),
            ),
            (
                "a2".to_string(),
                agent("a2", Some("impl-x"), AgentState::Working, Some("corral")),
            ),
            (
                "a3".to_string(),
                agent("a3", Some("impl-y"), AgentState::Idle, Some("corral")),
            ),
            (
                "other".to_string(),
                agent("other", Some("impl-z"), AgentState::Working, Some("unity")),
            ),
        ]);
        let mut seen = HashMap::new();
        seen.insert("a1".to_string(), 100_000);
        let entries = aggregate(&fleets, &agents, &seen, 104_000);
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert!(entry.orch_alive, "orch present");
        assert_eq!(entry.orch_state.as_deref(), Some("working"));
        assert_eq!(entry.workers, 2, "repo-group workers only");
        assert_eq!(entry.last_heartbeat, Some(100_000));
        assert!(!entry.degraded);
        assert!(entry.warnings.is_empty());
    }

    #[test]
    fn missing_orch_is_degraded_with_warfing_and_no_heartbeat() {
        let fleets = vec![fleet(
            "corral",
            "jirathip-dev/corral",
            "orch-corral",
            0,
            false,
        )];
        let agents = BTreeMap::new();
        let entries = aggregate(&fleets, &agents, &HashMap::new(), 104_000);
        let entry = &entries[0];
        assert!(!entry.orch_alive);
        assert_eq!(entry.orch_state, None);
        assert_eq!(entry.workers, 0);
        assert_eq!(entry.last_heartbeat, None);
        assert!(entry.degraded);
        assert!(entry.warned("orch_missing"));
    }

    #[test]
    fn stale_heartbeat_warns_but_orch_still_counts() {
        let fleets = vec![fleet(
            "corral",
            "jirathip-dev/corral",
            "orch-corral",
            0,
            false,
        )];
        let agents = BTreeMap::from([(
            "a1".to_string(),
            agent(
                "a1",
                Some("orch-corral"),
                AgentState::Working,
                Some("corral"),
            ),
        )]);
        let mut seen = HashMap::new();
        seen.insert("a1".to_string(), 1_000);
        let entries = aggregate(
            &fleets,
            &agents,
            &seen,
            1_000 + (STALE_HEARTBEAT_SECS * 1000) + 1,
        );
        let entry = &entries[0];
        assert!(entry.orch_alive);
        assert_eq!(entry.workers, 0);
        assert!(entry.degraded);
        assert!(entry.warned("heartbeat_stale"));
    }

    #[test]
    fn declared_workers_with_none_live_warns_workers_missing() {
        let fleets = vec![fleet(
            "corral",
            "jirathip-dev/corral",
            "orch-corral",
            2,
            false,
        )];
        let agents = BTreeMap::from([(
            "a1".to_string(),
            agent(
                "a1",
                Some("orch-corral"),
                AgentState::Working,
                Some("corral"),
            ),
        )]);
        let mut seen = HashMap::new();
        seen.insert("a1".to_string(), 100_000);
        let entries = aggregate(&fleets, &agents, &seen, 104_000);
        let entry = &entries[0];
        assert_eq!(entry.workers, 0);
        assert!(entry.warned("workers_missing"), "declared 2, live 0");
    }

    #[test]
    fn a_live_orch_uses_the_orch_repo_group_even_when_names_diverge() {
        // synergy fleet: fleet name and gh_repo basename differ from the
        // physical checkout/worktree dir (synergy-costing). With a live
        // orch the group is the orch's own repo, so workers match exactly.
        let fleets = vec![fleet(
            "synergy",
            "synergy-services-cooling-tower/synergy-apps",
            "orch-synergy",
            0,
            false,
        )];
        let agents = BTreeMap::from([
            (
                "s1".to_string(),
                agent(
                    "s1",
                    Some("orch-synergy"),
                    AgentState::Working,
                    Some("synergy-costing"),
                ),
            ),
            (
                "s2".to_string(),
                agent(
                    "s2",
                    Some("impl-266-luna"),
                    AgentState::Working,
                    Some("synergy-costing"),
                ),
            ),
        ]);
        let mut seen = HashMap::new();
        seen.insert("s1".to_string(), 100_000);
        let entries = aggregate(&fleets, &agents, &seen, 104_000);
        let entry = &entries[0];
        assert!(entry.orch_alive);
        assert_eq!(entry.workers, 1);
        assert!(!entry.degraded);
    }

    #[test]
    fn pause_suppresses_warnings() {
        let fleets = vec![fleet(
            "plush",
            "jirathip-dev/plush-meadow",
            "orch-plush",
            1,
            true,
        )];
        let agents = BTreeMap::new();
        let entries = aggregate(&fleets, &agents, &HashMap::new(), 104_000);
        let entry = &entries[0];
        assert!(entry.paused);
        assert!(!entry.orch_alive);
        assert!(!entry.degraded, "paused fleets never read as degraded");
        assert!(entry.warnings.is_empty());
    }
}
