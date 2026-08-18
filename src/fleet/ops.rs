//! #35 slices 1–2: registry-WRITING operations — `fleet add` / `remove`
//! (slice 1) and `pause` / `resume` / `models` (slice 2).
//!
//! These are the commands that mutate the registry, so the discipline here is
//! the load-bearing part:
//!
//! - **Repo resolves before add**: `fleet add` shells out to
//!   `gh repo view <owner/repo> --json nameWithOwner` and REFUSES (writing
//!   nothing) on any non-zero exit — including `gh` being absent.
//! - **Validate before write**: the candidate registry (existing fleets + the
//!   new one, or the existing fleets minus the removed one, or the existing
//!   fleets with `paused`/`models` mutated) is validated with the same
//!   [`Registry::validate`] `load()` uses before anything is written.
//! - **Atomic write**: [`config::write_atomic`] replaces the file via
//!   temp-file-in-same-dir + rename, so a refused or failed operation leaves
//!   the original byte-identical.
//! - **Idempotent pauses/resumes**: setting `paused` to the value it already
//!   has is a no-op SUCCESS ("already paused"/"not paused") — nothing is
//!   written, exit 0.

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
/// `models` inherits from the FIRST existing fleet if the registry has one
/// (array order), else the caller must supply `--models`
/// (see [`AddOptions::models`]).
pub struct Defaults {
    pub local: String,
    pub worktree_dir: String,
    pub orch: String,
}

impl Defaults {
    pub fn for_name(name: &str) -> Self {
        Self {
            local: format!("~/Projects/{name}"),
            worktree_dir: name.to_string(),
            orch: format!("orch-{name}"),
        }
    }
}

/// Resolve `gh repo view <repo> --json nameWithOwner` by shelling out to the
/// `gh` CLI. The exact invocation is injectable so tests can stub it without
/// network access. `Err` carries a one-line diagnostic (the first stderr
/// line from `gh`, or the spawn error) so an expired token is
/// distinguishable from a typo'd slug.
pub trait RepoResolver {
    fn repo_resolves(&self, repo: &str) -> Result<(), Option<String>>;
}

/// The production resolver: runs the real `gh` CLI. Any non-zero exit —
/// including a missing `gh` binary — means "does not resolve".
pub struct GhCli;

impl RepoResolver for GhCli {
    fn repo_resolves(&self, repo: &str) -> Result<(), Option<String>> {
        match std::process::Command::new("gh")
            .args(["repo", "view", repo, "--json", "nameWithOwner"])
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(stderr.lines().next().map(str::to_string))
            }
            Err(spawn) => Err(Some(format!("cannot run gh: {spawn}"))),
        }
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

    // `all` is the `fleet models` wildcard — refuse it as a name here,
    // before the resolver round-trip (validate() would also catch it).
    if opts.name == "all" {
        return Err(ConfigError::ReservedFleetName {
            name: opts.name.clone(),
        });
    }
    if registry.fleets.iter().any(|f| f.name == opts.name) {
        return Err(ConfigError::DuplicateFleet {
            name: opts.name.clone(),
        });
    }
    if let Err(detail) = resolver.repo_resolves(&opts.gh_repo) {
        return Err(ConfigError::AddRepoUnresolved {
            repo: opts.gh_repo.clone(),
            detail,
        });
    }

    let defaults = Defaults::for_name(&opts.name);
    let models = match &opts.models {
        Some(models) => models.clone(),
        None => registry
            .fleets
            .first()
            .map(|f| f.models.clone())
            .ok_or(ConfigError::AddNeedsModels)?,
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

/// `corrald fleet pause <name>`: set `paused` on exactly one fleet, then
/// validate and write. Pausing an already-paused fleet is a no-op SUCCESS
/// (`Ok(false)` — nothing written, exit 0). An unknown name is a refusal.
pub fn pause(path: &Path, name: &str) -> Result<bool, ConfigError> {
    let mut registry = load(path)?;
    let Some(fleet) = registry.fleets.iter_mut().find(|f| f.name == name) else {
        return Err(ConfigError::FleetNotFound {
            name: name.to_string(),
        });
    };
    if fleet.paused {
        return Ok(false);
    }
    fleet.paused = true;
    registry.validate()?;
    write_atomic(path, &registry)?;
    Ok(true)
}

/// `corrald fleet resume <name>`: clear `paused` on exactly one fleet, then
/// validate and write. Resuming an unpaused fleet is a no-op SUCCESS
/// (`Ok(false)` — nothing written, exit 0). An unknown name is a refusal.
pub fn resume(path: &Path, name: &str) -> Result<bool, ConfigError> {
    let mut registry = load(path)?;
    let Some(fleet) = registry.fleets.iter_mut().find(|f| f.name == name) else {
        return Err(ConfigError::FleetNotFound {
            name: name.to_string(),
        });
    };
    if !fleet.paused {
        return Ok(false);
    }
    fleet.paused = false;
    registry.validate()?;
    write_atomic(path, &registry)?;
    Ok(true)
}

/// The model-slot update request carried by `corrald fleet models`. Every
/// `Option` maps to one CLI flag; `None` = flag absent (leave the slot
/// untouched), `Some` = flag given (the value; empty string CLEARS the slot).
#[derive(Debug, Clone, Default)]
pub struct ModelUpdate {
    pub orch: Option<String>,
    pub impl_: Option<String>,
    pub impl_alt: Option<String>,
    pub impl_alt2: Option<String>,
    pub review: Option<String>,
}

impl ModelUpdate {
    /// Whether at least one slot is being updated — `fleet models` with no
    /// flags at all is a usage error at the CLI layer, and this guards the
    /// ops layer too.
    pub fn is_empty(&self) -> bool {
        self.orch.is_none()
            && self.impl_.is_none()
            && self.impl_alt.is_none()
            && self.impl_alt2.is_none()
            && self.review.is_none()
    }
}

/// One fleet's model-map change, so the CLI can print what changed (old →
/// new) on success and tests can assert exactly which slots moved.
#[derive(Debug, Clone)]
pub struct ModelsChange {
    pub name: String,
    pub before: Models,
    pub after: Models,
}

/// `corrald fleet models <name>`: update only the model slots the caller
/// named, leaving every other slot (including the optional alt slots)
/// untouched. `<name>` may be `all` — apply to every fleet (legacy
/// semantics). Empty values for the three REQUIRED slots (`orch`, `impl`,
/// `review`) are a usage error (exit 2 via [`ConfigError::Empty`]); empty
/// values for the optional alt slots CLEAR them (`Some("")` writes the slot
/// away).
///
/// The candidate registry is validated before the write, and an unknown
/// fleet name is a refusal that writes nothing. `Ok` carries, per affected
/// fleet, the before/after model maps.
pub fn models(
    path: &Path,
    name: &str,
    update: &ModelUpdate,
) -> Result<Vec<ModelsChange>, ConfigError> {
    // Validate the caller's request shape BEFORE touching the registry —
    // even an unreadable file must not mask a malformed request: the
    // required slots must not be cleared, and an empty request is a usage
    // error. (The CLI pre-checks these too; this is the ops-layer contract.)
    if update.is_empty() {
        return Err(ConfigError::ModelsRequest {
            field: "models".to_string(),
        });
    }
    for (field, value) in [
        ("orch", update.orch.as_deref()),
        ("impl", update.impl_.as_deref()),
        ("review", update.review.as_deref()),
    ] {
        if let Some(value) = value
            && value.is_empty()
        {
            return Err(ConfigError::ModelsRequest {
                field: format!("models.{field}"),
            });
        }
    }

    let mut registry = load(path)?;

    let apply: Vec<String> = if name == "all" {
        // `all` is a reserved wildcard (validate() refuses it as a fleet
        // name), so this expansion can never shadow a real fleet. An empty
        // registry is a refusal, not a silent no-op: `models all` exists
        // for bulk mutation, and "updated zero fleets" exiting 0 would let
        // a bootstrap script march on believing every fleet was switched.
        if registry.fleets.is_empty() {
            return Err(ConfigError::NoFleets);
        }
        registry.fleets.iter().map(|f| f.name.clone()).collect()
    } else {
        if !registry.fleets.iter().any(|f| f.name == name) {
            return Err(ConfigError::FleetNotFound {
                name: name.to_string(),
            });
        }
        vec![name.to_string()]
    };

    let mut changes = Vec::new();
    for fleet in registry
        .fleets
        .iter_mut()
        .filter(|f| apply.contains(&f.name))
    {
        let before = fleet.models.clone();
        if let Some(orch) = &update.orch {
            fleet.models.orch = orch.clone();
        }
        if let Some(impl_) = &update.impl_ {
            fleet.models.impl_ = impl_.clone();
        }
        // Empty alt values clear the slot; any other value writes it.
        match update.impl_alt.as_deref() {
            Some("") => fleet.models.impl_alt = None,
            Some(value) => fleet.models.impl_alt = Some(value.to_string()),
            None => {}
        }
        match update.impl_alt2.as_deref() {
            Some("") => fleet.models.impl_alt2 = None,
            Some(value) => fleet.models.impl_alt2 = Some(value.to_string()),
            None => {}
        }
        if let Some(review) = &update.review {
            fleet.models.review = review.clone();
        }
        changes.push(ModelsChange {
            after: fleet.models.clone(),
            before,
            name: fleet.name.clone(),
        });
    }

    // Idempotence, same discipline as pause/resume: when nothing moved,
    // nothing is written — no pointless rename widening the documented
    // no-lock race window. Callers see it via `before == after`.
    if changes.iter().all(|c| c.before == c.after) {
        return Ok(changes);
    }

    registry.validate()?;
    write_atomic(path, &registry)?;
    Ok(changes)
}
