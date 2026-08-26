//! Editable fleet registry surface for #206.
//!
//! The daemon projection remains the read path. Mutations are deliberately
//! explicit: the client edits the returned `fleets.json` in place with an
//! atomic replacement, then uses the shipped `corrald fleet check` command for
//! the Send to fleet verification path. Unknown registry keys are preserved by
//! updating only the fields owned by this form.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use eframe::egui::{RichText, ScrollArea, TextEdit, Ui};

use crate::model::{FleetRegistry, FleetRegistryEntry};
use crate::state::Level;
use crate::theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetDraft {
    pub original_name: String,
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
        Self {
            original_name: fleet.name.clone(),
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
        }
    }
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
            "Edit fleets.json in place. Save & apply writes the current fleet atomically; Send to fleet validates the shared registry; Pause changes the fleet's live admission flag.",
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
                        registry.path
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
                if ui.button("Send to fleet").clicked() {
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
    validate_draft(draft)?;
    update_raw_fleet(Path::new(path), draft, Some(draft.paused))
}

/// Change only the pause bit while retaining every other registry field.
pub fn set_paused(path: &str, name: &str, paused: bool) -> Result<(), String> {
    let draft = FleetDraft {
        original_name: name.to_string(),
        name: name.to_string(),
        gh_repo: String::new(),
        local: String::new(),
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
    update_raw_fleet(Path::new(path), &draft, Some(paused))
}

/// Run the repository's real fleet validation path. This is intentionally
/// reported as validation rather than pretending there is a daemon mutation
/// endpoint for registry distribution in the #206 layout-only scope.
pub fn send_to_fleet(path: &str) -> Result<String, String> {
    let binary = std::env::var_os("CORRALD_BIN").unwrap_or_else(|| "corrald".into());
    let output = Command::new(binary)
        .args(["fleet", "check", "--registry", path])
        .output()
        .map_err(|error| format!("could not run corrald fleet check: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        Ok(if stdout.is_empty() {
            "registry checked".into()
        } else {
            stdout
        })
    } else {
        Err(if stderr.is_empty() { stdout } else { stderr })
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
    if draft.name.chars().any(char::is_whitespace) {
        return Err("name must not contain whitespace".into());
    }
    let mut repo_parts = draft.gh_repo.split('/');
    if repo_parts.next().is_none_or(str::is_empty)
        || repo_parts.next().is_none_or(str::is_empty)
        || repo_parts.next().is_some()
    {
        return Err("gh_repo must be owner/repo".into());
    }
    Ok(())
}

fn update_raw_fleet(path: &Path, draft: &FleetDraft, paused: Option<bool>) -> Result<(), String> {
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
                draft
                    .workers
                    .split(',')
                    .map(str::trim)
                    .filter(|worker| !worker.is_empty())
                    .map(|worker| serde_json::Value::String(worker.to_string()))
                    .collect(),
            ),
        );
        if let Some(models) = target
            .get_mut("models")
            .and_then(serde_json::Value::as_object_mut)
        {
            set_string(models, "orch", &draft.model_orch);
            set_string(models, "impl", &draft.model_impl);
            set_string(models, "review", &draft.model_review);
            set_optional_string(models, "impl_alt", &draft.model_impl_alt);
            set_optional_string(models, "impl_alt2", &draft.model_impl_alt2);
        }
    }
    if let Some(paused) = paused {
        target.insert("paused".to_string(), serde_json::Value::Bool(paused));
    }
    let encoded = serde_json::to_vec_pretty(&root).map_err(|error| error.to_string())?;
    atomic_replace(path, &encoded)
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
    fn draft_validation_rejects_bad_repositories() {
        let draft = FleetDraft {
            original_name: "corral".into(),
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
        std::fs::write(
            &path,
            serde_json::json!({
                "admit": {"default": "paused"},
                "fleets": [{
                    "name": "corral",
                    "gh_repo": "owner/corral",
                    "local": "/tmp/corral",
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
            name: "corral-renamed".into(),
            gh_repo: "owner/corral".into(),
            local: "/tmp/corral-renamed".into(),
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

        apply_draft(path.to_str().unwrap(), &draft).expect("apply valid registry draft");
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

        set_paused(path.to_str().unwrap(), "corral-renamed", true)
            .expect("pause the renamed fleet");
        let paused: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(paused["fleets"][0]["paused"], true);
        assert_eq!(paused["admit"]["default"], "paused");
        assert_eq!(paused["fleets"][0]["future_fleet_field"]["keep"], true);

        std::fs::remove_dir_all(root).expect("remove isolated registry test directory");
    }
}
