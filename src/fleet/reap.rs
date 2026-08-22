//! #35 reaper: finished agents plus idle agents in paused fleets.
//!
//! `corrald fleet reap <fleet>` mirrors the hardened legacy `agent-reaper`
//! safety model:
//!
//! - The only kill signal is an explicit herdr agent status: `done` /
//!   `completed` for finished agents, or `idle` whose cwd belongs to a
//!   fleet with `paused: true`.
//! - No session guessing and no orphan sweeps.
//! - The pane's foreground process group is verified before any signal:
//!   argv0 allowlist, not the pane shell, pid sanity, and the process cwd
//!   must equal the agent's recorded cwd.
//! - The agent and pane are re-fetched immediately before TERM/KILL, and an
//!   idle victim is re-checked against the registry's current paused state.
//! - A shrink guard refuses the whole sweep before any kill if the finished
//!   count would exceed the configured fraction/absolute cap.
//! - Dry-run is the default; `--apply` is required to signal anything.
//!
//! The decision layer is pure over plain inputs; the CLI wires the herdr
//! and `kill` commands through the production adapters.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::fleet::config::Registry;
use crate::fleet::watch::cwd_in_fleet;

/// Statuses that mean an agent has finished and its pane process is a reap
/// target. Kept explicit — a new status must be a deliberate decision.
pub const DONE_STATUSES: &[&str] = &["done", "completed"];

/// Foreground executable basenames that are permitted to be an agent pane.
/// `node` is included for opencode's hosted process tree, matching legacy.
pub const ALLOWED_PROC_NAMES: &[&str] = &["opencode", "opencode.exe", "claude", "codex", "node"];

/// One live agent as the reaper needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRecord {
    pub name: String,
    pub status: String,
    pub cwd: String,
    pub pane_id: Option<String>,
    pub revision: Option<u64>,
    pub state_change_seq: Option<u64>,
}

/// `None` means the herdr listing call failed or could not be parsed. An
/// empty map is a healthy zero-agent answer (exactly like the watchdog).
pub type AgentsView = Option<BTreeMap<String, AgentRecord>>;

/// One foreground process reported by `herdr pane process-info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: i64,
    pub argv0: String,
    pub cwd: String,
}

/// The subset of pane process info the reaper needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInfo {
    pub foreground_process_group_id: i64,
    pub shell_pid: i64,
    pub foreground_processes: Vec<ProcessInfo>,
}

/// Reaper configuration. `max_fraction` is in `(0, 1]`.
#[derive(Debug, Clone)]
pub struct ReapOptions {
    pub apply: bool,
    pub max_done: usize,
    pub max_fraction: f64,
}

impl Default for ReapOptions {
    fn default() -> Self {
        Self {
            apply: false,
            max_done: 5,
            max_fraction: 0.25,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReapKind {
    Done,
    IdlePaused,
}

impl fmt::Display for ReapKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Done => f.write_str("done"),
            Self::IdlePaused => f.write_str("idle-paused"),
        }
    }
}

/// A candidate that passed every non-destructive identity check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub kind: ReapKind,
    pub pane_id: String,
    pub foreground_process_group_id: i64,
    pub verified_pids: Vec<i64>,
    pub cwd: String,
}

/// The non-destructive decision for one run.
#[derive(Debug, Clone, Default)]
pub struct ReapPlan {
    pub fleet_size: usize,
    pub done_count: usize,
    pub candidates: Vec<Candidate>,
    pub diagnostics: Vec<String>,
}

impl ReapPlan {
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }
}

/// What the destructive run did.
#[derive(Debug, Clone, Default)]
pub struct ReapReport {
    pub killed: Vec<String>,
    pub skipped: Vec<String>,
    pub failures: Vec<String>,
}

#[derive(Debug)]
pub enum ReapError {
    AgentListUnavailable,
    FleetNotFound {
        name: String,
    },
    ShrinkGuard {
        done: usize,
        fleet_size: usize,
        max_done: usize,
        max_fraction: f64,
        floor: usize,
    },
    AgentChanged {
        name: String,
        detail: String,
    },
    PaneChanged {
        name: String,
        detail: String,
    },
    Kill {
        name: String,
        detail: String,
    },
}

impl fmt::Display for ReapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AgentListUnavailable => {
                f.write_str("herdr agent list failed or was unreadable — no action taken")
            }
            Self::FleetNotFound { name } => {
                write!(f, "fleet {name:?} is not in the registry")
            }
            Self::ShrinkGuard {
                done,
                fleet_size,
                max_done,
                max_fraction,
                floor,
            } => write!(
                f,
                "{done} finished agent(s) exceed the shrink guard: \
                 absolute cap {max_done}, fraction {max_fraction:.2} floor {floor} \
                 of {fleet_size} agent(s) — nothing was killed"
            ),
            Self::AgentChanged { name, detail } => {
                write!(f, "{name} changed since the snapshot: {detail} — skipped")
            }
            Self::PaneChanged { name, detail } => {
                write!(f, "{name} pane changed before signal: {detail} — skipped")
            }
            Self::Kill { name, detail } => {
                write!(f, "could not kill {name} process group: {detail}")
            }
        }
    }
}

impl std::error::Error for ReapError {}

/// The production herdr adapter used by the CLI.
pub fn list_agents() -> AgentsView {
    let output = Command::new("herdr")
        .args(["agent", "list"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_agent_listing(&stdout)
}

/// Parse `herdr agent list` JSON. Field names are pinned by tests so an
/// upstream rename degrades to `None`/`unknown` rather than mis-killing.
pub fn parse_agent_listing(raw: &str) -> AgentsView {
    let value: Value = serde_json::from_str(raw).ok()?;
    let agents = value.get("result")?.get("agents")?.as_array()?;
    let mut out = BTreeMap::new();
    for entry in agents {
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let status = entry
            .get("agent_status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let cwd = entry
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let pane_id = entry
            .get("pane_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let revision = entry.get("revision").and_then(Value::as_u64);
        let state_change_seq = entry.get("state_change_seq").and_then(Value::as_u64);
        out.insert(
            name.to_string(),
            AgentRecord {
                name: name.to_string(),
                status,
                cwd,
                pane_id,
                revision,
                state_change_seq,
            },
        );
    }
    Some(out)
}

/// Parse one `herdr pane process-info` response.
pub fn parse_pane_info(raw: &str) -> Option<PaneInfo> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let info = value.get("result")?.get("process_info")?.as_object()?;
    let foreground_process_group_id = info
        .get("foreground_process_group_id")
        .and_then(Value::as_i64)?;
    let shell_pid = info.get("shell_pid").and_then(Value::as_i64)?;
    let mut foreground_processes = Vec::new();
    for process in info
        .get("foreground_processes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(pid) = process.get("pid").and_then(Value::as_i64) else {
            continue;
        };
        let argv0 = process
            .get("argv0")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let cwd = process
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        foreground_processes.push(ProcessInfo { pid, argv0, cwd });
    }
    Some(PaneInfo {
        foreground_process_group_id,
        shell_pid,
        foreground_processes,
    })
}

/// The pane-inspection seam. Production uses herdr; tests use a map.
pub trait PaneInspector {
    fn pane_info(&self, pane_id: &str) -> Option<PaneInfo>;
}

pub struct HerdrPaneInspector;

impl PaneInspector for HerdrPaneInspector {
    fn pane_info(&self, pane_id: &str) -> Option<PaneInfo> {
        let output = Command::new("herdr")
            .args(["pane", "process-info", "--pane", pane_id])
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        parse_pane_info(&String::from_utf8_lossy(&output.stdout))
    }
}

/// Signal one verified process group. `kill -<signal> -<pgid>` addresses the
/// group itself; the negative pid is deliberately unsafe-proof by refusing
/// pgids `<= 1` before it gets here.
pub trait ProcessKiller {
    fn signal(&self, signal: &str, pgid: i64) -> Result<(), String>;

    /// True when at least one previously verified pid still belongs to the
    /// process group. This is checked after TERM so a reused pgid can never
    /// receive an unverified SIGKILL.
    fn any_pid_in_group(&self, pids: &[i64], pgid: i64) -> Result<bool, String>;
}

pub struct SystemKiller;

impl ProcessKiller for SystemKiller {
    fn signal(&self, signal: &str, pgid: i64) -> Result<(), String> {
        if pgid <= 1 {
            return Err("invalid process group id".to_string());
        }
        let output = Command::new("kill")
            .args([signal, &format!("-{pgid}")])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("spawn kill: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // ProcessLookupError is expected when the group exited between
            // verification and the signal; that is not a safety failure.
            if stderr.to_ascii_lowercase().contains("no such process") {
                Ok(())
            } else {
                Err(stderr.trim().to_string())
            }
        }
    }

    fn any_pid_in_group(&self, pids: &[i64], pgid: i64) -> Result<bool, String> {
        if pgid <= 1 {
            return Ok(false);
        }
        for pid in pids {
            if *pid <= 1 {
                continue;
            }
            let output = Command::new("ps")
                .args(["-o", "pgid=", "-p", &pid.to_string()])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .map_err(|e| format!("spawn ps: {e}"))?;
            if !output.status.success() {
                continue;
            }
            let reported = String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<i64>().ok());
            if reported == Some(pgid) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// A terminal agent whose pane contains a live claude/codex process is a
/// turn-complete resumable session, not a leaked finished process. Only
/// opencode-style `done` is safe to reap on this signal.
fn live_cli_session(info: &PaneInfo) -> bool {
    info.foreground_processes.iter().any(|p| {
        let basename = Path::new(&p.argv0)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&p.argv0);
        matches!(basename, "claude" | "codex")
    })
}

/// Verify the pane group and return the pgid plus verified pids. Any failure
/// is a non-kill direction.
pub(crate) fn verified_victim(info: &PaneInfo, agent: &AgentRecord) -> Option<(i64, Vec<i64>)> {
    let pgid = info.foreground_process_group_id;
    if pgid <= 1 {
        return None;
    }
    let mut pids = Vec::new();
    for process in &info.foreground_processes {
        if process.pid <= 1 || process.pid == info.shell_pid {
            continue;
        }
        let basename = Path::new(&process.argv0)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&process.argv0);
        if !ALLOWED_PROC_NAMES.contains(&basename) {
            continue;
        }
        if process.cwd.is_empty() || process.cwd != agent.cwd {
            continue;
        }
        pids.push(process.pid);
    }
    if pids.is_empty() {
        None
    } else {
        Some((pgid, pids))
    }
}

/// Which fleet (if any) an agent's cwd belongs to.
fn fleet_for_cwd<'a>(
    registry: &'a Registry,
    cwd: &str,
    home: &str,
) -> Option<&'a crate::fleet::config::Fleet> {
    registry.fleets.iter().find(|fleet| {
        let local = fleet.local_path();
        cwd_in_fleet(
            cwd,
            local.to_string_lossy().as_ref(),
            &fleet.worktree_dir,
            home,
        )
    })
}

/// Build the non-destructive plan. `fleet_name` is `all` for every fleet;
/// otherwise only agents whose cwd matches exactly that fleet are considered.
pub fn plan(
    registry: &Registry,
    fleet_name: &str,
    agents: &AgentsView,
    opts: &ReapOptions,
    home: &str,
    inspector: &dyn PaneInspector,
) -> Result<ReapPlan, ReapError> {
    let Some(agents) = agents else {
        return Err(ReapError::AgentListUnavailable);
    };
    if fleet_name != "all" && !registry.fleets.iter().any(|fleet| fleet.name == fleet_name) {
        return Err(ReapError::FleetNotFound {
            name: fleet_name.to_string(),
        });
    }
    let mut fleet_agents: Vec<&AgentRecord> = agents.values().collect();
    if fleet_name != "all" {
        fleet_agents.retain(|agent| {
            fleet_for_cwd(registry, &agent.cwd, home).is_some_and(|fleet| fleet.name == fleet_name)
        });
    }
    let fleet_size = fleet_agents.len();

    let mut done_count = 0usize;
    let mut candidates = Vec::new();
    let mut diagnostics = Vec::new();

    for agent in fleet_agents {
        let done = DONE_STATUSES.contains(&agent.status.as_str());
        let idle = agent.status == "idle";
        if !done && !idle {
            continue;
        }

        let paused = if idle {
            fleet_for_cwd(registry, &agent.cwd, home)
                .is_some_and(|fleet| fleet.paused && fleet.orch != agent.name)
        } else {
            false
        };

        let Some(pane_id) = agent.pane_id.as_deref() else {
            if done {
                done_count += 1;
            }
            diagnostics.push(format!("{}: no pane_id — skipped", agent.name));
            continue;
        };
        let Some(info) = inspector.pane_info(pane_id) else {
            if done {
                done_count += 1;
            }
            diagnostics.push(format!(
                "{}: pane {pane_id} not inspectable — skipped",
                agent.name
            ));
            continue;
        };
        if done && live_cli_session(&info) {
            diagnostics.push(format!(
                "{}: done but live claude/codex session — skipped",
                agent.name
            ));
            continue;
        }
        if done {
            done_count += 1;
        } else if !paused {
            diagnostics.push(format!(
                "{}: idle outside a paused fleet — skipped",
                agent.name
            ));
            continue;
        }
        let Some((pgid, verified_pids)) = verified_victim(&info, agent) else {
            diagnostics.push(format!(
                "{}: pane process failed identity checks — skipped",
                agent.name
            ));
            continue;
        };
        candidates.push(Candidate {
            name: agent.name.clone(),
            kind: if done {
                ReapKind::Done
            } else {
                ReapKind::IdlePaused
            },
            pane_id: pane_id.to_string(),
            foreground_process_group_id: pgid,
            verified_pids,
            cwd: agent.cwd.clone(),
        });
    }

    let floor = (fleet_size as f64 * opts.max_fraction).floor() as usize;
    let floor = floor.max(2);
    if done_count > opts.max_done || done_count > floor {
        return Err(ReapError::ShrinkGuard {
            done: done_count,
            fleet_size,
            max_done: opts.max_done,
            max_fraction: opts.max_fraction,
            floor,
        });
    }

    Ok(ReapPlan {
        fleet_size,
        done_count,
        candidates,
        diagnostics,
    })
}

/// Re-run the initial safety model against a fresh listing. This is called
/// right before each signal, after the initial plan was made.
fn revalidate(
    registry: &Registry,
    fleet_name: &str,
    candidate: &Candidate,
    original: Option<&AgentRecord>,
    fresh: &AgentRecord,
    home: &str,
    inspector: &dyn PaneInspector,
) -> Result<(i64, Vec<i64>), String> {
    let status_ok = match candidate.kind {
        ReapKind::Done => DONE_STATUSES.contains(&fresh.status.as_str()),
        ReapKind::IdlePaused => fresh.status == "idle",
    };
    if !status_ok {
        return Err(format!("status is now {:?}", fresh.status));
    }
    let original = original.ok_or_else(|| "original agent snapshot is missing".to_string())?;
    if fresh.pane_id != original.pane_id
        || fresh.revision != original.revision
        || fresh.state_change_seq != original.state_change_seq
    {
        return Err("agent identity changed since snapshot".to_string());
    }
    if candidate.kind == ReapKind::IdlePaused {
        let fleet = fleet_for_cwd(registry, &fresh.cwd, home);
        let still_paused = fleet.is_some_and(|fleet| fleet.paused && fleet.orch != fresh.name);
        if !still_paused {
            return Err("fleet is no longer paused or cwd moved".to_string());
        }
        if fleet_name != "all" && fleet.is_none_or(|fleet| fleet.name != fleet_name) {
            return Err("agent moved outside the requested fleet".to_string());
        }
    }
    let Some(pane_id) = fresh.pane_id.as_deref() else {
        return Err("pane_id disappeared".to_string());
    };
    let Some(info) = inspector.pane_info(pane_id) else {
        return Err("pane no longer inspectable".to_string());
    };
    if candidate.kind == ReapKind::Done && live_cli_session(&info) {
        return Err("done but now a live claude/codex session".to_string());
    }
    if info.foreground_process_group_id != candidate.foreground_process_group_id {
        return Err("foreground process group changed".to_string());
    }
    let (_, pids) = verified_victim(&info, fresh)
        .ok_or_else(|| "pane process failed identity checks".to_string())?;
    let still_present = candidate.verified_pids.iter().any(|pid| pids.contains(pid));
    if !still_present {
        return Err("no previously verified pid remains in the group".to_string());
    }
    Ok((info.foreground_process_group_id, pids))
}

/// Run one reap sweep. Dry-run returns the plan as a report; `--apply`
/// re-fetches and re-validates every victim immediately before signaling.
pub fn reap(
    registry: &Registry,
    fleet_name: &str,
    opts: &ReapOptions,
    home: &str,
    mut lister: impl FnMut() -> AgentsView,
    inspector: &dyn PaneInspector,
    killer: &dyn ProcessKiller,
) -> Result<ReapReport, ReapError> {
    let initial = lister();
    let plan = plan(registry, fleet_name, &initial, opts, home, inspector)?;
    let mut report = ReapReport {
        skipped: plan.diagnostics.clone(),
        ..ReapReport::default()
    };
    if !opts.apply || plan.candidates.is_empty() {
        return Ok(report);
    }

    for candidate in plan.candidates {
        let Some(agents) = lister() else {
            return Err(ReapError::AgentListUnavailable);
        };
        let Some(fresh) = agents.get(&candidate.name) else {
            report
                .skipped
                .push(format!("{}: agent disappeared — skipped", candidate.name));
            continue;
        };
        let original = initial.as_ref().and_then(|map| map.get(&candidate.name));
        match revalidate(
            registry, fleet_name, &candidate, original, fresh, home, inspector,
        ) {
            Ok((pgid, _)) => {
                if let Err(detail) = killer.signal("TERM", pgid) {
                    report
                        .failures
                        .push(format!("{}: {detail}", candidate.name));
                    continue;
                }
                std::thread::sleep(Duration::from_secs(2));
                match killer.any_pid_in_group(&candidate.verified_pids, pgid) {
                    Ok(false) => {}
                    Ok(true) => {
                        if let Err(detail) = killer.signal("KILL", pgid) {
                            report
                                .failures
                                .push(format!("{}: {detail}", candidate.name));
                            continue;
                        }
                    }
                    Err(detail) => {
                        report.failures.push(format!(
                            "{}: could not verify the group after TERM — no KILL sent: {detail}",
                            candidate.name
                        ));
                        continue;
                    }
                }
                report.killed.push(candidate.name);
            }
            Err(detail) => {
                report
                    .skipped
                    .push(format!("{}: {detail} — skipped", candidate.name));
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::config::{Fleet, Models, Registry};

    fn fleet(name: &str, paused: bool) -> Fleet {
        Fleet {
            name: name.to_string(),
            gh_repo: "jirathip-k/corral".to_string(),
            local: "/repos/corral".to_string(),
            worktree_dir: "corral".to_string(),
            orch: format!("orch-{name}"),
            workers: vec!["w1".to_string(), "w2".to_string()],
            paused,
            models: Models {
                orch: "codex/gpt-5.6".to_string(),
                impl_: "opencode-go/deepseek-v4-flash".to_string(),
                review: "opus".to_string(),
                impl_alt: None,
                impl_alt2: None,
            },
        }
    }

    fn agent(name: &str, status: &str, cwd: &str, pane: &str) -> AgentRecord {
        AgentRecord {
            name: name.to_string(),
            status: status.to_string(),
            cwd: cwd.to_string(),
            pane_id: Some(pane.to_string()),
            revision: Some(7),
            state_change_seq: Some(11),
        }
    }

    fn pane(pgid: i64, argv0: &str, pid: i64, cwd: &str) -> PaneInfo {
        PaneInfo {
            foreground_process_group_id: pgid,
            shell_pid: 1,
            foreground_processes: vec![ProcessInfo {
                pid,
                argv0: argv0.to_string(),
                cwd: cwd.to_string(),
            }],
        }
    }

    #[derive(Default)]
    struct FakeInspector(BTreeMap<String, PaneInfo>);

    impl PaneInspector for FakeInspector {
        fn pane_info(&self, pane_id: &str) -> Option<PaneInfo> {
            self.0.get(pane_id).cloned()
        }
    }

    #[derive(Default)]
    struct FakeKiller {
        calls: std::cell::RefCell<Vec<(String, i64)>>,
        group_alive: std::cell::Cell<bool>,
    }

    impl ProcessKiller for FakeKiller {
        fn signal(&self, signal: &str, pgid: i64) -> Result<(), String> {
            self.calls.borrow_mut().push((signal.to_string(), pgid));
            Ok(())
        }

        fn any_pid_in_group(&self, _pids: &[i64], _pgid: i64) -> Result<bool, String> {
            Ok(self.group_alive.get())
        }
    }

    #[test]
    fn parser_pins_agent_and_pane_fields() {
        let agents = parse_agent_listing(
            r#"{ "result": { "agents": [
                { "name": "w1", "agent_status": "idle", "cwd": "/repos/corral",
                  "pane_id": "p1", "revision": 3, "state_change_seq": 5 }
            ] } }"#,
        )
        .expect("listing parses");
        assert_eq!(agents.get("w1").unwrap().pane_id.as_deref(), Some("p1"));
        assert_eq!(agents.get("w1").unwrap().revision, Some(3));
        assert_eq!(
            parse_pane_info(
                r#"{ "result": { "process_info": {
                    "foreground_process_group_id": 40, "shell_pid": 2,
                    "foreground_processes": [
                        { "pid": 41, "argv0": "/bin/opencode", "cwd": "/repos/corral" }
                    ]
                } } }"#,
            )
            .unwrap()
            .foreground_processes[0]
                .argv0,
            "/bin/opencode"
        );
    }

    #[test]
    fn plan_reaps_done_and_paused_idle_but_not_canonical_orch() {
        let registry = Registry {
            fleets: vec![fleet("corral", true)],
        };
        let mut map = BTreeMap::new();
        map.insert(
            "orch-corral".to_string(),
            agent("orch-corral", "idle", "/repos/corral", "p-orch"),
        );
        map.insert(
            "w1".to_string(),
            agent("w1", "idle", "/repos/corral", "p-w1"),
        );
        map.insert(
            "w2".to_string(),
            agent("w2", "done", "/repos/corral", "p-w2"),
        );
        let mut panes = BTreeMap::new();
        panes.insert(
            "p-orch".to_string(),
            pane(100, "/bin/opencode", 200, "/repos/corral"),
        );
        panes.insert(
            "p-w1".to_string(),
            pane(101, "/bin/opencode", 201, "/repos/corral"),
        );
        panes.insert(
            "p-w2".to_string(),
            pane(102, "/bin/opencode", 202, "/repos/corral"),
        );
        let plan = plan(
            &registry,
            "corral",
            &Some(map),
            &ReapOptions::default(),
            "/Users/x",
            &FakeInspector(panes),
        )
        .expect("plan succeeds");
        let names: Vec<&str> = plan
            .candidates
            .iter()
            .map(|candidate| candidate.name.as_str())
            .collect();
        assert_eq!(names, vec!["w1", "w2"]);
        assert_eq!(plan.candidates[0].kind, ReapKind::IdlePaused);
        assert_eq!(plan.candidates[1].kind, ReapKind::Done);
        assert!(
            plan.diagnostics
                .iter()
                .any(|line| line.contains("orchestrator") || line.contains("orch-corral")),
            "canonical paused orchestrator is skipped: {:?}",
            plan.diagnostics
        );
    }

    #[test]
    fn shrink_guard_refuses_before_any_kill() {
        let registry = Registry {
            fleets: vec![fleet("corral", false)],
        };
        let mut map = BTreeMap::new();
        let mut panes = BTreeMap::new();
        for i in 0..8 {
            let name = format!("w{i}");
            let pane_id = format!("p{i}");
            map.insert(
                name.clone(),
                agent(&name, "done", "/repos/corral", &pane_id),
            );
            panes.insert(
                pane_id,
                pane(100 + i, "/bin/opencode", 200 + i, "/repos/corral"),
            );
        }
        let err = plan(
            &registry,
            "corral",
            &Some(map),
            &ReapOptions {
                apply: true,
                max_done: 3,
                max_fraction: 0.25,
            },
            "/Users/x",
            &FakeInspector(panes),
        )
        .expect_err("guard must refuse");
        assert!(matches!(err, ReapError::ShrinkGuard { .. }));
    }

    #[test]
    fn shrink_guard_counts_done_even_when_identity_is_uninspectable() {
        let registry = Registry {
            fleets: vec![fleet("corral", false)],
        };
        let mut map = BTreeMap::new();
        for i in 0..8 {
            let name = format!("w{i}");
            map.insert(
                name.clone(),
                agent(&name, "done", "/repos/corral", &format!("p{i}")),
            );
        }
        let err = plan(
            &registry,
            "corral",
            &Some(map),
            &ReapOptions {
                apply: true,
                max_done: 3,
                max_fraction: 0.25,
            },
            "/Users/x",
            &FakeInspector(BTreeMap::new()),
        )
        .expect_err("uninspectable done agents still count against the guard");
        assert!(matches!(err, ReapError::ShrinkGuard { .. }));
    }

    #[test]
    fn unknown_fleet_is_an_operational_refusal() {
        let registry = Registry {
            fleets: vec![fleet("corral", false)],
        };
        let err = plan(
            &registry,
            "nope",
            &Some(BTreeMap::new()),
            &ReapOptions::default(),
            "/Users/x",
            &FakeInspector(BTreeMap::new()),
        )
        .expect_err("unknown fleet must refuse");
        assert!(matches!(err, ReapError::FleetNotFound { name } if name == "nope"));
    }

    #[test]
    fn done_claude_session_is_not_a_reap_target() {
        let registry = Registry {
            fleets: vec![fleet("corral", true)],
        };
        let mut map = BTreeMap::new();
        map.insert("w1".to_string(), agent("w1", "done", "/repos/corral", "p1"));
        let mut panes = BTreeMap::new();
        panes.insert(
            "p1".to_string(),
            pane(100, "/bin/claude", 200, "/repos/corral"),
        );
        let plan = plan(
            &registry,
            "corral",
            &Some(map),
            &ReapOptions::default(),
            "/Users/x",
            &FakeInspector(panes),
        )
        .expect("no guard failure for one agent");
        assert!(plan.is_empty());
        assert!(plan.diagnostics[0].contains("live"));
    }

    #[test]
    fn dry_run_never_signals() {
        let registry = Registry {
            fleets: vec![fleet("corral", true)],
        };
        let mut map = BTreeMap::new();
        map.insert("w1".to_string(), agent("w1", "idle", "/repos/corral", "p1"));
        let mut panes = BTreeMap::new();
        panes.insert(
            "p1".to_string(),
            pane(100, "/bin/opencode", 200, "/repos/corral"),
        );
        let report = reap(
            &registry,
            "corral",
            &ReapOptions::default(),
            "/Users/x",
            || Some(map.clone()),
            &FakeInspector(panes),
            &FakeKiller::default(),
        )
        .expect("dry run succeeds");
        assert!(report.killed.is_empty());
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn apply_revalidates_then_terms_and_kills_verified_group() {
        let registry = Registry {
            fleets: vec![fleet("corral", true)],
        };
        let mut map = BTreeMap::new();
        map.insert("w1".to_string(), agent("w1", "done", "/repos/corral", "p1"));
        let mut panes = BTreeMap::new();
        panes.insert(
            "p1".to_string(),
            pane(100, "/bin/opencode", 200, "/repos/corral"),
        );
        let killer = FakeKiller {
            group_alive: std::cell::Cell::new(true),
            ..FakeKiller::default()
        };
        let report = reap(
            &registry,
            "corral",
            &ReapOptions {
                apply: true,
                ..ReapOptions::default()
            },
            "/Users/x",
            || Some(map.clone()),
            &FakeInspector(panes),
            &killer,
        )
        .expect("apply succeeds");
        assert_eq!(report.killed, vec!["w1"]);
        assert_eq!(
            *killer.calls.borrow(),
            vec![("TERM".to_string(), 100), ("KILL".to_string(), 100)]
        );
    }

    #[test]
    fn apply_skips_kill_when_verified_group_gone_after_term() {
        let registry = Registry {
            fleets: vec![fleet("corral", true)],
        };
        let mut map = BTreeMap::new();
        map.insert("w1".to_string(), agent("w1", "done", "/repos/corral", "p1"));
        let mut panes = BTreeMap::new();
        panes.insert(
            "p1".to_string(),
            pane(100, "/bin/opencode", 200, "/repos/corral"),
        );
        let killer = FakeKiller::default();
        let report = reap(
            &registry,
            "corral",
            &ReapOptions {
                apply: true,
                ..ReapOptions::default()
            },
            "/Users/x",
            || Some(map.clone()),
            &FakeInspector(panes),
            &killer,
        )
        .expect("apply succeeds");
        assert_eq!(report.killed, vec!["w1"]);
        assert_eq!(*killer.calls.borrow(), vec![("TERM".to_string(), 100)]);
    }

    #[test]
    fn apply_skips_agent_that_changed_after_snapshot() {
        let registry = Registry {
            fleets: vec![fleet("corral", true)],
        };
        let original = agent("w1", "done", "/repos/corral", "p1");
        let mut changed = original.clone();
        changed.revision = Some(8);
        let mut panes = BTreeMap::new();
        panes.insert(
            "p1".to_string(),
            pane(100, "/bin/opencode", 200, "/repos/corral"),
        );
        let mut calls = 0usize;
        let report = reap(
            &registry,
            "corral",
            &ReapOptions {
                apply: true,
                ..ReapOptions::default()
            },
            "/Users/x",
            || {
                calls += 1;
                let mut map = BTreeMap::new();
                map.insert(
                    "w1".to_string(),
                    if calls == 1 {
                        original.clone()
                    } else {
                        changed.clone()
                    },
                );
                Some(map)
            },
            &FakeInspector(panes),
            &FakeKiller {
                group_alive: std::cell::Cell::new(true),
                ..FakeKiller::default()
            },
        )
        .expect("apply returns a report for the changed agent");
        assert!(report.killed.is_empty());
        assert!(report.skipped.iter().any(|line| line.contains("changed")));
    }
}
