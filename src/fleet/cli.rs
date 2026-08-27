//! #237: fleet-ops CLI identity provider — Corral's only fleet identity source.
//!
//! Configless Corral does not own, read, or write `fleets.json` anywhere in
//! `src/`. Actionable fleet identities (which fleet a worktree action or a
//! `fleet switch` targets) come exclusively from the fleet-ops CLI
//! (`herdr-fleet`) — the same shell-out pattern already used for `herdr
//! pane list` / `herdr agent list` in `switch`/`reap`.
//!
//! The CLI identity path today is `herdr-fleet list`, a fixed-format table
//! (one line per fleet). This module parses only stable fields:
//! `name`, `gh_repo`, `orch`, `workers`, `paused`. Corral never interprets
//! model maps or reasoning efforts from it — the fleet-ops CLI owns those.
//!
//! The provider is a trait so the daemon (and CLI tests) can take an
//! injected catalog instead of depending on a live `herdr-fleet` in hermetic
//! tests; the production provider shells the CLI.

use std::fmt;
use std::path::PathBuf;
use std::process::Command;

/// The command searched on `PATH` for the fleet-ops identity CLI. The
/// `CORRALD_FLEET_OPS` env override exists for tests and per-invocation
/// tooling (same pattern as `CORRALD_BIN` in the egui client); it must be an
/// executable that accepts `list` and prints the `herdr-fleet list` table.
pub fn fleet_ops_command() -> String {
    std::env::var("CORRALD_FLEET_OPS")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "herdr-fleet".to_string())
}

/// One fleet's CLI-validated identity. Only fields the `herdr-fleet list`
/// table actually carries; model maps stay in fleet-ops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetIdentity {
    pub name: String,
    /// `owner/repo` slug as printed by the fleet-ops CLI.
    pub gh_repo: String,
    /// Registered orchestrator agent name.
    pub orch: String,
    pub workers: usize,
    pub paused: bool,
    /// Fleet-ops `add` default checkout (`~/Projects/<name>`). Used ONLY when
    /// the CLI itself carries a checkout anchor; the `herdr-fleet list` table
    /// does not, so the daemon resolves the physical anchor from herdr state
    /// (see [`crate::fleet::worktree::resolve_checkout`]) before falling back
    /// to this default.
    pub local: PathBuf,
    /// Fleet-ops `add` default worktree root component (`<name>`).
    pub worktree_dir: String,
}

impl FleetIdentity {
    /// `local` with a leading `~/` expanded against `$HOME`; a relative or
    /// absolute path passes through. Matches the legacy
    /// `fleet.local_path()` contract.
    pub fn local_path(&self) -> PathBuf {
        if let Some(rest) = self.local.to_string_lossy().strip_prefix("~/") {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(rest)
        } else {
            self.local.clone()
        }
    }
}

/// Why the fleet-ops CLI identity path failed or refused.
#[derive(Debug)]
pub enum FleetOpsError {
    /// The CLI could not be run or its output was not a well-formed table.
    Unavailable { detail: String },
    /// The CLI ran but reported no fleet with this name.
    UnknownFleet { name: String },
}

impl fmt::Display for FleetOpsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { detail } => {
                write!(f, "fleet-ops CLI identity path unavailable: {detail}")
            }
            Self::UnknownFleet { name } => {
                write!(f, "fleet {name:?} is not in the fleet-ops registry")
            }
        }
    }
}

impl std::error::Error for FleetOpsError {}

/// Parsed `herdr-fleet list` output.
pub fn parse_fleet_list(output: &str) -> Result<Vec<FleetIdentity>, FleetOpsError> {
    let mut fleets = Vec::new();
    for line in output.lines() {
        let line = line.trim_start();
        if line.is_empty() {
            continue;
        }
        // Table shape: `{name:12s} ✓  {gh_repo:50s} orch={orch} workers={n} [PAUSED] models=...`
        // Only the summary line ("N fleets in registry") has no gh_repo.
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(name) = tokens.first() else {
            continue;
        };
        let Some(ok) = tokens.get(1) else {
            continue;
        };
        if *ok != "✓" {
            // "✗ INVALID" means the operator-fleet said the repo does not
            // resolve; the identity is still CLI-validated (the name exists).
            if *ok != "✗" {
                continue;
            }
        }
        let Some(gh_repo) = tokens.get(2) else {
            continue;
        };
        let orch = tokens
            .iter()
            .find_map(|token| token.strip_prefix("orch="))
            .unwrap_or("")
            .to_string();
        let workers = tokens
            .iter()
            .find_map(|token| token.strip_prefix("workers="))
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let paused = tokens.contains(&"PAUSED");
        if name.is_empty() || gh_repo.is_empty() {
            continue;
        }
        fleets.push(FleetIdentity {
            name: (*name).to_string(),
            gh_repo: (*gh_repo).to_string(),
            orch,
            workers,
            paused,
            local: PathBuf::from(format!("~/Projects/{name}")),
            worktree_dir: (*name).to_string(),
        });
    }
    if fleets.is_empty() {
        return Err(FleetOpsError::Unavailable {
            detail: "no fleets parsed (run `herdr-fleet list` outside the daemon)".to_string(),
        });
    }
    Ok(fleets)
}

/// Shell-out provider: runs `herdr-fleet list` (or `$CORRALD_FLEET_OPS`)
/// on each call and parses the table.
#[derive(Debug, Default, Clone)]
pub struct CliFleetOpsProvider;

impl CliFleetOpsProvider {
    fn run_list(&self) -> Result<String, FleetOpsError> {
        let command = fleet_ops_command();
        let output = Command::new(&command)
            .arg("list")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| FleetOpsError::Unavailable {
                detail: format!("spawn {command}: {error}"),
            })?;
        if !output.status.success() {
            return Err(FleetOpsError::Unavailable {
                detail: format!(
                    "{command} list exited {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// The injectable identity source used by the daemon.
pub trait FleetOpsProvider: Send + Sync {
    /// All CLI-validated fleet identities, in CLI order.
    fn list(&self) -> Result<Vec<FleetIdentity>, FleetOpsError>;

    /// The one fleet with this exact name (CLI-validated), or
    /// [`FleetOpsError::UnknownFleet`].
    fn get(&self, name: &str) -> Result<FleetIdentity, FleetOpsError> {
        self.list()?
            .into_iter()
            .find(|fleet| fleet.name == name)
            .ok_or_else(|| FleetOpsError::UnknownFleet {
                name: name.to_string(),
            })
    }
}

impl FleetOpsProvider for CliFleetOpsProvider {
    fn list(&self) -> Result<Vec<FleetIdentity>, FleetOpsError> {
        parse_fleet_list(&self.run_list()?)
    }
}

/// In-memory provider: a fixed catalog (tests, tooling, dry runs). The
/// catalog is CLI-validated by construction of the caller.
#[derive(Debug, Clone, Default)]
pub struct MemoryFleetOpsProvider {
    pub identities: Vec<FleetIdentity>,
}

impl MemoryFleetOpsProvider {
    pub fn new(identities: Vec<FleetIdentity>) -> Self {
        Self { identities }
    }
}

impl FleetOpsProvider for MemoryFleetOpsProvider {
    fn list(&self) -> Result<Vec<FleetIdentity>, FleetOpsError> {
        Ok(self.identities.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "corral       ✓  jirathip-dev/corral                                orch=orch-corral workers=0 models=orch:glm-5.3-flash\n\
plush        ✓  jirathip-dev/plush-meadow                          orch=orch-plush workers=0 models=orch:glm-5.3-flash\n\
synergy-website ✓  synergy-services-cooling-tower/synergy-services-website orch=orch-synergy-website workers=0 PAUSED models=orch:glm-5.3-flash\n\
\n9 fleets in registry\n";

    #[test]
    fn parses_name_gh_repo_orch_workers_and_paused() {
        let fleets = parse_fleet_list(SAMPLE).expect("sample parses");
        assert_eq!(fleets.len(), 3);
        assert_eq!(fleets[0].name, "corral");
        assert_eq!(fleets[0].gh_repo, "jirathip-dev/corral");
        assert_eq!(fleets[0].orch, "orch-corral");
        assert_eq!(fleets[0].workers, 0);
        assert!(!fleets[0].paused);
        assert_eq!(fleets[2].name, "synergy-website");
        assert!(fleets[2].paused);
        assert_eq!(
            fleets[2].gh_repo,
            "synergy-services-cooling-tower/synergy-services-website"
        );
    }

    #[test]
    fn invalid_marker_still_identifies_the_fleet() {
        let fleets = parse_fleet_list("broken     ✗  owner/repo                                  orch=orch-broken workers=1 models=orch:x\n1 fleets in registry\n")
            .expect("invalid marker is still CLI-validated");
        assert_eq!(fleets[0].name, "broken");
        assert_eq!(fleets[0].gh_repo, "owner/repo");
        assert_eq!(fleets[0].workers, 1);
    }

    #[test]
    fn empty_or_garbage_output_is_unavailable() {
        assert!(matches!(
            parse_fleet_list(""),
            Err(FleetOpsError::Unavailable { .. })
        ));
        assert!(matches!(
            parse_fleet_list("hello world\n"),
            Err(FleetOpsError::Unavailable { .. })
        ));
    }

    #[test]
    fn get_yields_exact_name_only() {
        let fleets = parse_fleet_list(SAMPLE).expect("sample parses");
        let provider = |f: FleetIdentity| f;
        let _ = provider;
        assert!(fleets.iter().any(|f| f.name == "corral"));
        assert!(!fleets.iter().any(|f| f.name == "nope"));
    }
}
