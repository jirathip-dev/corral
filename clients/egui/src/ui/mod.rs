//! Shared UI widgets: badge rendering, connection pill, banner/toast area.

pub mod audit;
pub mod board;
pub mod issues;
pub mod register;
pub mod registry;

use eframe::egui::{Color32, CornerRadius, FontId, RichText, Sense, Stroke, Ui};

use crate::state::{ConnState, Level};

/// Render a small colored pill with `text` (used for state, waiting-on
/// kinds, CI verdicts).
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    let font = FontId::monospace(11.0);
    let galley = ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_string(), font, color));
    let size = galley.size() + egui::vec2(12.0, 6.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    let bg = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 40);
    painter.rect_filled(rect, CornerRadius::same(4), bg);
    painter.rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0, color),
        egui::StrokeKind::Outside,
    );
    painter.galley(rect.min + egui::vec2(6.0, 3.0), galley, color);
}

/// A dot + label pill describing the SSE connection.
pub fn connection_pill(ui: &mut Ui, state: ConnState) {
    let (color, label) = match state {
        ConnState::Connected => (crate::theme::ui::ACCENT, "live".to_string()),
        ConnState::Connecting => (crate::theme::ui::WARN, "connecting".to_string()),
        ConnState::Reconnecting { backoff_ms } => (
            crate::theme::ui::WARN,
            format!("reconnecting ({backoff_ms}ms)"),
        ),
        ConnState::Down => (crate::theme::ui::BAD, "down".to_string()),
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), 3.5, color);
    ui.add_space(4.0);
    ui.label(
        RichText::new(label)
            .monospace()
            .color(crate::theme::ui::TEXT_MUTED),
    );
}

/// Render queued toasts (errors/warnings/info) in the top-right corner.
pub fn toast_area(
    ctx: &egui::Context,
    toasts: &mut std::collections::VecDeque<crate::state::Toast>,
) {
    const LIFETIME: std::time::Duration = std::time::Duration::from_secs(10);
    toasts.retain(|t| t.at.elapsed() < LIFETIME);
    if toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("corral-ui-toasts"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 48.0))
        .show(ctx, |ui| {
            ui.set_max_width(420.0);
            for toast in toasts.iter() {
                let color = match toast.level {
                    Level::Info => crate::theme::ui::GOOD,
                    Level::Warn => crate::theme::ui::WARN,
                    Level::Error => crate::theme::ui::BAD,
                };
                egui::Frame::popup(ui.style())
                    .fill(crate::theme::ui::PANEL3)
                    .stroke(Stroke::new(1.0, color))
                    .corner_radius(CornerRadius::same(6))
                    .show(ui, |ui| {
                        ui.label(RichText::new(&toast.text).color(crate::theme::ui::TEXT_STRONG));
                    });
                ui.add_space(4.0);
            }
        });
}

/// Helper for a disabled-with-reason capability button.
pub fn disabled_button_with_reason(ui: &mut Ui, label: &str, reason: &str) {
    let response = ui.add_enabled_ui(false, |ui| ui.button(label)).inner;
    ui.interact(
        response.rect,
        ui.auto_id_with(("disabled-button-blocker", label)),
        egui::Sense::click(),
    )
    .on_hover_text(reason);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conn_state_labels_cover_all_states() {
        for s in [
            ConnState::Connecting,
            ConnState::Connected,
            ConnState::Reconnecting { backoff_ms: 100 },
            ConnState::Down,
        ] {
            assert!(!s.label().is_empty());
        }
        assert_eq!(ConnState::Connected.label(), "live");
        assert_eq!(ConnState::Down.label(), "down");
    }
}
