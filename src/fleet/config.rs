//! #35 phase 1: fleet registry config — parse, validate, default path, write.
//!
//! The registry is the `fleets.json` file corral shares with the
//! fleet-operations controller (#35/#66): the format originated in the
//! separate legacy fleet tooling, which still writes the legacy-path file
//! today — hence `default_path`'s migration fallback. corrald adopts the
//! format unchanged but reads a tolerant subset: fleet-operations owns the
//! full schema (`models.*`,
//! `reasoning_effort`, top-level `admit`, roles), while corral only needs
//! `fleets[].gh_repo` and `fleets[].local` for `/issues` and workspace
//! attribution. Unknown fields are retained in memory on load and re-applied
//! on rewrite, so a forward-compatible fleet-operations schema addition cannot
//! break the board or silently disappear through a corral registry write.
//! Corral still owns strict validation: an unknown key one edit away from an
//! owned field (`pausd`, `imp1_alt`, `puased`, `imlp_alt`) is refused rather
//! than silently defaulted.
//! Validation still fails loudly on empty required fields, whitespace inside
//! `name`/`gh_repo`, a `gh_repo` that is not a single `owner/repo`, a `local`
//! that begins with a bare `~`, and duplicate fleet names. The write side
//! (`write_atomic`) is the first registry-WRITING surface (#35 slice 1): it
//! serialises back to the same schema and replaces the file via
//! temp-file-in-same-dir + rename, so a refused or failed write leaves the
//! original byte-identical.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use serde_json::{Map, Value};

/// A whole fleet registry file: `{ "fleets": [...] }`.
#[derive(Debug, Clone, Serialize)]
pub struct Registry {
    pub fleets: Vec<Fleet>,
    /// Forward-compatible fleet-operations fields retained across a rewrite
    /// (`admit`, per-fleet/per-models unknown keys, `reasoning_effort`).
    /// Serde consumes this on deserialization and `write_atomic` merges it
    /// back into the serialized candidate; it is deliberately absent from
    /// manually-constructed registries.
    #[serde(skip)]
    foreign: RegistryForeign,
}

/// One fleet entry. Field rules (from #35):
/// - `name`, `gh_repo`, `local`, `worktree_dir`, `orch` — required, non-empty.
/// - `workers` — required array of strings; may be empty.
/// - `models` — required object with required string keys.
/// - `paused` — optional, defaults to `false`.
#[derive(Debug, Clone, Deserialize, Serialize)]
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
/// `load()` parses. `impl_alt` / `impl_alt2` (fallback implementer and
/// last-resort backend, #56) are optional; `skip_serializing_if` omits them
/// from the written JSON when absent, so an unrelated rewrite never grows
/// `"impl_alt": null` into a registry that did not have them.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Models {
    pub orch: String,
    #[serde(rename = "impl")]
    pub impl_: String,
    pub review: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impl_alt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impl_alt2: Option<String>,
}

/// Fleet-operations keys that corral recognizes but does not model as typed
/// Rust fields, retained verbatim so a rewrite cannot drop them.
#[derive(Debug, Clone, Default)]
struct RegistryForeign {
    admit: Option<Value>,
    unknown: Map<String, Value>,
    fleets: Vec<FleetForeign>,
}

#[derive(Debug, Clone, Default)]
struct FleetForeign {
    name: String,
    unknown: Map<String, Value>,
    reasoning_effort: Option<Value>,
    unknown_models: Map<String, Value>,
}

/// Raw deserialization intermediate: captures the current fleet-operations
/// schema explicitly while leaving future keys in each `unknown` map.
#[derive(Deserialize)]
struct RawRegistry {
    fleets: Vec<RawFleet>,
    #[serde(default)]
    admit: Option<Value>,
    #[serde(flatten)]
    unknown: Map<String, Value>,
}

#[derive(Deserialize)]
struct RawFleet {
    name: String,
    gh_repo: String,
    local: String,
    worktree_dir: String,
    orch: String,
    workers: Vec<String>,
    #[serde(default)]
    paused: bool,
    models: RawModels,
    #[serde(flatten)]
    unknown: Map<String, Value>,
}

#[derive(Deserialize)]
struct RawModels {
    orch: String,
    #[serde(rename = "impl")]
    impl_: String,
    review: String,
    #[serde(default)]
    impl_alt: Option<String>,
    #[serde(default)]
    impl_alt2: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<Value>,
    #[serde(flatten)]
    unknown: Map<String, Value>,
}

/// `paused` is `#[serde(default)]` and intentionally SKIPPED when false, so a
/// freshly added fleet (and any fleet a `load()` -> write round-trip touches)
/// omits the field exactly as the existing live registries do. `true` is
/// always written — dropping it would silently release the #35 ops-safety
/// pause gate, which is why `add_writes_and_round_trips_through_load` pins
/// its survival. Documented here because the decision must be stated
/// explicitly, not buried in a derive.
fn is_false(value: &bool) -> bool {
    !*value
}

/// Field names corral owns and validates. Fleet-operations current fields
/// (`admit`, `reasoning_effort`) are modeled explicitly below, so they never
/// appear in an unknown map; future foreign keys are accepted unless they are
/// a one-edit obvious typo (including an adjacent transposition) of one of
/// these owned names.
const CORRAL_OWNED_REGISTRY_FIELDS: &[&str] = &["fleets"];
const CORRAL_OWNED_FLEET_FIELDS: &[&str] = &[
    "name",
    "gh_repo",
    "local",
    "worktree_dir",
    "orch",
    "workers",
    "paused",
    "models",
];
const CORRAL_OWNED_MODEL_FIELDS: &[&str] = &["orch", "impl", "review", "impl_alt", "impl_alt2"];

/// Merely one edit away from a Corral-owned key is almost certainly a typo;
/// one edit includes substitution, insertion, deletion, and adjacent
/// transposition. Anything farther is treated as a genuine forward-compatible
/// foreign key and preserved rather than guessed at.
fn edit_distance_at_most_one(first: &str, second: &str) -> bool {
    let first: Vec<char> = first.chars().collect();
    let second: Vec<char> = second.chars().collect();
    let (first, second) = if first.len() <= second.len() {
        (first, second)
    } else {
        (second, first)
    };
    if second.len() - first.len() > 1 {
        return false;
    }
    if first.len() == second.len() {
        let differences: Vec<usize> = first
            .iter()
            .zip(&second)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(index, _)| index)
            .collect();
        return match differences.as_slice() {
            [] | [_] => true,
            [left, right]
                if *right == left + 1
                    && first[*left] == second[*right]
                    && first[*right] == second[*left] =>
            {
                true
            }
            _ => false,
        };
    }
    let mut first_index = 0;
    let mut second_index = 0;
    let mut skipped = false;
    while first_index < first.len() && second_index < second.len() {
        if first[first_index] == second[second_index] {
            first_index += 1;
            second_index += 1;
        } else if skipped {
            return false;
        } else {
            skipped = true;
            second_index += 1;
        }
    }
    true
}

fn validate_typo_map(
    unknown: &Map<String, Value>,
    owned: &[&str],
    location: &str,
) -> Result<(), String> {
    for key in unknown.keys() {
        if let Some(hint) = owned
            .iter()
            .find(|known| edit_distance_at_most_one(key, known))
        {
            return Err(format!(
                "unknown field {key:?} in {location} looks like a typo of {hint:?}"
            ));
        }
    }
    Ok(())
}

fn validate_forward_keys(raw: &RawRegistry) -> Result<(), String> {
    validate_typo_map(&raw.unknown, CORRAL_OWNED_REGISTRY_FIELDS, "the registry")?;
    for (index, fleet) in raw.fleets.iter().enumerate() {
        let location = format!("#{} (name {:?})", index + 1, fleet.name);
        validate_typo_map(&fleet.unknown, CORRAL_OWNED_FLEET_FIELDS, &location)?;
        validate_typo_map(
            &fleet.models.unknown,
            CORRAL_OWNED_MODEL_FIELDS,
            &format!("{location} models"),
        )?;
    }
    Ok(())
}

impl RegistryForeign {
    fn apply(&self, candidate: &mut Value) {
        let Some(candidate) = candidate.as_object_mut() else {
            return;
        };
        if let Some(admit) = &self.admit {
            candidate.insert("admit".to_string(), admit.clone());
        }
        for (key, value) in &self.unknown {
            candidate.insert(key.clone(), value.clone());
        }
        let Some(candidate_fleets) = candidate.get_mut("fleets").and_then(Value::as_array_mut)
        else {
            return;
        };
        for candidate_fleet in candidate_fleets {
            let Some(name) = candidate_fleet.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(foreign) = self.fleets.iter().find(|fleet| fleet.name == name) else {
                continue;
            };
            let Some(candidate_fleet) = candidate_fleet.as_object_mut() else {
                continue;
            };
            for (key, value) in &foreign.unknown {
                candidate_fleet.insert(key.clone(), value.clone());
            }
            let Some(candidate_models) = candidate_fleet
                .get_mut("models")
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            if let Some(reasoning_effort) = &foreign.reasoning_effort {
                candidate_models.insert("reasoning_effort".to_string(), reasoning_effort.clone());
            }
            for (key, value) in &foreign.unknown_models {
                candidate_models.insert(key.clone(), value.clone());
            }
        }
    }

    fn inherit_fleet(&mut self, source: &str, target: &str) {
        let mut inherited = self
            .fleets
            .iter()
            .find(|fleet| fleet.name == source)
            .cloned()
            .unwrap_or_default();
        inherited.name = target.to_string();
        self.fleets.retain(|fleet| fleet.name != target);
        self.fleets.push(inherited);
    }

    fn forget_fleet(&mut self, name: &str) {
        self.fleets.retain(|fleet| fleet.name != name);
    }
}

impl<'de> Deserialize<'de> for Registry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawRegistry::deserialize(deserializer)?;
        validate_forward_keys(&raw).map_err(serde::de::Error::custom)?;

        let mut foreign = RegistryForeign {
            admit: raw.admit,
            unknown: raw.unknown,
            fleets: Vec::with_capacity(raw.fleets.len()),
        };
        let mut fleets = Vec::with_capacity(raw.fleets.len());
        for raw_fleet in raw.fleets {
            let RawFleet {
                name,
                gh_repo,
                local,
                worktree_dir,
                orch,
                workers,
                paused,
                models: raw_models,
                unknown,
            } = raw_fleet;
            let RawModels {
                orch: model_orch,
                impl_: model_impl,
                review,
                impl_alt,
                impl_alt2,
                reasoning_effort,
                unknown: unknown_models,
            } = raw_models;
            foreign.fleets.push(FleetForeign {
                name: name.clone(),
                unknown,
                reasoning_effort,
                unknown_models,
            });
            fleets.push(Fleet {
                name,
                gh_repo,
                local,
                worktree_dir,
                orch,
                workers,
                paused,
                models: Models {
                    orch: model_orch,
                    impl_: model_impl,
                    review,
                    impl_alt,
                    impl_alt2,
                },
            });
        }
        Ok(Self { fleets, foreign })
    }
}

impl Registry {
    /// Build a registry without forward fields. Used by unit/integration
    /// fixtures and the code paths that synthesize registry data; `load()`
    /// always supplies the forward-fields wrapper.
    pub fn new(fleets: Vec<Fleet>) -> Self {
        Self {
            fleets,
            foreign: RegistryForeign::default(),
        }
    }

    pub(crate) fn inherit_forward_fields(&mut self, source: &str, target: &str) {
        self.foreign.inherit_fleet(source, target);
    }

    pub(crate) fn forget_forward_fields(&mut self, name: &str) {
        self.foreign.forget_fleet(name);
    }
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
            // `impl_alt` / `impl_alt2` are optional (absent stays fine) but a
            // PRESENT value is held to the same non-empty rule as the required
            // model slots.
            for (field, value) in [
                ("models.impl_alt", fleet.models.impl_alt.as_deref()),
                ("models.impl_alt2", fleet.models.impl_alt2.as_deref()),
            ] {
                if let Some(value) = value
                    && value.trim().is_empty()
                {
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
            // `models.*` join `name`/`gh_repo` in the whitespace-delimited
            // `fleet list` line, so internal whitespace in any of them would
            // corrupt that output contract the same way.
            for (field, value) in [
                ("name", &fleet.name),
                ("gh_repo", &fleet.gh_repo),
                ("models.orch", &fleet.models.orch),
                ("models.impl", &fleet.models.impl_),
                ("models.review", &fleet.models.review),
            ] {
                if value.chars().any(char::is_whitespace) {
                    return Err(ConfigError::Whitespace {
                        fleet: fleet_locator(index, fleet, field),
                        field: field.to_string(),
                    });
                }
            }
            // The alt-model slots get the same whitespace rule when present —
            // for CONSISTENCY with the required slots, not for the `fleet
            // list` contract (they are deliberately absent from that line).
            for (field, value) in [
                ("models.impl_alt", fleet.models.impl_alt.as_deref()),
                ("models.impl_alt2", fleet.models.impl_alt2.as_deref()),
            ] {
                if let Some(value) = value
                    && value.chars().any(char::is_whitespace)
                {
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
            // #113: `worktree_dir` is joined under the daemon's worktrees
            // root, so a separator/`..`/absolute value would let a fleet
            // escape that root. Refuse it at validation so the worktree
            // operation never builds an out-of-root path.
            if !is_safe_worktree_dir(&fleet.worktree_dir) {
                return Err(ConfigError::BadWorktreeDir {
                    fleet: fleet_locator(index, fleet, "worktree_dir"),
                    value: fleet.worktree_dir.clone(),
                });
            }
            // `all` is the `fleet models` wildcard (and pause/resume do NOT
            // accept it) — a fleet actually named `all` would be shadowed by
            // the keyword with no escape hatch, so the name is reserved.
            if fleet.name == "all" {
                return Err(ConfigError::ReservedFleetName {
                    name: fleet.name.clone(),
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

/// A worktree directory name must be a single path component that cannot
/// escape the daemon's worktrees root: no separator, no `..`, not absolute.
fn is_safe_worktree_dir(dir: &str) -> bool {
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        return false;
    }
    trimmed != "." && trimmed != ".." && !trimmed.contains('/') && !trimmed.contains('\\')
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

/// The registry file to use when the CLI gets no `--registry` (#66):
///
/// 1. `$CORRAL_FLEETS_PATH`, when set and non-empty — always wins.
/// 2. `<config dir>/fleets.json` — the corral-owned default, where the
///    config dir honours `$CORRAL_CONFIG_DIR` (falling back to
///    `$HOME/.config/corral`) like every other consumer of the config
///    dir — when it exists OR when the legacy path does not (so a fresh
///    machine gets the corral path).
/// 3. `$HOME/.hermes/scripts/fleets.json` — the pre-#66 legacy location,
///    honoured as a migration fallback ONLY while the corral-owned file
///    does not exist, announced by a stderr note each time. Machines that
///    predate the move keep working untouched; nothing is copied or
///    migrated implicitly.
pub fn default_path() -> PathBuf {
    // Empty-but-set env vars fall through like unset ones — `export
    // CORRAL_FLEETS_PATH=` in a profile must not produce a path-less error.
    let fleets_env = std::env::var("CORRAL_FLEETS_PATH")
        .ok()
        .filter(|v| !v.is_empty());
    let config_dir_env = std::env::var("CORRAL_CONFIG_DIR")
        .ok()
        .filter(|v| !v.is_empty());
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let resolved = default_path_in(
        Path::new(&home),
        fleets_env.as_deref(),
        config_dir_env.as_deref(),
    );
    // The legacy fallback is deliberately LOUD: a silent fallback would let
    // a cp-instead-of-mv migration split the registry in two with no clue.
    // The note disappears the moment the corral-owned file exists.
    if resolved.ends_with(".hermes/scripts/fleets.json") {
        eprintln!(
            "note: using legacy registry {} (#66: move it to the corral config \
             dir — see docs/OPERATIONS.md)",
            resolved.display()
        );
    }
    resolved
}

/// The resolution core, parameterised over `$HOME` and both env overrides
/// so tests can exercise the ladder against temp dirs without mutating
/// process-wide environment (test threads run in parallel).
///
/// Ladder: `$CORRAL_FLEETS_PATH` → `<config dir>/fleets.json` (where the
/// config dir honours `$CORRAL_CONFIG_DIR`, matching every other consumer
/// of the corral config dir) when it exists or nothing else does → the
/// legacy `~/.hermes/scripts/fleets.json` while it alone exists.
fn default_path_in(home: &Path, fleets_env: Option<&str>, config_dir_env: Option<&str>) -> PathBuf {
    if let Some(env) = fleets_env {
        return PathBuf::from(env);
    }
    let config_dir = config_dir_env
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config/corral"));
    let corral = config_dir.join("fleets.json");
    if corral.exists() {
        return corral;
    }
    let legacy = home.join(".hermes/scripts/fleets.json");
    if legacy.exists() {
        return legacy;
    }
    corral
}

#[cfg(test)]
mod default_path_tests {
    use super::default_path_in;

    /// #66: env override wins; corral-owned path when present or on a
    /// fresh machine; legacy honoured only while the corral file is absent.
    #[test]
    fn default_path_prefers_corral_then_legacy_then_corral_for_fresh() {
        let home = tempfile::tempdir().expect("temp home");
        let corral = home.path().join(".config/corral/fleets.json");
        let legacy = home.path().join(".hermes/scripts/fleets.json");

        // Fresh machine: neither exists -> the corral-owned path.
        assert_eq!(default_path_in(home.path(), None, None), corral);

        // Legacy machine: only the old file exists -> fallback honoured.
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, "{}").unwrap();
        assert_eq!(default_path_in(home.path(), None, None), legacy);

        // Migrated machine: corral file exists -> it wins over legacy.
        std::fs::create_dir_all(corral.parent().unwrap()).unwrap();
        std::fs::write(&corral, "{}").unwrap();
        assert_eq!(default_path_in(home.path(), None, None), corral);

        // Env override beats everything.
        assert_eq!(
            default_path_in(home.path(), Some("/x/y.json"), None),
            std::path::PathBuf::from("/x/y.json")
        );
    }

    /// F2: the registry follows a relocated config dir exactly like every
    /// other consumer of `$CORRAL_CONFIG_DIR` (setup script, daemon keys).
    #[test]
    fn default_path_honours_the_config_dir_override() {
        let home = tempfile::tempdir().expect("temp home");
        let alt = tempfile::tempdir().expect("alt config dir");
        let relocated = alt.path().join("fleets.json");

        // Relocated dir, no file yet: still the relocated path (fresh).
        assert_eq!(
            default_path_in(home.path(), None, alt.path().to_str()),
            relocated
        );

        // Legacy exists but the relocated file also exists -> relocated wins.
        let legacy = home.path().join(".hermes/scripts/fleets.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, "{}").unwrap();
        std::fs::write(&relocated, "{}").unwrap();
        assert_eq!(
            default_path_in(home.path(), None, alt.path().to_str()),
            relocated
        );

        // CORRAL_FLEETS_PATH still beats the config-dir override.
        assert_eq!(
            default_path_in(home.path(), Some("/x/y.json"), alt.path().to_str()),
            std::path::PathBuf::from("/x/y.json")
        );
    }
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
/// Write discipline (#35 slice 1): a PID-suffixed temp file in the SAME
/// directory as the (symlink-resolved) target, written, `sync_all()`-ed,
/// renamed over the target, then the parent directory is fsynced so the
/// rename survives power loss. The original is never truncated in place; the
/// temp file is removed on every failure path, so any single failed write
/// leaves the original byte-identical. The target's permissions are copied
/// onto the temp file before the rename, so a `chmod 600` registry stays
/// `600`. A symlinked registry is followed (the target file is replaced, the
/// link survives).
///
/// KNOWN LIMIT (reviewed, deferred): there is no cross-process lock, so a
/// concurrent writer — corrald or the legacy fleet tooling — can lose the
/// race between `load()` and the rename (last rename wins, the other
/// writer's fields are clobbered). Single-writer discipline is assumed for
/// the #35 migration window; a lockfile is a follow-up.
///
/// Serialisation uses `serde_json::Value` plus `to_string_pretty` (2-space
/// indent) and a trailing newline. The round-trip is semantically exact
/// — `Models::impl_` writes back as the JSON key `impl`, `paused` is skipped
/// when false (see [`is_false`]) — but the first write NORMALISES any
/// hand-maintained formatting (including serde_json's default object-key
/// ordering) to a canonical shape. Fleet-operations fields (`admit`,
/// `reasoning_effort`, and future keys) are carried in the in-memory
/// `Registry` and re-applied, so the write cannot drop schema the fleet
/// controller owns unless the candidate never loaded from disk. The current
/// on-disk file is re-parsed first, so a concurrent corrupt/typo'd rewrite
/// fails loudly instead of being overwritten.
pub fn write_atomic(path: &Path, registry: &Registry) -> Result<(), ConfigError> {
    // Follow a symlinked registry so the rename replaces the target file,
    // not the link (a dotfiles-checkout registry keeps working).
    let path = &std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    ensure_current_registry_parses(path)?;
    let bytes = serialize(path, registry)?;
    let temp_path = parent.join(format!(
        ".{}.corrald-tmp.{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("fleets"),
        std::process::id()
    ));
    let write_error = |source: std::io::Error| {
        // Every failure before the rename lands removes the partial temp
        // file, so nothing dot-named accumulates in the registry directory.
        let _ = std::fs::remove_file(&temp_path);
        ConfigError::Write {
            path: path.to_path_buf(),
            source,
        }
    };
    {
        let mut file = std::fs::File::create(&temp_path).map_err(write_error)?;
        if let Ok(meta) = std::fs::metadata(path) {
            file.set_permissions(meta.permissions())
                .map_err(write_error)?;
        }
        file.write_all(&bytes).map_err(write_error)?;
        file.sync_all().map_err(write_error)?;
    }
    std::fs::rename(&temp_path, path).map_err(write_error)?;
    // The rename is the atomic commit point; fsync the directory so the new
    // entry is durable across power loss, not just process crash.
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// A rewrite must not clobber an on-disk registry it can no longer read or
/// parse: that would silently replace fleet-operations schema with a
/// stripped candidate. A missing file is fine (first bootstrap write); an
/// unreadable or invalid one is a loud refusal that leaves the original
/// byte-identical.
fn ensure_current_registry_parses(path: &Path) -> Result<(), ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(_) => load(path).map(|_| ()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ConfigError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// `serde_json::to_string_pretty` + a trailing newline, so a written registry
/// stays diffable in git. Serialisation itself is fallible only on non-UTF-8
/// (this schema is all-strings), so errors are surfaced rather than written
/// off as impossible.
fn serialize(path: &Path, registry: &Registry) -> Result<Vec<u8>, ConfigError> {
    let mut value = serde_json::to_value(registry).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source: std::io::Error::other(source),
    })?;
    registry.foreign.apply(&mut value);
    let mut json = serde_json::to_string_pretty(&value).map_err(|source| ConfigError::Write {
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
    /// The file is not valid registry JSON (malformed JSON, missing required
    /// fields, type mismatches, or a one-edit typo (including an adjacent
    /// transposition) of a Corral-owned field).
    /// Genuine fleet-operations forward fields are accepted and preserved.
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
    /// `worktree_dir` is not a single path component (contains a separator,
    /// is `..`/`.`, or is absolute) — it would escape the worktrees root.
    BadWorktreeDir { fleet: String, value: String },
    /// Two fleets share a `name`.
    DuplicateFleet { name: String },
    /// The fleet name is a reserved word (`all` — the `fleet models`
    /// wildcard).
    ReservedFleetName { name: String },
    /// A `fleet add` `--gh <owner/repo>` could not be resolved upstream.
    /// Non-zero exit from `gh repo view` — including `gh` being absent — is a
    /// refusal, never a silent skip (#35's "repo resolves before add").
    /// `detail` carries the first stderr line from `gh` when there is one,
    /// so an expired token is distinguishable from a typo'd slug.
    AddRepoUnresolved {
        repo: String,
        detail: Option<String>,
    },
    /// A `fleet add` on a registry with no existing fleet to inherit
    /// `models` from, and no `--models` supplied.
    AddNeedsModels,
    /// A `fleet remove <name>` named a fleet that is not in the registry.
    RemoveNotFound { name: String },
    /// `fleet pause`/`resume`/`models` named a fleet that is not in the
    /// registry (or `models` used a name other than the legacy `all` while
    /// applying per-fleet). Refusal, exit 1, like [`ConfigError::RemoveNotFound`].
    FleetNotFound { name: String },
    /// `fleet models all` against a registry with no fleets: a bulk
    /// mutation that would update zero fleets is a refusal (exit 1), not a
    /// silent success — automation must not read "nothing happened" as
    /// "every fleet switched".
    NoFleets,
    /// A malformed `fleet models` request (no flags, or an empty value for
    /// a required slot). Usage-class (exit 2); named so the message cannot
    /// read as a registry problem in a fleet called "request".
    ModelsRequest { field: String },
    /// The registry file could not be replaced atomically.
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl ConfigError {
    /// The exit code contract (docs/corral/G35-registry.md): 1 = the
    /// operation failed (refusals and filesystem failures — retrying or
    /// paging may be appropriate), 2 = usage error, unreadable/unparseable
    /// registry, or validation failure (retrying is pointless).
    pub fn exit_code(&self) -> i32 {
        match self {
            ConfigError::DuplicateFleet { .. }
            | ConfigError::AddRepoUnresolved { .. }
            | ConfigError::AddNeedsModels
            | ConfigError::RemoveNotFound { .. }
            | ConfigError::FleetNotFound { .. }
            | ConfigError::NoFleets
            | ConfigError::Write { .. } => 1,
            ConfigError::Io { .. }
            | ConfigError::Parse { .. }
            | ConfigError::Empty { .. }
            | ConfigError::Whitespace { .. }
            | ConfigError::GhRepoShape { .. }
            | ConfigError::BadTilde { .. }
            | ConfigError::BadWorktreeDir { .. }
            | ConfigError::ModelsRequest { .. }
            | ConfigError::ReservedFleetName { .. } => 2,
        }
    }
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
            ConfigError::BadWorktreeDir { fleet, value } => {
                write!(
                    f,
                    "fleet {fleet}: worktree_dir {value:?} must be a single path component \
                     (no separators, not '..', not absolute)"
                )
            }
            ConfigError::DuplicateFleet { name } => {
                write!(f, "duplicate fleet name {name:?}")
            }
            ConfigError::ReservedFleetName { name } => {
                write!(
                    f,
                    "fleet name {name:?} is reserved (it is the `fleet models` wildcard)"
                )
            }
            ConfigError::AddRepoUnresolved { repo, detail } => {
                write!(
                    f,
                    "cannot add fleet: repo {repo:?} did not resolve via `gh repo view`"
                )?;
                if let Some(detail) = detail {
                    write!(f, " ({detail})")?;
                }
                Ok(())
            }
            ConfigError::AddNeedsModels => {
                write!(
                    f,
                    "cannot add fleet: the registry has no existing fleet to inherit \
                     `models` from; pass --models orch=..,impl=..,review=.."
                )
            }
            ConfigError::RemoveNotFound { name } => {
                write!(f, "no fleet named {name:?} in the registry")
            }
            ConfigError::NoFleets => {
                write!(
                    f,
                    "the registry has no fleets to update (bootstrap one with `fleet add`)"
                )
            }
            ConfigError::ModelsRequest { field } => {
                write!(
                    f,
                    "models update request: {field} must be non-empty (only the optional \
                     --impl-alt/--impl-alt2 accept '' to clear)"
                )
            }
            ConfigError::FleetNotFound { name } => {
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
            | ConfigError::BadWorktreeDir { .. }
            | ConfigError::DuplicateFleet { .. }
            | ConfigError::AddRepoUnresolved { .. }
            | ConfigError::AddNeedsModels
            | ConfigError::RemoveNotFound { .. }
            | ConfigError::FleetNotFound { .. }
            | ConfigError::NoFleets
            | ConfigError::ModelsRequest { .. }
            | ConfigError::ReservedFleetName { .. } => None,
        }
    }
}

#[cfg(test)]
mod worktree_dir_validation_tests {
    use super::*;

    fn fleet_with_worktree_dir(worktree_dir: &str) -> Registry {
        Registry::new(vec![Fleet {
            name: "corral".into(),
            gh_repo: "jirathip-dev/corral".into(),
            local: "~/Projects/corral".into(),
            worktree_dir: worktree_dir.into(),
            orch: "orch-corral".into(),
            workers: vec![],
            paused: false,
            models: Models {
                orch: "o".into(),
                impl_: "i".into(),
                review: "r".into(),
                impl_alt: None,
                impl_alt2: None,
            },
        }])
    }

    #[test]
    fn rejects_separator_dotdot_and_absolute_worktree_dir() {
        for bad in ["../corral", "a/b", "..", ".", "/corral", "\\corral"] {
            let err = fleet_with_worktree_dir(bad).validate().unwrap_err();
            assert!(
                matches!(err, ConfigError::BadWorktreeDir { .. }),
                "worktree_dir {bad:?} must be refused, got {err}"
            );
        }
    }

    #[test]
    fn accepts_a_single_component_worktree_dir() {
        let ok = fleet_with_worktree_dir("corral");
        assert!(ok.validate().is_ok());
    }
}
