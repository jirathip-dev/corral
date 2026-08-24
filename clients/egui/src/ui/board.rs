//! Fleet board: a cards master/detail split (default) plus an exact
//! nine-column conformance table. The master pane is attention-ranked by
//! [`crate::theme::AgentStateLike::rank`], searchable, state-chipped, and
//! optionally grouped by repo or flattened. The detail pane owns drive
//! controls, the full waiting-on claim, and Recent output/transcript.

use std::cmp::Ordering;

use eframe::egui::{CollapsingHeader, Color32, RichText, ScrollArea, TextEdit, Ui};

use crate::drive::{DriveIntent, DriveOutcome};
use crate::model::Agent;
use crate::state::DriveState;
use crate::state::Fleet;
use crate::theme::{self, ci, kind, state};
use crate::ui::badge;

/// Column layout (fixed widths so the board reads like a dashboard).
const COL_AGENT: f32 = 190.0;
const COL_STATE: f32 = 90.0;
const COL_WAITING: f32 = 220.0;
const COL_REPO: f32 = 130.0;
const COL_BRANCH: f32 = 160.0;
const COL_DIRTY: f32 = 46.0;
const COL_AB: f32 = 64.0;
const COL_PR: f32 = 56.0;
const COL_CI: f32 = 76.0;

/// Board columns in render order. Both the header and every agent row draw
/// from this one width source so labels and values start at identical x
/// positions.
const BOARD_COLUMNS: [(&str, f32); 9] = [
    ("AGENT", COL_AGENT),
    ("STATE", COL_STATE),
    ("WAITING ON", COL_WAITING),
    ("REPO", COL_REPO),
    ("BRANCH", COL_BRANCH),
    ("DIRTY", COL_DIRTY),
    ("A/B", COL_AB),
    ("PR", COL_PR),
    ("CI", COL_CI),
];

/// Keep at least this much branch text even when the inferred marker is
/// unusually long; the marker segment is bounded to the remaining width.
const BRANCH_MIN_TEXT_WIDTH: f32 = 36.0;

/// Header for the bucket of agents without `workspace.repo` (sorts last).
const NO_REPO_LABEL: &str = "(no repo)";

/// egui temp-memory key for the flat-list toggle (default: flat attention
/// order, matching the prototype's prioritized master list).
const FLAT_VIEW: &str = "corral-ui-board-flat";

const DEFAULT_FLAT: bool = true;

/// egui temp-memory key for the master/detail search query.
const SEARCH_QUERY: &str = "corral-ui-board-search";

/// egui temp-memory key for the cards/table view (default: cards).
const VIEW_MODE: &str = "corral-ui-board-view";

/// Cards or the exact nine-column conformance table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BoardView {
    #[default]
    Cards,
    Table,
}

/// State chips over the master list. `Idle` keeps both `Idle` and `Unknown`
/// so the five contract states remain reachable without an extra chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFilter {
    All,
    Blocked,
    Done,
    Working,
    Idle,
}

impl StateFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Blocked => crate::theme::AgentStateLike::Blocked.label(),
            Self::Done => crate::theme::AgentStateLike::Done.label(),
            Self::Working => crate::theme::AgentStateLike::Working.label(),
            Self::Idle => crate::theme::AgentStateLike::Idle.label(),
        }
    }

    fn keeps(self, state: crate::theme::AgentStateLike) -> bool {
        match self {
            Self::All => true,
            Self::Blocked => state == crate::theme::AgentStateLike::Blocked,
            Self::Done => state == crate::theme::AgentStateLike::Done,
            Self::Working => state == crate::theme::AgentStateLike::Working,
            Self::Idle => matches!(
                state,
                crate::theme::AgentStateLike::Idle | crate::theme::AgentStateLike::Unknown
            ),
        }
    }
}

const STATE_FILTERS: [StateFilter; 5] = [
    StateFilter::All,
    StateFilter::Blocked,
    StateFilter::Done,
    StateFilter::Working,
    StateFilter::Idle,
];

/// Chips that have at least one matching agent. Pure so the zero-state rule
/// (no empty `Needs you` / other buckets) is covered without an egui frame.
fn available_state_filters(fleet: &Fleet, query: &str) -> Vec<StateFilter> {
    let query = query.trim();
    let mut visible = vec![StateFilter::All];
    for candidate in STATE_FILTERS.into_iter().skip(1) {
        if fleet
            .agents
            .values()
            .any(|agent| candidate.keeps(agent.state.into()) && agent_matches_query(agent, query))
        {
            visible.push(candidate);
        }
    }
    visible
}

/// Callbacks the board issues to the app (drive dispatch + #64
/// transcript page fetches). Both are the deferred-action pattern: the
/// board renders against `&Fleet`, so the app collects intents and acts
/// after `show` returns.
pub struct BoardActions<'a> {
    pub drive: &'a mut dyn FnMut(DriveIntent),
    pub transcript: &'a mut dyn FnMut(crate::transcript::TranscriptRequest),
    /// #141: ask the app to expand/open (or close) the agent's Full chat
    /// transcript from either the row control or the nested header.
    pub full_chat: &'a mut dyn FnMut(&str),
}

/// Render the fleet board.
pub fn show(
    ui: &mut Ui,
    fleet: &mut Fleet,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) {
    if fleet.agents.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("no agents in the fleet yet — waiting for corrald")
                    .color(theme::ui::TEXT_MUTED),
            );
        });
        return;
    }

    let mut view = view_mode(ui);
    let mut flat = flat_view(ui);
    let mut query = search_query(ui);
    let mut filter = state_filter(ui);
    toolbar(ui, fleet, &mut view, &mut flat, &mut query, &mut filter);
    let visible_ids: Vec<String> = visible_agent_ids(fleet, filter, &query)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let visible: Vec<&str> = visible_ids.iter().map(String::as_str).collect();
    let selected = resolve_selection(fleet, &visible).map(str::to_owned);
    if let Some(id) = &selected {
        // Persist the resolved default before rendering so Table rows
        // highlight the same agent the detail pane is showing.
        fleet.select_agent(id);
    }

    match view {
        BoardView::Cards => show_cards(
            ui,
            fleet,
            &visible,
            flat,
            selected.as_deref(),
            allowed,
            actions,
        ),
        BoardView::Table => show_table(ui, fleet, &visible, flat, allowed, actions),
    }
}

/// Persistent sidebar state helpers are intentionally tiny; every query
/// string and toggle can be pure-tested without touching these.
fn toolbar(
    ui: &mut Ui,
    fleet: &Fleet,
    view: &mut BoardView,
    flat: &mut bool,
    query: &mut String,
    filter: &mut StateFilter,
) {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new("search")
                .small()
                .monospace()
                .color(theme::ui::TEXT_MUTED),
        );
        let response = ui.add(
            TextEdit::singleline(query)
                .id_salt(("corral-ui-board-search-input", SEARCH_QUERY))
                .hint_text("repo / branch / title / issue…")
                .desired_width(240.0),
        );
        if response.changed() {
            changed = true;
        }
        for candidate in available_state_filters(fleet, query) {
            let selected = *filter == candidate;
            let color = if selected {
                theme::ui::TEXT_STRONG
            } else {
                match candidate {
                    StateFilter::All => theme::ui::ACCENT,
                    StateFilter::Blocked => state::of(crate::theme::AgentStateLike::Blocked),
                    StateFilter::Done => state::of(crate::theme::AgentStateLike::Done),
                    StateFilter::Working => state::of(crate::theme::AgentStateLike::Working),
                    StateFilter::Idle => state::of(crate::theme::AgentStateLike::Idle),
                }
            };
            if ui
                .selectable_label(selected, RichText::new(candidate.label()).color(color))
                .clicked()
            {
                *filter = candidate;
                changed = true;
            }
        }
        let available = available_state_filters(fleet, query);
        if *filter != StateFilter::All && !available.contains(&*filter) {
            *filter = StateFilter::All;
            changed = true;
        }
        if ui
            .checkbox(flat, "flat sort")
            .on_hover_text("one flat list of every agent, instead of repo groups")
            .changed()
        {
            changed = true;
        }
        ui.separator();
        for candidate in [BoardView::Cards, BoardView::Table] {
            if ui
                .selectable_value(
                    view,
                    candidate,
                    match candidate {
                        BoardView::Cards => "Cards",
                        BoardView::Table => "Table",
                    },
                )
                .changed()
            {
                changed = true;
            }
        }
    });
    if changed {
        ui.ctx().memory_mut(|m| {
            m.data.insert_temp::<bool>(egui::Id::new(FLAT_VIEW), *flat);
            m.data
                .insert_temp::<String>(egui::Id::new(SEARCH_QUERY), query.clone());
            m.data.insert_temp(
                egui::Id::new(("corral-ui-board-filter", SEARCH_QUERY)),
                *filter,
            );
            m.data
                .insert_temp::<BoardView>(egui::Id::new(VIEW_MODE), *view);
        });
    }
}

fn flat_view(ui: &Ui) -> bool {
    ui.ctx().memory(|m| {
        m.data
            .get_temp::<bool>(egui::Id::new(FLAT_VIEW))
            .unwrap_or(DEFAULT_FLAT)
    })
}

fn view_mode(ui: &Ui) -> BoardView {
    ui.ctx().memory(|m| {
        m.data
            .get_temp::<BoardView>(egui::Id::new(VIEW_MODE))
            .unwrap_or_default()
    })
}

fn search_query(ui: &Ui) -> String {
    ui.ctx().memory(|m| {
        m.data
            .get_temp::<String>(egui::Id::new(SEARCH_QUERY))
            .unwrap_or_default()
    })
}

fn state_filter(ui: &Ui) -> StateFilter {
    ui.ctx().memory(|m| {
        m.data
            .get_temp::<StateFilter>(egui::Id::new(("corral-ui-board-filter", SEARCH_QUERY)))
            .unwrap_or(StateFilter::All)
    })
}

/// State-filtered agent ids in attention order (contract rank, then stable
/// id) so the master list is independent of `BTreeMap` insertion order.
pub fn visible_agent_ids<'a>(fleet: &'a Fleet, filter: StateFilter, query: &str) -> Vec<&'a str> {
    let mut ids: Vec<&'a str> = fleet
        .agents
        .iter()
        .filter(|(_, agent)| filter.keeps(agent.state.into()) && agent_matches_query(agent, query))
        .map(|(id, _)| id.as_str())
        .collect();
    ids.sort_by(|a, b| {
        let rank_a = agent_rank(fleet.agents.get(*a).expect("visible id is in fleet"));
        let rank_b = agent_rank(fleet.agents.get(*b).expect("visible id is in fleet"));
        rank_a.cmp(&rank_b).then_with(|| a.cmp(b))
    });
    ids
}

fn agent_rank(agent: &Agent) -> u8 {
    let state: crate::theme::AgentStateLike = agent.state.into();
    state.rank()
}

/// Pure search predicate over the requested fields: repo, branch, title,
/// and issue identity/title. Case-insensitive; a blank query matches all.
pub fn agent_matches_query(agent: &Agent, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let mut haystack: Vec<String> = [
        agent.workspace.repo.clone(),
        agent.workspace.branch.clone(),
        agent.title.clone(),
        agent.display_name.clone(),
        Some(agent.agent_id.clone()),
    ]
    .into_iter()
    .flatten()
    .collect();
    if let Some(pr) = agent.workspace.pr_number {
        haystack.push(format!("#{pr}"));
    }
    for issue in &agent.issues {
        haystack.push(issue.repo.clone());
        haystack.push(issue.title.clone());
        haystack.push(issue.number.to_string());
        haystack.push(format!("#{}", issue.number));
    }
    haystack
        .iter()
        .any(|part| part.to_lowercase().contains(&query))
}

/// One non-empty attention section of the flat master list.
#[derive(Debug, PartialEq, Eq)]
pub struct StateSection<'a> {
    pub state: crate::theme::AgentStateLike,
    pub agent_ids: Vec<&'a str>,
}

/// Split already-sorted ids into non-empty state sections. State section
/// order follows the contract rank; empty sections are never returned.
pub fn state_sections<'a>(ids: &[&'a str], fleet: &'a Fleet) -> Vec<StateSection<'a>> {
    let mut sections: Vec<StateSection<'a>> = Vec::new();
    for id in ids {
        let Some(agent) = fleet.agents.get(*id) else {
            continue;
        };
        let state: crate::theme::AgentStateLike = agent.state.into();
        match sections.iter_mut().find(|section| section.state == state) {
            Some(section) => section.agent_ids.push(id),
            None => sections.push(StateSection {
                state,
                agent_ids: vec![id],
            }),
        }
    }
    sections.sort_by_key(|section| section.state.rank());
    sections
}

/// Resolve the detail-pane selection: keep a still-visible selection, else
/// fall back to the first visible agent (highest attention rank). Pure so
/// selection defaults are unit-testable without an egui context.
pub fn resolve_selection<'a>(fleet: &'a Fleet, visible: &[&'a str]) -> Option<&'a str> {
    fleet
        .selected_agent
        .as_deref()
        .filter(|selected| visible.iter().any(|id| id == selected))
        .or_else(|| visible.first().copied())
}

fn show_cards(
    ui: &mut Ui,
    fleet: &mut Fleet,
    visible: &[&str],
    flat: bool,
    selected: Option<&str>,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) {
    let available = ui.available_size();
    let left_width = (available.x * 0.40).max(320.0);
    let right_width = (available.x - left_width - 8.0).max(380.0);
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.allocate_ui(egui::vec2(left_width, available.y), |ui| {
            clicked = master_list(ui, fleet, visible, flat, selected);
        });
        ui.separator();
        ui.allocate_ui(egui::vec2(right_width, available.y), |ui| {
            detail_pane(ui, fleet, selected, allowed, actions);
        });
    });
    if let Some(id) = clicked {
        fleet.select_agent(&id);
    }
}

fn master_list(
    ui: &mut Ui,
    fleet: &Fleet,
    visible: &[&str],
    flat: bool,
    selected: Option<&str>,
) -> Option<String> {
    let mut clicked = None;
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if flat {
                for section in state_sections(visible, fleet) {
                    state_section_header(ui, section.state, section.agent_ids.len());
                    for id in &section.agent_ids {
                        if let Some(id) = master_card(ui, fleet, id, selected == Some(id)) {
                            clicked = Some(id);
                        }
                    }
                    ui.add_space(4.0);
                }
            } else {
                for mut group in group_by_repo(fleet) {
                    group.agent_ids.retain(|id| visible.contains(id));
                    if group.agent_ids.is_empty() {
                        continue;
                    }
                    let title = group.repo.unwrap_or(NO_REPO_LABEL);
                    CollapsingHeader::new(
                        RichText::new(format!("{title}  ({})", group.agent_ids.len()))
                            .monospace()
                            .color(theme::ui::TEXT_STRONG),
                    )
                    .id_salt(("corral-ui-repo-group", title))
                    .default_open(true)
                    .show_unindented(ui, |ui| {
                        for id in &group.agent_ids {
                            if let Some(id) = master_card(ui, fleet, id, selected == Some(id)) {
                                clicked = Some(id);
                            }
                        }
                    });
                    ui.separator();
                }
            }
        });
    clicked
}

fn state_section_header(ui: &mut Ui, state: crate::theme::AgentStateLike, count: usize) {
    let color = theme::state::of(state);
    ui.horizontal(|ui| {
        badge(
            ui,
            &format!("{} {}", state.mark_glyph(), state.label()),
            color,
        );
        ui.label(
            RichText::new(format!("({count})"))
                .small()
                .monospace()
                .color(theme::ui::TEXT_MUTED),
        );
    });
}

fn master_card(ui: &mut Ui, fleet: &Fleet, id: &str, selected: bool) -> Option<String> {
    let agent = fleet.agents.get(id)?;
    let state: crate::theme::AgentStateLike = agent.state.into();
    let color = theme::state::of(state);
    let bg = if selected {
        color.gamma_multiply(0.16)
    } else {
        color.gamma_multiply(0.05)
    };
    let label = format!(
        "{} {}  {}",
        state.mark_glyph(),
        agent.row_label(),
        agent.tool
    );
    let meta = format!(
        "{} · {}",
        agent
            .workspace
            .repo
            .as_deref()
            .unwrap_or(agent.workspace.branch.as_deref().unwrap_or("—")),
        state.label()
    );
    let response = ui
        .scope_builder(
            egui::UiBuilder::new()
                .id_salt(("corral-ui-master-card", id))
                .sense(egui::Sense::click()),
            |ui| {
                egui::Frame::NONE
                    .fill(bg)
                    .stroke(if selected {
                        egui::Stroke::new(1.0, color)
                    } else {
                        egui::Stroke::NONE
                    })
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.add(
                            egui::Label::new(RichText::new(label).color(theme::ui::TEXT_STRONG))
                                .truncate(),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(meta)
                                    .small()
                                    .monospace()
                                    .color(theme::ui::TEXT_MUTED),
                            )
                            .truncate(),
                        );
                    });
            },
        )
        .response;
    if response.clicked() {
        Some(agent.agent_id.clone())
    } else {
        None
    }
}

fn detail_pane(
    ui: &mut Ui,
    fleet: &Fleet,
    selected: Option<&str>,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) {
    let Some(id) = selected else {
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui.label(
                RichText::new("select an agent for detail + Recent output")
                    .color(theme::ui::TEXT_MUTED),
            );
        });
        return;
    };
    let Some(agent) = fleet.agents.get(id) else {
        return;
    };
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            detail(ui, agent, fleet, allowed, actions);
        });
}

fn show_table(
    ui: &mut Ui,
    fleet: &mut Fleet,
    visible: &[&str],
    flat: bool,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) {
    let total_width: f32 = BOARD_COLUMNS.iter().map(|(_, width)| *width).sum();
    ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(total_width);
            header(ui);
            ui.separator();
            let mut toggles: Vec<String> = Vec::new();
            let mut selection = None;
            if flat {
                for id in visible {
                    board_row(
                        ui,
                        id,
                        fleet,
                        allowed,
                        actions,
                        &mut toggles,
                        &mut selection,
                    );
                }
            } else {
                for mut group in group_by_repo(fleet) {
                    group.agent_ids.retain(|id| visible.contains(id));
                    if group.agent_ids.is_empty() {
                        continue;
                    }
                    let title = group.repo.unwrap_or(NO_REPO_LABEL);
                    CollapsingHeader::new(
                        RichText::new(format!("{title}  ({})", group.agent_ids.len()))
                            .monospace()
                            .color(theme::ui::TEXT_STRONG),
                    )
                    .id_salt(("corral-ui-repo-group", title))
                    .default_open(true)
                    .show_unindented(ui, |ui| {
                        for id in &group.agent_ids {
                            board_row(
                                ui,
                                id,
                                fleet,
                                allowed,
                                actions,
                                &mut toggles,
                                &mut selection,
                            );
                        }
                    });
                    ui.separator();
                }
            }
            for id in &toggles {
                fleet.toggle_expanded(id);
            }
            if let Some(id) = selection {
                fleet.select_agent(&id);
            }
        });
}

/// One board section: a repo (or the "(no repo)" orphan bucket) and the
/// attention-ranked agent ids in it.
#[derive(Debug, PartialEq, Eq)]
pub struct RepoGroup<'a> {
    /// `None` = the orphan bucket (agents without `workspace.repo`).
    pub repo: Option<&'a str>,
    pub agent_ids: Vec<&'a str>,
}

/// Group agent ids by `workspace.repo`: named repos sorted by name, the
/// "(no repo)" bucket last. Within a group, ids are attention-ranked by
/// the shared contract rank, then stable id.
pub fn group_by_repo(fleet: &Fleet) -> Vec<RepoGroup<'_>> {
    let mut groups: Vec<RepoGroup<'_>> = Vec::new();
    for (id, agent) in &fleet.agents {
        let repo = agent.workspace.repo.as_deref();
        match groups.iter_mut().find(|g| g.repo == repo) {
            Some(group) => group.agent_ids.push(id.as_str()),
            None => groups.push(RepoGroup {
                repo,
                agent_ids: vec![id.as_str()],
            }),
        }
    }
    groups.sort_by(|a, b| match (a.repo, b.repo) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    for group in &mut groups {
        group.agent_ids.sort_by(|a, b| {
            let rank_a = agent_rank(&fleet.agents[*a]);
            let rank_b = agent_rank(&fleet.agents[*b]);
            rank_a.cmp(&rank_b).then_with(|| a.cmp(b))
        });
    }
    groups
}

/// One agent row + its expanded detail + row separator (shared by the flat
/// and grouped paths so per-agent rendering is identical).
fn board_row(
    ui: &mut Ui,
    id: &str,
    fleet: &Fleet,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
    toggles: &mut Vec<String>,
    selection: &mut Option<String>,
) {
    let Some(agent) = fleet.agents.get(id) else {
        return;
    };
    let is_expanded = fleet.is_expanded(id);
    let (clicked, _) = agent_row(ui, agent, is_expanded, allowed, fleet);
    if clicked {
        toggles.push(id.to_string());
        *selection = Some(id.to_string());
    }
    if is_expanded {
        detail(ui, agent, fleet, allowed, actions);
    }
    ui.separator();
}

fn header(ui: &mut Ui) {
    let _ = header_cells(ui);
}

fn header_cells(ui: &mut Ui) -> [egui::Response; 9] {
    ui.horizontal(|ui| BOARD_COLUMNS.map(|(label, width)| header_cell(ui, width, label)))
        .inner
}

fn header_cell(ui: &mut Ui, width: f32, text: &str) -> egui::Response {
    fixed_cell(ui, width, |ui| {
        ui.add_sized(
            [width, 20.0],
            egui::Label::new(RichText::new(text).monospace().color(theme::ui::TEXT_MUTED)),
        );
    })
}

/// Allocate exactly one board column and bind content to that width. Keeping
/// the minimum AND maximum at the shared column width prevents a long
/// truncated label, badge, or drive control from moving the next column.
fn fixed_cell<R>(
    ui: &mut Ui,
    width: f32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::Response {
    ui.vertical(|ui| {
        ui.set_width(width);
        add_contents(ui);
    })
    .response
}

fn agent_row(
    ui: &mut Ui,
    agent: &Agent,
    is_expanded: bool,
    _allowed: &dyn Fn(&str) -> bool,
    fleet: &Fleet,
) -> (bool, egui::Response) {
    let selected = fleet.selected_agent.as_deref() == Some(agent.agent_id.as_str());
    let bg = if is_expanded || selected {
        theme::ui::ACCENT_DIM.gamma_multiply(0.10)
    } else {
        Color32::TRANSPARENT
    };
    let mut expanded = false;
    let response = ui
        .scope_builder(
            egui::UiBuilder::new()
                .id_salt(("corral-ui-agent-row", &agent.agent_id))
                .sense(egui::Sense::click()),
            |ui| {
                egui::Frame::NONE
                    .fill(bg)
                    // Keep only vertical padding: any left/right margin would
                    // shift row cells against the header, which has none.
                    .inner_margin(egui::Margin::symmetric(0, 4))
                    .show(ui, |ui| {
                        agent_row_cells(ui, agent);
                    })
                    .inner
            },
        )
        .response;

    // Expand/collapse on a click anywhere in the row except on widgets
    // (widgets consume their own clicks, so a plain row click reaches here).
    if response.clicked() {
        expanded = true;
    }
    (expanded, response)
}

fn agent_row_cells(ui: &mut Ui, agent: &Agent) -> [egui::Response; 9] {
    let ws = &agent.workspace;
    let ab = if ws.ahead == 0 && ws.behind == 0 {
        "".to_string()
    } else {
        format!("+{}/−{}", ws.ahead, ws.behind)
    };
    let ab_color = if ws.ahead > 0 {
        theme::ui::WARN
    } else {
        theme::ui::TEXT_MUTED
    };
    ui.horizontal(|ui| {
        [
            agent_cell(ui, agent),
            state_cell(ui, agent),
            waiting_cell(ui, agent),
            topology_cell(
                ui,
                COL_REPO,
                ws.repo.clone().unwrap_or_else(|| "—".into()),
                theme::ui::TEXT_MUTED,
            ),
            branch_cell(ui, agent),
            topology_cell(
                ui,
                COL_DIRTY,
                if ws.dirty { "●".into() } else { "".into() },
                theme::ui::DIRTY,
            ),
            topology_cell(ui, COL_AB, ab, ab_color),
            topology_cell(
                ui,
                COL_PR,
                ws.pr_number
                    .map(|n| format!("#{n}"))
                    .unwrap_or_else(|| "—".into()),
                theme::ui::TEXT_MUTED,
            ),
            ci_cell(ui, ws.ci_status),
        ]
    })
    .inner
}

fn agent_cell(ui: &mut Ui, agent: &Agent) -> egui::Response {
    fixed_cell(ui, COL_AGENT, |ui| {
        ui.add_sized(
            [COL_AGENT - 8.0, 18.0],
            egui::Label::new(RichText::new(agent.row_label()).color(theme::ui::TEXT_STRONG))
                .truncate(),
        );
        ui.add_sized(
            [COL_AGENT - 8.0, 14.0],
            egui::Label::new(
                RichText::new(format!("{} · {}", agent.source, agent.tool))
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            )
            .truncate(),
        );
    })
}

fn state_cell(ui: &mut Ui, agent: &Agent) -> egui::Response {
    fixed_cell(ui, COL_STATE, |ui| {
        let st: crate::theme::AgentStateLike = agent.state.into();
        // AC5: color is never the only channel — carry mark + label.
        badge(
            ui,
            &format!("{} {}", st.mark_glyph(), st.label()),
            state::of(st),
        );
        if let Some(reason) = &agent.reason {
            let truncated: String = reason.chars().take(40).collect();
            ui.add_sized(
                [COL_STATE - 8.0, 30.0],
                egui::Label::new(
                    RichText::new(truncated)
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                )
                .truncate(),
            )
            .on_hover_text(reason);
        }
    })
}

fn waiting_cell(ui: &mut Ui, agent: &Agent) -> egui::Response {
    fixed_cell(ui, COL_WAITING, |ui| {
        let Some(w) = &agent.waiting_on else {
            ui.add_sized([COL_WAITING - 8.0, 18.0], egui::Label::new(""));
            return;
        };
        badge(ui, w.kind.label(), kind::of(w.kind.into()));
        let prompt_preview: String = w.prompt.chars().take(120).collect();
        ui.add_sized(
            [COL_WAITING - 8.0, 30.0],
            egui::Label::new(
                RichText::new(prompt_preview)
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            )
            .truncate(),
        )
        .on_hover_text(w.prompt.clone());
    })
}

fn topology_cell(ui: &mut Ui, width: f32, text: String, color: Color32) -> egui::Response {
    fixed_cell(ui, width, |ui| topology_content(ui, width, text, color))
}

fn ci_cell(ui: &mut Ui, status: Option<crate::model::CiStatus>) -> egui::Response {
    fixed_cell(ui, COL_CI, |ui| match status {
        Some(status) => {
            badge(ui, status.label(), ci::of(status.into()));
        }
        None => topology_content(ui, COL_CI, "—".into(), theme::ui::TEXT_MUTED),
    })
}

fn topology_content(ui: &mut Ui, width: f32, text: String, color: Color32) {
    ui.add_sized(
        [width, 18.0],
        egui::Label::new(RichText::new(text).monospace().small().color(color)).truncate(),
    );
}

/// The branch cell: the branch name plus, when the name infers an issue
/// (D21, display-only), the distinct `~#N` / `~#N?` marker in a
/// validating/flagging color. The marker is pure + deterministic, so the
/// cell is stable across frames.
fn branch_cell(ui: &mut Ui, agent: &Agent) -> egui::Response {
    let ws = &agent.workspace;
    fixed_cell(ui, COL_BRANCH, |ui| {
        let Some(branch) = ws.branch.as_deref() else {
            topology_content(ui, COL_BRANCH, "—".into(), theme::ui::TEXT_STRONG);
            return;
        };
        let Some(inferred) =
            crate::infer::infer(ws.branch.as_deref(), &agent.known_issue_numbers())
        else {
            topology_content(ui, COL_BRANCH, branch.to_string(), theme::ui::TEXT_STRONG);
            return;
        };
        let marker = inferred.marker();
        let (color, tip) = inferred_marker_ui(&inferred);
        let marker_text = format!(" {marker}");
        let marker_font = egui::FontId::monospace(11.0);
        let marker_width = ui
            .fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(marker_text.clone(), marker_font.clone(), color)
                    .size()
                    .x
            })
            .min(COL_BRANCH - BRANCH_MIN_TEXT_WIDTH)
            .ceil();
        let branch_width = COL_BRANCH - marker_width;
        // F1 (review): the marker must survive truncation — truncate ONLY the
        // branch text and reserve an exact, bounded marker segment so long
        // branches (issue-431-embed-project-management) keep the ~#N signal
        // without pushing later columns out of the fixed cell.
        let mut job = egui::text::LayoutJob::default();
        job.append(
            branch,
            0.0,
            egui::TextFormat {
                font_id: marker_font,
                color: theme::ui::TEXT_STRONG,
                ..Default::default()
            },
        );
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_sized([branch_width, 18.0], egui::Label::new(job).truncate());
            ui.add_sized(
                [marker_width, 18.0],
                egui::Label::new(
                    egui::RichText::new(marker_text)
                        .monospace()
                        .size(11.0)
                        .color(color),
                )
                .truncate(),
            );
        })
        .response
        .on_hover_text(tip);
    })
}

/// Marker color + hover explanation for an inferred issue (D21: the `~`
/// prefix and distinct colors make it clearly non-authoritative).
fn inferred_marker_ui(inferred: &crate::infer::InferredIssue) -> (Color32, String) {
    if inferred.known {
        (
            theme::ui::ACCENT,
            format!(
                "~#{}: inferred from the branch name; matches a fetched closing issue — display-only, not authoritative",
                inferred.number
            ),
        )
    } else {
        (
            theme::ui::WARN,
            format!(
                "~#{}?: inferred from the branch name; NOT in the fetched issue set — display-only, never asserted as real",
                inferred.number
            ),
        )
    }
}

/// Pure D21 surface text for an agent's branch-inferred issue: the
/// `~#N` / `~#N?` marker, or `None` when the branch name infers nothing.
/// Shared by the row renderer and tests.
pub fn inferred_marker(agent: &Agent) -> Option<String> {
    crate::infer::infer(
        agent.workspace.branch.as_deref(),
        &agent.known_issue_numbers(),
    )
    .map(|inferred| inferred.marker())
}

/// Whether a canonical drive control is ready, needs a host grant, or is
/// not implemented for this source. Missing capability takes precedence:
/// a capability absent from the snapshot cannot be driven even if the
/// device has a grant for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveControlState {
    Ready,
    MissingGrant,
    NotImplemented,
}

/// Pure classifier shared by the row renderer and tests.
pub fn drive_control_state(
    agent_caps: &[String],
    capability: &str,
    granted: bool,
) -> DriveControlState {
    if !agent_caps.iter().any(|c| c == capability) {
        DriveControlState::NotImplemented
    } else if !granted {
        DriveControlState::MissingGrant
    } else {
        DriveControlState::Ready
    }
}

/// Distinct human-readable reason for a disabled drive control.
pub fn drive_disabled_reason(capability: &str, state: DriveControlState) -> Option<String> {
    match state {
        DriveControlState::Ready => None,
        DriveControlState::MissingGrant => {
            Some(format!("requires the {capability} grant — ask the host"))
        }
        DriveControlState::NotImplemented => Some(format!("{capability}: not implemented yet")),
    }
}

fn disabled_drive_button(ui: &mut Ui, capability: &str, state: DriveControlState) {
    if let Some(reason) = drive_disabled_reason(capability, state) {
        crate::ui::disabled_button_with_reason(ui, capability, &reason);
    }
}

fn drive_controls(
    ui: &mut Ui,
    agent: &Agent,
    allowed: &dyn Fn(&str) -> bool,
    fleet: &Fleet,
    drive: &mut dyn FnMut(DriveIntent),
    full_chat: &mut dyn FnMut(&str),
) {
    let rev = fleet.rev;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.spacing_mut().item_spacing.y = 2.0;

        full_chat_control(ui, agent, allowed, full_chat);
        for cap in crate::drive::CAPABILITIES_ORDER {
            let state = drive_control_state(&agent.capabilities, cap, allowed(cap));
            match cap {
                "prompt" => match state {
                    DriveControlState::Ready => prompt_widget(ui, agent, rev, drive),
                    _ => disabled_drive_button(ui, cap, state),
                },
                "approve" => {
                    if agent.waiting_on.is_none() {
                        continue;
                    }
                    match state {
                        DriveControlState::Ready => approve_choices(ui, agent, rev, drive),
                        _ => disabled_drive_button(ui, cap, state),
                    }
                }
                _ => match state {
                    DriveControlState::Ready => {
                        if ui.small_button(cap).clicked() {
                            let intent = match cap {
                                "interrupt" => DriveIntent::interrupt(&agent.agent_id, rev),
                                "read_tail" => DriveIntent::read_tail(&agent.agent_id, rev),
                                "kill" => DriveIntent::kill(&agent.agent_id, rev),
                                _ => DriveIntent::attach(&agent.agent_id, rev),
                            };
                            drive(intent);
                        }
                    }
                    _ => disabled_drive_button(ui, cap, state),
                },
            }
        }
    });
    if let Some(w) = &agent.waiting_on
        && w.choices.is_empty()
        && allowed("approve")
        && agent.capabilities.iter().any(|c| c == "approve")
    {
        ui.label(
            RichText::new("waiting — no menu choices (reply via prompt)")
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
    }
}

fn full_chat_control(
    ui: &mut Ui,
    agent: &Agent,
    allowed: &dyn Fn(&str) -> bool,
    full_chat: &mut dyn FnMut(&str),
) {
    let state = drive_control_state(&agent.capabilities, "read_tail", allowed("read_tail"));
    match state {
        DriveControlState::Ready => {
            if ui.small_button("Full chat").clicked() {
                full_chat(&agent.agent_id);
            }
        }
        _ => {
            if let Some(reason) = drive_disabled_reason("read_tail", state) {
                crate::ui::disabled_button_with_reason(ui, "Full chat", &reason);
            }
        }
    }
}

fn approve_choices(
    ui: &mut Ui,
    agent: &Agent,
    rev: Option<u64>,
    drive: &mut dyn FnMut(DriveIntent),
) {
    let Some(w) = &agent.waiting_on else {
        return;
    };
    if w.choices.is_empty() {
        return;
    }
    ui.label(
        RichText::new("approve:")
            .small()
            .color(theme::ui::TEXT_MUTED),
    );
    for choice in &w.choices {
        let label: String = choice.chars().take(16).collect();
        if ui.small_button(label).clicked() {
            drive(DriveIntent::approve(agent, choice.clone(), rev));
        }
    }
}

/// Prompt input + Enter-to-send. The buffer lives in egui's temp memory
/// keyed by agent id (no per-frame churn in the fleet model).
fn prompt_widget(ui: &mut Ui, agent: &Agent, rev: Option<u64>, drive: &mut dyn FnMut(DriveIntent)) {
    let id = eframe::egui::Id::new(("corral-ui-prompt", &agent.agent_id));
    let mut text: String = ui
        .ctx()
        .memory(|m| m.data.get_temp::<String>(id).unwrap_or_default());
    let mut submitted = false;
    let response = ui.add(
        TextEdit::singleline(&mut text)
            .id(id)
            .hint_text("prompt…")
            .desired_width(180.0),
    );
    if response.has_focus() && ui.input(|i| i.key_pressed(eframe::egui::Key::Enter)) {
        submitted = true;
    }
    if submitted {
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() {
            drive(DriveIntent::prompt(&agent.agent_id, trimmed, rev));
        }
        ui.ctx().memory_mut(|m| m.data.remove::<String>(id));
        response.surrender_focus();
    } else {
        ui.ctx()
            .memory_mut(|m| m.data.insert_temp::<String>(id, text.clone()));
    }
}

fn detail(
    ui: &mut Ui,
    agent: &Agent,
    fleet: &Fleet,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) {
    egui::Frame::group(ui.style())
        .fill(Color32::from_rgb(0x10, 0x15, 0x1c))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                detail_kv(ui, "id", &agent.agent_id);
                detail_kv(ui, "seq", &agent.seq.to_string());
                detail_kv(ui, "ts", &crate::model::clock_of(agent.ts));
                detail_kv(ui, "host", agent.host.as_deref().unwrap_or("—"));
                if let Some(p) = &agent.parent_id {
                    detail_kv(ui, "parent", p);
                }
                if let Some(wt) = &agent.workspace.worktree_path {
                    detail_kv(ui, "worktree", wt);
                }
            });
            if let Some(title) = &agent.title {
                ui.label(RichText::new(title).color(theme::ui::TEXT_STRONG));
            }
            ui.horizontal_wrapped(|ui| {
                let state: crate::theme::AgentStateLike = agent.state.into();
                badge(
                    ui,
                    &format!("{} {}", state.mark_glyph(), state.label()),
                    theme::state::of(state),
                );
                ui.label(
                    RichText::new(agent.row_label())
                        .strong()
                        .color(theme::ui::TEXT_STRONG),
                );
                ui.label(
                    RichText::new(crate::model::clock_of(agent.ts))
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            });
            if let Some(inferred) = crate::infer::infer(
                agent.workspace.branch.as_deref(),
                &agent.known_issue_numbers(),
            ) {
                let (color, tip) = inferred_marker_ui(&inferred);
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    badge(ui, &inferred.marker(), color);
                    ui.label(RichText::new(tip).small().color(theme::ui::TEXT_MUTED));
                });
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new("drive")
                    .small()
                    .monospace()
                    .color(theme::ui::TEXT_MUTED),
            );
            drive_controls(
                ui,
                agent,
                allowed,
                fleet,
                &mut *actions.drive,
                &mut *actions.full_chat,
            );
            if let Some(w) = &agent.waiting_on {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("prompt (exact string the claim hashes):")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
                egui::Frame::NONE
                    .fill(Color32::from_rgb(0x16, 0x1b, 0x22))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(w.prompt.clone()).monospace()).wrap(),
                        );
                    });
                ui.horizontal_wrapped(|ui| {
                    detail_kv(ui, "prompt_hash", &w.prompt_hash);
                    detail_kv(ui, "approval_id", &w.approval_id);
                });
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new("Recent output")
                    .strong()
                    .color(theme::ui::TEXT_STRONG),
            );
            if let Some(tail) = fleet.tails.get(&agent.agent_id) {
                egui::Frame::NONE
                    .fill(Color32::from_rgb(0x16, 0x1b, 0x22))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        if tail.is_empty() {
                            ui.label(
                                RichText::new("no recent output for this agent")
                                    .small()
                                    .color(theme::ui::TEXT_MUTED),
                            );
                        } else {
                            ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                                for line in tail {
                                    ui.add(egui::Label::new(
                                        RichText::new(line).monospace().small(),
                                    ));
                                }
                            });
                        }
                    });
            } else if fleet
                .recent_drives
                .get(&agent.agent_id)
                .is_some_and(|drives| {
                    drives.iter().any(|d| {
                        matches!(
                            d,
                            DriveState::Ok { capability, .. } if capability == "read_tail"
                        )
                    })
                })
            {
                // Defensive: the drive dispatched, but no tail result ever
                // arrived (e.g. an older daemon without the result path).
                ui.add_space(4.0);
                ui.label(
                    RichText::new("read_tail dispatched + audited; the daemon returned no result")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            } else {
                ui.label(
                    RichText::new("no recent output tapped yet — use Recent output/read_tail")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            }
            transcript_section(ui, agent, fleet, allowed, actions);
            if let Some(recent) = fleet.recent_drives.get(&agent.agent_id) {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("recent drives")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
                for d in recent {
                    ui.label(
                        RichText::new(drive_state_text(d))
                            .monospace()
                            .small()
                            .color(drive_state_color(d)),
                    );
                }
            }
        });
}

/// #64: the lazy-paged full-transcript section inside the row detail.
/// Collapsed by default; OPENING it triggers the newest-page fetch
/// (review F11 — the brief's "open at the newest page"); "load older"
/// follows the cursor. Gated on the advertised read_tail capability AND
/// the grant ledger like every other capability surface (review F5).
/// Rows are virtualized (`show_rows` with a pitch measured from what is
/// actually drawn — review F3);
/// clicking a row shows a BOUNDED slice of its text below (review F4 —
/// a ScrollArea clips but does not virtualize a label, so an unbounded
/// galley would hang the UI thread on a multi-MB entry).
fn transcript_section(
    ui: &mut Ui,
    agent: &Agent,
    fleet: &Fleet,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) {
    use crate::transcript::TranscriptRequest;
    ui.add_space(4.0);
    let state = drive_control_state(&agent.capabilities, "read_tail", allowed("read_tail"));
    if state != DriveControlState::Ready {
        let reason =
            drive_disabled_reason("read_tail", state).expect("disabled transcript has a reason");
        ui.label(RichText::new(reason).small().color(theme::ui::TEXT_MUTED));
        return;
    }
    let pane = fleet.transcripts.get(&agent.agent_id);
    let title = match pane {
        Some(p) if !p.session.is_empty() => format!("transcript — {}", p.session),
        _ => "transcript".to_string(),
    };
    let open = fleet.is_transcript_open(&agent.agent_id);
    let header = egui::CollapsingHeader::new(RichText::new(title).small())
        .id_salt(("corral-ui-transcript", &agent.agent_id))
        .default_open(false)
        .open(Some(open))
        .show(ui, |ui| {
            let Some(pane) = pane else {
                // First open this frame: the fetch is dispatched below
                // (the pane exists from the next frame on).
                ui.spinner();
                return;
            };

            // Status line: bind provenance + honesty counters. Paging is
            // audited server-side (read_tail:transcript) — say so.
            ui.horizontal_wrapped(|ui| {
                if !pane.store.is_empty() {
                    detail_kv(ui, "store", &pane.store);
                    detail_kv(ui, "bind", &pane.bind);
                }
                if pane.skipped > 0 {
                    ui.label(
                        RichText::new(format!("{} torn lines skipped", pane.skipped))
                            .small()
                            .color(theme::ui::WARN),
                    );
                }
                if !pane.stores_unavailable.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "stores unavailable during this walk: {}",
                            pane.stores_unavailable.join(", ")
                        ))
                        .small()
                        .color(theme::ui::WARN),
                    )
                    .on_hover_text(
                        "a session store errored while binding — this view may not be \
                         the agent's newest session",
                    );
                }
                if pane.base_offset > 0 {
                    ui.label(
                        RichText::new(format!(
                            "{} newest entries slid out of the window — reload to return \
                             to the top",
                            pane.base_offset
                        ))
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                    );
                }
                ui.label(
                    RichText::new("reads are audited")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            });

            if let Some(error) = &pane.error {
                ui.label(
                    RichText::new(transcript_error_text(error))
                        .small()
                        .color(theme::ui::BAD),
                );
                for candidate in &error.candidates {
                    ui.label(
                        RichText::new(format!("  candidate: {candidate}"))
                            .monospace()
                            .small(),
                    );
                }
            }

            // Selection is salted with the pane GENERATION (review F10):
            // a reload rebuilds entries, so an index into the old pane
            // must not silently select a different message in the new one.
            let selected_id =
                egui::Id::new(("corral-ui-transcript-sel", &agent.agent_id, pane.generation));
            let mut selected: Option<usize> =
                ui.memory_mut(|m| m.data.get_temp(selected_id)).flatten();
            let row_height = transcript_row_pitch(ui);
            ScrollArea::vertical()
                .id_salt(("corral-ui-transcript-rows", &agent.agent_id))
                .max_height(240.0)
                .auto_shrink([false, true])
                .show_rows(ui, row_height, pane.entries.len(), |ui, range| {
                    for index in range {
                        let entry = &pane.entries[index];
                        // R1: selection is ABSOLUTE (base_offset + index)
                        // — the window slides under relative indices and
                        // a slid selection would silently highlight a
                        // different message.
                        let absolute = pane.base_offset + index;
                        let line = transcript_row_text(absolute, entry);
                        let is_selected = selected == Some(absolute);
                        // Review F3: each row occupies EXACTLY the pitch
                        // show_rows was given — uniform height holds by
                        // construction, not by coincidence of two style
                        // values (the pitch is sized to fit the label;
                        // pinned by test).
                        let desired = egui::vec2(ui.available_width(), row_height);
                        let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
                        // R2 (round-3 correction): an EXPLICIT child id —
                        // id_salt anchors only the child Ui's id, while
                        // the label's auto-id seeds from the parent
                        // counter and would still shift with scroll.
                        // IdSource::Explicit makes the seed derive from
                        // this id alone; (agent, absolute) is unique
                        // within the frame.
                        let mut row_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(rect)
                                .id(egui::Id::new((
                                    "corral-ui-transcript-row",
                                    &agent.agent_id,
                                    absolute,
                                )))
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        );
                        if row_ui
                            .selectable_label(is_selected, RichText::new(line).monospace())
                            .clicked()
                        {
                            selected = if is_selected { None } else { Some(absolute) };
                        }
                    }
                });
            ui.memory_mut(|m| m.data.insert_temp(selected_id, selected));

            if let Some(absolute) = selected
                && let Some(entry) = absolute
                    .checked_sub(pane.base_offset)
                    .and_then(|i| pane.entries.get(i))
            {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} {}",
                            entry.role,
                            entry.ts.map(crate::model::clock_of).unwrap_or_default()
                        ))
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                    );
                    // Review F4: lay out a BOUNDED slice, never the whole
                    // entry. Since #86 the daemon truncates every entry to
                    // its page budget (first entry included), but that cap
                    // is ~256KiB — still far too big to lay out — and the
                    // client should not trust the server's cap anyway.
                    let (shown, truncated) = transcript_detail_text(&entry.text);
                    ScrollArea::vertical()
                        .id_salt(("corral-ui-transcript-full", &agent.agent_id))
                        .max_height(160.0)
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(shown).monospace().small()));
                        });
                    if let Some(note) = truncated {
                        ui.label(RichText::new(note).small().color(theme::ui::WARN));
                    }
                });
            }

            ui.horizontal(|ui| {
                if pane.loading {
                    ui.spinner();
                    ui.label(
                        RichText::new("loading…")
                            .small()
                            .color(theme::ui::TEXT_MUTED),
                    );
                }
                if pane.can_load_older() && ui.small_button("load older").clicked() {
                    (actions.transcript)(TranscriptRequest {
                        agent_id: agent.agent_id.clone(),
                        cursor: pane.next_cursor.clone(),
                    });
                }
                // Review F7: a transient failure keeps the cursor — retry
                // re-issues it instead of throwing the walk away.
                if pane.can_retry() && ui.small_button("retry").clicked() {
                    (actions.transcript)(TranscriptRequest {
                        agent_id: agent.agent_id.clone(),
                        cursor: pane.next_cursor.clone(),
                    });
                }
                if pane.next_cursor.is_none() && !pane.loading && pane.error.is_none() {
                    ui.label(
                        RichText::new("start of transcript")
                            .small()
                            .color(theme::ui::TEXT_MUTED),
                    );
                }
                if !pane.loading && ui.small_button("reload").clicked() {
                    (actions.transcript)(TranscriptRequest {
                        agent_id: agent.agent_id.clone(),
                        cursor: None,
                    });
                }
            });
        });

    // Keep the Fleet-controlled open state in sync with the nested header;
    // the app applies the same deferred Full chat action after this frame.
    if header.header_response.clicked() {
        (actions.full_chat)(&agent.agent_id);
    }

    // Review F11: opening the header IS the fetch trigger — no pane yet
    // and the body is open means this is the first look. One request:
    // the pane exists (loading) from this same frame's dispatch on.
    if pane.is_none() && header.body_returned.is_some() {
        (actions.transcript)(TranscriptRequest {
            agent_id: agent.agent_id.clone(),
            cursor: None,
        });
    }
}

/// Review F3: the virtualized-row pitch. Every row is ALLOCATED at
/// exactly this height (see the show_rows body), so show_rows' uniform
/// assumption holds by construction; this only has to be big enough to
/// fit the row's label without clipping — pinned by a test that renders
/// a real row and asserts pitch >= its natural height.
pub fn transcript_row_pitch(ui: &Ui) -> f32 {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let galley = ui.fonts_mut(|fonts| fonts.row_height(&font));
    (galley + 2.0 * ui.spacing().button_padding.y).max(ui.spacing().interact_size.y)
}

/// Review F4: the bounded detail slice — at most 64 KiB is ever laid
/// out; the note says what was withheld.
pub fn transcript_detail_text(text: &str) -> (&str, Option<String>) {
    const DETAIL_MAX_BYTES: usize = 64 * 1024;
    if text.len() <= DETAIL_MAX_BYTES {
        return (text, None);
    }
    let mut end = DETAIL_MAX_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (
        &text[..end],
        Some(format!(
            "… truncated for display ({end} of {} bytes shown)",
            text.len()
        )),
    )
}

/// One virtualized row: absolute-index labeled, single line, truncated.
/// Pure so the format is testable. The role is truncated client-side
/// (review F12): the daemon normalizes onto a short closed set, but the
/// row's uniform-height invariant must not depend on the other side of
/// the wire.
pub fn transcript_row_text(index: usize, entry: &crate::transcript::TranscriptEntry) -> String {
    let first_line = entry.text.lines().next().unwrap_or("");
    let mut shown: String = first_line.chars().take(120).collect();
    if shown.len() < first_line.len() || entry.text.lines().nth(1).is_some() {
        shown.push('\u{2026}');
    }
    let role: String = entry.role.chars().take(12).collect();
    format!("{index:>4} {role:>9}  {shown}")
}

/// Pure error copy per typed kind — testable, and the not_granted case
/// names the grant the operator must issue.
pub fn transcript_error_text(error: &crate::transcript::TranscriptFailure) -> String {
    match error.kind.as_str() {
        // Defensive: after F5's gating a refusal demotes the ledger and
        // the section is replaced next frame — this copy renders only in
        // the frame(s) before that propagates. Kept deliberately.
        "not_granted" => "needs the read_tail grant (host: corrald-grant.sh)".to_string(),
        "ambiguous_session" => format!("{} — candidates:", error.message),
        "bad_cursor" => "session changed again while paging — reload to continue".to_string(),
        "no_session" => "no session store found for this agent's worktree".to_string(),
        _ => format!("{}: {}", error.kind, error.message),
    }
}

fn detail_kv(ui: &mut Ui, key: &str, value: &str) {
    ui.label(
        RichText::new(format!("{key}:"))
            .small()
            .color(theme::ui::TEXT_MUTED),
    );
    ui.label(
        RichText::new(value)
            .small()
            .monospace()
            .color(theme::ui::TEXT_STRONG),
    );
    ui.add_space(8.0);
}

/// Outcome display for a drive state (shared with tests).
pub fn drive_state_text(state: &DriveState) -> String {
    match state {
        DriveState::Sending {
            request_id,
            capability,
        } => format!("{capability} sending {request_id}"),
        DriveState::Ok { rev, capability } => format!("{capability} → ok  rev {rev}"),
        DriveState::Failed {
            failure,
            capability,
        } => {
            format!("{capability} {kind}: {failure}", kind = failure.kind())
        }
    }
}

fn drive_state_color(state: &DriveState) -> Color32 {
    match state {
        DriveState::Sending { .. } => theme::ui::WARN,
        DriveState::Ok { .. } => theme::ui::GOOD,
        DriveState::Failed { .. } => theme::ui::BAD,
    }
}

/// Outcome classifier used by the app when a drive round-trips.
pub fn classify_drive_state(outcome: &DriveOutcome, capability: &str) -> DriveState {
    match outcome {
        DriveOutcome::Ok { rev, .. } => DriveState::Ok {
            rev: *rev,
            capability: capability.to_string(),
        },
        DriveOutcome::Refused(failure) => DriveState::Failed {
            failure: failure.clone(),
            capability: capability.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::DriveFailure;

    fn agent_with_caps(caps: &[&str]) -> Agent {
        Agent {
            agent_id: "herdr:a".into(),
            source: "herdr".into(),
            tool: "claude".into(),
            state: crate::model::AgentState::Working,
            reason: None,
            seq: 1,
            ts: 0,
            capabilities: caps.iter().map(|c| c.to_string()).collect(),
            waiting_on: None,
            parent_id: None,
            host: None,
            workspace: Default::default(),
            attachment: None,
            display_name: None,
            title: None,
            issues: vec![],
        }
    }

    fn agent_in_repo(id: &str, repo: Option<&str>) -> Agent {
        let mut agent = agent_with_caps(&[]);
        agent.agent_id = id.into();
        agent.workspace.repo = repo.map(str::to_string);
        agent
    }

    fn agent_with_state(id: &str, state: crate::model::AgentState) -> Agent {
        let mut agent = agent_in_repo(id, None);
        agent.state = state;
        agent
    }

    #[test]
    fn board_view_defaults_to_cards() {
        assert_eq!(BoardView::default(), BoardView::Cards);
    }

    #[test]
    fn board_columns_are_the_nine_conformance_columns_within_limit() {
        assert_eq!(BOARD_COLUMNS.len(), 9);
        assert!(
            BOARD_COLUMNS.iter().all(|(label, _)| *label != "DRIVE"),
            "drive is no longer a table column"
        );
        let width: f32 = BOARD_COLUMNS.iter().map(|(_, width)| *width).sum();
        assert_eq!(width, 1032.0);
        assert!(
            width <= 1032.0,
            "table must avoid horizontal scroll at ~1200px"
        );
    }

    #[test]
    fn state_filters_only_show_non_empty_buckets() {
        let mut fleet = Fleet::default();
        fleet.agents.insert(
            "herdr:idle".into(),
            agent_with_state("herdr:idle", crate::model::AgentState::Idle),
        );
        fleet.agents.insert(
            "herdr:working".into(),
            agent_with_state("herdr:working", crate::model::AgentState::Working),
        );
        assert_eq!(
            available_state_filters(&fleet, ""),
            vec![StateFilter::All, StateFilter::Working, StateFilter::Idle],
            "every non-empty bucket keeps its chip and empty buckets stay hidden"
        );
        assert_eq!(
            available_state_filters(&fleet, "working"),
            vec![StateFilter::All, StateFilter::Working],
            "chips compose with search over the agent fields"
        );

        let mut unknown_fleet = Fleet::default();
        unknown_fleet.agents.insert(
            "herdr:unknown".into(),
            agent_with_state("herdr:unknown", crate::model::AgentState::Unknown),
        );
        assert_eq!(
            available_state_filters(&unknown_fleet, ""),
            vec![StateFilter::All, StateFilter::Idle],
            "unknown is represented by the Idle bucket, never its own chip"
        );
    }

    #[test]
    fn state_sections_skip_every_empty_bucket() {
        let mut fleet = Fleet::default();
        fleet.agents.insert(
            "herdr:blocked".into(),
            agent_with_state("herdr:blocked", crate::model::AgentState::Blocked),
        );
        fleet.agents.insert(
            "herdr:working".into(),
            agent_with_state("herdr:working", crate::model::AgentState::Working),
        );
        fleet.agents.insert(
            "herdr:idle".into(),
            agent_with_state("herdr:idle", crate::model::AgentState::Idle),
        );
        let ids = ["herdr:idle", "herdr:working"];
        let sections = state_sections(&ids, &fleet);
        assert_eq!(sections.len(), 2, "no empty section may be returned");
        assert_eq!(
            sections.iter().map(|s| s.state).collect::<Vec<_>>(),
            vec![
                crate::theme::AgentStateLike::Working,
                crate::theme::AgentStateLike::Idle,
            ],
            "sections follow contract rank with the empty blocked bucket omitted"
        );
    }

    #[test]
    fn visible_agent_ids_rank_sort_and_filter() {
        let mut fleet = Fleet::default();
        for (id, state) in [
            ("herdr:idle", crate::model::AgentState::Idle),
            ("herdr:unknown", crate::model::AgentState::Unknown),
            ("herdr:blocked", crate::model::AgentState::Blocked),
            ("herdr:done", crate::model::AgentState::Done),
            ("herdr:working", crate::model::AgentState::Working),
        ] {
            fleet.agents.insert(id.into(), agent_with_state(id, state));
        }
        assert_eq!(
            visible_agent_ids(&fleet, StateFilter::All, ""),
            vec![
                "herdr:blocked",
                "herdr:done",
                "herdr:working",
                "herdr:idle",
                "herdr:unknown",
            ],
            "priority order is blocked, review, working, idle, unknown"
        );
        assert_eq!(
            visible_agent_ids(&fleet, StateFilter::Blocked, ""),
            vec!["herdr:blocked"]
        );
        assert_eq!(
            visible_agent_ids(&fleet, StateFilter::Idle, ""),
            vec!["herdr:idle", "herdr:unknown"],
            "Idle chip includes unknown at stable-id order"
        );
    }

    #[test]
    fn selection_defaults_and_preserves_still_visible_choice() {
        let mut fleet = Fleet::default();
        fleet.agents.insert(
            "herdr:a".into(),
            agent_with_state("herdr:a", crate::model::AgentState::Blocked),
        );
        fleet.agents.insert(
            "herdr:b".into(),
            agent_with_state("herdr:b", crate::model::AgentState::Working),
        );
        let visible = ["herdr:a", "herdr:b"];
        assert_eq!(resolve_selection(&fleet, &visible), Some("herdr:a"));

        fleet.select_agent("herdr:b");
        assert_eq!(resolve_selection(&fleet, &visible), Some("herdr:b"));

        fleet.select_agent("herdr:hidden");
        assert_eq!(resolve_selection(&fleet, &visible), Some("herdr:a"));
        assert!(resolve_selection(&fleet, &[]).is_none());
    }

    #[test]
    fn table_row_click_selects_agent() {
        let ctx = row_test_context();
        let mut agent = agent_with_caps(&[]);
        agent.agent_id = "herdr:table".into();
        agent.display_name = Some("table agent".into());
        let mut fleet = Fleet::default();
        fleet.agents.insert(agent.agent_id.clone(), agent.clone());
        let mut actions = BoardActions {
            drive: &mut |_| {},
            transcript: &mut |_| {},
            full_chat: &mut |_| {},
        };
        let mut toggles = Vec::new();
        let mut selection = None;
        let mut row_rect = None;
        let mut output = ctx.run_ui(row_test_input(vec![]), |ui| {
            row_test_style(ui);
            row_rect = Some(agent_row(ui, &agent, false, &|_| true, &fleet).1.rect);
        });
        assert!(
            text_rect(&output, "table agent").is_some(),
            "table row rendered"
        );
        let pos = egui::pos2(
            row_rect.expect("table row rendered").left() + 2.0,
            row_rect.expect("table row rendered").top() + 2.0,
        );
        clear_textures(&mut output);
        for input in [pointer_down_input(pos), pointer_up_input(pos)] {
            let mut output = ctx.run_ui(input, |ui| {
                row_test_style(ui);
                board_row(
                    ui,
                    &agent.agent_id,
                    &fleet,
                    &|_| true,
                    &mut actions,
                    &mut toggles,
                    &mut selection,
                );
            });
            clear_textures(&mut output);
        }
        assert_eq!(
            selection.as_deref(),
            Some("herdr:table"),
            "clicking a Table row selects it for the master/detail model"
        );
    }

    #[test]
    fn agent_search_covers_repo_branch_title_display_name_issue_and_pr() {
        let mut agent = agent_with_caps(&[]);
        agent.agent_id = "herdr:agent".into();
        agent.display_name = Some("Charlie".into());
        agent.title = Some("Fix the Widget".into());
        agent.workspace.repo = Some("alpha/corral".into());
        agent.workspace.branch = Some("issue-42-widget".into());
        agent.workspace.pr_number = Some(987);
        agent.issues = vec![crate::model::GhIssueRef {
            repo: "plush".into(),
            number: 777,
            state: "open".into(),
            title: "Deep Dive".into(),
            labels: vec![],
            url: String::new(),
        }];
        for query in [
            "",
            "ALPHA",
            "ISSUE-42",
            "WIDGET",
            "Charlie",
            "deep dive",
            "777",
            "#777",
            "plush",
            "#987",
        ] {
            assert!(agent_matches_query(&agent, query), "query {query:?}");
        }
        assert!(!agent_matches_query(&agent, "zzz-no-match"));
    }

    #[test]
    fn state_filter_labels_stay_on_the_contract_tokens() {
        let cases = [
            (StateFilter::Blocked, crate::theme::AgentStateLike::Blocked),
            (StateFilter::Done, crate::theme::AgentStateLike::Done),
            (StateFilter::Working, crate::theme::AgentStateLike::Working),
            (StateFilter::Idle, crate::theme::AgentStateLike::Idle),
        ];
        for (filter, state) in cases {
            assert_eq!(filter.label(), state.label());
        }
        assert_eq!(StateFilter::All.label(), "All");
    }

    #[test]
    fn group_by_repo_sorts_names_and_pushes_orphans_last() {
        let mut fleet = Fleet::default();
        for (id, repo) in [
            ("herdr:z", Some("zeta")),
            ("herdr:a", Some("alpha")),
            ("herdr:o", None),
            ("herdr:b", Some("alpha")),
        ] {
            fleet.agents.insert(id.into(), agent_in_repo(id, repo));
        }
        fleet.agents.get_mut("herdr:a").unwrap().state = crate::model::AgentState::Done;
        fleet.agents.get_mut("herdr:b").unwrap().state = crate::model::AgentState::Blocked;
        let groups = group_by_repo(&fleet);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].repo, Some("alpha"));
        assert_eq!(
            groups[0].agent_ids,
            vec!["herdr:b", "herdr:a"],
            "within a repo, contract rank beats BTreeMap order"
        );
        assert_eq!(groups[1].repo, Some("zeta"));
        assert_eq!(groups[1].agent_ids, vec!["herdr:z"]);
        assert_eq!(groups[2].repo, None, "orphan bucket is last");
        assert_eq!(groups[2].agent_ids, vec!["herdr:o"]);
    }

    #[test]
    fn group_by_repo_ranks_agents_within_groups() {
        let mut fleet = Fleet::default();
        for (id, repo) in [
            ("herdr:c", Some("one")),
            ("herdr:a", Some("one")),
            ("herdr:b", Some("one")),
        ] {
            fleet.agents.insert(id.into(), agent_in_repo(id, repo));
        }
        fleet.agents.get_mut("herdr:a").unwrap().state = crate::model::AgentState::Blocked;
        fleet.agents.get_mut("herdr:b").unwrap().state = crate::model::AgentState::Done;
        fleet.agents.get_mut("herdr:c").unwrap().state = crate::model::AgentState::Idle;
        let groups = group_by_repo(&fleet);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0].agent_ids,
            vec!["herdr:a", "herdr:b", "herdr:c"],
            "group order follows contract rank, not BTreeMap id order"
        );
    }

    #[test]
    fn group_by_repo_partitions_all_agents_exactly_once() {
        let mut fleet = Fleet::default();
        for i in 0..29 {
            let id = format!("herdr:agent-{i:02}");
            let repo = match i % 3 {
                0 => Some("herdr-board"),
                1 => Some("sendmeter"),
                _ => None,
            };
            fleet.agents.insert(id.clone(), agent_in_repo(&id, repo));
        }
        let groups = group_by_repo(&fleet);
        let total: usize = groups.iter().map(|g| g.agent_ids.len()).sum();
        assert_eq!(total, 29, "every agent lands in exactly one group");
        assert_eq!(groups.last().unwrap().repo, None);
        let mut seen: Vec<&str> = groups
            .iter()
            .flat_map(|g| g.agent_ids.iter().copied())
            .collect();
        seen.sort_unstable();
        let mut unique = seen.clone();
        unique.dedup();
        assert_eq!(seen, unique, "no agent id appears in two groups");
        assert_eq!(groups.last().unwrap().agent_ids.len(), 9);
    }

    #[test]
    fn group_by_repo_empty_fleet_has_no_groups() {
        assert!(group_by_repo(&Fleet::default()).is_empty());
    }

    #[test]
    fn board_header_and_agent_row_columns_start_at_identical_x_positions() {
        let ctx = egui::Context::default();
        ctx.set_visuals(crate::theme::dark_dashboard());
        let mut agent = agent_with_caps(&[]);
        agent.reason = Some("state reason ".repeat(80));
        agent.waiting_on = Some(crate::model::WaitingOn {
            kind: crate::model::WaitingOnKind::Menu,
            prompt: "waiting prompt ".repeat(200),
            prompt_hash: "sha256:test".into(),
            approval_id: String::new(),
            choices: vec![],
        });
        agent.workspace.repo = Some("very-long-repository/name/that/keeps/going ".repeat(30));
        // Regression (#131 review): exercise the inferred-marker path with a
        // long, truncated branch so an unbound marker cannot shift later cells.
        agent.workspace.branch = Some(format!("issue-431-{}", "a".repeat(200)));
        agent.issues = vec![crate::model::GhIssueRef {
            repo: "corral".into(),
            number: 431,
            state: "open".into(),
            title: "long branch marker".into(),
            labels: vec![],
            url: String::new(),
        }];
        assert_eq!(
            inferred_marker(&agent).as_deref(),
            Some("~#431"),
            "the branch must actually render the inferred marker"
        );
        agent.workspace.pr_number = Some(123_456);
        agent.workspace.ci_status = Some(crate::model::CiStatus::Pending);
        agent.workspace.dirty = true;
        agent.workspace.ahead = 12;
        agent.workspace.behind = 3;

        let mut measured = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            // Match the app's board style (app.rs configure_fonts).
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
            ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
            let header = header_cells(ui)
                .into_iter()
                .map(|cell| cell.rect)
                .collect::<Vec<_>>();
            let row = egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(0, 4))
                .show(ui, |ui| {
                    agent_row_cells(ui, &agent)
                        .into_iter()
                        .map(|cell| cell.rect)
                        .collect::<Vec<_>>()
                })
                .inner;
            measured = Some((header, row));
        });
        output.textures_delta.clear();
        let (header, row) = measured.expect("header and row rendered");

        assert_eq!(header.len(), BOARD_COLUMNS.len());
        assert_eq!(row.len(), BOARD_COLUMNS.len());
        for (i, (header, row)) in header.iter().zip(&row).enumerate() {
            let expected_width = BOARD_COLUMNS[i].1;
            assert!(
                (header.left() - row.left()).abs() <= 0.01,
                "column {i} starts at header.x={} but row.x={}",
                header.left(),
                row.left()
            );
            assert!(
                (header.width() - expected_width).abs() <= 0.01,
                "header column {i} width {} != {}",
                header.width(),
                expected_width
            );
            assert!(
                (row.width() - expected_width).abs() <= 0.01,
                "row column {i} width {} != {}",
                row.width(),
                expected_width
            );
        }
    }

    fn row_test_screen() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 800.0))
    }

    fn row_test_context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_visuals(crate::theme::dark_dashboard());
        ctx
    }

    fn row_test_style(ui: &mut egui::Ui) {
        // Match the app's board style (app.rs configure_fonts).
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
        ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
    }

    fn row_test_input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(row_test_screen()),
            events,
            ..Default::default()
        }
    }

    fn pointer_down_input(pos: egui::Pos2) -> egui::RawInput {
        row_test_input(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ])
    }

    fn pointer_up_input(pos: egui::Pos2) -> egui::RawInput {
        row_test_input(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ])
    }

    fn clear_textures(output: &mut egui::FullOutput) {
        output.textures_delta.clear();
    }

    fn text_rects(output: &egui::FullOutput, needle: &str) -> Vec<egui::Rect> {
        fn walk(shape: &egui::epaint::Shape, needle: &str, rects: &mut Vec<egui::Rect>) {
            match shape {
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, needle, rects);
                    }
                }
                egui::epaint::Shape::Text(text) if text.galley.job.text.contains(needle) => {
                    rects.push(text.visual_bounding_rect());
                }
                _ => {}
            }
        }
        let mut rects = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, needle, &mut rects);
        }
        rects
    }

    fn text_rect(output: &egui::FullOutput, needle: &str) -> Option<egui::Rect> {
        text_rects(output, needle).into_iter().next()
    }

    fn board_row_frame(
        ctx: &egui::Context,
        fleet: &Fleet,
        id: &str,
        input: egui::RawInput,
        actions: &mut BoardActions,
    ) -> (Vec<String>, egui::FullOutput) {
        board_row_frame_with_allowed(ctx, fleet, id, input, &|_| true, actions)
    }

    fn board_row_frame_with_allowed(
        ctx: &egui::Context,
        fleet: &Fleet,
        id: &str,
        input: egui::RawInput,
        allowed: &dyn Fn(&str) -> bool,
        actions: &mut BoardActions,
    ) -> (Vec<String>, egui::FullOutput) {
        let mut toggles = Vec::new();
        let mut selection = None;
        let output = ctx.run_ui(input, |ui| {
            row_test_style(ui);
            board_row(
                ui,
                id,
                fleet,
                allowed,
                actions,
                &mut toggles,
                &mut selection,
            );
        });
        (toggles, output)
    }

    fn board_row_click(
        ctx: &egui::Context,
        fleet: &Fleet,
        id: &str,
        pos: egui::Pos2,
        actions: &mut BoardActions,
    ) -> Vec<String> {
        board_row_click_with_allowed(ctx, fleet, id, pos, &|_| true, actions)
    }

    fn board_row_click_with_allowed(
        ctx: &egui::Context,
        fleet: &Fleet,
        id: &str,
        pos: egui::Pos2,
        allowed: &dyn Fn(&str) -> bool,
        actions: &mut BoardActions,
    ) -> Vec<String> {
        let (down_toggles, mut output) =
            board_row_frame_with_allowed(ctx, fleet, id, pointer_down_input(pos), allowed, actions);
        assert!(
            down_toggles.is_empty(),
            "pointer press alone must not emit a row toggle"
        );
        clear_textures(&mut output);
        let (up_toggles, mut output) =
            board_row_frame_with_allowed(ctx, fleet, id, pointer_up_input(pos), allowed, actions);
        clear_textures(&mut output);
        up_toggles
    }

    fn apply_row_toggles(fleet: &mut Fleet, toggles: Vec<String>) {
        for id in toggles {
            fleet.toggle_expanded(&id);
        }
    }

    #[test]
    fn agent_row_board_row_blank_click_toggles_and_renders_tail_detail() {
        let ctx = row_test_context();
        let mut agent = agent_with_caps(&[]);
        agent.agent_id = "herdr:e2e".into();
        let mut fleet = Fleet::default();
        fleet.agents.insert(agent.agent_id.clone(), agent.clone());
        fleet
            .tails
            .insert(agent.agent_id.clone(), vec!["tail line".into()]);
        let mut intents = Vec::new();
        let mut actions = BoardActions {
            drive: &mut |intent| intents.push(intent),
            transcript: &mut |_| {},
            full_chat: &mut |_| {},
        };

        let mut row_rect = None;
        let mut output = ctx.run_ui(row_test_input(vec![]), |ui| {
            row_test_style(ui);
            row_rect = Some(agent_row(ui, &agent, false, &|_| false, &fleet).1.rect);
        });
        let row_rect = row_rect.expect("row rendered");
        assert!(
            text_rect(&output, "e2e").is_some(),
            "display_name=None must fall back to a bounded agent-id label"
        );
        assert!(
            text_rect(&output, &agent.agent_id).is_none(),
            "the raw agent id must not be the collapsed row label"
        );
        clear_textures(&mut output);

        let blank_click = egui::pos2(row_rect.left() + 2.0, row_rect.top() + 2.0);
        let toggles = board_row_click(&ctx, &fleet, &agent.agent_id, blank_click, &mut actions);
        assert_eq!(
            toggles,
            vec![agent.agent_id.clone()],
            "blank board-row click must request the row toggle"
        );
        apply_row_toggles(&mut fleet, toggles);
        assert!(
            fleet.is_expanded(&agent.agent_id),
            "applying the board toggle must expand the row"
        );

        let (toggles, mut output) = board_row_frame(
            &ctx,
            &fleet,
            &agent.agent_id,
            row_test_input(vec![]),
            &mut actions,
        );
        assert!(
            toggles.is_empty(),
            "an idle expanded frame must not emit another toggle"
        );
        assert!(
            text_rect(&output, "Recent output").is_some(),
            "expanded detail must render the Recent output header"
        );
        assert!(
            text_rect(&output, "tail line").is_some(),
            "expanded detail must render the cached tail"
        );
        assert!(
            text_rect(&output, &agent.agent_id).is_some(),
            "expanded detail must still expose the stable agent id"
        );
        clear_textures(&mut output);

        let toggles = board_row_click(&ctx, &fleet, &agent.agent_id, blank_click, &mut actions);
        assert_eq!(
            toggles,
            vec![agent.agent_id.clone()],
            "clicking the expanded row again must request another toggle"
        );
        apply_row_toggles(&mut fleet, toggles);
        assert!(
            !fleet.is_expanded(&agent.agent_id),
            "applying the second board toggle must collapse the row"
        );

        fleet.tails.insert(agent.agent_id.clone(), Vec::new());
        let toggles = board_row_click(&ctx, &fleet, &agent.agent_id, blank_click, &mut actions);
        assert_eq!(
            toggles,
            vec![agent.agent_id.clone()],
            "an empty-tail row must still expand on click"
        );
        apply_row_toggles(&mut fleet, toggles);
        let (toggles, mut output) = board_row_frame(
            &ctx,
            &fleet,
            &agent.agent_id,
            row_test_input(vec![]),
            &mut actions,
        );
        assert!(toggles.is_empty());
        assert!(
            text_rect(&output, "no recent output for this agent").is_some(),
            "empty tail must keep its existing empty-state copy"
        );
        clear_textures(&mut output);
    }

    #[test]
    fn detail_read_tail_click_dispatches_once_without_toggling_table_row() {
        let ctx = row_test_context();
        let mut agent = agent_with_caps(&["read_tail"]);
        agent.agent_id = "herdr:read-tail".into();
        let mut fleet = Fleet::default();
        fleet.agents.insert(agent.agent_id.clone(), agent.clone());
        fleet.expanded.push(agent.agent_id.clone());
        let mut intents = Vec::new();
        let mut actions = BoardActions {
            drive: &mut |intent| intents.push(intent),
            transcript: &mut |_| {},
            full_chat: &mut |_| {},
        };

        let (_, mut output) = board_row_frame_with_allowed(
            &ctx,
            &fleet,
            &agent.agent_id,
            row_test_input(vec![]),
            &|_| true,
            &mut actions,
        );
        let button_rect = text_rect(&output, "read_tail").expect("read_tail button rendered");
        clear_textures(&mut output);

        let toggles = board_row_click_with_allowed(
            &ctx,
            &fleet,
            &agent.agent_id,
            button_rect.center(),
            &|_| true,
            &mut actions,
        );
        assert!(
            toggles.is_empty(),
            "drive-button click must not request a row toggle"
        );
        assert_eq!(
            intents.len(),
            1,
            "read_tail click must dispatch exactly one intent"
        );
        assert_eq!(intents[0].capability, crate::drive::Capability::ReadTail);
        assert_eq!(intents[0].target, "herdr:read-tail");
    }

    #[test]
    fn full_chat_button_opens_transcript_and_toggles_it_closed() {
        let ctx = row_test_context();
        let mut agent = agent_with_caps(&["read_tail"]);
        agent.agent_id = "herdr:full-chat".into();
        let mut fleet = Fleet::default();
        fleet.agents.insert(agent.agent_id.clone(), agent.clone());
        fleet.expanded.push(agent.agent_id.clone());
        let full_chat_requests = std::cell::RefCell::new(Vec::new());
        let transcript_requests = std::cell::RefCell::new(Vec::new());
        let mut actions = BoardActions {
            drive: &mut |_| {},
            transcript: &mut |request| transcript_requests.borrow_mut().push(request),
            full_chat: &mut |agent_id| full_chat_requests.borrow_mut().push(agent_id.to_string()),
        };

        let (_, mut output) = board_row_frame_with_allowed(
            &ctx,
            &fleet,
            &agent.agent_id,
            row_test_input(vec![]),
            &|_| true,
            &mut actions,
        );
        let full_chat_rect = text_rect(&output, "Full chat").expect("Full chat button rendered");
        clear_textures(&mut output);

        let toggles = board_row_click_with_allowed(
            &ctx,
            &fleet,
            &agent.agent_id,
            full_chat_rect.center(),
            &|_| true,
            &mut actions,
        );
        assert!(
            toggles.is_empty(),
            "Full chat click must not request a separate row toggle"
        );
        assert_eq!(
            full_chat_requests.borrow().as_slice(),
            vec![agent.agent_id.clone()],
            "granted Full chat must ask the app to open the pane"
        );
        let requests = std::mem::take(&mut *full_chat_requests.borrow_mut());
        for agent_id in requests {
            fleet.toggle_full_chat(&agent_id);
        }
        assert!(fleet.is_expanded(&agent.agent_id));
        assert!(fleet.is_transcript_open(&agent.agent_id));

        let (toggles, mut output) = board_row_frame_with_allowed(
            &ctx,
            &fleet,
            &agent.agent_id,
            row_test_input(vec![]),
            &|_| true,
            &mut actions,
        );
        assert!(toggles.is_empty());
        assert_eq!(
            transcript_requests.borrow().len(),
            1,
            "opening the controlled transcript must dispatch the newest-page fetch"
        );
        let full_chat_rect = text_rect(&output, "Full chat").expect("Full chat still rendered");
        clear_textures(&mut output);

        let toggles = board_row_click_with_allowed(
            &ctx,
            &fleet,
            &agent.agent_id,
            full_chat_rect.center(),
            &|_| true,
            &mut actions,
        );
        assert!(toggles.is_empty());
        assert_eq!(
            full_chat_requests.borrow().as_slice(),
            vec![agent.agent_id.clone()],
            "a second Full chat click must toggle the open pane closed"
        );
        let requests = std::mem::take(&mut *full_chat_requests.borrow_mut());
        for agent_id in requests {
            fleet.toggle_full_chat(&agent_id);
        }
        assert!(
            !fleet.is_transcript_open(&agent.agent_id),
            "second click closes the transcript"
        );
        assert!(
            fleet.is_expanded(&agent.agent_id),
            "closing the transcript keeps the row detail open"
        );
    }

    #[test]
    fn nested_transcript_header_toggles_fleet_state_and_dispatches_first_read_once() {
        let ctx = row_test_context();
        let mut agent = agent_with_caps(&["read_tail"]);
        agent.agent_id = "herdr:nested-chat".into();
        let mut fleet = Fleet::default();
        fleet.agents.insert(agent.agent_id.clone(), agent.clone());
        fleet.expanded.push(agent.agent_id.clone());
        let full_chat_requests = std::cell::RefCell::new(Vec::new());
        let transcript_requests = std::cell::RefCell::new(Vec::new());
        let mut actions = BoardActions {
            drive: &mut |_| {},
            transcript: &mut |request| transcript_requests.borrow_mut().push(request),
            full_chat: &mut |agent_id| full_chat_requests.borrow_mut().push(agent_id.to_string()),
        };

        let (_, mut output) = board_row_frame_with_allowed(
            &ctx,
            &fleet,
            &agent.agent_id,
            row_test_input(vec![]),
            &|_| true,
            &mut actions,
        );
        let header_rect = text_rect(&output, "transcript").expect("nested header rendered");
        clear_textures(&mut output);

        let toggles = board_row_click_with_allowed(
            &ctx,
            &fleet,
            &agent.agent_id,
            header_rect.center(),
            &|_| true,
            &mut actions,
        );
        assert!(
            toggles.is_empty(),
            "header clicks are consumed by the widget"
        );
        assert_eq!(
            full_chat_requests.borrow().as_slice(),
            vec![agent.agent_id.clone()],
            "opening the nested header must sync Fleet open state"
        );
        let requests = std::mem::take(&mut *full_chat_requests.borrow_mut());
        for agent_id in requests {
            fleet.toggle_full_chat(&agent_id);
        }
        assert!(fleet.is_transcript_open(&agent.agent_id));
        assert!(fleet.is_expanded(&agent.agent_id));

        let (_, mut output) = board_row_frame_with_allowed(
            &ctx,
            &fleet,
            &agent.agent_id,
            row_test_input(vec![]),
            &|_| true,
            &mut actions,
        );
        assert_eq!(
            transcript_requests.borrow().len(),
            1,
            "the existing first-open fetch must fire exactly once"
        );
        let pane = fleet.transcript_pane_mut(&agent.agent_id);
        pane.loading = true;
        let header_rect = text_rect(&output, "transcript").expect("open header still rendered");
        clear_textures(&mut output);
        let toggles = board_row_click_with_allowed(
            &ctx,
            &fleet,
            &agent.agent_id,
            header_rect.center(),
            &|_| true,
            &mut actions,
        );
        assert!(toggles.is_empty());
        assert_eq!(
            full_chat_requests.borrow().as_slice(),
            vec![agent.agent_id.clone()],
            "closing the nested header must sync Fleet open state"
        );
        assert_eq!(
            transcript_requests.borrow().len(),
            1,
            "closing must not reissue the newest-page fetch"
        );
        let requests = std::mem::take(&mut *full_chat_requests.borrow_mut());
        for agent_id in requests {
            fleet.toggle_full_chat(&agent_id);
        }
        assert!(!fleet.is_transcript_open(&agent.agent_id));
        assert!(fleet.is_expanded(&agent.agent_id));
    }

    #[test]
    fn transcript_header_hidden_without_advertised_read_tail() {
        let ctx = row_test_context();
        let mut agent = agent_with_caps(&["kill"]);
        agent.agent_id = "herdr:no-read-capability".into();
        let mut fleet = Fleet::default();
        fleet.agents.insert(agent.agent_id.clone(), agent.clone());
        fleet.expanded.push(agent.agent_id.clone());
        let mut actions = BoardActions {
            drive: &mut |_| {},
            transcript: &mut |_| {},
            full_chat: &mut |_| {},
        };
        let (_, mut output) = board_row_frame_with_allowed(
            &ctx,
            &fleet,
            &agent.agent_id,
            row_test_input(vec![]),
            &|_| true,
            &mut actions,
        );
        assert!(
            text_rect(&output, "transcript").is_none(),
            "a capability not advertised by the agent must not expose the nested header"
        );
        assert!(
            text_rect(&output, "read_tail: not implemented yet").is_some(),
            "the expanded row must explain why the capability is unavailable"
        );
        clear_textures(&mut output);
    }

    #[test]
    fn full_chat_is_disabled_without_read_tail_grant() {
        let ctx = row_test_context();
        let mut agent = agent_with_caps(&["read_tail"]);
        agent.agent_id = "herdr:no-read-grant".into();
        let mut fleet = Fleet::default();
        fleet.agents.insert(agent.agent_id.clone(), agent.clone());
        fleet.expanded.push(agent.agent_id.clone());
        let mut full_chat_requests = Vec::new();
        let mut transcript_requests = Vec::new();
        let mut actions = BoardActions {
            drive: &mut |_| {},
            transcript: &mut |request| transcript_requests.push(request),
            full_chat: &mut |agent_id| full_chat_requests.push(agent_id.to_string()),
        };

        let (_, mut output) = board_row_frame_with_allowed(
            &ctx,
            &fleet,
            &agent.agent_id,
            row_test_input(vec![]),
            &|_| false,
            &mut actions,
        );
        let full_chat_rect = text_rect(&output, "Full chat").expect("gated Full chat rendered");
        clear_textures(&mut output);

        let toggles = board_row_click_with_allowed(
            &ctx,
            &fleet,
            &agent.agent_id,
            full_chat_rect.center(),
            &|_| false,
            &mut actions,
        );
        assert!(toggles.is_empty());
        assert!(
            full_chat_requests.is_empty(),
            "missing grant must keep Full chat unclickable"
        );
        assert!(
            transcript_requests.is_empty(),
            "missing grant must never dispatch a transcript read"
        );
    }

    #[test]
    fn drive_control_reasons_distinguish_grant_miss_from_not_implemented() {
        let caps = vec!["kill".to_string()];
        assert_eq!(
            drive_control_state(&caps, "kill", true),
            DriveControlState::Ready
        );
        assert_eq!(
            drive_control_state(&caps, "kill", false),
            DriveControlState::MissingGrant
        );
        assert_eq!(
            drive_control_state(&[], "kill", true),
            DriveControlState::NotImplemented
        );
        assert_eq!(
            drive_disabled_reason("kill", DriveControlState::Ready),
            None
        );

        let grant =
            drive_disabled_reason("kill", DriveControlState::MissingGrant).expect("grant reason");
        let not_implemented = drive_disabled_reason("kill", DriveControlState::NotImplemented)
            .expect("not implemented reason");
        assert!(grant.contains("requires the kill grant"));
        assert!(grant.contains("ask the host"));
        assert!(not_implemented.contains("kill: not implemented yet"));
        assert!(!not_implemented.contains("grant"));
        assert_ne!(grant, not_implemented, "reasons must never be conflated");
    }

    #[test]
    fn inferred_marker_flags_branch_hints_display_only() {
        // D21: `~#N` when validated against the fetched issue set,
        // `~#N?` when not; no marker for branches that infer nothing.
        let mut agent = agent_with_caps(&[]);
        agent.workspace.branch = Some("issue-24-widget".into());
        assert_eq!(
            inferred_marker(&agent).as_deref(),
            Some("~#24?"),
            "no fetched set (pre-G23 daemon) → flagged, never asserted"
        );
        agent.issues = vec![crate::model::GhIssueRef {
            repo: "corral".into(),
            number: 24,
            state: "open".into(),
            title: "widget".into(),
            labels: vec![],
            url: String::new(),
        }];
        assert_eq!(
            inferred_marker(&agent).as_deref(),
            Some("~#24"),
            "present in the fetched set → validated marker"
        );
        agent.workspace.branch = Some("main".into());
        assert_eq!(inferred_marker(&agent), None);
        agent.workspace.branch = None;
        assert_eq!(inferred_marker(&agent), None);
    }

    #[test]
    fn drive_state_text_covers_all_variants() {
        let sending = drive_state_text(&DriveState::Sending {
            request_id: "r1".into(),
            capability: "read_tail".into(),
        });
        assert!(sending.contains("r1"));
        assert!(sending.contains("read_tail"));
        assert!(
            drive_state_text(&DriveState::Ok {
                rev: 7,
                capability: "interrupt".into(),
            })
            .contains("rev 7")
        );
        assert!(
            drive_state_text(&DriveState::Failed {
                failure: DriveFailure::NotGranted("n".into()),
                capability: "kill".into(),
            })
            .contains("not_granted")
        );
    }

    #[test]
    fn classify_drive_state_maps_outcomes() {
        assert_eq!(
            classify_drive_state(
                &DriveOutcome::Ok {
                    rev: 3,
                    result: None
                },
                "read_tail"
            ),
            DriveState::Ok {
                rev: 3,
                capability: "read_tail".into(),
            }
        );
        assert!(matches!(
            classify_drive_state(
                &DriveOutcome::Refused(DriveFailure::StaleApproval),
                "approve"
            ),
            DriveState::Failed { .. }
        ));
    }

    /// #64: virtualized rows are one line, truncated, index-stable —
    /// a multiline or over-long body gets an ellipsis, never a second
    /// layout line (uniform show_rows height depends on it).
    #[test]
    fn transcript_row_text_is_single_line_and_truncated() {
        let short = crate::transcript::TranscriptEntry {
            role: "user".into(),
            text: "hello".into(),
            ts: None,
        };
        assert_eq!(transcript_row_text(3, &short), "   3      user  hello");

        let multiline = crate::transcript::TranscriptEntry {
            role: "assistant".into(),
            text: "first line\nsecond".into(),
            ts: Some(1),
        };
        let row = transcript_row_text(0, &multiline);
        assert!(row.ends_with('\u{2026}'), "{row:?}");
        assert!(!row.contains('\n'), "single layout line");

        let long = crate::transcript::TranscriptEntry {
            role: "assistant".into(),
            text: "x".repeat(500),
            ts: None,
        };
        let row = transcript_row_text(12, &long);
        assert!(row.chars().count() < 140, "truncated: {}", row.len());
        assert!(row.ends_with('\u{2026}'));
    }

    /// #64 review F3: rows are allocated at exactly the pitch, so the
    /// pitch must FIT a real rendered row — measured against a real
    /// selectable_label in a real egui pass with the app's spacing. A
    /// theme change that would clip row text fails here, not on screen.
    #[test]
    fn transcript_row_pitch_matches_a_rendered_row() {
        let ctx = egui::Context::default();
        ctx.set_visuals(crate::theme::dark_dashboard());
        let mut measured: Option<(f32, f32)> = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            // The app's board spacing (app.rs sets these per frame).
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
            let pitch = transcript_row_pitch(ui);
            let entry = crate::transcript::TranscriptEntry {
                role: "assistant".into(),
                text: "measured row".into(),
                ts: None,
            };
            let line = transcript_row_text(0, &entry);
            let response = ui.selectable_label(false, egui::RichText::new(line).monospace());
            measured = Some((pitch, response.rect.height()));
        });
        // A headless pass never uploads textures; acknowledge the delta
        // so its drop guard stays quiet.
        output.textures_delta.clear();
        let (pitch, actual) = measured.expect("rendered");
        assert!(
            pitch >= actual,
            "pitch {pitch} must fit the rendered row height {actual} (no clipping)"
        );
        assert!(
            pitch <= actual + 8.0,
            "pitch {pitch} should not be wildly larger than the row {actual} (dead space)"
        );

        // R7: the load-bearing invariant is that a row can NEVER wrap to
        // a second line — pinned with a long line in a deliberately
        // NARROW rect using the exact render structure (exact-size
        // allocation + left_to_right child, whose wrap mode is Extend).
        let mut narrow: Option<(f32, f32)> = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
            let pitch = transcript_row_pitch(ui);
            let entry = crate::transcript::TranscriptEntry {
                role: "assistant".into(),
                text: "w".repeat(300),
                ts: None,
            };
            let line = transcript_row_text(7, &entry);
            let desired = egui::vec2(80.0, pitch);
            let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
            let mut row_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .id(egui::Id::new(("corral-ui-transcript-row", "test", 7usize)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            let response = row_ui.selectable_label(false, egui::RichText::new(line).monospace());
            narrow = Some((pitch, response.rect.height()));
        });
        // Clear BEFORE asserting: a failed assert must not double-panic
        // through the TexturesDelta drop guard.
        output.textures_delta.clear();
        let (pitch, height) = narrow.expect("narrow row rendered");
        assert!(
            height <= pitch,
            "a long line in a narrow rect must not wrap: row {height} > pitch {pitch}"
        );
    }

    /// #64 review F4: the selected-entry detail lays out a BOUNDED slice
    /// — a multi-MB entry yields a capped slice plus an honest note.
    #[test]
    fn transcript_detail_text_is_bounded() {
        let (shown, note) = transcript_detail_text("short");
        assert_eq!(shown, "short");
        assert!(note.is_none());

        let big = "é".repeat(200_000); // 2 bytes/char: boundary-safe slice
        let (shown, note) = transcript_detail_text(&big);
        assert!(shown.len() <= 64 * 1024);
        assert!(shown.len() >= 64 * 1024 - 4, "cap honored tightly");
        let note = note.expect("truncation is announced");
        assert!(note.contains("truncated"), "{note}");
        assert!(note.contains("400000"), "names the full size: {note}");
    }

    /// #64: typed error copy — the grant refusal names the fix; a stale
    /// cursor after the one auto-reload tells the user what happened.
    #[test]
    fn transcript_error_text_maps_kinds() {
        let f = |kind: &str, message: &str| crate::transcript::TranscriptFailure {
            kind: kind.into(),
            message: message.into(),
            candidates: vec![],
        };
        assert!(transcript_error_text(&f("not_granted", "x")).contains("read_tail grant"));
        assert!(transcript_error_text(&f("bad_cursor", "x")).contains("reload"));
        assert!(transcript_error_text(&f("no_session", "x")).contains("no session store"));
        assert!(
            transcript_error_text(&f("ambiguous_session", "more than one")).contains("candidates")
        );
        assert_eq!(
            transcript_error_text(&f("query_timeout", "slow")),
            "query_timeout: slow"
        );
    }
}
