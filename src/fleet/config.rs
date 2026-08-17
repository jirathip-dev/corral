//! #35 phase 1: fleet registry config — parse, validate, default path.
//!
//! The registry is the `fleets.json` file that the separate fleet tooling
//! already treats as its single source of truth; corrald adopts that format
//! unchanged. Everything in here is read-only: `load()` parses and validates,
//! and validation fails loudly (hard error, not silent acceptance) on unknown
//! fields anywhere, empty required fields, and duplicate fleet names.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// A whole fleet registry file: `{ "fleets": [...] }`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub fleets: Vec<Fleet>,
}

/// One fleet entry. Field rules (from #35):
/// - `name`, `gh_repo`, `local`, `worktree_dir`, `orch` — required, non-empty.
/// - `workers` — required array of strings; may be empty.
/// - `models` — required object with required string keys.
/// - `paused` — optional, defaults to `false`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fleet {
    pub name: String,
    pub gh_repo: String,
    /// Local checkout; may start with `~/` (expanded via [`Fleet::local_path`]).
    pub local: String,
    pub worktree_dir: String,
    pub orch: String,
    pub workers: Vec<String>,
    #[serde(default)]
    pub paused: bool,
    pub models: Models,
}

/// Per-role model map. JSON key `impl` (a Rust keyword) maps to [`Self::impl_`].
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Models {
    pub orch: String,
    #[serde(rename = "impl")]
    pub impl_: String,
    pub review: String,
}

impl Registry {
    /// Field-level validation beyond what serde enforces: the required
    /// strings must be non-empty and fleet names must be unique.
    fn validate(&self) -> Result<(), ConfigError> {
        let mut seen: Vec<&str> = Vec::new();
        for fleet in &self.fleets {
            for (field, value) in [
                ("name", &fleet.name),
                ("gh_repo", &fleet.gh_repo),
                ("local", &fleet.local),
                ("worktree_dir", &fleet.worktree_dir),
                ("orch", &fleet.orch),
            ] {
                if value.trim().is_empty() {
                    return Err(ConfigError::Empty {
                        fleet: fleet.name.clone(),
                        field,
                    });
                }
            }
            if seen.contains(&fleet.name.as_str()) {
                return Err(ConfigError::DuplicateFleet {
                    name: fleet.name.clone(),
                });
            }
            seen.push(&fleet.name);
        }
        Ok(())
    }
}

impl Fleet {
    /// `local` with a leading `~/` expanded against `$HOME`; anything else is
    /// used as-is. `$HOME` unset falls back to `.` (matching the daemon's
    /// other HOME-derived defaults).
    pub fn local_path(&self) -> PathBuf {
        if let Some(rest) = self.local.strip_prefix("~/") {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(rest)
        } else {
            PathBuf::from(&self.local)
        }
    }
}

/// The registry file to use when the CLI gets no `--registry`:
/// `$CORRAL_FLEETS_PATH` if set, else `$HOME/.hermes/scripts/fleets.json`.
pub fn default_path() -> PathBuf {
    std::env::var("CORRAL_FLEETS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".hermes/scripts/fleets.json")
        })
}

/// Read + parse + validate a registry. Errors carry the offending fleet and
/// field where one exists, so an operator can locate the failure.
pub fn load(path: &Path) -> Result<Registry, ConfigError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let registry: Registry = serde_json::from_str(&raw).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    registry.validate()?;
    Ok(registry)
}

/// Why a registry failed to load or validate.
#[derive(Debug)]
pub enum ConfigError {
    /// The file could not be read.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The file is not valid registry JSON (missing required fields,
    /// unknown fields, type mismatches).
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    /// A required string field is empty.
    Empty { fleet: String, field: &'static str },
    /// Two fleets share a `name`.
    DuplicateFleet { name: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io { path, source } => {
                write!(f, "cannot read fleet registry {}: {source}", path.display())
            }
            ConfigError::Parse { path, source } => {
                write!(
                    f,
                    "cannot parse fleet registry {}: {source}",
                    path.display()
                )
            }
            ConfigError::Empty { fleet, field } => {
                write!(
                    f,
                    "fleet {fleet:?}: field {field:?} must be a non-empty string"
                )
            }
            ConfigError::DuplicateFleet { name } => {
                write!(f, "duplicate fleet name {name:?}")
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } => Some(source),
            ConfigError::Parse { source, .. } => Some(source),
            ConfigError::Empty { .. } | ConfigError::DuplicateFleet { .. } => None,
        }
    }
}
