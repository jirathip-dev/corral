//! #135: read-only fleet registry view (daemon `GET /fleet-registry`).
//!
//! Shows the configured `fleets.json` path, every fleet's identity fields,
//! model map (including forward-compatible `reasoning_effort`), and pause
//! state. Daemon-side parse failures and transport errors are rendered as
//! prominent failures rather than an empty registry.

use eframe::egui::{RichText, ScrollArea, Ui};

use crate::model::{FleetModels, FleetRegistry, FleetRegistryEntry};
use crate::theme;

/// Renders the registry tab. `view` is the last fetch outcome; `loading`
/// drives the spinner; `request_refresh` is the manual refresh button.
pub fn show(
    ui: &mut Ui,
    view: &Option<Result<FleetRegistry, String>>,
    loading: bool,
    request_refresh: &mut dyn FnMut(),
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("FLEET REGISTRY")
                .strong()
                .color(theme::ui::TEXT_STRONG),
        );
        if loading {
            ui.spinner();
        }
        if ui.button("refresh").clicked() {
            request_refresh();
        }
    });
    ui.label(
        RichText::new(
            "read-only view of the same registry /issues uses to group fleets; \
             no mutation is served here.",
        )
        .small()
        .color(theme::ui::TEXT_MUTED),
    );

    match view {
        None => {
            if loading {
                ui.add_space(12.0);
                ui.label(RichText::new("loading registry…").color(theme::ui::TEXT_MUTED));
            } else {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("no registry data yet — press refresh.")
                        .color(theme::ui::TEXT_MUTED),
                );
            }
        }
        Some(Err(error)) => {
            ui.add_space(12.0);
            ui.label(
                RichText::new(format!("registry unavailable: {error}"))
                    .strong()
                    .size(16.0)
                    .color(theme::ui::BAD),
            );
            ui.label(
                RichText::new("the endpoint/transport failed — press refresh to retry.")
                    .color(theme::ui::TEXT_MUTED),
            );
        }
        Some(Ok(registry)) => {
            let (status_text, status_color) = if registry.failed() {
                ("ERROR", theme::ui::BAD)
            } else {
                ("ok", theme::ui::GOOD)
            };
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(status_text)
                        .monospace()
                        .strong()
                        .color(status_color),
                );
                ui.label(
                    RichText::new(format!("path {}", registry.path))
                        .monospace()
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            });
            if let Some(error) = &registry.error {
                ui.label(
                    RichText::new(format!("registry error: {error}"))
                        .strong()
                        .size(16.0)
                        .color(theme::ui::BAD),
                );
            }
            ui.separator();
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if registry.fleets.is_empty() {
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(
                                "no fleets in the registry (or the registry failed to load).",
                            )
                            .color(theme::ui::TEXT_MUTED),
                        );
                    } else {
                        for fleet in &registry.fleets {
                            fleet_row(ui, fleet);
                            ui.add_space(8.0);
                        }
                    }
                });
        }
    }
}

fn fleet_row(ui: &mut Ui, fleet: &FleetRegistryEntry) {
    eframe::egui::Frame::group(ui.style())
        .inner_margin(eframe::egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&fleet.name)
                        .strong()
                        .monospace()
                        .color(theme::ui::TEXT_STRONG),
                );
                let (label, color) = if fleet.paused {
                    ("PAUSED", theme::ui::BAD)
                } else {
                    ("active", theme::ui::GOOD)
                };
                crate::ui::badge(ui, label, color);
            });
            detail_kv(ui, "name", &fleet.name);
            detail_kv(ui, "gh_repo", &fleet.gh_repo);
            detail_kv(ui, "local", &fleet.local);
            detail_kv(ui, "worktree_dir", &fleet.worktree_dir);
            detail_kv(ui, "orch", &fleet.orch);
            detail_kv(ui, "workers", &workers_text(&fleet.workers));
            detail_kv(ui, "paused", paused_label(fleet.paused));
            ui.add_space(4.0);
            models_block(ui, &fleet.models);
        });
}

fn models_block(ui: &mut Ui, models: &FleetModels) {
    ui.label(
        RichText::new("models")
            .strong()
            .small()
            .color(theme::ui::TEXT_MUTED),
    );
    detail_kv(ui, "orch", &models.orch);
    detail_kv(ui, "impl", &models.impl_);
    detail_kv(ui, "review", &models.review);
    detail_kv(ui, "impl_alt", &model_or_unset(models.impl_alt.as_ref()));
    detail_kv(ui, "impl_alt2", &model_or_unset(models.impl_alt2.as_ref()));
    detail_kv(
        ui,
        "reasoning_effort",
        &reasoning_effort_text(models.reasoning_effort.as_ref()),
    );
}

fn detail_kv(ui: &mut Ui, key: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("{key}:"))
                .monospace()
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        ui.label(
            RichText::new(value)
                .monospace()
                .small()
                .color(theme::ui::TEXT_STRONG),
        );
    });
}

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
    fn reasoning_effort_renders_forward_keys_and_never_panics_when_absent() {
        assert_eq!(reasoning_effort_text(None), "unset");
        assert_eq!(
            reasoning_effort_text(Some(&serde_json::Value::Null)),
            "unset"
        );
        assert_eq!(
            reasoning_effort_text(Some(&serde_json::json!({
                "orch": "medium",
                "impl": "max",
                "review": "xhigh",
                "future_effort": "high"
            }))),
            "future_effort=high, impl=max, orch=medium, review=xhigh"
        );
    }
}
