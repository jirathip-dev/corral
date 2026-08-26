//! Editable fleet registry surface for #206.
//!
//! The daemon projection remains the read path. Mutations are deliberately
//! explicit: the client edits the returned `fleets.json` in place with an
//! atomic replacement, then uses the shipped `corrald fleet check` command for
//! candidate validation and the explicit Send-to-fleet verification path.
//! Unknown registry keys are preserved by updating only the fields owned by
//! this form. Every editable draft carries a fingerprint of the projection it
//! was loaded from, so a refresh or another writer cannot be silently lost.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use eframe::egui::{RichText, ScrollArea, TextEdit, Ui};
use sha2::{Digest, Sha256};

use crate::model::{FleetRegistry, FleetRegistryEntry};
use crate::state::Level;
use crate::theme;

type CandidateValidator = dyn Fn(&Path, &[u8]) -> Result<(), String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetDraft {
    pub original_name: String,
    /// Fingerprint of the editable projection when this draft was loaded.
    /// It is deliberately not rendered or written to fleets.json.
    pub source_fingerprint: String,
    pub name: String,
    pub gh_repo: String,
    pub local: String,
    pub worktree_dir: String,
    pub orch: String,
    pub workers: String,
    pub paused: bool,
    pub model_orch: String,
    pub model_impl: String,
    pub model_review: String,
    pub model_impl_alt: String,
    pub model_impl_alt2: String,
    pub reasoning_effort: String,
}

impl From<&FleetRegistryEntry> for FleetDraft {
    fn from(fleet: &FleetRegistryEntry) -> Self {
        let mut draft = Self {
            original_name: fleet.name.clone(),
            source_fingerprint: String::new(),
            name: fleet.name.clone(),
            gh_repo: fleet.gh_repo.clone(),
            local: fleet.local.clone(),
            worktree_dir: fleet.worktree_dir.clone(),
            orch: fleet.orch.clone(),
            workers: workers_text(&fleet.workers),
            paused: fleet.paused,
            model_orch: fleet.models.orch.clone(),
            model_impl: fleet.models.impl_.clone(),
            model_review: fleet.models.review.clone(),
            model_impl_alt: model_or_unset(fleet.models.impl_alt.as_ref()),
            model_impl_alt2: model_or_unset(fleet.models.impl_alt2.as_ref()),
            reasoning_effort: reasoning_effort_text(fleet.models.reasoning_effort.as_ref()),
        };
        draft.source_fingerprint = fingerprint_for_draft(&draft);
        draft
    }
}

/// Compare an editable draft with a fresh daemon projection without mutating
/// either value. A mismatch means the draft is still useful to display, but
/// Save & apply must re-check the on-disk source and refuse a stale write.
pub(crate) fn draft_source_matches_entry(draft: &FleetDraft, entry: &FleetRegistryEntry) -> bool {
    draft.source_fingerprint == FleetDraft::from(entry).source_fingerprint
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Refresh,
    Save(Box<FleetDraft>),
    Send(String),
    Pause { fleet_name: String, paused: bool },
}

/// Render the editable registry and return one deferred mutation request.
pub fn show(
    ui: &mut Ui,
    view: &Option<Result<FleetRegistry, String>>,
    loading: bool,
    drafts: &mut BTreeMap<String, FleetDraft>,
    notice: &mut Option<(Level, String)>,
    request: &mut dyn FnMut(Action),
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Registry")
                .heading()
                .color(theme::ui::TEXT_STRONG),
        );
        ui.label(
            RichText::new("fleet registry")
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        if loading {
            ui.spinner();
        }
        if ui.button("refresh").clicked() {
            request(Action::Refresh);
        }
    });
    ui.label(
        RichText::new(
            "Edit fleets.json in place. Save & apply validates the complete candidate with corrald before an atomic write; Send to fleet runs the same validation only (there is no daemon distribution endpoint); Pause changes the fleet's live admission flag.",
        )
        .small()
        .color(theme::ui::TEXT_MUTED),
    );
    ui.add_space(8.0);

    match view {
        None => {
            ui.label(
                RichText::new(if loading {
                    "loading registry…"
                } else {
                    "no registry data yet — press refresh"
                })
                .color(theme::ui::TEXT_MUTED),
            );
        }
        Some(Err(error)) => {
            ui.label(
                RichText::new(format!("registry unavailable: {error}"))
                    .strong()
                    .color(theme::ui::BAD),
            );
        }
        Some(Ok(registry)) => {
            let (status, color) = if registry.failed() {
                ("ERROR", theme::ui::BAD)
            } else {
                ("ACTIVE", theme::ui::GOOD)
            };
            ui.horizontal_wrapped(|ui| {
                crate::ui::badge(ui, status, color);
                ui.label(
                    RichText::new(format!(
                        "{} fleet(s) · {}",
                        registry.fleets.len(),
                        registry.reported_path
                    ))
                    .monospace()
                    .small()
                    .color(theme::ui::TEXT_MUTED),
                );
            });
            if let Some(error) = &registry.error {
                ui.label(RichText::new(format!("registry error: {error}")).color(theme::ui::BAD));
            }
            ui.separator();
            ScrollArea::vertical()
                .id_salt("corral-ui-registry-list")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for fleet in &registry.fleets {
                        let draft = drafts
                            .entry(fleet.name.clone())
                            .or_insert_with(|| FleetDraft::from(fleet));
                        fleet_card(ui, draft, request);
                        ui.add_space(10.0);
                    }
                });
        }
    }
    if let Some((level, text)) = notice {
        let color = match level {
            Level::Info => theme::ui::GOOD,
            Level::Warn => theme::ui::WARN,
            Level::Error => theme::ui::BAD,
        };
        ui.add_space(6.0);
        ui.label(RichText::new(text.as_str()).color(color));
    }
}

fn fleet_card(ui: &mut Ui, draft: &mut FleetDraft, request: &mut dyn FnMut(Action)) {
    eframe::egui::Frame::group(ui.style())
        .inner_margin(eframe::egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&draft.name)
                        .strong()
                        .monospace()
                        .color(theme::ui::TEXT_STRONG),
                );
                let (label, color) = if draft.paused {
                    ("PAUSED", theme::ui::BAD)
                } else {
                    ("ACTIVE", theme::ui::GOOD)
                };
                crate::ui::badge(ui, label, color);
            });
            ui.add_space(6.0);
            field(ui, "name", &mut draft.name);
            field(ui, "gh_repo", &mut draft.gh_repo);
            field(ui, "local", &mut draft.local);
            field(ui, "worktree_dir", &mut draft.worktree_dir);
            field(ui, "orch", &mut draft.orch);
            field(ui, "workers", &mut draft.workers);
            ui.add_space(4.0);
            ui.label(
                RichText::new("models")
                    .strong()
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
            field(ui, "orch", &mut draft.model_orch);
            field(ui, "impl", &mut draft.model_impl);
            field(ui, "review", &mut draft.model_review);
            field(ui, "impl_alt", &mut draft.model_impl_alt);
            field(ui, "impl_alt2", &mut draft.model_impl_alt2);
            if draft.reasoning_effort != "unset" {
                ui.label(
                    RichText::new(format!("reasoning_effort: {}", draft.reasoning_effort))
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Save & apply").clicked() {
                    request(Action::Save(Box::new(draft.clone())));
                }
                if ui.button("Send to fleet (validate only)").clicked() {
                    request(Action::Send(draft.original_name.clone()));
                }
                let pause_label = if draft.paused { "Resume" } else { "Pause" };
                if ui.button(pause_label).clicked() {
                    draft.paused = !draft.paused;
                    request(Action::Pause {
                        fleet_name: draft.original_name.clone(),
                        paused: draft.paused,
                    });
                }
            });
        });
}

fn field(ui: &mut Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .monospace()
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        ui.add(
            TextEdit::singleline(value)
                .desired_width(ui.available_width().max(180.0))
                .hint_text(label),
        );
    });
}

/// Apply a draft to the shared registry using a validate-before-rename update.
/// The raw JSON object is edited in place so fields owned by fleet-operations
/// remain intact.
pub fn apply_draft(path: &str, draft: &FleetDraft) -> Result<(), String> {
    apply_draft_with_validator(path, draft, &validate_candidate_with_corrald)
}

fn apply_draft_with_validator(
    path: &str,
    draft: &FleetDraft,
    validate_candidate: &CandidateValidator,
) -> Result<(), String> {
    validate_draft(draft)?;
    update_raw_fleet(
        Path::new(path),
        draft,
        Some(draft.paused),
        validate_candidate,
    )
}

/// Change only the pause bit while retaining every other registry field.
pub fn set_paused(path: &str, name: &str, paused: bool) -> Result<(), String> {
    set_paused_with_validator(path, name, paused, &validate_candidate_with_corrald)
}

fn set_paused_with_validator(
    path: &str,
    name: &str,
    paused: bool,
    validate_candidate: &CandidateValidator,
) -> Result<(), String> {
    let draft = FleetDraft {
        original_name: name.to_string(),
        name: name.to_string(),
        gh_repo: String::new(),
        local: String::new(),
        source_fingerprint: String::new(),
        worktree_dir: String::new(),
        orch: String::new(),
        workers: String::new(),
        paused,
        model_orch: String::new(),
        model_impl: String::new(),
        model_review: String::new(),
        model_impl_alt: String::new(),
        model_impl_alt2: String::new(),
        reasoning_effort: String::new(),
    };
    update_raw_fleet(Path::new(path), &draft, Some(paused), validate_candidate)
}

/// Run the repository's real fleet validation path. This is intentionally
/// reported as validation rather than pretending there is a daemon mutation
/// endpoint for registry distribution in the #206 layout-only scope.
pub fn send_to_fleet(path: &str) -> Result<String, String> {
    let binary = resolve_corrald_binary()?;
    let output = Command::new(binary)
        .args(["fleet", "check", "--registry", path])
        .output()
        .map_err(|error| format_corrald_run_error(&error))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        Ok(send_validation_success_message(&stdout))
    } else {
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn path_command(path: Option<&OsStr>, command: &str) -> Option<PathBuf> {
    let path = path?;
    std::env::split_paths(path)
        .map(|directory| directory.join(command))
        .find(|candidate| is_executable_file(candidate))
}

fn resolve_corrald_binary() -> Result<PathBuf, String> {
    let explicit = std::env::var_os("CORRALD_BIN");
    let current_exe = std::env::current_exe().ok();
    let install_root = std::env::var_os("CORRAL_INSTALL_DIR");
    let home = std::env::var_os("HOME").map(PathBuf::from);
    resolve_corrald_binary_from(
        explicit.as_deref(),
        current_exe.as_deref(),
        install_root.as_deref().map(Path::new),
        home.as_deref(),
        std::env::var_os("PATH").as_deref(),
    )
}

fn resolve_corrald_binary_from(
    explicit: Option<&OsStr>,
    current_exe: Option<&Path>,
    install_root: Option<&Path>,
    home: Option<&Path>,
    path: Option<&OsStr>,
) -> Result<PathBuf, String> {
    if let Some(explicit) = explicit.filter(|value| !value.is_empty()) {
        let explicit_path = PathBuf::from(explicit);
        if is_executable_file(&explicit_path) {
            return Ok(explicit_path);
        }
        // A bare CORRALD_BIN is still a useful explicit command override; it
        // is resolved through PATH while preserving the override's priority.
        if explicit_path.components().count() == 1
            && let Some(found) = path_command(path, explicit_path.to_string_lossy().as_ref())
        {
            return Ok(found);
        }
        return Err(format!(
            "CORRALD_BIN={} is not an executable corrald binary; set CORRALD_BIN to the full path of the corrald executable",
            explicit.to_string_lossy()
        ));
    }

    let mut candidates = Vec::new();
    let mut add_candidate = |candidate: PathBuf| {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    };

    // A source/release install places corrald beside corrald-ui. This also
    // covers a raw release directory launched by a desktop entry.
    if let Some(current_exe) = current_exe
        && let Some(parent) = current_exe.parent()
    {
        add_candidate(parent.join("corrald"));
    }

    // The packaged installer keeps the daemon below CORRAL_INSTALL_DIR/release
    // while the Finder-launched UI lives in Corral.app. Honor an explicit
    // install root first, then the documented per-user default.
    let mut roots = Vec::new();
    if let Some(root) = install_root {
        roots.push(root.to_path_buf());
    }
    if let Some(home) = home {
        roots.push(home.join(".local/share/corral"));
    }
    for root in roots {
        add_candidate(root.join("corrald"));
        add_candidate(root.join("release/corrald"));
        add_candidate(root.join("bin/corrald"));
    }

    if let Some(found) = candidates
        .into_iter()
        .find(|candidate| is_executable_file(candidate))
    {
        return Ok(found);
    }
    if let Some(found) = path_command(path, "corrald") {
        return Ok(found);
    }

    Err(
        "could not find an executable corrald for registry validation; set CORRALD_BIN to the full path of the corrald executable (for example CORRALD_BIN=/path/to/corrald)"
            .to_string(),
    )
}

fn format_corrald_run_error(error: &std::io::Error) -> String {
    format!(
        "could not run corrald fleet check ({error}); set CORRALD_BIN to the full path of the corrald executable"
    )
}

fn send_validation_success_message(stdout: &str) -> String {
    let prefix =
        "validation-only passed; no daemon distribution endpoint is available (registry unchanged)";
    let stdout = stdout.trim();
    if stdout.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}: {stdout}")
    }
}

fn validate_draft(draft: &FleetDraft) -> Result<(), String> {
    for (label, value) in [
        ("name", draft.name.as_str()),
        ("gh_repo", draft.gh_repo.as_str()),
        ("local", draft.local.as_str()),
        ("worktree_dir", draft.worktree_dir.as_str()),
        ("orch", draft.orch.as_str()),
        ("models.orch", draft.model_orch.as_str()),
        ("models.impl", draft.model_impl.as_str()),
        ("models.review", draft.model_review.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(format!("{label} must not be empty"));
        }
    }
    for (label, value) in [
        ("name", draft.name.as_str()),
        ("gh_repo", draft.gh_repo.as_str()),
        ("models.orch", draft.model_orch.as_str()),
        ("models.impl", draft.model_impl.as_str()),
        ("models.review", draft.model_review.as_str()),
    ] {
        if value.chars().any(char::is_whitespace) {
            return Err(format!("{label} must not contain whitespace"));
        }
    }
    let mut repo_parts = draft.gh_repo.split('/');
    if repo_parts.next().is_none_or(str::is_empty)
        || repo_parts.next().is_none_or(str::is_empty)
        || repo_parts.next().is_some()
    {
        return Err("gh_repo must be owner/repo".into());
    }
    for (label, value) in [
        ("models.impl_alt", draft.model_impl_alt.as_str()),
        ("models.impl_alt2", draft.model_impl_alt2.as_str()),
    ] {
        if value != "unset" && value.chars().any(char::is_whitespace) {
            return Err(format!("{label} must not contain whitespace"));
        }
    }
    if draft.name == "all" {
        return Err("name all is reserved for fleet models wildcard operations".into());
    }
    if draft.local.starts_with('~') && !draft.local.starts_with("~/") {
        return Err("local must use ~/path or an absolute/relative path".into());
    }
    if !is_safe_worktree_dir(&draft.worktree_dir) {
        return Err("worktree_dir must be one safe path component".into());
    }
    let _ = parse_workers(&draft.workers)?;
    Ok(())
}

fn update_raw_fleet(
    path: &Path,
    draft: &FleetDraft,
    paused: Option<bool>,
    validate_candidate: &CandidateValidator,
) -> Result<(), String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut root: serde_json::Value = serde_json::from_str(&source)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
    let fleets = root
        .get_mut("fleets")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| "registry has no fleets array".to_string())?;
    let target_index = fleets
        .iter()
        .position(|fleet| {
            fleet.get("name").and_then(serde_json::Value::as_str)
                == Some(draft.original_name.as_str())
        })
        .ok_or_else(|| format!("fleet {} was not found", draft.original_name))?;
    if !draft.gh_repo.is_empty() {
        validate_draft(draft)?;
        if !draft.source_fingerprint.is_empty() {
            let current_fingerprint =
                raw_fleet_fingerprint(fleets.get(target_index).ok_or_else(|| {
                    "fleet entry disappeared while reading registry".to_string()
                })?)?;
            if current_fingerprint != draft.source_fingerprint {
                return Err(
                    "registry changed since this draft was loaded; refresh before saving to avoid losing another update"
                        .into(),
                );
            }
        }
        let duplicate = fleets.iter().enumerate().any(|(index, fleet)| {
            index != target_index
                && fleet.get("name").and_then(serde_json::Value::as_str)
                    == Some(draft.name.as_str())
        });
        if duplicate {
            return Err(format!("fleet {} already exists", draft.name));
        }
    }
    let target = fleets
        .get_mut(target_index)
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| "fleet entry is not an object".to_string())?;
    if !draft.gh_repo.is_empty() {
        set_string(target, "name", &draft.name);
        set_string(target, "gh_repo", &draft.gh_repo);
        set_string(target, "local", &draft.local);
        set_string(target, "worktree_dir", &draft.worktree_dir);
        set_string(target, "orch", &draft.orch);
        target.insert(
            "workers".to_string(),
            serde_json::Value::Array(
                parse_workers(&draft.workers)?
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
        let models = target
            .get_mut("models")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| "fleet entry has no models object".to_string())?;
        set_string(models, "orch", &draft.model_orch);
        set_string(models, "impl", &draft.model_impl);
        set_string(models, "review", &draft.model_review);
        set_optional_string(models, "impl_alt", &draft.model_impl_alt);
        set_optional_string(models, "impl_alt2", &draft.model_impl_alt2);
    }
    if let Some(paused) = paused {
        if paused {
            target.insert("paused".to_string(), serde_json::Value::Bool(true));
        } else {
            target.remove("paused");
        }
    }
    let encoded = serde_json::to_vec_pretty(&root).map_err(|error| error.to_string())?;
    validate_candidate(path, &encoded)?;
    atomic_replace(path, &encoded)
}

/// Validate the complete candidate with the same shipped `corrald fleet
/// check` command that operators use. The candidate is written to a private
/// sibling path and removed before this function returns; the live registry
/// is never touched on a validation failure, so every rejection is
/// byte-identical and cannot self-lock-out the daemon.
fn validate_candidate_with_corrald(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("cannot create candidate validation stamp: {error}"))?
        .as_nanos();
    let candidate = parent.join(format!(
        ".{}.corral-ui-validate-{}-{stamp}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("fleets.json"),
        std::process::id()
    ));
    std::fs::write(&candidate, bytes).map_err(|error| {
        format!(
            "cannot write candidate registry {}: {error}",
            candidate.display()
        )
    })?;
    if let Ok(metadata) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&candidate, metadata.permissions());
    }
    let binary = resolve_corrald_binary()?;
    let output = Command::new(binary)
        .args(["fleet", "check", "--registry"])
        .arg(&candidate)
        .stdin(std::process::Stdio::null())
        .output();
    let _ = std::fs::remove_file(&candidate);
    let output = output.map_err(|error| format_corrald_run_error(&error))?;
    if output.status.success() {
        Ok(())
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        Err(format!(
            "corrald fleet check rejected candidate (exit {}): {}",
            output.status,
            if detail.is_empty() {
                "no diagnostic"
            } else {
                &detail
            }
        ))
    }
}

fn parse_workers(value: &str) -> Result<Vec<String>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "none" {
        return Ok(Vec::new());
    }
    trimmed
        .split(',')
        .map(str::trim)
        .enumerate()
        .map(|(index, worker)| {
            if worker.is_empty() {
                Err(format!("workers[{index}] must not be empty"))
            } else {
                Ok(worker.to_string())
            }
        })
        .collect()
}

fn is_safe_worktree_dir(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
}

fn raw_fleet_fingerprint(value: &serde_json::Value) -> Result<String, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "fleet entry is not an object".to_string())?;
    let models = object
        .get("models")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "fleet entry has no models object".to_string())?;
    let name = required_string(object, "name")?;
    let workers = object
        .get("workers")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "fleet entry workers is not an array".to_string())?
        .iter()
        .map(|worker| {
            worker
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "fleet worker is not a string".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let draft = FleetDraft {
        original_name: name.to_string(),
        source_fingerprint: String::new(),
        name: name.to_string(),
        gh_repo: required_string(object, "gh_repo")?.to_string(),
        local: required_string(object, "local")?.to_string(),
        worktree_dir: required_string(object, "worktree_dir")?.to_string(),
        orch: required_string(object, "orch")?.to_string(),
        workers,
        paused: object
            .get("paused")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        model_orch: required_string(models, "orch")?.to_string(),
        model_impl: required_string(models, "impl")?.to_string(),
        model_review: required_string(models, "review")?.to_string(),
        model_impl_alt: models
            .get("impl_alt")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unset")
            .to_string(),
        model_impl_alt2: models
            .get("impl_alt2")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unset")
            .to_string(),
        reasoning_effort: reasoning_effort_text(models.get("reasoning_effort")),
    };
    Ok(fingerprint_for_draft(&draft))
}

fn required_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("registry field {key} is not a string"))
}

fn fingerprint_for_draft(draft: &FleetDraft) -> String {
    let mut digest = Sha256::new();
    let workers = parse_workers(&draft.workers)
        .unwrap_or_else(|_| vec![draft.workers.trim().to_string()])
        .join(",");
    for (key, value) in [
        ("name", draft.name.as_str()),
        ("gh_repo", draft.gh_repo.as_str()),
        ("local", draft.local.as_str()),
        ("worktree_dir", draft.worktree_dir.as_str()),
        ("orch", draft.orch.as_str()),
        ("workers", workers.as_str()),
        ("paused", if draft.paused { "true" } else { "false" }),
        ("models.orch", draft.model_orch.as_str()),
        ("models.impl", draft.model_impl.as_str()),
        ("models.review", draft.model_review.as_str()),
        ("models.impl_alt", draft.model_impl_alt.as_str()),
        ("models.impl_alt2", draft.model_impl_alt2.as_str()),
        ("models.reasoning_effort", draft.reasoning_effort.as_str()),
    ] {
        digest.update(key.as_bytes());
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value.as_bytes());
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn set_string(object: &mut serde_json::Map<String, serde_json::Value>, key: &str, value: &str) {
    object.insert(
        key.to_string(),
        serde_json::Value::String(value.to_string()),
    );
}

fn set_optional_string(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: &str,
) {
    if value.trim().is_empty() || value == "unset" {
        object.remove(key);
    } else {
        set_string(object, key, value);
    }
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.corral-ui-tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("fleets.json"),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes)
        .map_err(|error| format!("cannot write {}: {error}", tmp.display()))?;
    if let Ok(metadata) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, metadata.permissions());
    }
    std::fs::rename(&tmp, path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot atomically replace {}: {error}", path.display())
    })
}

#[allow(dead_code)]
pub(crate) fn paused_label(paused: bool) -> &'static str {
    if paused { "paused" } else { "active" }
}

pub(crate) fn workers_text(workers: &[String]) -> String {
    if workers.is_empty() {
        "none".to_string()
    } else {
        workers.join(", ")
    }
}

pub(crate) fn model_or_unset(value: Option<&String>) -> String {
    value.cloned().unwrap_or_else(|| "unset".to_string())
}

pub(crate) fn reasoning_effort_text(value: Option<&serde_json::Value>) -> String {
    let Some(value) = value else {
        return "unset".to_string();
    };
    match value {
        serde_json::Value::Null => "unset".to_string(),
        serde_json::Value::Object(map) if map.is_empty() => "unset".to_string(),
        serde_json::Value::Object(map) => {
            let mut parts: Vec<String> = map
                .iter()
                .map(|(key, value)| format!("{key}={}", scalar_text(value)))
                .collect();
            parts.sort();
            parts.join(", ")
        }
        other => other.to_string(),
    }
}

fn scalar_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_executable(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    fn authoritative_test_validator(path: &Path, bytes: &[u8]) -> Result<(), String> {
        if std::env::var_os("CORRALD_BIN").is_some() {
            validate_candidate_with_corrald(path, bytes)
        } else {
            corrald::fleet::config::load(path)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
    }

    #[test]
    fn pause_workers_and_model_helpers_have_distinct_states() {
        assert_eq!(paused_label(true), "paused");
        assert_eq!(paused_label(false), "active");
        assert_eq!(workers_text(&[]), "none");
        assert_eq!(workers_text(&["a".into(), "b".into()]), "a, b");
        assert_eq!(model_or_unset(None), "unset");
        assert_eq!(model_or_unset(Some(&"codex/x".into())), "codex/x");
    }

    #[test]
    fn reasoning_effort_renders_forward_keys() {
        assert_eq!(reasoning_effort_text(None), "unset");
        assert_eq!(
            reasoning_effort_text(Some(&serde_json::json!({
                "orch": "medium",
                "impl": "max"
            }))),
            "impl=max, orch=medium"
        );
    }

    #[test]
    fn send_success_text_does_not_claim_distribution() {
        let without_daemon_output = send_validation_success_message("");
        assert!(without_daemon_output.starts_with("validation-only passed"));
        assert!(without_daemon_output.contains("registry unchanged"));

        let with_daemon_output = send_validation_success_message("fleet check: 1 valid");
        assert!(with_daemon_output.contains("fleet check: 1 valid"));
        assert!(with_daemon_output.contains("no daemon distribution endpoint"));
    }

    #[test]
    fn corrald_resolution_honors_override_then_sibling_install_root_and_path() {
        let root = std::env::temp_dir().join(format!(
            "corral-ui-corrald-resolution-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock is after the Unix epoch")
                .as_nanos()
        ));
        let current_exe = root.join("bin/corrald-ui");
        let sibling = root.join("bin/corrald");
        let install_root = root.join("install");
        let installed = install_root.join("corrald");
        let path_dir = root.join("path");
        let path_binary = path_dir.join("corrald");
        write_executable(&sibling);
        write_executable(&installed);
        write_executable(&path_binary);
        let path_env = std::env::join_paths([path_dir.as_path()]).unwrap();

        assert_eq!(
            resolve_corrald_binary_from(
                Some(sibling.as_os_str()),
                Some(&current_exe),
                Some(&install_root),
                None,
                Some(&path_env),
            )
            .unwrap(),
            sibling
        );
        std::fs::remove_file(&sibling).unwrap();
        assert_eq!(
            resolve_corrald_binary_from(
                None,
                Some(&current_exe),
                Some(&install_root),
                None,
                Some(&path_env),
            )
            .unwrap(),
            installed
        );
        std::fs::remove_file(&installed).unwrap();
        assert_eq!(
            resolve_corrald_binary_from(
                None,
                Some(&current_exe),
                Some(&install_root),
                None,
                Some(&path_env),
            )
            .unwrap(),
            path_binary
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrald_resolution_error_explains_corrald_bin_override() {
        let error = resolve_corrald_binary_from(
            None,
            Some(Path::new("/missing/corrald-ui")),
            Some(Path::new("/missing/install")),
            Some(Path::new("/missing/home")),
            Some(OsStr::new("")),
        )
        .unwrap_err();
        assert!(error.contains("CORRALD_BIN"));
        assert!(error.contains("full path"));
    }

    #[test]
    fn corrald_resolution_finds_the_per_user_release_install() {
        let root = std::env::temp_dir().join(format!(
            "corral-ui-corrald-home-resolution-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock is after the Unix epoch")
                .as_nanos()
        ));
        let home = root.join("home");
        let release = home.join(".local/share/corral/release/corrald");
        write_executable(&release);

        assert_eq!(
            resolve_corrald_binary_from(
                None,
                Some(Path::new("/missing/corrald-ui")),
                Some(Path::new("/missing/install")),
                Some(&home),
                Some(OsStr::new("")),
            )
            .unwrap(),
            release
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refreshed_registry_projection_detects_a_stale_draft_without_discarding_it() {
        let entry = FleetRegistryEntry {
            name: "corral".into(),
            gh_repo: "owner/corral".into(),
            local: "/tmp/corral".into(),
            worktree_dir: "corral".into(),
            orch: "orch-corral".into(),
            workers: vec!["worker-a".into()],
            paused: false,
            models: crate::model::FleetModels {
                orch: "codex/orch".into(),
                impl_: "codex/impl".into(),
                review: "claude/review".into(),
                impl_alt: None,
                impl_alt2: None,
                reasoning_effort: None,
            },
        };
        let mut draft = FleetDraft::from(&entry);
        draft.local = "/tmp/operator-edit".into();
        let mut refreshed = entry.clone();
        refreshed.workers = vec!["another-operator".into()];
        assert!(!draft_source_matches_entry(&draft, &refreshed));
        assert_eq!(draft.local, "/tmp/operator-edit");
    }

    #[test]
    fn draft_validation_rejects_bad_repositories() {
        let draft = FleetDraft {
            original_name: "corral".into(),
            source_fingerprint: String::new(),
            name: "corral".into(),
            gh_repo: "corral".into(),
            local: "/tmp/corral".into(),
            worktree_dir: "/tmp/worktrees".into(),
            orch: "orch".into(),
            workers: String::new(),
            paused: false,
            model_orch: "orch/model".into(),
            model_impl: "impl/model".into(),
            model_review: "review/model".into(),
            model_impl_alt: String::new(),
            model_impl_alt2: String::new(),
            reasoning_effort: "unset".into(),
        };
        assert!(validate_draft(&draft).is_err());
    }

    #[test]
    fn edits_are_atomic_and_preserve_forward_compatible_registry_fields() {
        let root = std::env::temp_dir().join(format!(
            "corral-ui-registry-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock is after the Unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir(&root).expect("create isolated registry test directory");
        let path = root.join("fleets.json");
        let local = std::env::var("CORRAL_UI_TEST_REPO")
            .unwrap_or_else(|_| std::env::current_dir().unwrap().display().to_string());
        std::fs::write(
            &path,
            serde_json::json!({
                "admit": {"default": "paused"},
                "fleets": [{
                    "name": "corral",
                    "gh_repo": "owner/corral",
                    "local": local,
                    "worktree_dir": "corral",
                    "orch": "orch-corral",
                    "workers": ["worker-a"],
                    "models": {
                        "orch": "codex/orch",
                        "impl": "codex/impl",
                        "review": "claude/review",
                        "reasoning_effort": {"impl": "high"},
                        "future_model_slot": "preserve-me"
                    },
                    "future_fleet_field": {"keep": true}
                }]
            })
            .to_string(),
        )
        .expect("write registry fixture");
        let draft = FleetDraft {
            original_name: "corral".into(),
            source_fingerprint: String::new(),
            name: "corral-renamed".into(),
            gh_repo: "owner/corral".into(),
            local: local.clone(),
            worktree_dir: "corral".into(),
            orch: "orch-corral".into(),
            workers: "worker-a, worker-b".into(),
            paused: false,
            model_orch: "codex/orch".into(),
            model_impl: "codex/impl-v2".into(),
            model_review: "claude/review".into(),
            model_impl_alt: "unset".into(),
            model_impl_alt2: "codex/fallback".into(),
            reasoning_effort: "impl=high".into(),
        };

        apply_draft_with_validator(
            path.to_str().unwrap(),
            &draft,
            &authoritative_test_validator,
        )
        .expect("apply valid registry draft");
        let updated: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let fleet = &updated["fleets"][0];
        assert_eq!(fleet["name"], "corral-renamed");
        assert_eq!(fleet["models"]["impl"], "codex/impl-v2");
        assert_eq!(
            fleet["workers"],
            serde_json::json!(["worker-a", "worker-b"])
        );
        assert_eq!(fleet["future_fleet_field"]["keep"], true);
        assert_eq!(fleet["models"]["future_model_slot"], "preserve-me");
        assert_eq!(fleet["models"]["reasoning_effort"]["impl"], "high");
        assert!(
            !std::fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .contains("corral-ui-tmp"))
        );

        set_paused_with_validator(
            path.to_str().unwrap(),
            "corral-renamed",
            true,
            &authoritative_test_validator,
        )
        .expect("pause the renamed fleet");
        let paused: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(paused["fleets"][0]["paused"], true);
        assert_eq!(paused["admit"]["default"], "paused");
        assert_eq!(paused["fleets"][0]["future_fleet_field"]["keep"], true);

        std::fs::remove_dir_all(root).expect("remove isolated registry test directory");
    }

    #[test]
    fn authoritative_rejections_leave_the_registry_byte_identical() {
        let root = std::env::temp_dir().join(format!(
            "corral-ui-registry-rejection-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock is after the Unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir(&root).expect("create isolated registry test directory");
        let path = root.join("fleets.json");
        let local = std::env::var("CORRAL_UI_TEST_REPO")
            .unwrap_or_else(|_| std::env::current_dir().unwrap().display().to_string());
        let source = serde_json::json!({
            "admit": {"preserve": true},
            "fleets": [
                {
                    "name": "corral",
                    "gh_repo": "owner/corral",
                    "local": local,
                    "worktree_dir": "corral",
                    "orch": "orch-corral",
                    "workers": ["worker-a"],
                    "models": {
                        "orch": "codex/orch",
                        "impl": "codex/impl",
                        "review": "claude/review"
                    },
                    "future": {"keep": true}
                },
                {
                    "name": "other",
                    "gh_repo": "owner/other",
                    "local": local,
                    "worktree_dir": "other",
                    "orch": "orch-other",
                    "workers": [],
                    "models": {
                        "orch": "codex/orch",
                        "impl": "codex/impl",
                        "review": "claude/review"
                    }
                }
            ]
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&source).unwrap())
            .expect("write registry fixture");
        let source_bytes = std::fs::read(&path).unwrap();
        let source_fingerprint = raw_fleet_fingerprint(&source["fleets"][0]).unwrap();
        let base = FleetDraft {
            original_name: "corral".into(),
            source_fingerprint,
            name: "corral".into(),
            gh_repo: "owner/corral".into(),
            local: local.clone(),
            worktree_dir: "corral".into(),
            orch: "orch-corral".into(),
            workers: "worker-a".into(),
            paused: false,
            model_orch: "codex/orch".into(),
            model_impl: "codex/impl".into(),
            model_review: "claude/review".into(),
            model_impl_alt: "unset".into(),
            model_impl_alt2: "unset".into(),
            reasoning_effort: "unset".into(),
        };
        type DraftMutation = fn(&mut FleetDraft);
        let cases: [(&str, DraftMutation); 5] = [
            ("unsafe worktree_dir", |draft: &mut FleetDraft| {
                draft.worktree_dir = "../escape".into();
            }),
            ("bare tilde local", |draft: &mut FleetDraft| {
                draft.local = "~not-expanded".into();
            }),
            ("reserved all", |draft: &mut FleetDraft| {
                draft.name = "all".into();
            }),
            ("model whitespace", |draft: &mut FleetDraft| {
                draft.model_impl = "codex/impl model".into();
            }),
            ("duplicate name", |draft: &mut FleetDraft| {
                draft.name = "other".into();
            }),
        ];
        for (label, mutate) in cases {
            let mut draft = base.clone();
            mutate(&mut draft);
            assert!(
                apply_draft_with_validator(
                    &path.to_string_lossy(),
                    &draft,
                    &authoritative_test_validator
                )
                .is_err(),
                "{label} must be rejected"
            );
            assert_eq!(
                std::fs::read(&path).unwrap(),
                source_bytes,
                "{label} changed the live file"
            );
        }

        let stale = base.clone();
        let mut changed = source.clone();
        changed["fleets"][0]["workers"] = serde_json::json!(["operator-update"]);
        std::fs::write(&path, serde_json::to_vec_pretty(&changed).unwrap()).unwrap();
        assert!(
            apply_draft_with_validator(
                &path.to_string_lossy(),
                &stale,
                &authoritative_test_validator
            )
            .is_err(),
            "a stale draft must not overwrite another operator's update"
        );
        let changed_bytes = std::fs::read(&path).unwrap();
        assert_eq!(changed_bytes, serde_json::to_vec_pretty(&changed).unwrap());

        std::fs::remove_dir_all(root).expect("remove isolated registry test directory");
    }
}
