//! #35 auth-gated per-role model switch.
//!
//! `corrald fleet switch <name>` is the re-arm half of pause/resume. It:
//!
//! 1. validates every model id in the fleet's model map against the
//!    known harness mapping (claude / codex / opencode), refusing unknown
//!    providers before anything is touched;
//! 2. checks authentication for every harness the fleet's model map implies
//!    — not just the orchestrator — and refuses before killing the
//!    incumbent when any check fails or is unavailable;
//! 3. kills the registered incumbent orchestrator only after the same
//!    verified-pane identity checks the reaper uses;
//! 4. starts the orchestrator on the harness/model its registry entry now
//!    implies.
//!
//! The registry's `paused` flag is never written by this module. A failed
//! switch therefore cannot un-pause a fleet; after a successful switch the
//! fleet remains paused and requires the explicit operator `fleet resume`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::fleet::config::{Fleet, load};
use crate::fleet::reap::{
    AgentRecord, HerdrPaneInspector, PaneInspector, ProcessKiller, SystemKiller, list_agents,
    verified_victim,
};

/// Provider prefixes that route to the opencode harness. Kept explicit
/// (same decision as the legacy tooling): an unknown qualified model must
/// fail rather than silently fall through to claude.
const OPENCODE_PROVIDERS: &[&str] = &[
    "commandcode",
    "deepseek",
    "openai",
    "opencode",
    "opencode-go",
];

/// The auth-check seam. `None` means the check was unavailable, which is
/// fail-closed for a destructive re-arm.
pub trait AuthChecker {
    fn authenticated(&self, kind: &str) -> Option<bool>;
}

pub struct CliAuthChecker;

impl AuthChecker for CliAuthChecker {
    fn authenticated(&self, kind: &str) -> Option<bool> {
        match kind {
            "claude" => {
                let output = Command::new("claude")
                    .args(["auth", "status"])
                    .stdin(std::process::Stdio::null())
                    .output()
                    .ok()?;
                serde_json::from_str::<Value>(&String::from_utf8_lossy(&output.stdout))
                    .ok()?
                    .get("loggedIn")
                    .and_then(Value::as_bool)
            }
            "codex" => {
                let output = Command::new("codex")
                    .args(["login", "status"])
                    .stdin(std::process::Stdio::null())
                    .output()
                    .ok()?;
                output.status.success().then_some(true)
            }
            "opencode" => {
                let output = Command::new("opencode")
                    .args(["auth", "list"])
                    .stdin(std::process::Stdio::null())
                    .output()
                    .ok()?;
                output.status.success().then_some(true)
            }
            _ => None,
        }
    }
}

/// Harness implied by one model id.
pub fn kind_for(model: &str) -> Option<&'static str> {
    if let Some((provider, _)) = model.split_once('/') {
        if provider == "codex" {
            return Some("codex");
        }
        if OPENCODE_PROVIDERS.contains(&provider) {
            return Some("opencode");
        }
        return None;
    }
    Some("claude")
}

/// One fleet's unique implied harnesses, sorted for stable diagnostics.
pub fn fleet_kinds(fleet: &Fleet) -> Result<Vec<String>, String> {
    let mut kinds = BTreeMap::new();
    for (role, model) in [
        ("orch", Some(&fleet.models.orch)),
        ("impl", Some(&fleet.models.impl_)),
        ("review", Some(&fleet.models.review)),
        ("impl_alt", fleet.models.impl_alt.as_ref()),
        ("impl_alt2", fleet.models.impl_alt2.as_ref()),
    ] {
        let Some(model) = model else {
            continue;
        };
        let Some(kind) = kind_for(model) else {
            return Err(format!(
                "{role} model {model:?} has an unknown provider — known providers: \
                 codex, {}",
                OPENCODE_PROVIDERS.join(", ")
            ));
        };
        kinds.insert(kind.to_string(), model.clone());
    }
    Ok(kinds.into_keys().collect())
}

#[derive(Debug)]
pub enum SwitchError {
    FleetNotFound { name: String },
    AgentListUnavailable,
    ModelInvalid { detail: String },
    AuthFailed { kind: String },
    PaneNotFound,
    PaneIdentityFailed { name: String, detail: String },
    KillFailed { name: String, detail: String },
    StartFailed { detail: String },
    Registry(Box<crate::fleet::config::ConfigError>),
}

impl fmt::Display for SwitchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FleetNotFound { name } => write!(f, "fleet {name:?} is not in the registry"),
            Self::AgentListUnavailable => f.write_str("herdr agent list failed or was unreadable"),
            Self::ModelInvalid { detail } => f.write_str(detail),
            Self::AuthFailed { kind } => write!(
                f,
                "{kind} CLI is not authenticated (or auth status is unavailable) — \
                 run the runtime's login command once, then re-run switch"
            ),
            Self::PaneNotFound => {
                f.write_str("no registered orchestrator and no unambiguous pane; pass --pane <id>")
            }
            Self::PaneIdentityFailed { name, detail } => {
                write!(f, "{name} pane identity check failed: {detail}")
            }
            Self::KillFailed { name, detail } => {
                write!(f, "could not kill old {name}: {detail}")
            }
            Self::StartFailed { detail } => write!(f, "orchestrator start failed: {detail}"),
            Self::Registry(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SwitchError {}

impl SwitchError {
    /// The CLI exit contract: 1 for operational refusals/failures, 2 for
    /// registry parse/validation errors (delegated to the config error).
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Registry(error) => error.exit_code(),
            _ => 1,
        }
    }
}

fn registry_error(error: crate::fleet::config::ConfigError) -> SwitchError {
    SwitchError::Registry(Box::new(error))
}

fn parse_panes(raw: &str) -> Option<Vec<(String, String)>> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let panes = value.get("result")?.get("panes")?.as_array()?;
    Some(
        panes
            .iter()
            .filter_map(|pane| {
                let id = pane.get("pane_id")?.as_str()?.to_string();
                let cwd = pane.get("cwd")?.as_str()?.to_string();
                Some((id, cwd))
            })
            .collect(),
    )
}

fn list_panes() -> Option<Vec<(String, String)>> {
    let output = Command::new("herdr")
        .args(["pane", "list"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    parse_panes(&String::from_utf8_lossy(&output.stdout))
}

fn find_agent<'a>(
    agents: &'a BTreeMap<String, AgentRecord>,
    name: &str,
) -> Option<&'a AgentRecord> {
    agents.get(name)
}

fn find_pane(fleet: &Fleet, pane_override: Option<&str>) -> Result<String, SwitchError> {
    if let Some(pane) = pane_override {
        return Ok(pane.to_string());
    }
    let local = fleet.local_path();
    let panes = list_panes().ok_or(SwitchError::PaneNotFound)?;
    let matches: Vec<String> = panes
        .into_iter()
        .filter(|(_, cwd)| Path::new(cwd) == local)
        .map(|(id, _)| id)
        .collect();
    if matches.len() == 1 {
        Ok(matches[0].clone())
    } else {
        Err(SwitchError::PaneNotFound)
    }
}

fn kill_incumbent(
    fleet: &Fleet,
    agents: &BTreeMap<String, AgentRecord>,
    inspector: &dyn PaneInspector,
    killer: &dyn ProcessKiller,
) -> Result<bool, SwitchError> {
    let Some(agent) = find_agent(agents, &fleet.orch) else {
        // An unregistered orchestrator is never killed (auto-discovered pane
        // identity is too weak); `agent start` fails loudly on an occupied
        // pane if the operator picked one.
        return Ok(false);
    };
    let Some(pane_id) = agent.pane_id.as_deref() else {
        return Err(SwitchError::PaneIdentityFailed {
            name: agent.name.clone(),
            detail: "registered agent has no pane_id".to_string(),
        });
    };
    let info = inspector
        .pane_info(pane_id)
        .ok_or_else(|| SwitchError::PaneIdentityFailed {
            name: agent.name.clone(),
            detail: "pane process info unavailable".to_string(),
        })?;
    let (pgid, pids) =
        verified_victim(&info, agent).ok_or_else(|| SwitchError::PaneIdentityFailed {
            name: agent.name.clone(),
            detail: "no allowlisted foreground process with the recorded cwd".to_string(),
        })?;
    killer
        .signal("TERM", pgid)
        .map_err(|detail| SwitchError::KillFailed {
            name: agent.name.clone(),
            detail,
        })?;
    thread::sleep(Duration::from_secs(2));
    if killer
        .any_pid_in_group(&pids, pgid)
        .map_err(|detail| SwitchError::KillFailed {
            name: agent.name.clone(),
            detail,
        })?
    {
        killer
            .signal("KILL", pgid)
            .map_err(|detail| SwitchError::KillFailed {
                name: agent.name.clone(),
                detail,
            })?;
    }
    Ok(true)
}

fn wait_name_free(name: &str, timeout: Duration) -> Result<bool, SwitchError> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let agents = list_agents().ok_or(SwitchError::AgentListUnavailable)?;
        if !agents.contains_key(name) {
            return Ok(true);
        }
        thread::sleep(Duration::from_secs(1));
    }
    Ok(false)
}

fn start_args(kind: &str, model: &str) -> Result<(String, Vec<String>), SwitchError> {
    let prompt = "Read the fleet continuation brief and continue the fleet's work.";
    match kind {
        "codex" => {
            let model_id = model
                .split_once('/')
                .map(|(_, model)| model)
                .unwrap_or(model);
            let mut tail = vec!["-m".to_string(), model_id.to_string()];
            if model_id.contains("deepseek") {
                tail.push("-c".to_string());
                tail.push("model_provider=deepseek".to_string());
            }
            tail.push("--dangerously-bypass-approvals-and-sandbox".to_string());
            tail.push(prompt.to_string());
            Ok(("codex".to_string(), tail))
        }
        "opencode" => Ok((
            "opencode".to_string(),
            vec![
                "--model".to_string(),
                model.to_string(),
                "--prompt".to_string(),
                prompt.to_string(),
            ],
        )),
        "claude" => Ok((
            "claude".to_string(),
            vec![
                "--model".to_string(),
                model.to_string(),
                "--dangerously-skip-permissions".to_string(),
            ],
        )),
        _ => Err(SwitchError::ModelInvalid {
            detail: format!("unknown harness {kind:?}"),
        }),
    }
}

fn start_orchestrator(fleet: &Fleet, pane: &str, model: &str) -> Result<(), SwitchError> {
    let kind = kind_for(model).ok_or_else(|| SwitchError::ModelInvalid {
        detail: format!("unknown provider in {model:?}"),
    })?;
    let (kind_id, tail) = start_args(kind, model)?;
    let mut command = Command::new("herdr");
    command
        .arg("agent")
        .arg("start")
        .arg(&fleet.orch)
        .args([
            "--kind",
            &kind_id,
            "--pane",
            pane,
            "--timeout",
            "90000",
            "--",
        ])
        .args(&tail)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = command.output().map_err(|e| SwitchError::StartFailed {
        detail: format!("spawn herdr: {e}"),
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(SwitchError::StartFailed {
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

/// The complete switch operation for one fleet. The registry is loaded but
/// never rewritten here; the fleet's `paused` state is untouched.
pub fn switch_fleet(
    path: &Path,
    name: &str,
    pane: Option<&str>,
    auth: &dyn AuthChecker,
) -> Result<(), SwitchError> {
    let registry = load(path).map_err(registry_error)?;
    let fleet = registry
        .fleets
        .iter()
        .find(|fleet| fleet.name == name)
        .cloned()
        .ok_or_else(|| SwitchError::FleetNotFound {
            name: name.to_string(),
        })?;
    let kinds = fleet_kinds(&fleet).map_err(|detail| SwitchError::ModelInvalid { detail })?;
    for kind in &kinds {
        match auth.authenticated(kind) {
            Some(true) => {}
            _ => {
                return Err(SwitchError::AuthFailed { kind: kind.clone() });
            }
        }
    }

    let agents = list_agents().ok_or(SwitchError::AgentListUnavailable)?;
    let inspector = HerdrPaneInspector;
    let killer = SystemKiller;
    let killed = kill_incumbent(&fleet, &agents, &inspector, &killer)?;
    if killed && !wait_name_free(&fleet.orch, Duration::from_secs(25))? {
        return Err(SwitchError::StartFailed {
            detail: format!(
                "old {} registration is still present after the group was killed",
                fleet.orch
            ),
        });
    }
    let pane = find_pane(&fleet, pane)?;
    start_orchestrator(&fleet, &pane, &fleet.models.orch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::config::Models;
    use crate::fleet::reap::{PaneInfo, ProcessInfo};

    fn fleet() -> Fleet {
        Fleet {
            name: "corral".to_string(),
            gh_repo: "jirathip-k/corral".to_string(),
            local: "/repos/corral".to_string(),
            worktree_dir: "corral".to_string(),
            orch: "orch-corral".to_string(),
            workers: vec![],
            paused: true,
            models: Models {
                orch: "codex/gpt-5.6".to_string(),
                impl_: "opencode-go/deepseek-v4-flash".to_string(),
                review: "opus".to_string(),
                impl_alt: None,
                impl_alt2: None,
            },
        }
    }

    #[test]
    fn model_kinds_match_harness_mapping() {
        assert_eq!(kind_for("opus"), Some("claude"));
        assert_eq!(kind_for("codex/gpt-5.6"), Some("codex"));
        assert_eq!(kind_for("opencode-go/deepseek-v4-flash"), Some("opencode"));
        assert_eq!(kind_for("anthropic/claude-opus-4"), None);
        assert_eq!(
            fleet_kinds(&fleet()).expect("all roles map"),
            vec!["claude", "codex", "opencode"]
        );
    }

    #[test]
    fn unknown_qualified_provider_is_refused_for_every_role() {
        let mut fleet = fleet();
        fleet.models.impl_ = "unknown/vendor-model".to_string();
        let err = fleet_kinds(&fleet).expect_err("unknown provider must refuse");
        assert!(err.contains("impl"));
        assert!(err.contains("unknown/vendor-model"));
    }

    #[test]
    fn parse_panes_requires_stable_results_shape() {
        let panes = parse_panes(
            r#"{ "result": { "panes": [
                { "pane_id": "wM:p1", "cwd": "/repos/corral" },
                { "pane_id": "wM:p2", "cwd": "/repos/other" }
            ] } }"#,
        )
        .expect("pane list parses");
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].0, "wM:p1");
        assert_eq!(
            parse_panes(r#"{ "result": { "panes": [] } }"#),
            Some(vec![])
        );
        assert_eq!(parse_panes("not json"), None);
    }

    #[test]
    fn start_args_match_legacy_backend_mapping() {
        let (kind, tail) =
            start_args("codex", "codex/deepseek-v4-flash").expect("codex args are valid");
        assert_eq!(kind, "codex");
        assert!(tail.starts_with(&["-m".to_string(), "deepseek-v4-flash".to_string()]));
        assert!(
            tail.windows(2)
                .any(|w| w == ["-c", "model_provider=deepseek"])
        );
        assert!(tail.iter().any(|arg| arg.contains("dangerously-bypass")));

        let (kind, tail) =
            start_args("opencode", "opencode-go/deepseek-v4-flash").expect("opencode args");
        assert_eq!(kind, "opencode");
        assert!(
            tail.iter()
                .any(|arg| arg == "opencode-go/deepseek-v4-flash")
        );

        let (kind, tail) = start_args("claude", "opus").expect("claude args");
        assert_eq!(kind, "claude");
        assert!(tail.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(!tail.iter().any(|arg| arg.starts_with("Read ")));
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
        present: std::cell::Cell<bool>,
    }

    impl ProcessKiller for FakeKiller {
        fn signal(&self, signal: &str, pgid: i64) -> Result<(), String> {
            self.calls.borrow_mut().push((signal.to_string(), pgid));
            Ok(())
        }

        fn any_pid_in_group(&self, _pids: &[i64], _pgid: i64) -> Result<bool, String> {
            Ok(self.present.get())
        }
    }

    #[test]
    fn incumbent_kill_uses_verified_group_and_guards_kill_escalation() {
        let fleet = fleet();
        let mut agents = BTreeMap::new();
        agents.insert(
            fleet.orch.clone(),
            AgentRecord {
                name: fleet.orch.clone(),
                status: "working".to_string(),
                cwd: "/repos/corral".to_string(),
                pane_id: Some("p1".to_string()),
                revision: Some(1),
                state_change_seq: Some(2),
            },
        );
        let mut panes = BTreeMap::new();
        panes.insert(
            "p1".to_string(),
            PaneInfo {
                foreground_process_group_id: 100,
                shell_pid: 1,
                foreground_processes: vec![ProcessInfo {
                    pid: 200,
                    argv0: "/bin/opencode".to_string(),
                    cwd: "/repos/corral".to_string(),
                }],
            },
        );
        let killer = FakeKiller {
            present: std::cell::Cell::new(true),
            ..FakeKiller::default()
        };
        let killed =
            kill_incumbent(&fleet, &agents, &FakeInspector(panes), &killer).expect("kill succeeds");
        assert!(killed);
        assert_eq!(
            *killer.calls.borrow(),
            vec![("TERM".to_string(), 100), ("KILL".to_string(), 100)]
        );
    }

    #[test]
    fn registry_is_never_rewritten_by_switch_api() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("fleets.json");
        let body = r#"{"fleets":[{"name":"corral","gh_repo":"jirathip-k/corral",
            "local":"/repos/corral","worktree_dir":"corral","orch":"orch-corral",
            "workers":[],"paused":true,
            "models":{"orch":"codex/gpt-5.6","impl":"opencode-go/deepseek-v4-flash","review":"opus"}}]}"#;
        std::fs::write(&path, body).expect("write registry");
        struct RejectAuth;
        impl AuthChecker for RejectAuth {
            fn authenticated(&self, _kind: &str) -> Option<bool> {
                None
            }
        }
        let err = switch_fleet(&path, "corral", None, &RejectAuth)
            .expect_err("unavailable auth must block");
        assert!(matches!(err, SwitchError::AuthFailed { .. }));
        assert_eq!(
            std::fs::read_to_string(&path).expect("registry unchanged"),
            body
        );
    }
}
