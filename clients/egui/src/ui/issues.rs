//! #270: repo-level GitHub issue browser.
//!
//! Renders the daemon's `GET /issues` view: a per-repository rail, open/closed
//! filter, title/number search, refresh, and the issue-linked worktree action.
//! The daemon remains the authority on which issue is startable. There is no
//! issue-free worktree box; creation is available only for a selected, open
//! issue.

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{Color32, RichText, ScrollArea, TextEdit, Ui};

use crate::drive::DriveIntent;
use crate::model::GhIssueRef;
use crate::state::Fleet;
use crate::theme;
use crate::ui::badge;

type IssueKey = (String, String, u64);

#[derive(Debug, Default)]
struct IssueGroupBuilder {
    issues: BTreeMap<(String, u64), DisplayIssueBuilder>,
}

#[derive(Debug)]
struct DisplayIssueBuilder {
    issue: GhIssueRef,
    action_targets: BTreeSet<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct DisplayIssue {
    source: String,
    issue: GhIssueRef,
    action_targets: Vec<String>,
}

/// A display category after aliases such as a fleet name and its canonical
/// `gh_repo` basename have been folded together. Action targets stay on each
/// issue: two different repositories may share a basename, and must not gain
/// each other's fleet actions merely because they share a display category.
#[derive(Debug, PartialEq, Eq)]
struct DisplayIssueGroup {
    display_repo: String,
    issues: Vec<DisplayIssue>,
}

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

const REPO_RAIL_WIDTH: f32 = 190.0;
const SELECTED_REPO_MEMORY: &str = "corral-ui-issues-selected-repo";

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
    let groups = display_issue_groups(fleet);
    let mut selected_repo = selected_display_repo(ui, &groups);
    let available_size = ui.available_size();
    ui.allocate_ui_with_layout(
        available_size,
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            let rail_width = REPO_RAIL_WIDTH.min(ui.available_width());
            let clicked_repo = ui
                .allocate_ui_with_layout(
                    egui::vec2(rail_width, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| repo_rail(ui, &groups, selected_repo.as_deref()),
                )
                .inner;
            if let Some(repo) = clicked_repo {
                set_selected_repo(ui, &repo);
                set_selected(ui, None);
                selected_repo = Some(repo);
            }
            ui.separator();
            ui.add_space(12.0);
            ui.vertical(|ui| {
                issue_content(
                    ui,
                    fleet,
                    &groups,
                    selected_repo.as_deref(),
                    allowed,
                    drive,
                    refresh_issues,
                );
            });
        },
    );
}

fn repo_rail(
    ui: &mut Ui,
    groups: &[DisplayIssueGroup],
    selected_repo: Option<&str>,
) -> Option<String> {
    ui.label(
        RichText::new("repositories")
            .small()
            .strong()
            .color(theme::ui::TEXT_MUTED),
    );
    ui.add_space(4.0);
    let mut clicked = None;
    ScrollArea::vertical()
        .id_salt("corral-ui-issues-repo-rail")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for group in groups {
                let open_count = group
                    .issues
                    .iter()
                    .filter(|display_issue| is_open(&display_issue.issue.state))
                    .count();
                ui.horizontal(|ui| {
                    let response = ui.selectable_label(
                        selected_repo == Some(group.display_repo.as_str()),
                        RichText::new(group.display_repo.clone()).monospace(),
                    );
                    if response.clicked() {
                        clicked = Some(group.display_repo.clone());
                    }
                    ui.label(
                        RichText::new(format!("({open_count})"))
                            .small()
                            .monospace()
                            .color(if selected_repo == Some(group.display_repo.as_str()) {
                                theme::ui::ACCENT
                            } else {
                                theme::ui::TEXT_MUTED
                            }),
                    );
                });
            }
            ui.add_space(8.0);
            ui.separator();
            ui.label(
                RichText::new("repo categories from the daemon snapshot / issues")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        });
    clicked
}

fn issue_content(
    ui: &mut Ui,
    fleet: &Fleet,
    groups: &[DisplayIssueGroup],
    selected_repo: Option<&str>,
    allowed: &dyn Fn(&str) -> bool,
    drive: &mut dyn FnMut(DriveIntent),
    refresh_issues: &mut dyn FnMut(),
) {
    let selected_group =
        selected_repo.and_then(|repo| groups.iter().find(|group| group.display_repo == repo));
    let total = selected_group.map_or(0, |group| group.issues.len());
    let filter = StateFilter::from_memory(ui);
    let title = if fleet.issues_loaded {
        format!("Issues  ({total})")
    } else if fleet.issues_loading {
        "Issues  (loading…)".to_string()
    } else {
        "Issues".to_string()
    };
    let subtitle = selected_group
        .map(|group| {
            format!(
                "{} · {} · per-repo browser",
                group.display_repo,
                filter.label()
            )
        })
        .unwrap_or_else(|| "select a repository".to_string());
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).heading().color(theme::ui::TEXT_STRONG));
        ui.label(RichText::new(subtitle).small().color(theme::ui::TEXT_MUTED));
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
    } else {
        let Some(group) = selected_group else {
            ui.label(
                RichText::new("no repo-level issues fetched")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
            return;
        };
        let query = search_query(ui).to_lowercase();
        let shown = group
            .issues
            .iter()
            .filter(|display_issue| filter.keeps(&display_issue.issue.state))
            .filter(|display_issue| matches_query(&display_issue.issue, &query))
            .count();
        if total == 0 {
            ui.label(
                RichText::new("no issues fetched for this repository")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        } else if shown == 0 {
            ui.label(
                RichText::new("no issues match the current filter or search")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        } else {
            ScrollArea::vertical()
                .id_salt("corral-ui-issues-list")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for display_issue in &group.issues {
                        if filter.keeps(&display_issue.issue.state)
                            && matches_query(&display_issue.issue, &query)
                        {
                            issue_row(
                                ui,
                                fleet,
                                &group.display_repo,
                                display_issue,
                                allowed,
                                drive,
                            );
                        }
                    }
                });
        }
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
    display_repo: &str,
    display_issue: &DisplayIssue,
    allowed: &dyn Fn(&str) -> bool,
    drive: &mut dyn FnMut(DriveIntent),
) {
    let issue = &display_issue.issue;
    let key = (
        display_repo.to_string(),
        display_issue.source.clone(),
        issue.number,
    );
    let selected = selected_key(ui) == Some(key.clone());
    let row_label = format!("#{}  {}  {}", issue.number, issue.title, issue.state);
    if ui
        .selectable_label(selected, RichText::new(row_label).monospace())
        .clicked()
    {
        set_selected(ui, (!selected).then_some(key.clone()));
    }
    if selected {
        egui::Frame::new()
            .fill(theme::ui::PANEL2)
            .stroke(egui::Stroke::new(1.0, theme::ui::LINE))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let color = if is_open(&issue.state) {
                        theme::ui::GOOD
                    } else {
                        theme::ui::TEXT_MUTED
                    };
                    badge(ui, &issue.state, color);
                    for label in &issue.labels {
                        if label.name.is_empty() {
                            continue;
                        }
                        badge(ui, &label.name, label_color(&label.color));
                    }
                });
                ui.add_space(4.0);
                let meta = format!(
                    "{} · #{} · {}",
                    display_repo,
                    issue.number,
                    issue.state.to_lowercase()
                );
                ui.label(
                    RichText::new(meta)
                        .small()
                        .monospace()
                        .color(theme::ui::TEXT_MUTED),
                );
                if !issue.url.is_empty() && ui.link(issue.url.clone()).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(issue.url.clone()));
                }
                ui.add_space(6.0);
                match issue.body.as_deref() {
                    Some(body) if !body.trim().is_empty() => render_issue_body(ui, body),
                    Some(_) => {
                        ui.label(
                            RichText::new("issue body is empty")
                                .small()
                                .color(theme::ui::TEXT_MUTED),
                        );
                    }
                    None => {
                        ui.label(
                            RichText::new("issue body unavailable from this daemon")
                                .small()
                                .color(theme::ui::TEXT_MUTED),
                        );
                    }
                };
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("ⓘ").color(theme::ui::ACCENT));
                    ui.label(
                        RichText::new(
                            "read-only · issue data comes from corrald; no GitHub mutations from the board",
                        )
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                    );
                });
                ui.add_space(8.0);
                if !is_open(&issue.state) {
                    ui.label(
                        RichText::new("closed issue — not startable (the daemon refuses too)")
                            .small()
                            .color(theme::ui::TEXT_MUTED),
                    );
                } else if display_issue.action_targets.is_empty() {
                    crate::ui::disabled_button_with_reason(
                        ui,
                        "start worktree",
                        "no validated fleet identity owns this repo category — refresh Issues",
                    );
                } else if !allowed("start_worktree") {
                    crate::ui::disabled_button_with_reason(
                        ui,
                        "start worktree",
                        "not granted the start_worktree capability — refresh grants in Settings",
                    );
                } else {
                    confirm_buttons(ui, &key, &display_issue.action_targets, fleet, issue, drive);
                }
                if ui.small_button("▴ collapse").clicked() {
                    set_selected(ui, None);
                }
            });
    }
}

fn render_issue_body(ui: &mut Ui, body: &str) {
    for line in body.lines() {
        let line = line.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            ui.add_space(4.0);
        } else if let Some(heading) = trimmed
            .strip_prefix("### ")
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("# "))
        {
            ui.add(
                egui::Label::new(
                    RichText::new(heading)
                        .strong()
                        .color(theme::ui::TEXT_STRONG),
                )
                .wrap(),
            );
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            ui.horizontal(|ui| {
                ui.label(RichText::new("•").color(theme::ui::ACCENT));
                ui.add(egui::Label::new(RichText::new(item).color(theme::ui::INK)).wrap());
            });
        } else {
            ui.add(egui::Label::new(RichText::new(line).color(theme::ui::INK)).wrap());
        }
    }
}

fn confirm_buttons(
    ui: &mut Ui,
    key: &IssueKey,
    action_targets: &[String],
    fleet: &Fleet,
    issue: &GhIssueRef,
    drive: &mut dyn FnMut(DriveIntent),
) {
    for action_target in action_targets {
        // #113 review 7: a visible in-flight indicator while the daemon
        // creates the worktree. The drive state is keyed by fleet target.
        if matches!(
            fleet.latest_drive(action_target),
            Some(crate::state::DriveState::Sending { .. })
        ) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    RichText::new(format!("creating worktree ({action_target})…"))
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            });
            continue;
        }
        let multiple = action_targets.len() > 1;
        let confirming = confirming(ui, key, action_target);
        if !confirming {
            let label = action_label("start worktree", action_target, multiple);
            if ui.small_button(label).clicked() {
                set_confirming(ui, key, action_target, true);
            }
            continue;
        }
        ui.horizontal(|ui| {
            let label = action_label("✓ confirm create", action_target, multiple);
            if ui.small_button(label).clicked() {
                let intent = DriveIntent::start_worktree_issue(
                    action_target,
                    issue.number,
                    &issue.url,
                    fleet.rev,
                );
                drive(intent);
                set_confirming(ui, key, action_target, false);
                set_selected(ui, None);
            }
            if ui
                .small_button(action_label("cancel", action_target, multiple))
                .clicked()
            {
                set_confirming(ui, key, action_target, false);
            }
            ui.label(
                RichText::new("creates exactly one isolated worktree/branch")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        });
    }
}

fn action_label(prefix: &str, target: &str, multiple: bool) -> String {
    if multiple {
        format!("{prefix} ({target})")
    } else {
        prefix.to_string()
    }
}

/// Fold the native issue map into display categories. The daemon's native repo
/// key is also the validated worktree target, so no second identity request is
/// needed by the client.
fn display_issue_groups(fleet: &Fleet) -> Vec<DisplayIssueGroup> {
    let mut groups: BTreeMap<String, IssueGroupBuilder> = BTreeMap::new();
    for (key, issues) in &fleet.issues {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let group = groups.entry(key.to_string()).or_default();
        for issue in issues {
            let entry = group
                .issues
                .entry((key.to_string(), issue.number))
                .or_insert_with(|| DisplayIssueBuilder {
                    issue: issue.clone(),
                    action_targets: BTreeSet::new(),
                });
            entry.action_targets.insert(key.to_string());
        }
    }

    groups
        .into_iter()
        .map(|(display_repo, group)| DisplayIssueGroup {
            display_repo,
            issues: group
                .issues
                .into_iter()
                .map(|((source, _), issue)| DisplayIssue {
                    source,
                    issue: issue.issue,
                    action_targets: issue.action_targets.into_iter().collect(),
                })
                .collect(),
        })
        .collect()
}

fn selected_display_repo(ui: &Ui, groups: &[DisplayIssueGroup]) -> Option<String> {
    let stored = ui.ctx().memory(|memory| {
        memory
            .data
            .get_temp::<String>(egui::Id::new(SELECTED_REPO_MEMORY))
    });
    stored
        .filter(|repo| groups.iter().any(|group| group.display_repo == *repo))
        .or_else(|| {
            groups
                .iter()
                .find(|group| !group.issues.is_empty())
                .or_else(|| groups.first())
                .map(|group| group.display_repo.clone())
        })
}

fn set_selected_repo(ui: &Ui, repo: &str) {
    ui.ctx().memory_mut(|memory| {
        memory
            .data
            .insert_temp(egui::Id::new(SELECTED_REPO_MEMORY), repo.to_string());
    });
}

type SelectedIssueKey = IssueKey;

fn selected_key(ui: &Ui) -> Option<SelectedIssueKey> {
    ui.ctx()
        .memory(|m| {
            m.data
                .get_temp::<Option<SelectedIssueKey>>(egui::Id::new("corral-ui-issues-selected"))
        })
        .flatten()
}

fn set_selected(ui: &Ui, key: Option<SelectedIssueKey>) {
    ui.ctx().memory_mut(|m| {
        m.data
            .insert_temp(egui::Id::new("corral-ui-issues-selected"), key)
    });
}

fn confirming(ui: &Ui, key: &IssueKey, target: &str) -> bool {
    let id = egui::Id::new(("corral-ui-issues-confirm", &key.0, &key.1, key.2, target));
    ui.ctx()
        .memory(|m| m.data.get_temp::<bool>(id).unwrap_or(false))
}

fn set_confirming(ui: &Ui, key: &IssueKey, target: &str, value: bool) {
    let id = egui::Id::new(("corral-ui-issues-confirm", &key.0, &key.1, key.2, target));
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

    fn issue(repo: &str, number: u64, state: &str, title: &str, labels: &[&str]) -> GhIssueRef {
        GhIssueRef {
            repo: repo.to_string(),
            number,
            state: state.to_string(),
            title: title.to_string(),
            labels: labels
                .iter()
                .map(|name| crate::model::GhIssueLabel {
                    name: (*name).to_string(),
                    color: "d4c5f9".to_string(),
                })
                .collect(),
            url: format!("https://github.com/example/{repo}/issues/{number}"),
            body: None,
        }
    }

    fn ready_fleet(fleets: &[(&str, &str)], issues: &[(&str, GhIssueRef)]) -> Fleet {
        let _ = fleets;
        Fleet {
            issues: issues
                .iter()
                .map(|(key, issue)| ((*key).to_string(), vec![issue.clone()]))
                .collect(),
            issues_loaded: true,
            ..Default::default()
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
        let i = issue("corral", 113, "OPEN", "browse issues", &["ui"]);
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

    #[test]
    fn selected_issue_renders_body_inline() {
        let mut issue = issue("corral", 270, "OPEN", "issues browser", &["enhancement"]);
        issue.body = Some("Body shown when the row expands.".to_string());
        let fleet = ready_fleet(&[], &[("corral", issue)]);
        let ctx = egui::Context::default();
        let intents = std::cell::RefCell::new(Vec::new());

        let mut output = render(&ctx, &fleet, test_input(vec![]), &intents);
        let row = text_rect(&output, "issues browser").map(|rect| rect.center());
        clear(&mut output);
        let Some(row) = row else {
            panic!("issue row");
        };
        for pressed in [true, false] {
            let mut frame = render(&ctx, &fleet, pointer_input(row, pressed), &intents);
            clear(&mut frame);
        }

        let mut output = render(&ctx, &fleet, test_input(vec![]), &intents);
        let rendered = text_rect(&output, "Body shown when the row expands.").is_some();
        clear(&mut output);
        assert!(rendered, "expanded issue body should be visible");
    }

    #[test]
    fn repo_rail_switches_the_visible_issue_group() {
        let corral = issue("corral", 270, "OPEN", "corral issue", &[]);
        let sendmeter = issue("sendmeter", 17, "OPEN", "sendmeter issue", &[]);
        let fleet = Fleet {
            issues: BTreeMap::from([
                ("agent-fleet-doctrine".to_string(), Vec::new()),
                ("corral".to_string(), vec![corral]),
                ("sendmeter".to_string(), vec![sendmeter]),
            ]),
            issues_loaded: true,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let intents = std::cell::RefCell::new(Vec::new());

        let mut output = render(&ctx, &fleet, test_input(vec![]), &intents);
        let rail_visible = text_rect(&output, "repositories").is_some();
        let corral_visible = text_rect(&output, "corral issue").is_some();
        let sendmeter_visible = text_rect(&output, "sendmeter issue").is_some();
        let sendmeter_item = text_rect(&output, "sendmeter").map(|rect| rect.center());
        clear(&mut output);
        assert!(rail_visible);
        assert!(corral_visible);
        assert!(!sendmeter_visible);
        let Some(sendmeter_item) = sendmeter_item else {
            panic!("repo rail item");
        };
        for pressed in [true, false] {
            let mut frame = render(
                &ctx,
                &fleet,
                pointer_input(sendmeter_item, pressed),
                &intents,
            );
            clear(&mut frame);
        }

        let mut output = render(&ctx, &fleet, test_input(vec![]), &intents);
        let sendmeter_visible = text_rect(&output, "sendmeter issue").is_some();
        let corral_visible = text_rect(&output, "corral issue").is_some();
        clear(&mut output);
        assert!(sendmeter_visible);
        assert!(!corral_visible);
    }

    fn test_input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 600.0),
            )),
            events,
            ..Default::default()
        }
    }

    fn pointer_input(pos: egui::Pos2, pressed: bool) -> egui::RawInput {
        test_input(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            },
        ])
    }

    fn text_rect(output: &egui::FullOutput, needle: &str) -> Option<egui::Rect> {
        fn walk(shape: &egui::epaint::Shape, needle: &str) -> Option<egui::Rect> {
            match shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text.contains(needle) => {
                    Some(text.visual_bounding_rect())
                }
                egui::epaint::Shape::Vec(shapes) => {
                    shapes.iter().find_map(|shape| walk(shape, needle))
                }
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|clipped| walk(&clipped.shape, needle))
    }

    fn render(
        ctx: &egui::Context,
        fleet: &Fleet,
        input: egui::RawInput,
        intents: &std::cell::RefCell<Vec<DriveIntent>>,
    ) -> egui::FullOutput {
        ctx.run_ui(input, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
            ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
            let mut drive = |intent| intents.borrow_mut().push(intent);
            show(ui, fleet, &|_| true, &mut drive, &mut || {});
        })
    }

    fn clear(output: &mut egui::FullOutput) {
        output.textures_delta.clear();
    }

    #[test]
    fn frame_issue_action_dispatches_the_selected_shared_fleet_target() {
        let issue = issue("foo", 42, "OPEN", "shared issue", &[]);
        let fleet = ready_fleet(&[], &[("foo", issue)]);
        let ctx = egui::Context::default();
        ctx.memory_mut(|memory| {
            memory.data.insert_temp(
                egui::Id::new("corral-ui-issues-selected"),
                Some(("foo".to_string(), "foo".to_string(), 42_u64)),
            );
        });
        let intents = std::cell::RefCell::new(Vec::new());
        let mut output = render(&ctx, &fleet, test_input(vec![]), &intents);
        let start = text_rect(&output, "start worktree")
            .expect("the native repository action is rendered")
            .center();
        clear(&mut output);
        for pressed in [true, false] {
            let mut frame = render(&ctx, &fleet, pointer_input(start, pressed), &intents);
            clear(&mut frame);
        }

        let mut frame = render(&ctx, &fleet, test_input(vec![]), &intents);
        let confirm = text_rect(&frame, "✓ confirm create")
            .expect("the confirmation is keyed independently")
            .center();
        clear(&mut frame);
        for pressed in [true, false] {
            let mut frame = render(&ctx, &fleet, pointer_input(confirm, pressed), &intents);
            clear(&mut frame);
        }

        let intents = intents.into_inner();
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].target, "foo");
        assert_eq!(
            intents[0].capability,
            crate::drive::Capability::StartWorktree
        );
        assert_eq!(intents[0].payload["mode"], "issue");
        assert_eq!(intents[0].payload["number"], 42);
    }
}
