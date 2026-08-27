//! #237: read-only fleet identities tab — configless re-scope of the old
//! Registry surface.
//!
//! Corral no longer owns, reads, or writes `fleets.json`; the registry is
//! fleet-ops' opinionated config (`herdr-fleet`). This tab displays the
//! daemon's `GET /fleets` catalog — the fleet-ops CLI validated identities —
//! and is deliberately READ-ONLY: mutations (add/remove/pause/resume/
//! models/switch) run through `herdr-fleet` on the host, never from the
//! client, so there is no fleets.json write path anywhere in Corral.
//!
//! Display repo categories are NEVER actionable identities; the action
//! targets the Issues tab offers are exactly the validated names shown here.

use std::collections::BTreeMap;

use eframe::egui::{RichText, ScrollArea, Ui};

use crate::model::{FleetIdentities, FleetIdentityEntry};
use crate::theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Refresh,
}

/// Render the read-only fleet identity catalog and return a deferred
/// refresh request.
pub fn show(
    ui: &mut Ui,
    view: &Option<Result<FleetIdentities, String>>,
    loading: bool,
    request: &mut dyn FnMut(Action),
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Fleets")
                .heading()
                .color(theme::ui::TEXT_STRONG),
        );
        ui.label(
            RichText::new("fleet-ops CLI validated identities")
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
            "Corral is configless: it never reads or writes the fleet registry \\
             file. These are the fleet-ops CLI validated fleet identities only \\
             (no model maps, no paths — those stay in fleet-ops). Use \\
             `herdr-fleet list|add|remove|pause|resume|models` on the host to \\
             change fleets; `corrald fleet switch <name>` re-arms via the \\
             fleet-ops CLI. Display repo categories are never actionable \\
             identities.",
        )
        .small()
        .color(theme::ui::TEXT_MUTED),
    );
    ui.add_space(8.0);

    match view {
        None => {
            ui.label(
                RichText::new(if loading {
                    "loading fleet identities…"
                } else {
                    "no fleet data yet — press refresh"
                })
                .color(theme::ui::TEXT_MUTED),
            );
        }
        Some(Err(error)) => {
            ui.label(
                RichText::new(format!("fleet identities unavailable: {error}"))
                    .strong()
                    .color(theme::ui::BAD),
            );
        }
        Some(Ok(fleets)) => {
            let (status, color) = if fleets.failed() {
                ("ERROR", theme::ui::BAD)
            } else {
                ("OK", theme::ui::GOOD)
            };
            ui.horizontal_wrapped(|ui| {
                crate::ui::badge(ui, status, color);
                ui.label(
                    RichText::new(format!("{} validated fleet(s)", fleets.fleets.len()))
                        .monospace()
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            });
            if let Some(error) = &fleets.error {
                ui.label(
                    RichText::new(format!("fleet-ops CLI error: {error}")).color(theme::ui::BAD),
                );
            }
            ui.separator();
            ScrollArea::vertical()
                .id_salt("corral-ui-fleets-list")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if fleets.fleets.is_empty() {
                        ui.label(
                            RichText::new("no validated fleets (the fleet-ops registry is empty)")
                                .color(theme::ui::TEXT_MUTED),
                        );
                    }
                    for fleet in &fleets.fleets {
                        fleet_row(ui, fleet);
                        ui.add_space(8.0);
                    }
                });
        }
    }
}

fn fleet_row(ui: &mut Ui, fleet: &FleetIdentityEntry) {
    eframe::egui::Frame::group(ui.style())
        .inner_margin(eframe::egui::Margin::symmetric(14, 10))
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
                    ("ACTIVE", theme::ui::GOOD)
                };
                crate::ui::badge(ui, label, color);
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("gh_repo: {}", fleet.gh_repo))
                        .monospace()
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
                ui.label(
                    RichText::new(format!("orch: {}", fleet.orch))
                        .monospace()
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
                ui.label(
                    RichText::new(format!("workers: {}", fleet.workers))
                        .monospace()
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            });
        });
}

/// Sort helper for deterministic tests: names only, in catalog order.
#[allow(dead_code)]
pub(crate) fn names(view: &Option<Result<FleetIdentities, String>>) -> Vec<String> {
    match view {
        Some(Ok(fleets)) if !fleets.failed() => {
            fleets.fleets.iter().map(|f| f.name.clone()).collect()
        }
        _ => Vec::new(),
    }
}

/// Keep the BTreeMap import used by callers of the old signature shape.
#[allow(dead_code)]
pub(crate) type FleetDraftMap = BTreeMap<String, ()>;
