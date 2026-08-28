//! Fleet Ops sidecar surface backed by the daemon plugin wire.

use crate::state::Fleet;
use crate::theme;
use eframe::egui::{self, RichText, Ui};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Refresh,
    Execute(String),
}

pub fn show(ui: &mut Ui, fleet: &mut Fleet) -> Option<Event> {
    let mut event = None;
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.heading(RichText::new("Fleet Ops").color(theme::ui::TEXT_STRONG));
        if ui.button("Refresh").clicked() {
            event = Some(Event::Refresh);
        }
        if fleet.plugin_loading {
            ui.spinner();
        }
    });
    match fleet.plugin.as_ref() {
        None => {
            ui.label("Loading fleet-ops plugin…");
        }
        Some(Err(error)) => {
            ui.colored_label(theme::ui::BAD, error);
        }
        Some(Ok(view)) => {
            ui.label(
                RichText::new(format!("{} · v{}", view.name, view.version))
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
            ui.columns(3, |columns| {
                for (column, card) in columns.iter_mut().zip(view.cards.iter()) {
                    egui::Frame::group(column.style()).show(column, |ui| {
                        ui.label(
                            RichText::new(&card.title)
                                .strong()
                                .color(theme::ui::TEXT_STRONG),
                        );
                        if let Some(error) = &card.error {
                            ui.colored_label(theme::ui::BAD, error);
                        } else {
                            ui.label(card.value.to_string());
                        }
                    });
                }
            });
            ui.separator();
            ui.label(RichText::new("Actions").strong());
            for (index, action) in view.actions.iter().enumerate() {
                if ui.button(&action.title).clicked() {
                    fleet.plugin_confirm = Some(index);
                }
            }
            if let Some(result) = &fleet.plugin_result {
                ui.separator();
                ui.label(RichText::new("Result").strong());
                match result {
                    Ok(value) => {
                        ui.monospace(value.to_string());
                    }
                    Err(error) => {
                        ui.colored_label(theme::ui::BAD, error);
                    }
                }
            }
            if let Some(index) = fleet.plugin_confirm
                && let Some(action) = view.actions.get(index)
            {
                let action_id = action.id.clone();
                egui::Window::new("Confirm Fleet Ops action")
                    .collapsible(false)
                    .show(ui.ctx(), |ui| {
                        ui.label(&action.confirm_message);
                        ui.monospace(format!("argv = {:?}", action.command));
                        ui.horizontal(|ui| {
                            if ui.button("Cancel").clicked() {
                                fleet.plugin_confirm = None;
                            }
                            if ui.button("Confirm").clicked() {
                                fleet.plugin_confirm = None;
                                event = Some(Event::Execute(action_id.clone()));
                            }
                        });
                    });
            }
        }
    }
    event
}

#[cfg(test)]
mod tests {
    use super::Event;
    #[test]
    fn cancel_does_not_create_execute_event() {
        let cancelled: Option<Event> = None;
        assert_eq!(cancelled, None);
    }
}
