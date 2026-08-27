//! #237: auth-gated per-role model switch, delegated to the fleet-ops CLI.
//!
//! `corrald fleet switch <name>` is the re-arm half of pause/resume. Since
//! #237 Corral is configless: the fleet registry (models, harnesses,
//! reasoning efforts, paused) is fleet-ops' opinionated config, and corrald
//! deos not read it. The whole switch therefore delegates to the fleet-ops
//! CLI validated identity path — `herdr-fleet switch` — which is
//! lanes-aware (hermes profile in the fleet re-arm brief) and validates the
//! fleet identity, the harness its orch model implies, and the auth gates
//! itself. corrald only adapts the CLI exit contract.
//!
//! Display repo categories are never actionable identities; the only fleet
//! identity this command accepts is the CLI-validated name the fleet-ops
//! registry owns.

use std::fmt;
use std::process::Command;

use crate::fleet::cli::fleet_ops_command;

#[derive(Debug)]
pub enum SwitchError {
    /// The fleet-ops CLI could not be run at all.
    Unavailable { detail: String },
    /// The fleet-ops CLI reported the fleet is not in its registry.
    UnknownFleet { name: String },
    /// The fleet-ops CLI refused or failed the switch (auth, pane, model).
    Refused { detail: String },
}

impl fmt::Display for SwitchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { detail } => {
                write!(f, "fleet-ops CLI switch path unavailable: {detail}")
            }
            Self::UnknownFleet { name } => {
                write!(f, "fleet {name:?} is not in the fleet-ops registry")
            }
            Self::Refused { detail } => f.write_str(detail),
        }
    }
}

impl std::error::Error for SwitchError {}

impl SwitchError {
    /// CLI exit contract: 1 for every operational refusal/failure (the
    /// fleet-ops CLI owns the registry-parse exit codes; a corrald-side
    /// delegation maps them all to 1, the legacy `switch` failure code).
    pub fn exit_code(&self) -> i32 {
        1
    }
}

/// Build the fleet-ops CLI invocation for one switch.
/// Exposed for tests: `--pane <id>` passes through; otherwise no extra args.
pub fn switch_command(command: &str, name: &str, pane: Option<&str>) -> Vec<String> {
    let mut args = vec!["switch".to_string(), name.to_string()];
    if let Some(pane) = pane {
        args.push("--pane".to_string());
        args.push(pane.to_string());
    }
    let _ = command;
    args
}

/// Delegate `corrald fleet switch <name>` to the fleet-ops CLI. The CLI's
/// stdout/stderr are streamed to the operator verbatim (the CLI is the
/// authority on auth/pane/model diagnostics) and its exit code is mapped.
pub fn switch_fleet(name: &str, pane: Option<&str>) -> Result<(), SwitchError> {
    let command = fleet_ops_command();
    let args = switch_command(&command, name, pane);
    let mut invocation = Command::new(&command);
    invocation
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    let status = invocation
        .status()
        .map_err(|error| SwitchError::Unavailable {
            detail: format!("spawn {command}: {error}"),
        })?;
    if status.success() {
        return Ok(());
    }
    // The fleet-ops CLI is the identity authority: a missing registry entry
    // is its typical non-zero failure. We only know the exit code and the
    // name here (its diagnostics streamed above), so unknown-fleet keeps the
    // same wording as the CLI: preserve exit 1 and a concise refusal.
    Err(SwitchError::Refused {
        detail: format!(
            "{command} switch {name} failed (exit {status}); the fleet is NOT re-armed"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_args_pass_name_and_optional_pane() {
        assert_eq!(
            switch_command("herdr-fleet", "corral", None),
            vec!["switch".to_string(), "corral".to_string()]
        );
        assert_eq!(
            switch_command("herdr-fleet", "corral", Some("wM:p1")),
            vec![
                "switch".to_string(),
                "corral".to_string(),
                "--pane".to_string(),
                "wM:p1".to_string()
            ]
        );
    }

    #[test]
    fn all_delegation_failures_exit_one() {
        assert_eq!(
            SwitchError::Unavailable {
                detail: "boom".to_string()
            }
            .exit_code(),
            1
        );
        assert_eq!(
            SwitchError::Refused {
                detail: "boom".to_string()
            }
            .exit_code(),
            1
        );
        assert_eq!(
            SwitchError::UnknownFleet {
                name: "x".to_string()
            }
            .exit_code(),
            1
        );
    }

    #[test]
    fn unknown_fleet_message_names_the_fleet() {
        let error = SwitchError::UnknownFleet {
            name: "nope".to_string(),
        };
        assert!(error.to_string().contains("nope"));
    }
}
