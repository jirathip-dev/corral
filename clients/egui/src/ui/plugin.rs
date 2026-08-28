//! Fleet Ops sidecar surface. The daemon remains the source of card values and
//! action execution; this widget only provides the bounded board shell.

use crate::theme;
use eframe::egui::{RichText, Ui};

pub fn show(ui: &mut Ui) {
    ui.add_space(12.0);
    ui.heading(RichText::new("Fleet Ops").color(theme::ui::TEXT_STRONG));
    ui.label(
        RichText::new("Sidecar plugin")
            .small()
            .color(theme::ui::TEXT_MUTED),
    );
    ui.columns(3, |columns| {
        for (column, title) in
            columns
                .iter_mut()
                .zip(["Registry", "Orchestrator state", "Admission state"])
        {
            egui::Frame::group(column.style()).show(column, |ui| {
                ui.label(RichText::new(title).strong().color(theme::ui::TEXT_STRONG));
                ui.label(
                    RichText::new("Waiting for fleet-ops plugin…").color(theme::ui::TEXT_MUTED),
                );
            });
        }
    });
}
