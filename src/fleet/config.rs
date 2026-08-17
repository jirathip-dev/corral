//! #35 phase 1: fleet registry config — parse, validate, default path, write.
//!
//! The registry is the `fleets.json` file that the separate fleet tooling
//! already treats as its single source of truth; corrald adopts that format
//! unchanged. `load()` parses and validates, and validation fails loudly
//! (hard error, not silent acceptance) on unknown fields anywhere, empty
//! required fields, whitespace inside `name`/`gh_repo`, a `gh_repo` that is
//! not a single `owner/repo`, a `local` that begins with a bare `~`, and
//! duplicate fleet names. The write side (`write_atomic`) is the first
//! registry-WRITING surface (#35 slice 1): it serialises back to the same
//! schema and replaces the file via temp-file-in-same-dir + rename, so a
//! refused or failed write leaves the original byte-identical.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A whole fleet registry file: `{ "fleets": [...] }`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub fleets: Vec<Fleet>,
}

/// One fleet entry. Field rules (from #35):
/// - `name`, `gh_repo`, `local`, `worktree_dir`, `orch` — required, non-empty.
/// - `workers` — required array of strings; may be empty.
/// - `models` — required object with required string keys.
/// - `paused` — optional, defaults to `false`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fleet {
    pub name: String,
    pub gh_repo: String,
    /// Local checkout; may start with `~/` (expanded via [`Fleet::local_path`]).
    pub local: String,
    pub worktree_dir: String,
    pub orch: String,
    pub workers: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub paused: bool,
    pub models: Models,
}

/// Per-role model map. JSON key `impl` (a Rust keyword) maps to [`Self::impl_`]
/// in both directions: serde's `rename` round-trips `impl_` back to the JSON
/// key `impl` on write, so a written registry is byte-compatible with what
/// `load()` parses.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Models {
    pub orch: String,
    #[serde(rename = "impl")]
    pub impl_: String,
    pub review: String,
}

/// `paused` is `#[serde(default)]` and intentionally SKIPPED when false, so a
/// freshly added fleet (and any fleet a `load()` -> write round-trip touches)
/// omits the field exactly as the existing live registries do. `true` is
/// always written. This keeps `deny_unknown_fields`/`default` symmetric and
/// the diff minimal; documented here because the brief asked for the decision
/// to be stated explicitly, not buried in a derive.
fn is_false(value: &bool) -> bool {
    !*value
}

impl Registry {
    /// Field-level validation beyond what serde enforces: the required
    /// strings (including `models.*` and every `workers` entry) must be
    /// non-empty, `name`/`gh_repo` must be free of internal whitespace,
    /// `gh_repo` must be a single `owner/repo`, `local` must not begin with a
    /// bare `~`, and fleet names must be unique. The write path
    /// (`fleet::ops`) runs this on the candidate registry BEFORE any file is
    /// touched, so a refusal leaves the original byte-identical.
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        let mut seen: Vec<&str> = Vec::new();
        for (index, fleet) in self.fleets.iter().enumerate() {
            for (field, value) in [
                ("name", &fleet.name),
                ("gh_repo", &fleet.gh_repo),
                ("local", &fleet.local),
                ("worktree_dir", &fleet.worktree_dir),
                ("orch", &fleet.orch),
                ("models.orch", &fleet.models.orch),
                ("models.impl", &fleet.models.impl_),
                ("models.review", &fleet.models.review),
            ] {
                if value.trim().is_empty() {
                    return Err(ConfigError::Empty {
                        fleet: fleet_locator(index, fleet, field),
                        field: field.to_string(),
                    });
                }
            }
            for (worker_index, worker) in fleet.workers.iter().enumerate() {
                if worker.trim().is_empty() {
                    return Err(ConfigError::Empty {
                        fleet: fleet_locator(index, fleet, "workers"),
                        field: format!("workers[{worker_index}]"),
                    });
                }
            }
            for (field, value) in [("name", &fleet.name), ("gh_repo", &fleet.gh_repo)] {
                if value.chars().any(char::is_whitespace) {
                    return Err(ConfigError::Whitespace {
                        fleet: fleet_locator(index, fleet, field),
                        field: field.to_string(),
                    });
                }
            }
            if !is_repo_slug(&fleet.gh_repo) {
                return Err(ConfigError::GhRepoShape {
                    fleet: fleet_locator(index, fleet, "gh_repo"),
                    value: fleet.gh_repo.clone(),
                });
            }
            if fleet.local.starts_with('~') && !fleet.local.starts_with("~/") {
                return Err(ConfigError::BadTilde {
                    fleet: fleet_locator(index, fleet, "local"),
                    value: fleet.local.clone(),
                });
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

/// A human-locatable label for a fleet, e.g. `#2 (gh_repo "x/y")`. When the
/// offending field is `name` itself the name is unusable as a locator, so it
/// falls back to `gh_repo`.
fn fleet_locator(index: usize, fleet: &Fleet, field: &str) -> String {
    if field == "name" {
        format!("#{} (gh_repo {:?})", index + 1, fleet.gh_repo)
    } else {
        format!("#{} (name {:?})", index + 1, fleet.name)
    }
}

/// A single `owner/repo` slug: exactly one `/`, both halves non-empty.
/// (Internal whitespace is rejected separately.)
fn is_repo_slug(slug: &str) -> bool {
    let (owner, repo) = slug.split_once('/').unwrap_or(("", ""));
    !owner.is_empty() && !repo.is_empty() && !repo.contains('/')
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

/// Atomically replace `path` with `registry` serialised back to the registry
/// schema.
///
/// Write discipline (#35 slice 1): a temp file in the SAME directory as the
/// target (so the rename is same-filesystem), written, `sync_all()`-ed, then
/// renamed over the target. The original is never truncated in place — on any
/// error the original file is left byte-identical. Serialisation uses
/// `serde_json::to_string_pretty` plus a trailing newline so the registry
/// stays diffable in git. This is the first WRITE surface for `Fleet` /
/// `Models` / `Registry`; the round-trip is exact: `Models::impl_` writes back
/// as the JSON key `impl`, and `paused` is skipped when false (see
/// [`is_false`]).
pub fn write_atomic(path: &Path, registry: &Registry) -> Result<(), ConfigError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let bytes = serialize(path, registry)?;
    let temp_path = parent.join(format!(
        ".{}.corrald-tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("fleets")
    ));
    {
        let mut file = std::fs::File::create(&temp_path).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
        file.write_all(&bytes)
            .map_err(|source| ConfigError::Write {
                path: path.to_path_buf(),
                source,
            })?;
        file.sync_all().map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }
    std::fs::rename(&temp_path, path).map_err(|source| {
        // The rename is the atomic commit point. If it fails the temp file is
        // cleaned up and the original is untouched.
        let _ = std::fs::remove_file(&temp_path);
        ConfigError::Write {
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok(())
}

/// `serde_json::to_string_pretty` + a trailing newline, so a written registry
/// stays diffable in git. Serialisation itself is fallible only on non-UTF-8
/// (this schema is all-strings), so errors are surfaced rather than written
/// off as impossible.
fn serialize(path: &Path, registry: &Registry) -> Result<Vec<u8>, ConfigError> {
    let mut json = serde_json::to_string_pretty(registry).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source: std::io::Error::other(source),
    })?;
    json.push('\n');
    Ok(json.into_bytes())
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
    /// A required string field is empty. `fleet` is a locator (index plus a
    /// usable identifier) rather than the fleet name, which may itself be the
    /// empty offender.
    Empty { fleet: String, field: String },
    /// `name` or `gh_repo` contains internal whitespace, which would corrupt
    /// the whitespace-delimited `fleet list` output contract.
    Whitespace { fleet: String, field: String },
    /// `gh_repo` is not a single `owner/repo` slug.
    GhRepoShape { fleet: String, value: String },
    /// `local` begins with a bare `~` (not `~/`), which would be passed
    /// through literally instead of expanded against `$HOME`.
    BadTilde { fleet: String, value: String },
    /// Two fleets share a `name`.
    DuplicateFleet { name: String },
    /// A `fleet add` `--gh <owner/repo>` could not be resolved upstream.
    /// Non-zero exit from `gh repo view` — including `gh` being absent — is a
    /// refusal, never a silent skip (#35's "repo resolves before add").
    AddRepoUnresolved { repo: String },
    /// A `fleet remove <name>` named a fleet that is not in the registry.
    RemoveNotFound { name: String },
    /// The registry file could not be replaced atomically.
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
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
                    "fleet {fleet}: field {field:?} must be a non-empty string"
                )
            }
            ConfigError::Whitespace { fleet, field } => {
                write!(
                    f,
                    "fleet {fleet}: field {field:?} must contain no whitespace"
                )
            }
            ConfigError::GhRepoShape { fleet, value } => {
                write!(
                    f,
                    "fleet {fleet}: gh_repo {value:?} must be a single owner/repo"
                )
            }
            ConfigError::BadTilde { fleet, value } => {
                write!(
                    f,
                    "fleet {fleet}: local {value:?} must start with ~/ to be tilde-expanded"
                )
            }
            ConfigError::DuplicateFleet { name } => {
                write!(f, "duplicate fleet name {name:?}")
            }
            ConfigError::AddRepoUnresolved { repo } => {
                write!(
                    f,
                    "cannot add fleet: repo {repo:?} did not resolve via `gh repo view`"
                )
            }
            ConfigError::RemoveNotFound { name } => {
                write!(f, "no fleet named {name:?} in the registry")
            }
            ConfigError::Write { path, source } => {
                write!(
                    f,
                    "cannot write fleet registry {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } => Some(source),
            ConfigError::Parse { source, .. } => Some(source),
            ConfigError::Write { source, .. } => Some(source),
            ConfigError::Empty { .. }
            | ConfigError::Whitespace { .. }
            | ConfigError::GhRepoShape { .. }
            | ConfigError::BadTilde { .. }
            | ConfigError::DuplicateFleet { .. }
            | ConfigError::AddRepoUnresolved { .. }
            | ConfigError::RemoveNotFound { .. } => None,
        }
    }
}
