//! #35 watchdog: `corrald fleet watch` — one health pass over the
//! registry's fleets, cron-able like `corrald digest`, READ-ONLY end to
//! end (never writes the registry, never prompts or kills an agent).
//!
//! The rules mirror the legacy `fleet-watch.py`, with the deviations
//! enumerated in docs/corral/G35-registry.md, per unpaused fleet (paused
//! fleets are skipped ENTIRELY — orchestrator and worker checks both, so
//! pausing genuinely silences the watchdog for a fleet):
//!
//! 1. herdr server unreachable — the agent listing CALL failed or was
//!    unparseable after one retry. A successful listing with ZERO agents
//!    is a healthy answer, never server-down. Reported once; every
//!    per-fleet agent check is then suppressed (they would all
//!    false-alarm as MISSING).
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

/// The herdr agent listing: `None` = the CALL failed or was unparseable
/// (after the caller's retry) — genuine unreachability. `Some(map)` = the
/// server answered; an EMPTY map is a healthy answer (the steady state
/// when every fleet is parked), never a server-down signal (review F3).
pub type AgentsView = Option<BTreeMap<String, AgentInfo>>;

/// Open-PR count per `gh_repo`: `None` = the gh check was unavailable
/// (network/auth failure) — reported as such, never silently treated as
/// zero in a way that changes the verdict wording.
pub type PrCounts = BTreeMap<String, Option<u64>>;

/// The pure decision layer: every problem line for this registry given
/// this world-view, SORTED (stable, diffable output — the legacy watcher
/// sorts too).
pub fn problems(
    registry: &Registry,
    agents: &AgentsView,
    prs: &PrCounts,
    home: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    let Some(agents) = agents else {
        // The CALL failed (after retry): one line, and no per-fleet spam —
        // every agent check below would false-alarm. An EMPTY listing does
        // NOT land here (review F3): that is a healthy answer, and the
        // per-fleet checks below then report the truthful, actionable
        // MISSING lines (or nothing at all when every fleet is paused).
        return vec!["herdr server NOT reachable (agent list call failed after retry)".to_string()];
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
                            || cwd_in_fleet(&a.cwd, &local_str, &fleet.worktree_dir, home))
                });
                // The PR count being UNAVAILABLE is stated on every stalled
                // flavor (review F5): a gh failure must never silently
                // relabel a with-open-PRs stall as something else — which
                // is the exact legacy defect this deviation exists to fix,
                // and it is likeliest during a stalled wave.
                let pr_note = if matches!(prs.get(&fleet.gh_repo), Some(None) | None) {
                    "; open-PR check unavailable"
                } else {
                    ""
                };
                match prs.get(&fleet.gh_repo) {
                    Some(Some(n)) if *n > 0 => out.push(format!(
                        "{}: {} STALLED with {n} open PR(s)",
                        fleet.name, fleet.orch
                    )),
                    _ if workers_working => out.push(format!(
                        "{}: {} STALLED while worker(s) working — wave spawned, not collecting{pr_note}",
                        fleet.name, fleet.orch
                    )),
                    _ => out.push(format!(
                        "{}: {} STALLED (status={}{pr_note})",
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
/// local checkout or its herdr worktree directory? Ports all three legacy
/// fleet_matcher rules:
/// - S5: component-exact — a prefix match would let `~/w/corral-x` count
///   as inside `~/w/corral`;
/// - M1/S4: the worktree leg is ANCHORED at `<home>/.herdr/worktrees/` —
///   a nested or foreign `.herdr/worktrees` anywhere else on disk never
///   matches (review F6). The legacy orca-era `~/orca/workspaces` leg is
///   deliberately dropped (orca is retired — declared deviation);
/// - M2: a `local` shallower than two components below `<home>` is
///   ignored — `~/Projects` or `<home>` itself must not authorize every
///   main-checkout cwd on the machine (review F7). Trailing slashes are
///   normalized first (review F10).
pub fn cwd_in_fleet(cwd: &str, local: &str, worktree_dir: &str, home: &str) -> bool {
    if cwd.is_empty() {
        return false;
    }
    let local = local.trim_end_matches('/');
    if local_authorized(local, home) && (cwd == local || cwd.starts_with(&format!("{local}/"))) {
        return true;
    }
    if worktree_dir.is_empty() || home.is_empty() {
        return false;
    }
    // Fresh-review N5: worktree_dir gets the same trailing-slash
    // normalisation F10 gave `local` — a registry value of "corral/"
    // must not silently disable the whole worktree leg.
    let worktree_dir = worktree_dir.trim_matches('/');
    let root = format!(
        "{}/.herdr/worktrees/{worktree_dir}",
        home.trim_end_matches('/')
    );
    cwd == root || cwd.starts_with(&format!("{root}/"))
}

/// Legacy `local_ok` (M2): a usable `local` sits at least two path
/// components below `home`. Anything shallower — `/`, home itself,
/// `~/Projects` — would let one fleet claim every checkout on the box.
fn local_authorized(local: &str, home: &str) -> bool {
    if local.is_empty() {
        return false;
    }
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        // No home to judge depth against: require an absolute path with
        // at least three components as a conservative stand-in.
        return local.split('/').filter(|c| !c.is_empty()).count() >= 3;
    }
    let Some(rest) = local.strip_prefix(&format!("{home}/")) else {
        // A local OUTSIDE home (e.g. /opt/board). Legacy rejects these
        // outright (relpath from home starts with ".."); accepting them is
        // a declared deviation (G35-registry.md) — but the SAME two-plus
        // component depth rule applies, so a misconfigured "/Users" or
        // "/opt" cannot authorize every cwd on the box (review R3).
        return local != home && local.split('/').filter(|c| !c.is_empty()).count() >= 2;
    };
    rest.split('/').filter(|c| !c.is_empty()).count() >= 2
}

/// Parse `herdr agent list` JSON stdout into the watchdog's view. The
/// field names this parser depends on — `result.agents[].name`,
/// `.agent_status`, `.cwd` — are pinned by unit test so they cannot
/// drift silently under refactoring (review R4). Note the test cannot
/// catch an UPSTREAM herdr rename: that degrades every agent to status
/// "unknown" (stall-alarming each fleet) — a documented choice, made
/// visible in the test rather than prevented by it. `None` = not a valid
/// listing (the caller treats it like a failed call and retries).
pub fn parse_agent_listing(stdout: &str) -> Option<BTreeMap<String, AgentInfo>> {
    let value = serde_json::from_str::<serde_json::Value>(stdout).ok()?;
    let list = value.get("result")?.get("agents")?.as_array()?;
    let mut map = BTreeMap::new();
    for a in list {
        let Some(name) = a.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        map.insert(
            name.to_string(),
            AgentInfo {
                status: a
                    .get("agent_status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                cwd: a
                    .get("cwd")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string(),
            },
        );
    }
    Some(map)
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
        let out = problems(
            &registry(false),
            &Some(healthy_agents()),
            &no_prs(),
            "/Users/x",
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn server_down_is_one_line_with_no_per_fleet_spam() {
        let out = problems(&registry(false), &None, &no_prs(), "/Users/x");
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("herdr server NOT reachable"));
    }

    /// F3: a SUCCESSFUL empty listing is a healthy answer — never a
    /// server-down false alarm; per-fleet checks stay truthful.
    #[test]
    fn empty_listing_is_not_server_down() {
        let paused = problems(
            &registry(true),
            &Some(BTreeMap::new()),
            &no_prs(),
            "/Users/x",
        );
        // corral paused (silent); board's missing orch is the only line.
        assert_eq!(
            paused,
            vec!["board: orchestrator orch-board MISSING".to_string()]
        );
        let out = problems(
            &registry(false),
            &Some(BTreeMap::new()),
            &no_prs(),
            "/Users/x",
        );
        assert!(
            out.iter().all(|l| !l.contains("NOT reachable")),
            "empty listing must not claim the server is down: {out:?}"
        );
        assert!(
            out.iter()
                .any(|l| l.contains("orchestrator orch-corral MISSING"))
        );
    }

    #[test]
    fn paused_fleet_is_skipped_entirely_orch_and_workers() {
        // corral paused: its missing orch AND missing workers are silent;
        // board still checked.
        let mut agents = BTreeMap::new();
        agents.insert("orch-board".to_string(), agent("working", "/repos/board"));
        let out = problems(&registry(true), &Some(agents), &no_prs(), "/Users/x");
        assert!(out.is_empty(), "paused fleet fully silenced: {out:?}");
    }

    #[test]
    fn missing_orchestrator_and_workers_are_reported() {
        let mut agents = healthy_agents();
        agents.remove("orch-corral");
        agents.remove("w2");
        let out = problems(&registry(false), &Some(agents), &no_prs(), "/Users/x");
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
        let out = problems(&registry(false), &Some(agents.clone()), &prs, "/Users/x");
        assert_eq!(
            out,
            vec!["corral: orch-corral STALLED with 3 open PR(s)".to_string()]
        );

        // 2) Workers working (by name) comes next.
        agents.insert("w1".to_string(), agent("working", "/elsewhere"));
        let out = problems(
            &registry(false),
            &Some(agents.clone()),
            &no_prs(),
            "/Users/x",
        );
        assert_eq!(
            out,
            vec!["corral: orch-corral STALLED while worker(s) working — wave spawned, not collecting".to_string()]
        );

        // 2b) F5: gh unavailable is stated on the wave-spawned flavor too.
        let prs_unavailable_all = BTreeMap::from([
            ("o/corral".to_string(), None),
            ("o/board".to_string(), Some(0)),
        ]);
        let out = problems(
            &registry(false),
            &Some(agents.clone()),
            &prs_unavailable_all,
            "/Users/x",
        );
        assert_eq!(
            out,
            vec!["corral: orch-corral STALLED while worker(s) working — wave spawned, not collecting; open-PR check unavailable".to_string()]
        );

        // 3) Plain stall with the status named.
        agents.insert("w1".to_string(), agent("idle", "/repos/corral"));
        let out = problems(
            &registry(false),
            &Some(agents.clone()),
            &no_prs(),
            "/Users/x",
        );
        assert_eq!(
            out,
            vec!["corral: orch-corral STALLED (status=idle)".to_string()]
        );

        // 3b) Unavailable PR check is stated, not hidden.
        let prs_unavailable = BTreeMap::from([
            ("o/corral".to_string(), None),
            ("o/board".to_string(), Some(0)),
        ]);
        let out = problems(
            &registry(false),
            &Some(agents),
            &prs_unavailable,
            "/Users/x",
        );
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
        let out = problems(
            &registry(false),
            &Some(agents.clone()),
            &no_prs(),
            "/Users/x",
        );
        assert!(
            out[0].contains("wave spawned"),
            "cwd-matched worker counts: {out:?}"
        );

        // Component-exactness: corral-x is NOT corral.
        agents.insert(
            "g99-1".to_string(),
            agent("working", "/Users/x/.herdr/worktrees/corral-x/g99-fix"),
        );
        let out = problems(&registry(false), &Some(agents), &no_prs(), "/Users/x");
        assert_eq!(
            out,
            vec!["corral: orch-corral STALLED (status=idle)".to_string()],
            "another fleet's worker must not count"
        );
    }

    #[test]
    fn cwd_matcher_edges() {
        assert!(cwd_in_fleet(
            "/repos/corral",
            "/repos/corral",
            "corral",
            "/Users/x"
        ));
        assert!(cwd_in_fleet(
            "/repos/corral/sub",
            "/repos/corral",
            "corral",
            "/Users/x"
        ));
        assert!(!cwd_in_fleet(
            "/repos/corral-x",
            "/repos/corral",
            "corral",
            "/Users/x"
        ));
        assert!(cwd_in_fleet(
            "/u/.herdr/worktrees/corral",
            "",
            "corral",
            "/u"
        ));
        assert!(cwd_in_fleet(
            "/u/.herdr/worktrees/corral/wt1",
            "",
            "corral",
            "/u"
        ));
        assert!(!cwd_in_fleet(
            "/u/.herdr/worktrees/corral-x/wt1",
            "",
            "corral",
            "/u"
        ));
        assert!(!cwd_in_fleet("", "/repos/corral", "corral", "/Users/x"));
        assert!(!cwd_in_fleet("/anything", "", "", "/Users/x"));
        // F6: worktree leg anchored at <home>/.herdr/worktrees.
        assert!(!cwd_in_fleet(
            "/Users/x/Projects/sandbox/.herdr/worktrees/corral/wt",
            "",
            "corral",
            "/Users/x"
        ));
        assert!(!cwd_in_fleet(
            "/Users/other/.herdr/worktrees/corral/wt",
            "",
            "corral",
            "/Users/x"
        ));
        // F7: shallow locals authorize nothing.
        assert!(!cwd_in_fleet(
            "/Users/x/Projects/anything",
            "/Users/x/Projects",
            "corral",
            "/Users/x"
        ));
        assert!(!cwd_in_fleet(
            "/Users/x/whatever",
            "/Users/x",
            "corral",
            "/Users/x"
        ));
        assert!(cwd_in_fleet(
            "/Users/x/Projects/corral/sub",
            "/Users/x/Projects/corral",
            "corral",
            "/Users/x"
        ));
        assert!(cwd_in_fleet(
            "/opt/board/x",
            "/opt/board",
            "board",
            "/Users/x"
        ));
        // F10: trailing slash on local must not disable the leg.
        assert!(cwd_in_fleet(
            "/Users/x/Projects/corral",
            "/Users/x/Projects/corral/",
            "corral",
            "/Users/x"
        ));
        // N5: a trailing slash on worktree_dir must not disable the leg.
        assert!(cwd_in_fleet(
            "/u/.herdr/worktrees/corral/wt1",
            "",
            "corral/",
            "/u"
        ));
        // R3: the depth rule holds OUTSIDE home too — a misconfigured
        // one-component local must not authorize every cwd under it.
        assert!(!cwd_in_fleet(
            "/Users/other/anything",
            "/Users",
            "corral",
            "/Users/x"
        ));
        assert!(!cwd_in_fleet("/opt/anything", "/opt", "corral", "/Users/x"));
        assert!(!cwd_in_fleet("/anything", "/", "corral", "/Users/x"));
    }

    /// R4: the herdr wire contract — `result.agents[].name` /
    /// `.agent_status` / `.cwd`. Pins the field names THIS parser
    /// depends on (refactoring drift fails here); an upstream herdr
    /// rename still degrades to "unknown" — asserted below so that
    /// degradation stays a visible choice, not an accident.
    #[test]
    fn agent_listing_parse_pins_field_names() {
        let listing = r#"{ "result": { "agents": [
            { "name": "orch-corral", "agent_status": "working", "cwd": "/repos/corral" },
            { "name": "w1", "agent_status": "idle", "cwd": "/repos/corral" },
            { "agent_status": "working", "cwd": "/no/name/skipped" }
        ] } }"#;
        let map = parse_agent_listing(listing).expect("valid listing parses");
        assert_eq!(
            map.get("orch-corral"),
            Some(&agent("working", "/repos/corral"))
        );
        assert_eq!(map.get("w1"), Some(&agent("idle", "/repos/corral")));
        assert_eq!(map.len(), 2, "nameless entries skipped: {map:?}");

        // A healthy zero-agent answer parses as Some(empty), not None.
        assert_eq!(
            parse_agent_listing(r#"{ "result": { "agents": [] } }"#),
            Some(BTreeMap::new())
        );
        // Not a listing at all → None (caller retries, then server-down).
        assert_eq!(parse_agent_listing("not json"), None);
        assert_eq!(parse_agent_listing(r#"{ "result": {} }"#), None);
        assert_eq!(parse_agent_listing(r#"{ "agents": [] }"#), None);
        // A renamed status field degrades to "unknown" — visible here so
        // the degradation is a CHOICE, not an accident.
        let renamed = r#"{ "result": { "agents": [
            { "name": "a", "status": "working", "cwd": "/x/y" }
        ] } }"#;
        assert_eq!(
            parse_agent_listing(renamed).expect("parses").get("a"),
            Some(&agent("unknown", "/x/y"))
        );
    }

    #[test]
    fn output_is_sorted_and_stable() {
        // Exact expected vector: pins the ordering CONTRACT, not just
        // that some sort happened (review F12b).
        let out = problems(
            &registry(false),
            &Some(BTreeMap::new()),
            &no_prs(),
            "/Users/x",
        );
        assert_eq!(
            out,
            vec![
                "board: orchestrator orch-board MISSING".to_string(),
                "corral: orchestrator orch-corral MISSING".to_string(),
                "corral: worker w1 MISSING (server restart?)".to_string(),
                "corral: worker w2 MISSING (server restart?)".to_string(),
            ]
        );
    }
}
