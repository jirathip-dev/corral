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
    let total: usize = fleet.issues.values().map(Vec::len).sum();
    let title = if fleet.issues_loaded {
        format!("issues  ({total})")
    } else {
        "issues".to_string()
    };
    egui::CollapsingHeader::new(
        RichText::new(title)
            .monospace()
            .color(theme::ui::TEXT_STRONG),
    )
    .id_salt("corral-ui-issues")
    .default_open(true)
    .show(ui, |ui| {
        toolbar(ui, fleet, allowed, drive, refresh_issues);
        ui.separator();
        if !fleet.issues_loaded {
            ui.label(
                RichText::new("issue view not loaded — connect to corrald and refresh")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        } else if total == 0 {
            // The issue-free path is explicitly reachable even when a
            // configured fleet has zero fetched issues (or before the first
            // poll); the section below renders unconditionally.
            ui.label(
                RichText::new("no repo-level issues fetched")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        } else {
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
        }
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

/// The explicit, intentional issue-free path. This is its OWN section — a
/// failed issue lookup never reaches it, because the daemon refuses the
/// issue-linked request and this section is only shown via this button.
fn free_repos(fleet: &Fleet) -> Vec<String> {
    // Offer every repo the board knows: the fetched issue view PLUS any
    // agent workspace repo. An issue-free start can target a repo even before
    // its issues are fetched (the daemon's `/issues` view lists every
    // configured fleet, so a repo with zero fetched issues still appears).
    let mut repos: Vec<String> = fleet.issues.keys().cloned().collect();
    for agent in fleet.agents.values() {
        if let Some(repo) = &agent.workspace.repo
            && !repos.contains(repo)
        {
            repos.push(repo.clone());
        }
    }
    repos.sort();
    repos
}

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
    let repos = free_repos(fleet);
    // The section ALWAYS renders so the explicit choice stays reachable even
    // when no issues were fetched (or before the first poll). A fleet with a
    // configured repo but zero fetched issues appears once `/issues` lists
    // the configured fleet; if the board truly knows no repo, show that.
    if repos.is_empty() {
        ui.label(
            RichText::new("no repo available — connect a fleet (or refresh issues) first")
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        return;
    }
    let mut repo = free_repo(ui).unwrap_or_else(|| repos[0].clone());
    // Capture the value we read from temp memory (or its `repos[0]` default)
    // before the ComboBox can change it. We write the effective selection
    // back below when it differs, which both persists a user's pick and cleans
    // up a stale value for a repo that no longer exists.
    let repo_before = repo.clone();
    if !repos.contains(&repo) {
        repo = repos[0].clone();
    }
    let creating_free = matches!(
        fleet.latest_drive(&repo),
        Some(crate::state::DriveState::Sending { .. })
    );
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
        if repo != repo_before {
            ui.ctx().memory_mut(|m| {
                m.data
                    .insert_temp(egui::Id::new("corral-ui-issues-free-repo"), repo.clone())
            });
        }
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
        if creating_free {
            ui.spinner();
            ui.label(
                RichText::new("creating issue-free worktree…")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        } else if name.trim().is_empty() {
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

    #[test]
    fn free_repos_reachable_with_zero_fetched_issues() {
        // A configured fleet that has been polled but returned zero issues is
        // still offered in the issue-free path (the daemon `/issues` view
        // lists every configured fleet). The explicit choice must stay
        // reachable even when no issue rows are rendered.
        let mut fleet = Fleet::default();
        fleet.issues.insert("corral".to_string(), Vec::new());
        assert_eq!(free_repos(&fleet), vec!["corral"]);

        // Fresh connect: no issues fetched and no agent workspace repo. The
        // free path shows the "no repo available" hint instead of assuming an
        // issue repo from branch inference.
        let empty = Fleet::default();
        assert!(free_repos(&empty).is_empty());
    }

    #[test]
    fn free_repo_selection_persists_across_frames() {
        use eframe::egui;

        fn run_frame(
            ctx: &egui::Context,
            fleet: &Fleet,
            input: egui::RawInput,
        ) -> egui::FullOutput {
            ctx.run_ui(input, |ui| {
                free_path(ui, fleet, &|_| true, &mut |_intent| {});
            })
        }

        fn frame_input(time: f64, pos: Option<egui::Pos2>) -> egui::RawInput {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
            let mut input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..Default::default()
            };
            if let Some(pos) = pos {
                input.events = vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ];
            }
            input
        }

        // Locate the center of a rendered text shape so the test can click the
        // ComboBox button and its options at their on-screen positions.
        fn text_center(output: &egui::FullOutput, text: &str) -> egui::Pos2 {
            use eframe::egui::Shape;
            fn walk(shape: &Shape, text: &str) -> Option<egui::Pos2> {
                match shape {
                    Shape::Text(ts) if ts.galley.text() == text => {
                        let size = ts.galley.size();
                        Some(ts.pos + egui::vec2(size.x * 0.5, size.y * 0.5))
                    }
                    Shape::Vec(shapes) => shapes.iter().find_map(|s| walk(s, text)),
                    _ => None,
                }
            }
            output
                .shapes
                .iter()
                .find_map(|clipped| walk(&clipped.shape, text))
                .unwrap_or_else(|| panic!("no rendered text shape for {text:?}"))
        }

        let mut fleet = Fleet::default();
        fleet.issues.insert("acme".to_string(), Vec::new());
        fleet.issues.insert("zephyr".to_string(), Vec::new());

        let ctx = egui::Context::default();

        // Frame 1: the ComboBox starts on repos[0] ("acme").
        let out1 = run_frame(&ctx, &fleet, frame_input(0.0, None));
        let button_pos = text_center(&out1, "acme");
        out1.drop_without_applying_deltas();

        // Frame 2: open the ComboBox popup. The popup does a sizing pass on its
        // opening frame, so let it settle before reading the option positions.
        let out2 = run_frame(&ctx, &fleet, frame_input(1.0 / 60.0, Some(button_pos)));
        out2.drop_without_applying_deltas();

        // Frame 3: the popup is now rendered, so read the non-first option's
        // rendered position and click it.
        let out3 = run_frame(&ctx, &fleet, frame_input(2.0 / 60.0, None));
        let option_pos = text_center(&out3, "zephyr");
        out3.drop_without_applying_deltas();

        let out4 = run_frame(&ctx, &fleet, frame_input(3.0 / 60.0, Some(option_pos)));
        out4.drop_without_applying_deltas();

        // Frame 5: a fresh frame must read the chosen repo back from temp
        // memory instead of snapping back to repos[0].
        let mut repo = String::new();
        let out5 = ctx.run_ui(frame_input(4.0 / 60.0, None), |ui| {
            repo = free_repo(ui).unwrap_or_else(|| "acme".to_string());
        });
        out5.drop_without_applying_deltas();
        assert_eq!(
            repo, "zephyr",
            "non-first repo selection must persist across frames"
        );
    }
}
