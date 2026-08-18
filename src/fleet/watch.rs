//! #35 watchdog: `corrald fleet watch` — one health pass over the
//! registry's fleets, cron-able like `corrald digest`, READ-ONLY end to
//! end (never writes the registry, never prompts or kills an agent).
//!
//! The rules mirror the legacy `fleet-watch.py` verbatim, per unpaused
//! fleet (paused fleets are skipped ENTIRELY — orchestrator and worker
//! checks both, so pausing genuinely silences the watchdog for a fleet):
//!
//! 1. herdr server unreachable — the agent listing failed or came back
//!    empty after one retry. Reported once; every per-fleet agent check is
//!    then suppressed (they would all false-alarm as MISSING).
//! 2. orchestrator MISSING — the fleet's `orch` name is not in the listing.
//! 3. orchestrator STALLED — status != "working", three flavors in
//!    priority order: with open PRs; while fleet workers are working
//!    ("wave spawned, not collecting"); plain.
//! 4. worker MISSING — a `workers[]` name absent from the listing.
//!
//! The decision layer below is PURE — plain inputs in, sorted problem
//! lines out — so every rule is unit-tested without herdr, gh, or a
//! network. The CLI (`run_fleet_watch` in main.rs) wires the shell-outs.

use std::collections::BTreeMap;

use crate::fleet::config::Registry;

/// What the watchdog needs to know about one live agent.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentInfo {
    pub status: String,
    pub cwd: String,
}

/// The herdr agent listing: `None` = the server did not answer (after the
/// caller's retry); `Some(map)` = name → info.
pub type AgentsView = Option<BTreeMap<String, AgentInfo>>;

/// Open-PR count per `gh_repo`: `None` = the gh check was unavailable
/// (network/auth failure) — reported as such, never silently treated as
/// zero in a way that changes the verdict wording.
pub type PrCounts = BTreeMap<String, Option<u64>>;

/// The pure decision layer: every problem line for this registry given
/// this world-view, SORTED (stable, diffable output — the legacy watcher
/// sorts too).
pub fn problems(registry: &Registry, agents: &AgentsView, prs: &PrCounts) -> Vec<String> {
    let mut out = Vec::new();
    let Some(agents) = agents else {
        // Server down: one line, and no per-fleet spam — every agent
        // check below would false-alarm.
        return vec![
            "herdr server NOT reachable (agent list failed/empty after retry)".to_string(),
        ];
    };

    for fleet in &registry.fleets {
        if fleet.paused {
            continue;
        }
        let local = fleet.local_path();
        let local_str = local.to_string_lossy();
        match agents.get(&fleet.orch) {
            None => out.push(format!(
                "{}: orchestrator {} MISSING",
                fleet.name, fleet.orch
            )),
            Some(orch) if orch.status != "working" => {
                let workers_working = agents.iter().any(|(name, a)| {
                    a.status == "working"
                        && (fleet.workers.iter().any(|w| w == name)
                            || cwd_in_fleet(&a.cwd, &local_str, &fleet.worktree_dir))
                });
                match prs.get(&fleet.gh_repo) {
                    Some(Some(n)) if *n > 0 => out.push(format!(
                        "{}: {} STALLED with {n} open PR(s)",
                        fleet.name, fleet.orch
                    )),
                    _ if workers_working => out.push(format!(
                        "{}: {} STALLED while worker(s) working — wave spawned, not collecting",
                        fleet.name, fleet.orch
                    )),
                    Some(None) => out.push(format!(
                        "{}: {} STALLED (status={}; open-PR check unavailable)",
                        fleet.name, fleet.orch, orch.status
                    )),
                    _ => out.push(format!(
                        "{}: {} STALLED (status={})",
                        fleet.name, fleet.orch, orch.status
                    )),
                }
            }
            Some(_) => {}
        }
        for worker in &fleet.workers {
            if !agents.contains_key(worker) {
                out.push(format!(
                    "{}: worker {worker} MISSING (server restart?)",
                    fleet.name
                ));
            }
        }
    }
    out.sort();
    out
}

/// Component-exact path membership: does `cwd` sit inside the fleet's
/// local checkout or its herdr worktree directory? Component-exact on
/// purpose — a prefix match would let `~/w/corral-x` count as inside
/// `~/w/corral` and another fleet's workers false-trigger the stall
/// heuristics (the legacy fleet_matcher's S5 rule).
pub fn cwd_in_fleet(cwd: &str, local: &str, worktree_dir: &str) -> bool {
    if cwd.is_empty() {
        return false;
    }
    if !local.is_empty() && (cwd == local || cwd.starts_with(&format!("{local}/"))) {
        return true;
    }
    if worktree_dir.is_empty() {
        return false;
    }
    let marker = format!("/.herdr/worktrees/{worktree_dir}");
    match cwd.find(&marker) {
        Some(idx) => {
            let after = &cwd[idx + marker.len()..];
            after.is_empty() || after.starts_with('/')
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::config::load;

    fn registry(paused_corral: bool) -> Registry {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("fleets.json");
        let paused = if paused_corral {
            r#""paused": true,"#
        } else {
            ""
        };
        std::fs::write(
            &path,
            format!(
                r#"{{ "fleets": [
                    {{ "name": "corral", "gh_repo": "o/corral", "local": "/repos/corral",
                      "worktree_dir": "corral", "orch": "orch-corral",
                      "workers": ["w1", "w2"], {paused}
                      "models": {{ "orch": "f", "impl": "i", "review": "r" }} }},
                    {{ "name": "board", "gh_repo": "o/board", "local": "/repos/board",
                      "worktree_dir": "board", "orch": "orch-board", "workers": [],
                      "models": {{ "orch": "f", "impl": "i", "review": "r" }} }}
                ] }}"#
            ),
        )
        .expect("fixture");
        load(&path).expect("loads")
    }

    fn agent(status: &str, cwd: &str) -> AgentInfo {
        AgentInfo {
            status: status.to_string(),
            cwd: cwd.to_string(),
        }
    }

    fn healthy_agents() -> BTreeMap<String, AgentInfo> {
        BTreeMap::from([
            ("orch-corral".to_string(), agent("working", "/repos/corral")),
            ("orch-board".to_string(), agent("working", "/repos/board")),
            ("w1".to_string(), agent("idle", "/repos/corral")),
            ("w2".to_string(), agent("done", "/repos/corral")),
        ])
    }

    fn no_prs() -> PrCounts {
        BTreeMap::from([
            ("o/corral".to_string(), Some(0)),
            ("o/board".to_string(), Some(0)),
        ])
    }

    #[test]
    fn healthy_fleet_reports_nothing() {
        let out = problems(&registry(false), &Some(healthy_agents()), &no_prs());
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn server_down_is_one_line_with_no_per_fleet_spam() {
        let out = problems(&registry(false), &None, &no_prs());
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("herdr server NOT reachable"));
    }

    #[test]
    fn paused_fleet_is_skipped_entirely_orch_and_workers() {
        // corral paused: its missing orch AND missing workers are silent;
        // board still checked.
        let mut agents = BTreeMap::new();
        agents.insert("orch-board".to_string(), agent("working", "/repos/board"));
        let out = problems(&registry(true), &Some(agents), &no_prs());
        assert!(out.is_empty(), "paused fleet fully silenced: {out:?}");
    }

    #[test]
    fn missing_orchestrator_and_workers_are_reported() {
        let mut agents = healthy_agents();
        agents.remove("orch-corral");
        agents.remove("w2");
        let out = problems(&registry(false), &Some(agents), &no_prs());
        assert_eq!(
            out,
            vec![
                "corral: orchestrator orch-corral MISSING".to_string(),
                "corral: worker w2 MISSING (server restart?)".to_string(),
            ]
        );
    }

    #[test]
    fn stalled_flavors_have_priority_order() {
        let mut agents = healthy_agents();
        agents.insert("orch-corral".to_string(), agent("idle", "/repos/corral"));

        // 1) Open PRs win over everything.
        let prs = BTreeMap::from([
            ("o/corral".to_string(), Some(3)),
            ("o/board".to_string(), Some(0)),
        ]);
        let out = problems(&registry(false), &Some(agents.clone()), &prs);
        assert_eq!(
            out,
            vec!["corral: orch-corral STALLED with 3 open PR(s)".to_string()]
        );

        // 2) Workers working (by name) comes next.
        agents.insert("w1".to_string(), agent("working", "/elsewhere"));
        let out = problems(&registry(false), &Some(agents.clone()), &no_prs());
        assert_eq!(
            out,
            vec!["corral: orch-corral STALLED while worker(s) working — wave spawned, not collecting".to_string()]
        );

        // 3) Plain stall with the status named.
        agents.insert("w1".to_string(), agent("idle", "/repos/corral"));
        let out = problems(&registry(false), &Some(agents.clone()), &no_prs());
        assert_eq!(
            out,
            vec!["corral: orch-corral STALLED (status=idle)".to_string()]
        );

        // 3b) Unavailable PR check is stated, not hidden.
        let prs_unavailable = BTreeMap::from([
            ("o/corral".to_string(), None),
            ("o/board".to_string(), Some(0)),
        ]);
        let out = problems(&registry(false), &Some(agents), &prs_unavailable);
        assert_eq!(
            out,
            vec![
                "corral: orch-corral STALLED (status=idle; open-PR check unavailable)".to_string()
            ]
        );
    }

    #[test]
    fn dynamic_workers_match_by_cwd_component_exact() {
        let mut agents = healthy_agents();
        agents.insert("orch-corral".to_string(), agent("idle", "/repos/corral"));
        // A dynamically-named batch worker inside the fleet's worktree dir.
        agents.insert(
            "g99-1".to_string(),
            agent("working", "/Users/x/.herdr/worktrees/corral/g99-fix"),
        );
        let out = problems(&registry(false), &Some(agents.clone()), &no_prs());
        assert!(
            out[0].contains("wave spawned"),
            "cwd-matched worker counts: {out:?}"
        );

        // Component-exactness: corral-x is NOT corral.
        agents.insert(
            "g99-1".to_string(),
            agent("working", "/Users/x/.herdr/worktrees/corral-x/g99-fix"),
        );
        let out = problems(&registry(false), &Some(agents), &no_prs());
        assert_eq!(
            out,
            vec!["corral: orch-corral STALLED (status=idle)".to_string()],
            "another fleet's worker must not count"
        );
    }

    #[test]
    fn cwd_matcher_edges() {
        assert!(cwd_in_fleet("/repos/corral", "/repos/corral", "corral"));
        assert!(cwd_in_fleet("/repos/corral/sub", "/repos/corral", "corral"));
        assert!(!cwd_in_fleet("/repos/corral-x", "/repos/corral", "corral"));
        assert!(cwd_in_fleet("/u/.herdr/worktrees/corral", "", "corral"));
        assert!(cwd_in_fleet("/u/.herdr/worktrees/corral/wt1", "", "corral"));
        assert!(!cwd_in_fleet(
            "/u/.herdr/worktrees/corral-x/wt1",
            "",
            "corral"
        ));
        assert!(!cwd_in_fleet("", "/repos/corral", "corral"));
        assert!(!cwd_in_fleet("/anything", "", ""));
    }

    #[test]
    fn output_is_sorted_and_stable() {
        let out = problems(&registry(false), &Some(BTreeMap::new()), &no_prs());
        let mut sorted = out.clone();
        sorted.sort();
        assert_eq!(out, sorted);
        assert_eq!(
            out.len(),
            4,
            "both orchs + both corral workers missing: {out:?}"
        );
    }
}
