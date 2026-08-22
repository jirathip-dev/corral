//! #113: repo-level GitHub issue browser (read-only).
//!
//! Renders the daemon's `GET /issues` view: open/closed filter, title/number
//! search, and the two explicit worktree-start actions (issue-linked from a
//! selected issue, and a clearly marked issue-free path). The daemon remains
//! the authority on which issue is startable — the browser only renders the
//! fetched set and dispatches a confirmed, authorized drive intent. A failed
//! issue lookup can NEVER fall through to the free path, because the free
//! path is a separate UI section with its own user-chosen name.

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
    if !fleet.issues_loaded {
        egui::CollapsingHeader::new("issues")
            .id_salt("corral-ui-issues")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(
                    RichText::new("issue view not loaded — connect to corrald and refresh")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            });
        return;
    }

    let total: usize = fleet.issues.values().map(Vec::len).sum();
    egui::CollapsingHeader::new(
        RichText::new(format!("issues  ({total})"))
            .monospace()
            .color(theme::ui::TEXT_STRONG),
    )
    .id_salt("corral-ui-issues")
    .default_open(true)
    .show(ui, |ui| {
        toolbar(ui, fleet, allowed, drive, refresh_issues);
        ui.separator();
        if total == 0 {
            ui.label(
                RichText::new("no repo-level issues fetched")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
            return;
        }
        let filter = StateFilter::from_memory(ui);
        let query = search_query(ui).to_lowercase();
        ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for (repo, issues) in &fleet.issues {
                if issues.is_empty() {
                    continue;
                }
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
                        if !filter.keeps(&issue.state) || !matches_query(issue, &query) {
                            continue;
                        }
                        issue_row(ui, fleet, repo, issue, allowed, drive);
                    }
                });
            }
        });
        ui.separator();
        free_path(ui, fleet, allowed, drive);
    });
}

fn toolbar(
    ui: &mut Ui,
    _fleet: &Fleet,
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
        if ui.small_button("↻ refresh").clicked() {
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

/// The explicit, intentional issue-free path. This is its OWN section — a
/// failed issue lookup never reaches it, because the daemon refuses the
/// issue-linked request and this section is only shown via this button.
fn free_path(
    ui: &mut Ui,
    fleet: &Fleet,
    allowed: &dyn Fn(&str) -> bool,
    drive: &mut dyn FnMut(DriveIntent),
) {
    ui.label(
        RichText::new("issue-free worktree (unlinked — explicit choice)")
            .strong()
            .color(theme::ui::TEXT_STRONG),
    );
    // The target is a fleet/repo name, so offer every repo the board knows:
    // the fetched issue view PLUS any agent workspace repo (an issue-free
    // start can target a repo even before its issues are fetched).
    let mut repos: Vec<String> = fleet.issues.keys().cloned().collect();
    for agent in fleet.agents.values() {
        if let Some(repo) = &agent.workspace.repo
            && !repos.contains(repo)
        {
            repos.push(repo.clone());
        }
    }
    repos.sort();
    if repos.is_empty() {
        ui.label(
            RichText::new("no repo available — connect a fleet first")
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        return;
    }
    let mut repo = free_repo(ui).unwrap_or_else(|| repos[0].clone());
    if !repos.contains(&repo) {
        repo = repos[0].clone();
    }
    let mut name = free_name(ui);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("repo").small().color(theme::ui::TEXT_MUTED));
        egui::ComboBox::from_id_salt("corral-ui-issues-free-repo")
            .selected_text(&repo)
            .show_ui(ui, |ui| {
                for r in &repos {
                    ui.selectable_value(&mut repo, r.clone(), r);
                }
            });
        ui.label(RichText::new("label").small().color(theme::ui::TEXT_MUTED));
        let response = ui.add(
            TextEdit::singleline(&mut name)
                .id_salt("corral-ui-issues-free-name")
                .hint_text("issue-free label")
                .desired_width(180.0),
        );
        if response.changed() || response.lost_focus() {
            ui.ctx().memory_mut(|m| {
                m.data
                    .insert_temp(egui::Id::new("corral-ui-issues-free-name"), name.clone())
            });
        }
        if name.trim().is_empty() {
            crate::ui::disabled_button_with_reason(
                ui,
                "start issue-free worktree",
                "label required",
            );
        } else if !allowed("start_worktree") {
            crate::ui::disabled_button_with_reason(
                ui,
                "start issue-free worktree",
                "not granted the start_worktree capability",
            );
        } else {
            let confirming = free_confirming(ui);
            if !confirming {
                if ui.small_button("start issue-free worktree").clicked() {
                    set_free_confirming(ui, true);
                }
            } else {
                if ui.small_button("✓ confirm create").clicked() {
                    let intent = DriveIntent::start_worktree_free(&repo, name.trim(), fleet.rev);
                    drive(intent);
                    set_free_confirming(ui, false);
                    ui.ctx().memory_mut(|m| {
                        m.data
                            .insert_temp(egui::Id::new("corral-ui-issues-free-name"), String::new())
                    });
                }
                if ui.small_button("cancel").clicked() {
                    set_free_confirming(ui, false);
                }
            }
        }
    });
    ui.label(
        RichText::new("the branch is prefixed w2/free- and never carries an issue number")
            .small()
            .color(theme::ui::TEXT_MUTED),
    );
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

fn free_repo(ui: &Ui) -> Option<String> {
    ui.ctx().memory(|m| {
        m.data
            .get_temp::<String>(egui::Id::new("corral-ui-issues-free-repo"))
    })
}

fn free_name(ui: &Ui) -> String {
    ui.ctx().memory(|m| {
        m.data
            .get_temp::<String>(egui::Id::new("corral-ui-issues-free-name"))
            .unwrap_or_default()
    })
}

fn free_confirming(ui: &Ui) -> bool {
    ui.ctx().memory(|m| {
        m.data
            .get_temp::<bool>(egui::Id::new("corral-ui-issues-free-confirm"))
            .unwrap_or(false)
    })
}

fn set_free_confirming(ui: &Ui, value: bool) {
    ui.ctx().memory_mut(|m| {
        m.data
            .insert_temp::<bool>(egui::Id::new("corral-ui-issues-free-confirm"), value)
    });
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
