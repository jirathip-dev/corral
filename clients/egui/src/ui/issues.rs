//! #206: repo-level GitHub issue browser.
//!
//! Renders the daemon's `GET /issues` view: open/closed filter, title/number
//! search, refresh, repo grouping, and the issue-linked worktree action. The
//! daemon remains the authority on which issue is startable. There is no
//! issue-free worktree box; creation is available only for a selected, open
//! issue.

use eframe::egui::{Color32, RichText, ScrollArea, TextEdit, Ui};

use crate::drive::DriveIntent;
use crate::model::GhIssueRef;
use crate::state::Fleet;
use crate::theme;
use crate::ui::badge;

/// State filter for the browser (open/closed are the two state buckets;
/// `All` shows every fetched issue regardless of state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StateFilter {
    All,
    Open,
    Closed,
}

impl StateFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }

    fn keeps(self, state: &str) -> bool {
        let open = is_open(state);
        match self {
            Self::All => true,
            Self::Open => open,
            Self::Closed => !open,
        }
    }
}

/// An issue is "open" for the startable gate only when the daemon's state
/// is the uppercase GraphQL `OPEN`. Anything else (closed, merged, unknown)
/// is not startable and renders a disabled action.
fn is_open(state: &str) -> bool {
    state.trim() == "OPEN"
}

/// Render the issue browser section. `refresh_issues` is invoked when the
/// user taps the manual refresh button; the app owns the actual fetch (so
/// this module stays immediate-mode and never runs a network call).
pub fn show(
    ui: &mut Ui,
    fleet: &Fleet,
    allowed: &dyn Fn(&str) -> bool,
    drive: &mut dyn FnMut(DriveIntent),
    refresh_issues: &mut dyn FnMut(),
) {
    let total: usize = fleet.issues.values().map(Vec::len).sum();
    let title = if fleet.issues_loaded {
        format!("Issues  ({total})")
    } else if fleet.issues_loading {
        "Issues  (loading…)".to_string()
    } else {
        "Issues".to_string()
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).heading().color(theme::ui::TEXT_STRONG));
        ui.label(
            RichText::new("all issues grouped by repository")
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
    });
    ui.add_space(8.0);
    toolbar(ui, fleet, allowed, drive, refresh_issues);
    ui.separator();
    if !fleet.issues_loaded {
        let message = if fleet.issues_loading {
            "loading repo-level issues…"
        } else if fleet.issues_error.is_some() {
            "issue view unavailable — refresh to retry"
        } else {
            "issue view not loaded — connect to corrald and refresh"
        };
        ui.label(RichText::new(message).small().color(theme::ui::TEXT_MUTED));
    } else if total == 0 {
        ui.label(
            RichText::new("no repo-level issues fetched")
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
    } else {
        let filter = StateFilter::from_memory(ui);
        let query = search_query(ui).to_lowercase();
        ScrollArea::vertical()
            .id_salt("corral-ui-issues-list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (repo, issues) in &fleet.issues {
                    let shown = issues
                        .iter()
                        .filter(|i| filter.keeps(&i.state))
                        .filter(|i| matches_query(i, &query))
                        .count();
                    if shown == 0 {
                        continue;
                    }
                    let title = format!("{repo}  ({shown})");
                    egui::CollapsingHeader::new(
                        RichText::new(title)
                            .monospace()
                            .color(theme::ui::TEXT_STRONG),
                    )
                    .id_salt(("corral-ui-issues-repo", repo))
                    .default_open(true)
                    .show(ui, |ui| {
                        for issue in issues {
                            if filter.keeps(&issue.state) && matches_query(issue, &query) {
                                issue_row(ui, fleet, repo, issue, allowed, drive);
                            }
                        }
                    });
                }
            });
    }
    if let Some(error) = &fleet.issues_error {
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!("latest refresh failed: {error}"))
                .small()
                .color(theme::ui::WARN),
        );
    }
}

fn toolbar(
    ui: &mut Ui,
    fleet: &Fleet,
    _allowed: &dyn Fn(&str) -> bool,
    _drive: &mut dyn FnMut(DriveIntent),
    refresh_issues: &mut dyn FnMut(),
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for filter in [StateFilter::All, StateFilter::Open, StateFilter::Closed] {
            let mut current = StateFilter::from_memory(ui);
            if ui
                .selectable_label(current == filter, filter.label())
                .clicked()
            {
                current = filter;
                StateFilter::to_memory(ui, current);
                ui.ctx().request_repaint();
            }
        }
        let mut query = search_query(ui);
        let response = ui.add(
            TextEdit::singleline(&mut query)
                .id_salt("corral-ui-issues-search")
                .hint_text("search title or #number")
                .desired_width(220.0),
        );
        if response.changed() || response.lost_focus() {
            ui.ctx().memory_mut(|m| {
                m.data
                    .insert_temp(egui::Id::new("corral-ui-issues-search"), query.clone())
            });
        }
        let refresh_label = if fleet.issues_loading {
            "↻ refreshing…"
        } else {
            "↻ refresh"
        };
        if ui
            .add_enabled(
                !fleet.issues_loading,
                egui::Button::new(RichText::new(refresh_label).small()),
            )
            .clicked()
        {
            refresh_issues();
        }
    });
}

fn search_query(ui: &Ui) -> String {
    ui.ctx().memory(|m| {
        m.data
            .get_temp::<String>(egui::Id::new("corral-ui-issues-search"))
            .unwrap_or_default()
    })
}

fn matches_query(issue: &GhIssueRef, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let title = issue.title.to_lowercase();
    let number = issue.number.to_string();
    title.contains(query) || number.contains(query) || format!("#{number}").contains(query)
}

fn issue_row(
    ui: &mut Ui,
    fleet: &Fleet,
    repo: &str,
    issue: &GhIssueRef,
    allowed: &dyn Fn(&str) -> bool,
    drive: &mut dyn FnMut(DriveIntent),
) {
    let key = (repo.to_string(), issue.number);
    let selected = selected_key(ui) == Some(key.clone());
    let row_label = format!("#{}  {}  {}", issue.number, issue.title, issue.state);
    if ui
        .selectable_label(selected, RichText::new(row_label).monospace())
        .clicked()
    {
        set_selected(ui, Some(key.clone()));
    }
    if selected {
        ui.indent(("corral-ui-issue-detail", repo, issue.number), |ui| {
            ui.horizontal_wrapped(|ui| {
                let color = if is_open(&issue.state) {
                    theme::ui::GOOD
                } else {
                    theme::ui::TEXT_MUTED
                };
                badge(ui, &issue.state, color);
                if !issue.url.is_empty() && ui.link(issue.url.clone()).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(issue.url.clone()));
                }
            });
            if !issue.labels.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for label in &issue.labels {
                        if label.name.is_empty() {
                            continue;
                        }
                        let color = label_color(&label.color);
                        badge(ui, &label.name, color);
                    }
                });
            }
            if !is_open(&issue.state) {
                ui.label(
                    RichText::new("closed issue — not startable (the daemon refuses too)")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            } else if !allowed("start_worktree") {
                crate::ui::disabled_button_with_reason(
                    ui,
                    "start worktree",
                    "not granted the start_worktree capability — refresh grants in Settings",
                );
            } else {
                confirm_buttons(ui, &key, fleet, issue, drive);
            }
        });
    }
}

fn confirm_buttons(
    ui: &mut Ui,
    key: &(String, u64),
    fleet: &Fleet,
    issue: &GhIssueRef,
    drive: &mut dyn FnMut(DriveIntent),
) {
    // #113 review 7: a visible in-flight indicator while the daemon creates
    // the worktree. The drive state is keyed by the repo/fleet target.
    if matches!(
        fleet.latest_drive(&key.0),
        Some(crate::state::DriveState::Sending { .. })
    ) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                RichText::new("creating worktree…")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        });
        return;
    }
    let confirming = confirming(ui, key);
    if !confirming {
        if ui.small_button("start worktree").clicked() {
            set_confirming(ui, key, true);
        }
        return;
    }
    ui.horizontal(|ui| {
        if ui.small_button("✓ confirm create").clicked() {
            let intent =
                DriveIntent::start_worktree_issue(&key.0, issue.number, &issue.url, fleet.rev);
            drive(intent);
            set_confirming(ui, key, false);
            set_selected(ui, None);
        }
        if ui.small_button("cancel").clicked() {
            set_confirming(ui, key, false);
        }
        ui.label(
            RichText::new("creates exactly one isolated worktree/branch")
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
    });
}

fn selected_key(ui: &Ui) -> Option<(String, u64)> {
    ui.ctx()
        .memory(|m| {
            m.data
                .get_temp::<Option<(String, u64)>>(egui::Id::new("corral-ui-issues-selected"))
        })
        .flatten()
}

fn set_selected(ui: &Ui, key: Option<(String, u64)>) {
    ui.ctx().memory_mut(|m| {
        m.data
            .insert_temp(egui::Id::new("corral-ui-issues-selected"), key)
    });
}

fn confirming(ui: &Ui, key: &(String, u64)) -> bool {
    let id = egui::Id::new(("corral-ui-issues-confirm", &key.0, key.1));
    ui.ctx()
        .memory(|m| m.data.get_temp::<bool>(id).unwrap_or(false))
}

fn set_confirming(ui: &Ui, key: &(String, u64), value: bool) {
    let id = egui::Id::new(("corral-ui-issues-confirm", &key.0, key.1));
    ui.ctx()
        .memory_mut(|m| m.data.insert_temp::<bool>(id, value));
}

impl StateFilter {
    fn from_memory(ui: &Ui) -> Self {
        ui.ctx()
            .memory(|m| {
                m.data
                    .get_temp::<u8>(egui::Id::new("corral-ui-issues-filter"))
            })
            .and_then(|v| match v {
                0 => Some(Self::All),
                1 => Some(Self::Open),
                2 => Some(Self::Closed),
                _ => None,
            })
            .unwrap_or(Self::All)
    }

    fn to_memory(ui: &Ui, filter: Self) {
        let v = match filter {
            Self::All => 0,
            Self::Open => 1,
            Self::Closed => 2,
        };
        ui.ctx().memory_mut(|m| {
            m.data
                .insert_temp::<u8>(egui::Id::new("corral-ui-issues-filter"), v)
        });
    }
}

/// Parse a GitHub label color hex (`"d4c5f9"`) into an RGB `Color32`. Any
/// malformed/empty color falls back to the muted UI text color.
fn label_color(color: &str) -> Color32 {
    let hex = color.trim_start_matches('#');
    if hex.len() != 6 {
        return theme::ui::TEXT_MUTED;
    }
    let (r, g, b) = (
        u8::from_str_radix(&hex[0..2], 16),
        u8::from_str_radix(&hex[2..4], 16),
        u8::from_str_radix(&hex[4..6], 16),
    );
    match (r, g, b) {
        (Ok(r), Ok(g), Ok(b)) => Color32::from_rgb(r, g, b),
        _ => theme::ui::TEXT_MUTED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(number: u64, state: &str, title: &str, labels: &[&str]) -> GhIssueRef {
        GhIssueRef {
            repo: "corral".to_string(),
            number,
            state: state.to_string(),
            title: title.to_string(),
            labels: labels
                .iter()
                .map(|n| crate::model::GhIssueLabel {
                    name: (*n).to_string(),
                    color: "d4c5f9".to_string(),
                })
                .collect(),
            url: format!("https://github.com/jirathip-dev/corral/issues/{number}"),
        }
    }

    #[test]
    fn filter_keeps_open_and_closed() {
        assert!(StateFilter::Open.keeps("OPEN"));
        assert!(!StateFilter::Open.keeps("CLOSED"));
        assert!(StateFilter::Closed.keeps("CLOSED"));
        assert!(!StateFilter::Closed.keeps("OPEN"));
        assert!(StateFilter::All.keeps("OPEN") && StateFilter::All.keeps("CLOSED"));
    }

    #[test]
    fn is_open_only_accepts_uppercase_open() {
        assert!(is_open("OPEN"));
        assert!(!is_open("CLOSED"));
        assert!(!is_open("open"));
        assert!(!is_open(""));
    }

    #[test]
    fn matches_query_by_title_or_number() {
        let i = issue(113, "OPEN", "browse issues", &["ui"]);
        assert!(matches_query(&i, "browse"));
        assert!(matches_query(&i, "113"));
        assert!(matches_query(&i, "#113"));
        assert!(!matches_query(&i, "nope"));
    }

    #[test]
    fn label_color_parses_hex_and_falls_back() {
        assert_eq!(label_color("d4c5f9"), Color32::from_rgb(0xd4, 0xc5, 0xf9));
        assert_eq!(label_color(""), theme::ui::TEXT_MUTED);
        assert_eq!(label_color("#zzzzzz"), theme::ui::TEXT_MUTED);
    }
}
