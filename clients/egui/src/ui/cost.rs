//! Cost-meter dashboard (G34): one tile per provider showing the rolling
//! 5h / weekly / monthly windows as USD + % of cap, colour-graded by the
//! daemon's alert status. Three honesty rules, all enforced here:
//!
//! - A percentage against a **placeholder cap** is prefixed `~` and the
//!   tile is annotated — an invented cap must never look authoritative.
//! - A provider with **no session store** renders "no store", never
//!   `$0.00 / 0%` (which would read as "you have spent nothing").
//! - A `problem` in any window is the **before-exhaustion alert** and
//!   renders as a banner, not just a coloured digit.

use eframe::egui::{Color32, RichText, Ui};

use crate::model::{CostProviderUsage, CostReport, CostStatus, CostWindowUsage};
use crate::theme;

/// Body text when a provider's session store is absent — shown plainly
/// instead of fabricating `$0.00 / 0%`.
pub const NO_STORE_LABEL: &str = "no store";

/// The before-exhaustion alert banner (any window at/above the alert
/// threshold). Deliberately loud — this is the "flag before agents idle"
/// signal, not a routine stat.
pub const EXHAUSTION_ALERT: &str = "EXHAUSTION RISK — WINDOW AT/ABOVE ALERT THRESHOLD";

/// Palette for an alert status, from the shared theme (never hardcoded).
pub fn status_color(status: CostStatus) -> Color32 {
    match status {
        CostStatus::Ok => theme::ui::GOOD,
        CostStatus::Warning => theme::ui::WARN,
        CostStatus::Problem => theme::ui::BAD,
    }
}

/// The `% of cap` text for one window. A `~` prefix marks a percentage
/// computed against a placeholder cap (invented until real plan limits are
/// configured) so it is never presented as authoritative.
pub fn pct_text(window: &CostWindowUsage) -> String {
    let pct = format!("{:.1}%", norm(window.pct_of_cap));
    if window.cap_is_placeholder {
        format!("~{pct}")
    } else {
        pct
    }
}

/// The USD amount for one window, 2dp.
pub fn usd_text(window: &CostWindowUsage) -> String {
    format!("${:.2}", norm(window.usd))
}

/// Squash floating-point negative zero and denormal noise to a clean `0.0`
/// for display — the daemon can hand back `-0.0` for an empty window, and
/// `-0.0%` / `$-0.00` read as a bug even though the number is honest.
fn norm(x: f64) -> f64 {
    if x.abs() < 1e-9 { 0.0 } else { x }
}

/// `Some(NO_STORE_LABEL)` when the provider has no session store — the
/// tile body says so plainly instead of rendering zeroes as spend.
pub fn store_marker(provider: &CostProviderUsage) -> Option<&'static str> {
    if provider.store_found {
        None
    } else {
        Some(NO_STORE_LABEL)
    }
}

/// Whether any of a provider's windows is at/above the alert threshold —
/// the before-exhaustion signal that triggers the banner.
pub fn has_exhaustion_risk(provider: &CostProviderUsage) -> bool {
    provider
        .windows
        .iter()
        .any(|w| w.status == CostStatus::Problem)
}

/// Whether any window uses a placeholder cap — the tile annotates it.
pub fn has_placeholder_cap(provider: &CostProviderUsage) -> bool {
    provider.windows.iter().any(|w| w.cap_is_placeholder)
}

/// One window's summary line, e.g. `5h   $12.34   ~12.3%`.
pub fn window_summary(window: &CostWindowUsage) -> String {
    format!(
        "{}  {}  {}",
        window.window.label(),
        usd_text(window),
        pct_text(window)
    )
}

/// Renders the cost-meter dashboard: a header row plus one tile per
/// provider. `cost` is the app-held [`crate::state::CostState`] report
/// (`None` before the first poll, `Err` degrades to "unknown").
pub fn show(ui: &mut Ui, report: &Option<Result<CostReport, String>>) {
    match report {
        None => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("COST").strong().color(theme::ui::TEXT_STRONG));
                ui.label(
                    RichText::new("waiting for corrald…")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            });
        }
        Some(Err(e)) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("COST").strong().color(theme::ui::TEXT_STRONG));
                ui.label(
                    RichText::new(format!("unavailable: {e}"))
                        .small()
                        .color(theme::ui::WARN),
                )
                .on_hover_text("a missing or malformed /cost response degrades to unknown — the fleet board is unaffected");
            });
        }
        Some(Ok(report)) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("COST").strong().color(theme::ui::TEXT_STRONG));
                ui.label(
                    RichText::new(format!(
                        "generated {}",
                        crate::model::clock_of(report.generated_at)
                    ))
                    .small()
                    .color(theme::ui::TEXT_MUTED),
                );
                ui.label(
                    RichText::new("polled every 60s")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.spacing_mut().item_spacing.y = 8.0;
                for provider in &report.providers {
                    tile(ui, provider);
                }
            });
        }
    }
}

/// One provider tile: name, the three windows as USD + % of cap (coloured
/// by status), the placeholder-cap annotation when applicable, and the
/// before-exhaustion banner on any `problem`.
fn tile(ui: &mut Ui, provider: &CostProviderUsage) {
    let risk = has_exhaustion_risk(provider);
    let bg = if risk {
        theme::ui::BAD.gamma_multiply(0.12)
    } else {
        Color32::from_rgb(0x10, 0x15, 0x1c)
    };
    egui::Frame::NONE
        .fill(bg)
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(0x30, 0x36, 0x3d)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(240.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(provider.provider.as_str())
                        .monospace()
                        .strong()
                        .color(theme::ui::TEXT_STRONG),
                );
            });
            match store_marker(provider) {
                Some(marker) => {
                    ui.label(RichText::new(marker).small().color(theme::ui::TEXT_MUTED));
                }
                None => {
                    for window in &provider.windows {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(window.window.label())
                                    .monospace()
                                    .small()
                                    .color(theme::ui::TEXT_MUTED),
                            );
                            ui.label(
                                RichText::new(usd_text(window))
                                    .monospace()
                                    .small()
                                    .color(theme::ui::TEXT_STRONG),
                            );
                            ui.label(
                                RichText::new(pct_text(window))
                                    .monospace()
                                    .small()
                                    .color(status_color(window.status)),
                            )
                            .on_hover_text(if window.cap_is_placeholder {
                                "percentage is against a placeholder cap (CORRAL_COST_CAP_* unset) — provisional, not your real limit"
                            } else {
                                "percentage of the configured cap"
                            });
                        });
                    }
                }
            }
            if provider.store_found && has_placeholder_cap(provider) {
                ui.label(
                    RichText::new("placeholder cap — percentages provisional")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            }
            if risk {
                ui.add_space(4.0);
                egui::Frame::NONE
                    .fill(theme::ui::BAD.gamma_multiply(0.18))
                    .stroke(egui::Stroke::new(1.5, theme::ui::BAD))
                    .corner_radius(egui::CornerRadius::same(4))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(EXHAUSTION_ALERT)
                                .strong()
                                .small()
                                .color(theme::ui::BAD),
                        );
                    });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(pct: f64, placeholder: bool) -> CostWindowUsage {
        CostWindowUsage {
            window: crate::model::CostWindow::FiveHour,
            usd: 1.0,
            cap_usd: 10.0,
            cap_is_placeholder: placeholder,
            pct_of_cap: pct,
            status: CostStatus::Ok,
        }
    }

    fn provider(found: bool) -> CostProviderUsage {
        CostProviderUsage {
            provider: crate::model::CostProvider::Opencode,
            store_found: found,
            windows: vec![
                CostWindowUsage {
                    window: crate::model::CostWindow::FiveHour,
                    usd: 0.5,
                    cap_usd: 5.0,
                    cap_is_placeholder: false,
                    pct_of_cap: 10.0,
                    status: CostStatus::Ok,
                },
                CostWindowUsage {
                    window: crate::model::CostWindow::Weekly,
                    usd: 0.0,
                    cap_usd: 35.0,
                    cap_is_placeholder: false,
                    pct_of_cap: 0.0,
                    status: CostStatus::Ok,
                },
                CostWindowUsage {
                    window: crate::model::CostWindow::Monthly,
                    usd: 0.0,
                    cap_usd: 140.0,
                    cap_is_placeholder: false,
                    pct_of_cap: 0.0,
                    status: CostStatus::Ok,
                },
            ],
        }
    }

    #[test]
    fn status_color_maps_all_statuses_from_the_theme() {
        assert_eq!(status_color(CostStatus::Ok), theme::ui::GOOD);
        assert_eq!(status_color(CostStatus::Warning), theme::ui::WARN);
        assert_eq!(status_color(CostStatus::Problem), theme::ui::BAD);
    }

    #[test]
    fn pct_text_marks_placeholder_caps_as_provisional() {
        // G34: a percentage against an invented cap must be visibly
        // provisional — `~` prefix, never presented as authoritative.
        assert_eq!(pct_text(&window(50.0, false)), "50.0%");
        assert_eq!(pct_text(&window(50.0, true)), "~50.0%");
        assert_eq!(pct_text(&window(0.0, true)), "~0.0%");
    }

    #[test]
    fn negative_zero_normalizes_for_display() {
        // The daemon can hand back `-0.0` for an empty window; it must
        // render as a clean `0.0%` / `$0.00`, never `-0.0%` / `$-0.00`.
        let mut w = window(0.0, false);
        w.usd = -0.0;
        assert_eq!(pct_text(&w), "0.0%");
        assert_eq!(usd_text(&w), "$0.00");
        assert_eq!(window_summary(&w), "5h  $0.00  0.0%");
        w.pct_of_cap = -0.0;
        assert_eq!(pct_text(&w), "0.0%");
    }

    #[test]
    fn store_marker_says_no_store_when_absent() {
        // G34: a missing store is "no data", never "$0.00 / 0%".
        let mut p = provider(true);
        assert_eq!(store_marker(&p), None);
        p.store_found = false;
        assert_eq!(store_marker(&p), Some(NO_STORE_LABEL));
    }

    #[test]
    fn has_exhaustion_risk_flags_any_problem_window() {
        let mut p = provider(true);
        assert!(!has_exhaustion_risk(&p));
        p.windows[1].status = CostStatus::Problem;
        assert!(
            has_exhaustion_risk(&p),
            "a single problem window triggers the alert"
        );
        p.windows[1].status = CostStatus::Warning;
        assert!(
            !has_exhaustion_risk(&p),
            "warning alone is not the before-exhaustion signal"
        );
    }

    #[test]
    fn placeholder_cap_annotation_detects_any_window() {
        let mut p = provider(true);
        assert!(!has_placeholder_cap(&p));
        p.windows[0].cap_is_placeholder = true;
        assert!(has_placeholder_cap(&p));
    }

    #[test]
    fn window_summary_includes_window_usd_and_pct() {
        let w = window(12.3, true);
        let summary = window_summary(&w);
        assert!(summary.contains("5h"));
        assert!(summary.contains("$1.00"));
        assert!(summary.contains("~12.3%"));
        assert_eq!(window_summary(&window(0.0, false)), "5h  $1.00  0.0%");
    }
}
