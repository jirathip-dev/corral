//! #35 slice 1: registry-WRITING operations — `fleet add` / `fleet remove`.
//!
//! These are the first commands that mutate the registry, so the discipline
//! here is the load-bearing part:
//!
//! - **Repo resolves before add**: `fleet add` shells out to
//!   `gh repo view <owner/repo> --json nameWithOwner` and REFUSES (writing
//!   nothing) on any non-zero exit — including `gh` being absent.
//! - **Validate before write**: the candidate registry (existing fleets + the
//!   new one, or the existing fleets minus the removed one) is validated with
//!   the same [`Registry::validate`] `load()` uses before anything is written.
//! - **Atomic write**: [`config::write_atomic`] replaces the file via
//!   temp-file-in-same-dir + rename, so a refused or failed operation leaves
//!   the original byte-identical.

use std::path::Path;

use crate::fleet::config::{ConfigError, Fleet, Models, load, write_atomic};

/// Everything `fleet add` needs to build the new entry.
pub struct AddOptions {
    pub name: String,
    pub gh_repo: String,
    pub local: Option<String>,
    pub worktree_dir: Option<String>,
    pub orch: Option<String>,
    pub workers: Vec<String>,
    pub models: Option<Models>,
}

/// Where the defaults for a newly added fleet are resolved from: the layout
/// the live registry already encodes (`local` -> `~/Projects/<name>`,
/// `worktree_dir` -> `<name>`, `orch` -> `orch-<name>`, `workers` -> empty).
/// `models` inherits from an existing fleet if the registry has one, else the
/// caller must supply `--models` (see [`AddOptions::models`]).
pub struct Defaults {
    pub local: String,
    pub worktree_dir: String,
    pub orch: String,
    pub workers: Vec<String>,
}

impl Defaults {
    pub fn for_name(name: &str) -> Self {
        Self {
            local: format!("~/Projects/{name}"),
            worktree_dir: name.to_string(),
            orch: format!("orch-{name}"),
            workers: Vec::new(),
        }
    }
}

/// Resolve `gh repo view <repo> --json nameWithOwner` by shelling out to the
/// `gh` CLI. The exact invocation is injectable so tests can stub it without
/// network access.
pub trait RepoResolver {
    fn repo_resolves(&self, repo: &str) -> bool;
}

/// The production resolver: runs the real `gh` CLI. Any non-zero exit —
/// including a missing `gh` binary — means "does not resolve".
pub struct GhCli;

impl RepoResolver for GhCli {
    fn repo_resolves(&self, repo: &str) -> bool {
        std::process::Command::new("gh")
            .args(["repo", "view", repo, "--json", "nameWithOwner"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

/// `corrald fleet add`: validate + write the candidate registry. Refuses
/// (writing nothing) if `--gh` does not resolve, the name already exists, or
/// the candidate registry fails validation.
pub fn add(
    path: &Path,
    opts: &AddOptions,
    resolver: &dyn RepoResolver,
) -> Result<Fleet, ConfigError> {
    let mut registry = load(path)?;

    if registry.fleets.iter().any(|f| f.name == opts.name) {
        return Err(ConfigError::DuplicateFleet {
            name: opts.name.clone(),
        });
    }
    if !resolver.repo_resolves(&opts.gh_repo) {
        return Err(ConfigError::AddRepoUnresolved {
            repo: opts.gh_repo.clone(),
        });
    }

    let defaults = Defaults::for_name(&opts.name);
    let models = match &opts.models {
        Some(models) => models.clone(),
        None => registry
            .fleets
            .iter()
            .map(|f| &f.models)
            .next()
            .cloned()
            .ok_or(ConfigError::Empty {
                fleet: "new fleet".to_string(),
                field: "models".to_string(),
            })?,
    };
    let fleet = Fleet {
        name: opts.name.clone(),
        gh_repo: opts.gh_repo.clone(),
        local: opts.local.clone().unwrap_or_else(|| defaults.local.clone()),
        worktree_dir: opts
            .worktree_dir
            .clone()
            .unwrap_or_else(|| defaults.worktree_dir.clone()),
        orch: opts.orch.clone().unwrap_or_else(|| defaults.orch.clone()),
        workers: opts.workers.clone(),
        paused: false,
        models,
    };

    registry.fleets.push(fleet.clone());
    registry.validate()?;
    write_atomic(path, &registry)?;
    Ok(fleet)
}

/// `corrald fleet remove`: drop exactly one fleet by name, then validate and
/// write. An unknown name is a refusal that writes nothing.
pub fn remove(path: &Path, name: &str) -> Result<usize, ConfigError> {
    let mut registry = load(path)?;
    let before = registry.fleets.len();
    registry.fleets.retain(|f| f.name != name);
    if registry.fleets.len() == before {
        return Err(ConfigError::RemoveNotFound {
            name: name.to_string(),
        });
    }
    registry.validate()?;
    write_atomic(path, &registry)?;
    Ok(registry.fleets.len())
}
