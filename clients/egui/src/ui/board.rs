//! Fleet board: the Cards-only master/detail surface used by the #206
//! workspace. The master pane is attention-ranked by
//! [`crate::theme::AgentStateLike::rank`], searchable, state-chipped, and
//! grouped by repo. The detail pane owns drive controls, the full waiting-on
//! claim, and Recent output. The old table renderer remains only as
//! an internal conformance helper; no native navigation reaches it.

use std::cmp::Ordering;

use eframe::egui::{
    Align2, CollapsingHeader, Color32, CornerRadius, FontId, RichText, ScrollArea, Sense, Stroke,
    TextEdit, Ui,
};

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
/// #232: lazy diffstat on the agent row (`+N/−M`, filled from the per-agent
/// diff cache after the first read_diff — never prefetched fleet-wide).
const COL_DIFF: f32 = 72.0;
const COL_AB: f32 = 64.0;
const COL_PR: f32 = 56.0;
const COL_CI: f32 = 76.0;

/// Board columns in render order. Both the header and every agent row draw
/// from this one width source so labels and values start at identical x
/// positions.
#[allow(dead_code)]
const BOARD_COLUMNS: [(&str, f32); 10] = [
    ("AGENT", COL_AGENT),
    ("STATE", COL_STATE),
    ("WAITING ON", COL_WAITING),
    ("REPO", COL_REPO),
    ("BRANCH", COL_BRANCH),
    ("DIRTY", COL_DIRTY),
    ("DIFF", COL_DIFF),
    ("A/B", COL_AB),
    ("PR", COL_PR),
    ("CI", COL_CI),
];

/// Keep at least this much branch text even when the inferred marker is
/// unusually long; the marker segment is bounded to the remaining width.
#[allow(dead_code)]
const BRANCH_MIN_TEXT_WIDTH: f32 = 36.0;

/// Reserved `State · relative age` slot on a master card.
///
/// Header for the bucket of agents without `workspace.repo` (sorts last).
const NO_REPO_LABEL: &str = "(no repo)";

/// Legacy egui temp-memory key retained for the internal table conformance
/// renderer. The native #206 surface never reads or writes it.
const FLAT_VIEW: &str = "corral-ui-board-flat";

/// Legacy egui temp-memory key retained for migration of older client state.
/// The native Cards surface no longer exposes a flat-sort switch.
const CARDS_FLAT_VIEW: &str = "corral-ui-cards-flat";

const DEFAULT_FLAT: bool = false;
const DEFAULT_CARDS_FLAT: bool = false;

/// The approved desktop prototype is a true 42/58 master/detail split.
pub const MASTER_DETAIL_RATIO: (f32, f32) = (0.42, 0.58);
const MASTER_ROW_HEIGHT: f32 = 34.0;
const MASTER_HEADER_HEIGHT: f32 = 28.0;
const MIN_CARDS_WIDTH: f32 = 700.0;
const MIN_CARDS_HEIGHT: f32 = 420.0;
const TOOL_PILL_HEIGHT: f32 = 18.0;
// The row's identity text follows the 12px card inset, 11px state mark, and
// egui's 6px item gap plus the 7px label gap.
const MASTER_IDENTITY_INSET: f32 = 36.0;
/// Sized to hold the longest ordinary contract label (`Needs you · 100d 00h`)
/// without elision. The age is reserved before identity/repo text, so in a
/// narrow pane the left column is dropped first and ordinary ages are never
/// clipped; only extreme timestamps truncate inside this bound.
const MASTER_STATE_WIDTH: f32 = 160.0;
/// Keep the terminal state label clear of the master/detail divider and the
/// right-hand theme padding. This also leaves room for the final glyph in a
/// right-aligned label instead of painting it into the divider.
const MASTER_STATE_RIGHT_INSET: f32 = 8.0;

#[cfg(test)]
const CARD_AGE_WIDTH: f32 = MASTER_STATE_WIDTH;

/// egui temp-memory key for the master/detail search query.
const SEARCH_QUERY: &str = "corral-ui-board-search";

/// Legacy egui temp-memory key retained for old renderer tests. The native
/// #206 surface is Cards-only.
const VIEW_MODE: &str = "corral-ui-board-view";
const KILL_CONFIRM: &str = "corral-ui-kill-confirm";
const KILL_CONFIRM_STARTED: &str = "corral-ui-kill-confirm-started";
const KILL_CONFIRM_TIMEOUT_SECONDS: f64 = 10.0;

const LIVE_LABEL: &str = "live";
const PAUSED_LABEL: &str = "paused";
const EARLIER_OUTPUT_LABEL: &str = "Earlier output";
const LOAD_EARLIER_LABEL: &str = "Load earlier";
const USER_BLOCK_INSET: f32 = 24.0;

/// Cards or the exact nine-column internal conformance table. Only Cards is
/// reachable from the native workspace; the table variant exists so the
/// renderer's historical geometry tests remain honest without exposing it to
/// operators.
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
    StateFilter::Blocked,
    StateFilter::All,
    StateFilter::Done,
    StateFilter::Working,
    StateFilter::Idle,
];

/// State buckets that have at least one matching agent. The production
/// toolbar uses this result so the zero-state rule is enforced by the same
/// path the native board renders, not by a test-only projection. `Blocked`
/// intentionally precedes `All` to match the prototype's `Needs you / All`
/// chip order; `All` remains unconditional.
fn available_state_filters(fleet: &Fleet, query: &str) -> Vec<StateFilter> {
    let query = query.trim();
    STATE_FILTERS
        .into_iter()
        .filter(|candidate| {
            *candidate == StateFilter::All
                || fleet.agents.values().any(|agent| {
                    candidate.keeps(agent.state.into()) && agent_matches_query(agent, query)
                })
        })
        .collect()
}

/// Callbacks the board issues to the app (drive dispatch). Deferred-action
/// pattern: the board renders against `&Fleet`, so the app collects intents
/// and acts after `show` returns.
pub struct BoardActions<'a> {
    pub drive: &'a mut dyn FnMut(DriveIntent),
    /// #215 read-only web build: every drive control is replaced by a
    /// single disabled "read-only (web)" indicator and `drive` is never
    /// invoked. Always `false` on desktop — the native board has the full
    /// grant-gated drive plane.
    pub read_only: bool,
}

/// Render the fleet board.
pub fn show(
    ui: &mut Ui,
    fleet: &mut Fleet,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) -> Option<String> {
    // #210: the web/pages board renders through this entry point, so the
    // fleet-health strip lives here too (the desktop app shows it in the
    // persistent master bar). Renders even when the agent set is empty —
    // fleet health is the one surface that must survive a zero-agent board.
    show_health_strip(ui, &fleet.fleet_health, now_millis());
    if fleet.agents.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("no agents in the fleet yet — waiting for corrald")
                    .color(theme::ui::TEXT_MUTED),
            );
        });
        return None;
    }
    // The public board entry point is intentionally fixed to Cards. The
    // legacy view-mode memory cannot resurrect Table after a persisted client
    // state migration or a test harness restores an older value.
    let mut view = BoardView::Cards;
    let mut flat = false;
    show_cards(ui, fleet, &mut view, &mut flat, allowed, actions)
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
    let previous_view = *view;
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            let response = ui.add(
                TextEdit::singleline(query)
                    .id_salt(("corral-ui-board-search-input", SEARCH_QUERY))
                    .hint_text("Search repo / branch / issue…")
                    .desired_width(ui.available_width().max(180.0) - 158.0),
            );
            if response.changed() {
                changed = true;
            }
            for candidate in available_state_filters(fleet, query)
                .into_iter()
                .filter(|candidate| matches!(candidate, StateFilter::Blocked | StateFilter::All))
            {
                if filter_chip(ui, candidate, *filter == candidate).clicked() {
                    *filter = candidate;
                    changed = true;
                }
            }
        });

        // Cards-only navigation deliberately has no flat-sort control. The
        // `flat` argument remains only for compatibility with the internal
        // table geometry tests and is never changed by native UI.
    });
    if *filter == StateFilter::Blocked
        && !fleet
            .agents
            .values()
            .any(|agent| agent.state == crate::model::AgentState::Blocked)
    {
        *filter = StateFilter::All;
        changed = true;
    }
    if changed {
        if *view != previous_view {
            *flat = flat_view(ui.ctx(), *view);
        }
        persist_toolbar_state(ui, *view, *flat, query, *filter);
    }
}

/// Prototype filter chip: `All` is the active working-blue chip, while
/// `Needs you` retains its red-tinted affordance even when the zero-state has
/// no blocked agents to list.
fn filter_chip(ui: &mut Ui, filter: StateFilter, selected: bool) -> egui::Response {
    let label = filter.label();
    let color = match filter {
        StateFilter::Blocked => state::BLOCKED,
        StateFilter::All => state::WORKING,
        StateFilter::Done => state::DONE,
        StateFilter::Working => state::WORKING,
        StateFilter::Idle => state::IDLE,
    };
    let font = FontId::proportional(11.0);
    let galley = ui.fonts_mut(|fonts| fonts.layout_no_wrap(label.to_string(), font, color));
    let size = galley.size() + egui::vec2(20.0, 10.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = match filter {
        StateFilter::All if selected => state::WORKING,
        StateFilter::Blocked => Color32::from_rgba_unmultiplied(
            state::BLOCKED.r(),
            state::BLOCKED.g(),
            state::BLOCKED.b(),
            if selected { 56 } else { 41 },
        ),
        _ if selected => theme::ui::PANEL3,
        _ => theme::ui::PANEL2,
    };
    let text_color = if filter == StateFilter::All && selected {
        Color32::from_rgb(0x08, 0x13, 0x1f)
    } else {
        color
    };
    let stroke_color = if filter == StateFilter::All && selected {
        Color32::TRANSPARENT
    } else if filter == StateFilter::Blocked {
        state::BLOCKED
    } else {
        theme::ui::LINE
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(10), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(10),
        Stroke::new(1.0, stroke_color),
        egui::StrokeKind::Outside,
    );
    // `Painter::galley` keeps the color embedded in the measured galley on
    // some native font backends. Paint the final label explicitly so the
    // active blue chip cannot turn its dark `All` text into an invisible
    // same-color glyph.
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        text_color,
    );
    response
}

fn persist_toolbar_state(ui: &Ui, view: BoardView, flat: bool, query: &str, filter: StateFilter) {
    ui.ctx().memory_mut(|m| {
        m.data
            .insert_temp::<bool>(egui::Id::new(flat_key(view)), flat);
        m.data
            .insert_temp::<String>(egui::Id::new(SEARCH_QUERY), query.to_string());
        m.data.insert_temp(
            egui::Id::new(("corral-ui-board-filter", SEARCH_QUERY)),
            filter,
        );
        m.data
            .insert_temp::<BoardView>(egui::Id::new(VIEW_MODE), view);
    });
}

fn flat_key(view: BoardView) -> &'static str {
    match view {
        BoardView::Cards => CARDS_FLAT_VIEW,
        BoardView::Table => FLAT_VIEW,
    }
}

fn flat_view(ctx: &egui::Context, view: BoardView) -> bool {
    let (key, default) = match view {
        BoardView::Cards => (CARDS_FLAT_VIEW, DEFAULT_CARDS_FLAT),
        BoardView::Table => (FLAT_VIEW, DEFAULT_FLAT),
    };
    ctx.memory(|m| {
        m.data
            .get_temp::<bool>(egui::Id::new(key))
            .unwrap_or(default)
    })
}

fn search_query(ctx: &egui::Context) -> String {
    ctx.memory(|m| {
        m.data
            .get_temp::<String>(egui::Id::new(SEARCH_QUERY))
            .unwrap_or_default()
    })
}

fn state_filter(ctx: &egui::Context) -> StateFilter {
    ctx.memory(|m| {
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
    if [
        agent.workspace.repo.as_deref(),
        agent.workspace.branch.as_deref(),
        agent.title.as_deref(),
        agent.display_name.as_deref(),
        Some(agent.agent_id.as_str()),
    ]
    .into_iter()
    .flatten()
    .any(|part| part.to_lowercase().contains(&query))
    {
        return true;
    }
    if agent
        .workspace
        .pr_number
        .is_some_and(|number| number_matches_query(&query, &number.to_string()))
    {
        return true;
    }
    for issue in &agent.issues {
        if issue.repo.to_lowercase().contains(&query)
            || issue.title.to_lowercase().contains(&query)
            || number_matches_query(&query, &issue.number.to_string())
        {
            return true;
        }
    }
    false
}

fn number_matches_query(query: &str, number: &str) -> bool {
    let digits = query.strip_prefix('#').unwrap_or(query);
    number.contains(digits)
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
        let mut state: crate::theme::AgentStateLike = agent.state.into();
        if state == crate::theme::AgentStateLike::Unknown {
            state = crate::theme::AgentStateLike::Idle;
        }
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
    view: &mut BoardView,
    flat: &mut bool,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) -> Option<String> {
    ScrollArea::both()
        .id_salt("corral-ui-cards-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_size(egui::vec2(MIN_CARDS_WIDTH, MIN_CARDS_HEIGHT));
            show_cards_surface(ui, fleet, view, flat, allowed, actions)
        })
        .inner
}

fn show_cards_surface(
    ui: &mut Ui,
    fleet: &mut Fleet,
    view: &mut BoardView,
    flat: &mut bool,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) -> Option<String> {
    let available = ui
        .available_size()
        .max(egui::vec2(MIN_CARDS_WIDTH, MIN_CARDS_HEIGHT));
    let (board_rect, _) = ui.allocate_exact_size(available, Sense::hover());
    let left_width = board_rect.width() * MASTER_DETAIL_RATIO.0;
    let right_width = board_rect.width() - left_width - 1.0;
    let left_rect =
        egui::Rect::from_min_size(board_rect.min, egui::vec2(left_width, board_rect.height()));
    let right_rect = egui::Rect::from_min_size(
        egui::pos2(board_rect.left() + left_width + 1.0, board_rect.top()),
        egui::vec2(right_width.max(0.0), board_rect.height()),
    );
    let painter = ui.painter();
    painter.rect_filled(board_rect, CornerRadius::same(12), theme::ui::BG);
    painter.rect_stroke(
        board_rect,
        CornerRadius::same(12),
        Stroke::new(1.0, theme::ui::FRAME_BORDER),
        egui::StrokeKind::Outside,
    );
    painter.rect_filled(left_rect, CornerRadius::ZERO, theme::ui::PANEL);
    painter.line_segment(
        [
            egui::pos2(left_rect.right(), board_rect.top()),
            egui::pos2(left_rect.right(), board_rect.bottom()),
        ],
        Stroke::new(1.0, theme::ui::LINE),
    );

    let mut left_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(left_rect.shrink(1.0))
            .id(egui::Id::new("corral-ui-master-pane"))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let mut query = search_query(ui.ctx());
    let mut filter = state_filter(ui.ctx());
    toolbar(&mut left_ui, fleet, view, flat, &mut query, &mut filter);
    left_ui.add_space(2.0);
    let visible_ids: Vec<String> = visible_agent_ids(fleet, filter, &query)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let visible: Vec<&str> = visible_ids.iter().map(String::as_str).collect();
    let selected = resolve_selection(fleet, &visible).map(str::to_owned);
    let mut clicked = None;
    if visible.is_empty() {
        empty_pane_message(&mut left_ui, &query);
    } else {
        clicked = master_list(
            &mut left_ui,
            fleet,
            &visible,
            *flat,
            selected.as_deref(),
            now_millis(),
        );
    }
    let mut right_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(right_rect.shrink(1.0))
            .id(egui::Id::new("corral-ui-detail-pane-root"))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    right_pane(
        &mut right_ui,
        fleet,
        selected.as_deref(),
        view,
        *flat,
        allowed,
        actions,
    );
    if let Some(id) = clicked.as_deref() {
        fleet.select_agent(id);
    }
    clicked.or(selected)
}

/// Render the persistent repo-grouped master bar used by every workspace tab.
/// Search and state chips intentionally live here so changing the right-hand
/// tab never changes the selected fleet context.
pub fn show_master(
    ui: &mut Ui,
    fleet: &mut Fleet,
    group_by_repo: bool,
    show_idle_collapsed: bool,
) -> Option<String> {
    let mut query = search_query(ui.ctx());
    let mut filter = state_filter(ui.ctx());
    let mut cards = BoardView::Cards;
    let mut grouped = false;
    toolbar(ui, fleet, &mut cards, &mut grouped, &mut query, &mut filter);
    ui.add_space(2.0);
    show_health_strip(ui, &fleet.fleet_health, now_millis());
    ui.ctx().memory_mut(|memory| {
        memory.data.insert_temp(
            egui::Id::new("corral-ui-show-idle-collapsed"),
            show_idle_collapsed,
        );
    });
    let visible_ids: Vec<String> = visible_agent_ids(fleet, filter, &query)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let visible: Vec<&str> = visible_ids.iter().map(String::as_str).collect();
    let selected = resolve_selection(fleet, &visible).map(str::to_owned);
    let clicked = if visible.is_empty() {
        empty_pane_message(ui, &query);
        None
    } else {
        master_list(
            ui,
            fleet,
            &visible,
            !group_by_repo,
            selected.as_deref(),
            now_millis(),
        )
    };
    if let Some(id) = clicked.as_deref() {
        fleet.select_agent(id);
    }
    clicked.or(selected)
}

/// #210: compact per-fleet health strip above the master list — HEALTH
/// ONLY (orch alive, live worker count, presence-heartbeat age). One pill
/// per fleet, wrapped; degraded fleets get the warning tint + ⚠, paused
/// fleets render muted. No spend/balance value ever reaches this surface.
fn show_health_strip(ui: &mut Ui, health: &[crate::model::FleetHealthEntry], now_ms: u64) {
    if health.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for entry in health {
            let color = health_pill_color(entry);
            let text = health_pill_text(entry, now_ms);
            ui.label(RichText::new(text).monospace().size(10.5).color(color))
                .on_hover_text(health_pill_hover(entry, now_ms));
        }
    });
}

/// Pure text projection of one fleet-health pill (testable; mirrors the
/// daemon's `FleetHealthEntry` shape).
pub fn health_pill_text(entry: &crate::model::FleetHealthEntry, now_ms: u64) -> String {
    let marker = if entry.paused {
        "⏸"
    } else if entry.degraded {
        "⚠"
    } else {
        "●"
    };
    let orch = if entry.orch_alive {
        "orch ✓"
    } else {
        "orch ✗"
    };
    let workers = format!("{}w", entry.workers);
    let heartbeat = match entry.last_heartbeat {
        Some(at) => format!("♥{}", fleet_heartbeat_age(at, now_ms)),
        None => "♥—".to_string(),
    };
    let paused = if entry.paused { " paused" } else { "" };
    format!(
        "{marker} {}  {orch}  {workers}  {heartbeat}{paused}",
        entry.name
    )
}

/// Compact heartbeat-age label (`4s`, `3m`, `2h 05m`) — the same shape the
/// iOS strip renders, so both surfaces read identically.
pub fn fleet_heartbeat_age(at_ms: u64, now_ms: u64) -> String {
    let elapsed_secs = now_ms.saturating_sub(at_ms) / 1000;
    if elapsed_secs < 60 {
        return format!("{elapsed_secs}s");
    }
    let minutes = elapsed_secs / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h {:02}m", minutes % 60);
    }
    format!("{}d {}h", hours / 24, hours % 24)
}

fn health_pill_hover(entry: &crate::model::FleetHealthEntry, now_ms: u64) -> String {
    let state = entry.orch_state.as_deref().unwrap_or("absent");
    let mut detail = format!(
        "{} · orch {} ({state}) · {} workers",
        entry.name, entry.orch, entry.workers
    );
    if let Some(at) = entry.last_heartbeat {
        detail.push_str(&format!(
            " · heartbeat {} ago",
            fleet_heartbeat_age(at, now_ms)
        ));
    } else {
        detail.push_str(" · heartbeat unknown");
    }
    if !entry.warnings.is_empty() {
        detail.push_str(&format!(" · ⚠ {}", entry.warnings.join(", ")));
    }
    detail
}

fn health_pill_color(entry: &crate::model::FleetHealthEntry) -> Color32 {
    if entry.paused {
        theme::ui::TEXT_MUTED
    } else if entry.degraded {
        theme::ui::WARN
    } else {
        // A live fleet reads as healthy teal; the accent avoids the
        // system-red/green palette (design-system-patterns).
        theme::ui::ACCENT
    }
}

/// Render the Board detail pane after the app has drawn the common tab strip.
/// The view argument is deliberately fixed to Cards; Table is not a reachable
/// native navigation state for the #206 surface.
pub fn show_board_detail(
    ui: &mut Ui,
    fleet: &Fleet,
    selected: Option<&str>,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) {
    show_board_detail_with_options(ui, fleet, selected, allowed, actions, true);
}

/// Render the Cards detail with the persisted Board behavior toggles applied.
/// The default wrapper above keeps the pure rendering/conformance tests on the
/// approved prototype defaults while the app supplies the user's setting.
pub fn show_board_detail_with_options(
    ui: &mut Ui,
    fleet: &Fleet,
    selected: Option<&str>,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
    stick_to_bottom: bool,
) {
    ui.ctx().memory_mut(|memory| {
        memory
            .data
            .insert_temp(egui::Id::new("corral-ui-stick-to-bottom"), stick_to_bottom);
    });
    let mut view = BoardView::Cards;
    right_pane(ui, fleet, selected, &mut view, false, allowed, actions);
}

#[allow(dead_code)]
fn show_empty_table_state(ui: &mut Ui, query: &str) {
    let total_width: f32 = BOARD_COLUMNS.iter().map(|(_, width)| *width).sum();
    ScrollArea::both()
        .id_salt("corral-ui-table")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(total_width);
            header(ui);
            ui.separator();
            ui.label(RichText::new(no_match_message(query)).color(theme::ui::TEXT_MUTED));
        });
}

fn no_match_message(query: &str) -> String {
    format!("No agents match “{}”", query.trim())
}

fn empty_pane_message(ui: &mut Ui, query: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.label(RichText::new(no_match_message(query)).color(theme::ui::TEXT_MUTED));
    });
}

fn master_list(
    ui: &mut Ui,
    fleet: &Fleet,
    visible: &[&str],
    flat: bool,
    selected: Option<&str>,
    now_ms: u64,
) -> Option<String> {
    let mut clicked = None;
    ScrollArea::vertical()
        .id_salt("corral-ui-master-list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width();
            master_column_header(ui, width);
            if flat {
                for section in state_sections(visible, fleet) {
                    if section.state == crate::theme::AgentStateLike::Blocked {
                        // A blocked section is only emitted when it has rows;
                        // this is the zero-state rule's important boundary.
                        state_section_header(ui, section.state, section.agent_ids.len());
                    }
                    if section.state == crate::theme::AgentStateLike::Idle {
                        let count = section.agent_ids.len();
                        CollapsingHeader::new(
                            RichText::new(format!("Idle / done ({count}) — expandable"))
                                .small()
                                .color(theme::ui::TEXT_MUTED),
                        )
                        .id_salt("corral-ui-idle-done")
                        .default_open(!ui.ctx().memory(|memory| {
                            memory
                                .data
                                .get_temp::<bool>(egui::Id::new("corral-ui-show-idle-collapsed"))
                                .unwrap_or(true)
                        }))
                        .show_unindented(ui, |ui| {
                            for id in &section.agent_ids {
                                if let Some(id) =
                                    master_card(ui, fleet, id, selected == Some(id), now_ms)
                                {
                                    clicked = Some(id);
                                }
                            }
                        });
                        continue;
                    }
                    for id in &section.agent_ids {
                        if let Some(id) = master_card(ui, fleet, id, selected == Some(id), now_ms) {
                            clicked = Some(id);
                        }
                    }
                }
            } else {
                for mut group in group_by_repo(fleet) {
                    group.agent_ids.retain(|id| visible.contains(id));
                    if group.agent_ids.is_empty() {
                        continue;
                    }
                    let title = group.repo.unwrap_or(NO_REPO_LABEL);
                    let idle_only = group.agent_ids.iter().all(|id| {
                        matches!(
                            fleet.agents.get(*id).map(|agent| agent.state.into()),
                            Some(crate::theme::AgentStateLike::Idle)
                                | Some(crate::theme::AgentStateLike::Unknown)
                                | Some(crate::theme::AgentStateLike::Done)
                        )
                    });
                    CollapsingHeader::new(
                        RichText::new(format!("{title}  ({})", group.agent_ids.len()))
                            .monospace()
                            .color(theme::ui::TEXT_STRONG),
                    )
                    .id_salt(("corral-ui-repo-group", title))
                    .default_open(if idle_only {
                        !ui.ctx().memory(|memory| {
                            memory
                                .data
                                .get_temp::<bool>(egui::Id::new("corral-ui-show-idle-collapsed"))
                                .unwrap_or(true)
                        })
                    } else {
                        true
                    })
                    .show_unindented(ui, |ui| {
                        for id in &group.agent_ids {
                            if let Some(id) =
                                master_card(ui, fleet, id, selected == Some(id), now_ms)
                            {
                                clicked = Some(id);
                            }
                        }
                    });
                }
            }
        });
    clicked
}

fn master_column_header(ui: &mut Ui, width: f32) {
    let state_width = MASTER_STATE_WIDTH;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width.max(0.0), MASTER_HEADER_HEIGHT),
        Sense::hover(),
    );
    let identity_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + MASTER_IDENTITY_INSET, rect.top()),
        egui::pos2(rect.right() - state_width, rect.bottom()),
    );
    let mut identity_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(identity_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    identity_ui.add(
        egui::Label::new(
            RichText::new("Agent")
                .small()
                .monospace()
                .color(theme::ui::TEXT_MUTED),
        )
        .halign(egui::Align::LEFT),
    );
    let state_rect = egui::Rect::from_min_max(
        egui::pos2(rect.right() - state_width, rect.top()),
        egui::pos2(rect.right() - MASTER_STATE_RIGHT_INSET, rect.bottom()),
    );
    let mut state_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(state_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    state_ui.add(
        egui::Label::new(
            RichText::new("State · time")
                .small()
                .monospace()
                .color(theme::ui::TEXT_MUTED),
        )
        .halign(egui::Align::RIGHT),
    );
    ui.painter().line_segment(
        [
            egui::pos2(rect.left(), rect.bottom()),
            egui::pos2(rect.right(), rect.bottom()),
        ],
        Stroke::new(1.0, theme::ui::LINE),
    );
}

/// Epoch millis for relative-age rendering. `std::time::SystemTime` is
/// unimplemented on wasm32-unknown-unknown (#215), so the web build reads
/// the JS wall clock (`Date.now()` — true epoch millis, so the age math
/// stays identical to desktop).
#[cfg(not(target_arch = "wasm32"))]
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(target_arch = "wasm32")]
fn now_millis() -> u64 {
    js_sys::Date::now() as u64
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

fn master_card(
    ui: &mut Ui,
    fleet: &Fleet,
    id: &str,
    selected: bool,
    now_ms: u64,
) -> Option<String> {
    master_card_with_response(ui, fleet, id, selected, now_ms).and_then(|(clicked, _)| clicked)
}

#[cfg(test)]
fn master_card_left_text_width(left_width: f32) -> f32 {
    (left_width - 8.0).max(0.0)
}

fn master_card_with_response(
    ui: &mut Ui,
    fleet: &Fleet,
    id: &str,
    selected: bool,
    now_ms: u64,
) -> Option<(Option<String>, egui::Response)> {
    let agent = fleet.agents.get(id)?;
    let state: crate::theme::AgentStateLike = agent.state.into();
    let color = theme::state::of(state);
    let bg = if selected {
        theme::ui::PANEL2
    } else {
        color.gamma_multiply(match state {
            crate::theme::AgentStateLike::Blocked => 0.06,
            crate::theme::AgentStateLike::Done => 0.05,
            crate::theme::AgentStateLike::Working => 0.05,
            _ => 0.0,
        })
    };
    let state_time = format!(
        "{} · {}",
        state.label(),
        crate::model::relative_age(agent.ts, now_ms)
    );
    let width = ui.available_width().max(0.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, MASTER_ROW_HEIGHT), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, bg);
    if selected {
        painter.rect_stroke(
            rect,
            CornerRadius::ZERO,
            Stroke::new(1.0, color),
            egui::StrokeKind::Inside,
        );
    }
    if !matches!(
        state,
        crate::theme::AgentStateLike::Idle | crate::theme::AgentStateLike::Unknown
    ) {
        painter.rect_filled(
            egui::Rect::from_min_max(
                rect.left_top(),
                egui::pos2(rect.left() + 3.0, rect.bottom()),
            ),
            CornerRadius::ZERO,
            color,
        );
    }

    let state_width = MASTER_STATE_WIDTH;
    let left_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 12.0, rect.top()),
        egui::pos2(
            (rect.right() - state_width - 8.0).max(rect.left()),
            rect.bottom(),
        ),
    );
    let right_rect = egui::Rect::from_min_max(
        egui::pos2((rect.right() - state_width).max(rect.left()), rect.top()),
        egui::pos2(
            (rect.right() - MASTER_STATE_RIGHT_INSET).max(rect.left()),
            rect.bottom(),
        ),
    );
    let mut left_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(left_rect)
            .id(egui::Id::new(("corral-ui-master-card-left", id)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let (dot_rect, _) = left_ui.allocate_exact_size(egui::vec2(11.0, 11.0), Sense::hover());
    match state {
        crate::theme::AgentStateLike::Working => {
            left_ui.painter().circle_stroke(
                dot_rect.center(),
                5.0,
                Stroke::new(1.5, state::WORKING),
            );
        }
        crate::theme::AgentStateLike::Idle | crate::theme::AgentStateLike::Unknown => {
            left_ui
                .painter()
                .circle_filled(dot_rect.center(), 5.0, state::IDLE);
        }
        _ => {
            left_ui
                .painter()
                .circle_filled(dot_rect.center(), 5.0, color);
        }
    }
    left_ui.add_space(7.0);
    left_ui.add(
        egui::Label::new(
            RichText::new(agent.row_label())
                .size(13.0)
                .strong()
                .color(theme::ui::INK),
        )
        .truncate(),
    );
    tool_pill(&mut left_ui, &agent.tool);

    let mut right_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(right_rect)
            .id(egui::Id::new(("corral-ui-master-card-right", id)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    right_ui.add(
        egui::Label::new(
            RichText::new(state_time)
                .monospace()
                .size(10.0)
                .strong()
                .color(color),
        )
        .truncate(),
    );
    let clicked = response.clicked().then(|| agent.agent_id.clone());
    Some((clicked, response))
}

fn tool_pill(ui: &mut Ui, tool: &str) -> egui::Response {
    let font = FontId::monospace(9.0);
    let galley = ui
        .fonts_mut(|fonts| fonts.layout_no_wrap(tool.to_string(), font.clone(), theme::ui::MUTED));
    let size = egui::vec2(galley.size().x + 10.0, TOOL_PILL_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(3), theme::ui::PANEL3);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0, theme::ui::LINE),
        egui::StrokeKind::Outside,
    );
    ui.painter().galley(
        rect.min + egui::vec2(5.0, (TOOL_PILL_HEIGHT - galley.size().y) * 0.5),
        galley,
        theme::ui::MUTED,
    );
    response
}

fn right_pane(
    ui: &mut Ui,
    fleet: &Fleet,
    selected: Option<&str>,
    _view: &mut BoardView,
    _flat: bool,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) {
    sync_kill_confirmation(ui.ctx(), selected);
    let selected_agent = selected.and_then(|id| fleet.agents.get(id));
    let interrupt_state = if actions.read_only {
        DriveControlState::ReadOnly
    } else {
        selected_agent
            .map(|agent| {
                drive_control_state(&agent.capabilities, "interrupt", allowed("interrupt"))
            })
            .unwrap_or(DriveControlState::NotImplemented)
    };
    let kill_state = if actions.read_only {
        DriveControlState::ReadOnly
    } else {
        selected_agent
            .map(|agent| drive_control_state(&agent.capabilities, "kill", allowed("kill")))
            .unwrap_or(DriveControlState::NotImplemented)
    };
    let kill_pending =
        kill_state == DriveControlState::Ready && is_kill_confirmation_pending(ui.ctx(), selected);

    // Cards keeps the prototype's four primary controls in one stable inline
    // row. A pending confirmation is deliberately rendered below this row,
    // outside the original Kill trigger rect, so a double-click cannot turn
    // the second release into an immediate destructive action.
    ui.horizontal(|ui| {
        action_button(ui, "Cards", true, false);
        if let Some(button) =
            gated_action_button(ui, "Interrupt", "interrupt", false, interrupt_state)
            && button.clicked()
            && let Some(agent) = selected_agent
        {
            (actions.drive)(DriveIntent::interrupt(&agent.agent_id, fleet.rev));
        }

        if kill_state != DriveControlState::Ready {
            clear_kill_confirmation(ui.ctx());
            gated_action_button(ui, "Kill", "kill", false, kill_state);
        } else if kill_pending {
            disabled_action_button(ui, "Kill", true);
        } else if action_button(ui, "Kill", false, true).clicked() {
            set_kill_confirmation(ui.ctx(), selected, true);
        }
    });
    if kill_pending {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Confirm destructive action")
                    .small()
                    .color(theme::ui::WARN),
            );
            if action_button(ui, "Confirm kill", false, true).clicked()
                && let Some(agent) = selected_agent
            {
                (actions.drive)(DriveIntent::kill(&agent.agent_id, fleet.rev));
                clear_kill_confirmation(ui.ctx());
            }
            if action_button(ui, "Cancel", false, false).clicked() {
                clear_kill_confirmation(ui.ctx());
            }
        });
    }
    ui.add_space(2.0);
    ui.label(
        RichText::new("Recent output")
            .strong()
            .size(12.0)
            .color(theme::ui::INK),
    );
    recent_output_surface(ui, fleet, selected, allowed, actions);
}

fn action_button(ui: &mut Ui, label: &str, active: bool, danger: bool) -> egui::Response {
    let text = if danger {
        theme::state::BLOCKED
    } else if active {
        theme::ui::INK
    } else {
        theme::ui::MUTED
    };
    let fill = theme::ui::PANEL2;
    let stroke = if active {
        theme::ui::ACCENT
    } else {
        theme::ui::LINE
    };
    ui.add(
        egui::Button::new(RichText::new(label).size(11.0).strong().color(text))
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(CornerRadius::same(8))
            .min_size(egui::vec2(0.0, 28.0)),
    )
}

/// Keep a pending destructive trigger visually in place without leaving its
/// original hit target active. Confirmation is rendered in a separate row.
fn disabled_action_button(ui: &mut Ui, label: &str, danger: bool) -> egui::Response {
    let text = if danger {
        theme::state::BLOCKED
    } else {
        theme::ui::MUTED
    };
    ui.add_enabled(
        false,
        egui::Button::new(RichText::new(label).size(11.0).strong().color(text))
            .fill(theme::ui::PANEL2)
            .stroke(Stroke::new(1.0, theme::ui::LINE))
            .corner_radius(CornerRadius::same(8))
            .min_size(egui::vec2(0.0, 28.0)),
    )
}

fn gated_action_button(
    ui: &mut Ui,
    label: &str,
    capability: &str,
    danger: bool,
    state: DriveControlState,
) -> Option<egui::Response> {
    match state {
        DriveControlState::Ready => Some(action_button(ui, label, false, danger)),
        _ => {
            disabled_drive_button(ui, label, capability, state);
            None
        }
    }
}

fn is_kill_confirmation_pending(ctx: &egui::Context, selected: Option<&str>) -> bool {
    let Some(selected) = selected else {
        return false;
    };
    let now = ctx.input(|input| input.time);
    ctx.memory(|memory| {
        let owner = memory.data.get_temp::<String>(egui::Id::new(KILL_CONFIRM));
        let started = memory
            .data
            .get_temp::<f64>(egui::Id::new(KILL_CONFIRM_STARTED));
        owner.as_deref() == Some(selected)
            && started.is_some_and(|started| now - started <= KILL_CONFIRM_TIMEOUT_SECONDS)
    })
}

fn set_kill_confirmation(ctx: &egui::Context, selected: Option<&str>, pending: bool) {
    if let Some(agent_id) = selected {
        if pending {
            let now = ctx.input(|input| input.time);
            ctx.memory_mut(|memory| {
                memory
                    .data
                    .insert_temp::<String>(egui::Id::new(KILL_CONFIRM), agent_id.to_string());
                memory
                    .data
                    .insert_temp::<f64>(egui::Id::new(KILL_CONFIRM_STARTED), now);
            });
        } else {
            clear_kill_confirmation(ctx);
        }
    }
}

fn clear_kill_confirmation(ctx: &egui::Context) {
    ctx.memory_mut(|memory| {
        memory.data.remove::<String>(egui::Id::new(KILL_CONFIRM));
        memory
            .data
            .remove::<f64>(egui::Id::new(KILL_CONFIRM_STARTED));
    });
}

fn sync_kill_confirmation(ctx: &egui::Context, selected: Option<&str>) {
    let now = ctx.input(|input| input.time);
    let stale = ctx.memory(|memory| {
        let owner = memory.data.get_temp::<String>(egui::Id::new(KILL_CONFIRM));
        let started = memory
            .data
            .get_temp::<f64>(egui::Id::new(KILL_CONFIRM_STARTED));
        owner.as_deref() != selected
            || started.is_some_and(|started| now - started > KILL_CONFIRM_TIMEOUT_SECONDS)
    });
    if stale {
        clear_kill_confirmation(ctx);
    }
}

/// Native font bundles do not consistently contain the bullet glyph used by
/// the HTML reference, so paint the tiny live mark and keep the label textual.
fn live_indicator(ui: &mut Ui, live: bool) {
    let color = if live {
        theme::ui::ACCENT
    } else {
        theme::ui::MUTED
    };
    let label = if live { LIVE_LABEL } else { PAUSED_LABEL };
    ui.horizontal(|ui| {
        painted_dot(ui, color, 3.0);
        ui.label(RichText::new(label).small().strong().color(color));
    });
}

fn recent_should_show_live(state: Option<&DriveState>, has_visible_output: bool) -> bool {
    has_visible_output
        && matches!(
            state,
            Some(DriveState::Sending { .. } | DriveState::Ok { .. })
        )
}

fn painted_dot(ui: &mut Ui, color: Color32, radius: f32) {
    let diameter = radius * 2.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(diameter + 4.0, 12.0), Sense::hover());
    ui.painter().circle_filled(
        egui::pos2(rect.left() + 2.0 + radius, rect.center().y),
        radius,
        color,
    );
}

fn recent_output_surface(
    ui: &mut Ui,
    fleet: &Fleet,
    selected: Option<&str>,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) {
    let stick_to_bottom = ui.ctx().memory(|memory| {
        memory
            .data
            .get_temp::<bool>(egui::Id::new("corral-ui-stick-to-bottom"))
            .unwrap_or(true)
    });
    let Some(id) = selected else {
        live_indicator(ui, false);
        return;
    };
    let Some(agent) = fleet.agents.get(id) else {
        return;
    };
    let read_tail_state = latest_read_tail_state(fleet, id);
    let has_visible_output = fleet
        .tails
        .get(id)
        .map(|lines| !recent_visible_indices(lines.iter().map(String::as_str)).is_empty())
        .unwrap_or(false);
    let show_live = recent_should_show_live(read_tail_state, has_visible_output);
    let read_tail_control = if actions.read_only {
        DriveControlState::ReadOnly
    } else {
        drive_control_state(&agent.capabilities, "read_tail", allowed("read_tail"))
    };
    let prompt_control = if actions.read_only {
        DriveControlState::ReadOnly
    } else {
        drive_control_state(&agent.capabilities, "prompt", allowed("prompt"))
    };

    let mut metadata_texts: Vec<&str> = Vec::new();
    if let Some(lines) = fleet.tails.get(id) {
        metadata_texts.extend(lines.iter().map(String::as_str));
    }
    let metadata = recent_metadata_from_texts(&metadata_texts);

    egui::Frame::NONE
        .fill(theme::ui::PANEL2)
        .stroke(Stroke::new(1.0, theme::ui::LINE))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                live_indicator(ui, show_live);
                recent_metadata_chip(
                    ui,
                    metadata.model.as_deref().unwrap_or(&agent.tool),
                    theme::ui::ACCENT,
                );
                if let Some(effort) = metadata.effort.as_deref() {
                    recent_metadata_chip(ui, effort, theme::ui::INK);
                }
                if let Some(worktree) = metadata
                    .worktree
                    .as_deref()
                    .or(agent.workspace.worktree_path.as_deref())
                {
                    ui.add(
                        egui::Label::new(
                            RichText::new(worktree)
                                .monospace()
                                .small()
                                .color(theme::ui::MUTED),
                        )
                        .truncate(),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "stick-to-bottom: {}",
                            if stick_to_bottom { "on" } else { "off" }
                        ))
                        .small()
                        .color(theme::ui::MUTED),
                    );
                });
            });

            // This is deliberately outside the scroll area. It remains the
            // first history affordance while the tail reflows below it.
            let load_clicked = ui
                .horizontal(|ui| {
                    ui.add_space(2.0);
                    painted_dot(ui, theme::ui::MUTED, 2.0);
                    ui.label(
                        RichText::new(EARLIER_OUTPUT_LABEL)
                            .small()
                            .color(theme::ui::MUTED),
                    );
                    if read_tail_control == DriveControlState::Ready {
                        ui.add(
                            egui::Button::new(
                                RichText::new(LOAD_EARLIER_LABEL)
                                    .small()
                                    .strong()
                                    .color(theme::ui::ACCENT),
                            )
                            .frame(false),
                        )
                        .clicked()
                    } else {
                        disabled_drive_button(
                            ui,
                            LOAD_EARLIER_LABEL,
                            "read_tail",
                            read_tail_control,
                        );
                        false
                    }
                })
                .inner;
            if load_clicked {
                (actions.drive)(DriveIntent::read_tail(&agent.agent_id, fleet.rev));
            }

            ui.add_space(2.0);
            let available_height = ui.available_height();
            let max_height = if available_height.is_finite() {
                (available_height - 86.0).max(120.0)
            } else {
                260.0
            };
            ScrollArea::vertical()
                .id_salt(("corral-ui-recent-output", id))
                .auto_shrink([false, false])
                .stick_to_bottom(stick_to_bottom)
                .max_height(max_height)
                .show(ui, |ui| {
                    if let Some(lines) = fleet.tails.get(id) {
                        let visible_indices =
                            recent_visible_indices(lines.iter().map(String::as_str));
                        if visible_indices.is_empty() {
                            ui.label(
                                RichText::new("No readable recent output.")
                                    .small()
                                    .color(theme::ui::MUTED),
                            );
                        } else {
                            for position in
                                recent_output_indices(visible_indices.len(), stick_to_bottom)
                            {
                                let source_index = visible_indices[position];
                                recent_tail_entry(ui, &lines[source_index], source_index);
                            }
                        }
                    } else if let Some(state) = read_tail_state {
                        let feedback = match state {
                            DriveState::Sending { .. } => "Fetching recent output…".to_string(),
                            DriveState::Ok { .. } => {
                                "read_tail returned no output — use the history control to retry"
                                    .to_string()
                            }
                            DriveState::Failed { .. } => {
                                format!("Recent output unavailable: {}", drive_state_text(state))
                            }
                        };
                        ui.label(
                            RichText::new(feedback)
                                .small()
                                .color(drive_state_color(state)),
                        );
                    } else {
                        ui.label(
                            RichText::new("No recent output fetched yet — use the history control")
                                .small()
                                .color(theme::ui::INK),
                        );
                    }
                });

            recent_prompt_composer(ui, agent, fleet.rev, prompt_control, actions.drive);
        });
}

/// Recent output lines are newest-first in the tail result. Stick-to-bottom
/// paints the visible window oldest-to-newest; when disabled the newest
/// entry remains first so the operator can inspect the latest output
/// without automatic bottom bias.
pub(crate) fn recent_output_indices(len: usize, stick_to_bottom: bool) -> Vec<usize> {
    let count = len;
    if stick_to_bottom {
        (0..count).rev().collect()
    } else {
        (0..count).collect()
    }
}

fn recent_visible_indices<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<usize> {
    texts
        .into_iter()
        .enumerate()
        .filter_map(|(index, text)| recent_visible_text(text).map(|_| index))
        .collect()
}

fn latest_read_tail_state<'a>(fleet: &'a Fleet, agent_id: &str) -> Option<&'a DriveState> {
    fleet.recent_drives.get(agent_id)?.iter().find(|state| {
        matches!(
            state,
            DriveState::Sending { capability, .. }
                | DriveState::Ok { capability, .. }
                | DriveState::Failed { capability, .. }
                if capability == "read_tail"
        )
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RecentMetadata {
    model: Option<String>,
    effort: Option<String>,
    worktree: Option<String>,
}

fn recent_metadata_chip(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::NONE
        .fill(theme::ui::PANEL3)
        .stroke(Stroke::new(1.0, theme::ui::LINE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(7, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).monospace().small().color(color));
        });
}

fn recent_metadata_from_texts(texts: &[&str]) -> RecentMetadata {
    let mut found = RecentMetadata::default();
    for text in texts {
        for line in text.split('\n') {
            let Some(parsed) = parse_recent_metadata(line) else {
                continue;
            };
            if found.model.is_none() {
                found.model = parsed.model;
            }
            if found.effort.is_none() {
                found.effort = parsed.effort;
            }
            if found.worktree.is_none() {
                found.worktree = parsed.worktree;
            }
        }
    }
    found
}

fn parse_recent_metadata(line: &str) -> Option<RecentMetadata> {
    let value = line.trim();
    if value.is_empty() {
        return None;
    }

    let mut pieces: Vec<&str> = value.split('·').map(str::trim).collect();
    let path = pieces.pop()?;
    if !recent_worktree_path(path) {
        return None;
    }
    let left = pieces.join(" · ");
    let mut words: Vec<&str> = left.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let effort = words
        .last()
        .and_then(|word| recent_effort(word))
        .map(str::to_string);
    if effort.is_some() {
        words.pop();
    }
    let model = words.join(" ");
    if model.is_empty() || !recent_model_name(&model) {
        return None;
    }
    Some(RecentMetadata {
        model: Some(model),
        effort,
        worktree: Some(path.to_string()),
    })
}

fn recent_metadata_line(line: &str) -> bool {
    parse_recent_metadata(line).is_some()
}

fn recent_effort(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "minimal" => Some("minimal"),
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "max" => Some("max"),
        _ => None,
    }
}

fn recent_worktree_path(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.starts_with('~')
        || value.starts_with('/')
        || lower.contains("worktree")
        || value.contains('/')
}

fn recent_model_name(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("gpt")
        || lower.contains("claude")
        || lower.contains("gemini")
        || lower.contains("sonnet")
        || lower.contains("opus")
        || lower.contains("luna")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
}

fn recent_visible_text(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let first_content = lines.iter().position(|line| !line.trim().is_empty());
    let last_content = lines.iter().rposition(|line| !line.trim().is_empty());
    let strip_metadata = matches!(
        (first_content, last_content),
        (Some(first), Some(last))
            if first != last && recent_metadata_line(lines[last])
    );
    let lines: Vec<&str> = lines
        .into_iter()
        .enumerate()
        .filter_map(|(index, line)| {
            if strip_metadata && Some(index) == last_content {
                None
            } else {
                Some(line)
            }
        })
        .collect();
    if lines.is_empty() || lines.iter().all(|line| line.trim().is_empty()) {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn recent_prompt_composer(
    ui: &mut Ui,
    agent: &Agent,
    rev: Option<u64>,
    control: DriveControlState,
    drive: &mut dyn FnMut(DriveIntent),
) {
    let id = eframe::egui::Id::new(("corral-ui-recent-prompt", &agent.agent_id));
    let mut text: String = ui
        .ctx()
        .memory(|memory| memory.data.get_temp::<String>(id).unwrap_or_default());
    let enabled = control == DriveControlState::Ready;
    let mut submitted = false;
    egui::Frame::NONE
        .fill(theme::ui::BG)
        .stroke(Stroke::new(1.0, theme::ui::LINE))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let input_width = (ui.available_width() - 72.0).max(80.0);
                let response = ui.add_enabled(
                    enabled,
                    TextEdit::singleline(&mut text)
                        .id(id)
                        .hint_text("Reply to agent…")
                        .desired_width(input_width),
                );
                let enter = response.has_focus()
                    && ui.input(|input| input.key_pressed(eframe::egui::Key::Enter));
                let send = ui.add_enabled(
                    enabled && !text.trim().is_empty(),
                    egui::Button::new(RichText::new("Send").strong().color(theme::ui::SEND_INK))
                        .fill(theme::ui::ACCENT)
                        .corner_radius(CornerRadius::same(8))
                        .min_size(egui::vec2(56.0, 32.0)),
                );
                submitted = enter || send.clicked();
            });
            if control != DriveControlState::Ready
                && let Some(reason) = drive_disabled_reason("prompt", control)
            {
                ui.label(RichText::new(reason).small().color(theme::ui::MUTED));
            }
        });
    if submitted && enabled {
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() {
            drive(DriveIntent::prompt(&agent.agent_id, trimmed, rev));
        }
        ui.ctx()
            .memory_mut(|memory| memory.data.remove::<String>(id));
    } else {
        ui.ctx()
            .memory_mut(|memory| memory.data.insert_temp::<String>(id, text));
    }
}

fn recent_message_lines(ui: &mut Ui, text: &str, font: FontId) {
    for line in text.split('\n') {
        ui.add(
            egui::Label::new(RichText::new(line).font(font.clone()).color(theme::ui::INK)).wrap(),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecentBlockKind {
    User,
    Tool,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RecentBlockStyle {
    fill: Color32,
    inset: f32,
    monospace: bool,
}

/// Recover user/tool/assistant semantics from the terminal-shaped read_tail
/// fallback.
fn classify_tail_line(line: &str) -> RecentBlockKind {
    let trimmed = line.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    let typed_prompt = trimmed.strip_prefix('›').map(str::trim).filter(|payload| {
        !payload.is_empty() && !payload.eq_ignore_ascii_case("ask codex to do anything")
    });
    let typed_role_line = ["user:", "you:", "prompt:"]
        .into_iter()
        .find_map(|prefix| lower.strip_prefix(prefix).map(str::trim))
        .filter(|payload| !payload.is_empty());
    if typed_prompt.is_some() || typed_role_line.is_some() {
        RecentBlockKind::User
    } else if trimmed.starts_with('•')
        || trimmed.starts_with('●')
        || trimmed.starts_with('⏺')
        || lower.starts_with("tool:")
        || lower.starts_with("command:")
        || lower.starts_with("$ ")
    {
        RecentBlockKind::Tool
    } else {
        RecentBlockKind::Agent
    }
}

fn recent_block_style(kind: RecentBlockKind) -> RecentBlockStyle {
    match kind {
        RecentBlockKind::User => RecentBlockStyle {
            fill: theme::ui::USER_TINT,
            inset: USER_BLOCK_INSET,
            monospace: false,
        },
        RecentBlockKind::Tool => RecentBlockStyle {
            fill: theme::ui::PANEL3,
            inset: 0.0,
            monospace: true,
        },
        RecentBlockKind::Agent => RecentBlockStyle {
            fill: Color32::TRANSPARENT,
            inset: 0.0,
            monospace: false,
        },
    }
}

fn recent_tail_entry(ui: &mut Ui, line: &str, position: usize) {
    let Some(text) = recent_visible_text(line) else {
        return;
    };
    recent_chat_block(ui, classify_tail_line(line), &text, position);
}

fn recent_tool_summary(text: &str) -> String {
    let first = text
        .split('\n')
        .find(|line| !line.trim().is_empty())
        .unwrap_or(text)
        .trim();
    let command = first.strip_prefix("$ ").unwrap_or(first).trim();
    command.chars().take(48).collect()
}

fn recent_tool_disclosure_id(position: usize) -> egui::Id {
    egui::Id::new(("corral-ui-tool-block", position))
}

fn recent_chat_block(ui: &mut Ui, kind: RecentBlockKind, text: &str, position: usize) {
    let block_width = ui.available_width();
    let style = recent_block_style(kind);
    let frame = egui::Frame::NONE
        .fill(style.fill)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 7));

    if style.inset > 0.0 {
        // In right-to-left layout the frame is anchored to the right edge;
        // limiting its width leaves the prototype's 24px left inset.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            let width = (ui.available_width() - style.inset).max(0.0);
            ui.set_width(width);
            frame.show(ui, |ui| {
                ui.label(
                    RichText::new("you")
                        .small()
                        .strong()
                        .color(theme::ui::USER_BLUE),
                );
                recent_message_lines(ui, text, FontId::proportional(12.0));
            });
        });
    } else {
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            if style.monospace {
                if recent_is_code_or_diff(text) {
                    CollapsingHeader::new(
                        RichText::new(format!("tool  {}", recent_tool_summary(text)))
                            .small()
                            .strong()
                            .color(theme::ui::ACCENT),
                    )
                    .id_salt(recent_tool_disclosure_id(position))
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.set_max_width((block_width - 40.0).max(40.0));
                        egui::Frame::NONE
                            .fill(theme::ui::BG)
                            .stroke(Stroke::new(1.0, recent_code_line_color()))
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(egui::Margin::symmetric(8, 6))
                            .show(ui, |ui| {
                                for (number, line) in text.split('\n').enumerate() {
                                    recent_code_line(ui, line, number + 1);
                                }
                            });
                    });
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("tool")
                                .small()
                                .strong()
                                .color(theme::ui::MUTED),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(text)
                                    .monospace()
                                    .small()
                                    .color(theme::ui::MUTED),
                            )
                            .wrap(),
                        );
                    });
                }
            } else {
                ui.label(
                    RichText::new("assistant")
                        .small()
                        .strong()
                        .color(theme::ui::INK),
                );
                recent_message_lines(ui, text, FontId::proportional(12.0));
            }
        });
    }
    ui.add_space(4.0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecentCodeSegmentKind {
    Plain,
    Keyword,
    String,
    Addition,
    Deletion,
    Comment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecentCodeSegment {
    text: String,
    kind: RecentCodeSegmentKind,
}

fn recent_code_line_color() -> Color32 {
    Color32::from_rgb(0x21, 0x26, 0x2d)
}

fn recent_code_color(kind: RecentCodeSegmentKind) -> Color32 {
    match kind {
        RecentCodeSegmentKind::Plain => theme::ui::INK,
        RecentCodeSegmentKind::Keyword => Color32::from_rgb(0xff, 0x7b, 0x72),
        RecentCodeSegmentKind::String => Color32::from_rgb(0xa5, 0xd6, 0xff),
        RecentCodeSegmentKind::Addition => theme::ci::SUCCESS,
        RecentCodeSegmentKind::Deletion => theme::state::BLOCKED,
        RecentCodeSegmentKind::Comment => theme::ui::MUTED,
    }
}

fn recent_is_code_or_diff(text: &str) -> bool {
    let mut has_git_header = false;
    let mut has_file_header = false;
    let mut has_hunk = false;
    let mut has_change = false;
    let mut has_fence = false;
    let lines: Vec<&str> = text.split('\n').collect();
    for line in &lines {
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        has_git_header =
            has_git_header || lower.starts_with("git diff") || lower.starts_with("diff --git");
        has_file_header = has_file_header || lower.starts_with("+++ ") || lower.starts_with("--- ");
        has_hunk = has_hunk || lower.starts_with("@@");
        has_change = has_change
            || (trimmed.starts_with('+') && !trimmed.starts_with("+++"))
            || (trimmed.starts_with('-') && !trimmed.starts_with("---"));
        has_fence = has_fence || trimmed.starts_with("```");
    }
    let has_diff_evidence = (has_git_header && (has_hunk || (has_file_header && has_change)))
        || (has_hunk && has_change)
        || (has_file_header && has_hunk);
    has_fence || has_diff_evidence
}

fn append_recent_segment(
    segments: &mut Vec<RecentCodeSegment>,
    text: &str,
    kind: RecentCodeSegmentKind,
) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = segments.last_mut()
        && last.kind == kind
    {
        last.text.push_str(text);
        return;
    }
    segments.push(RecentCodeSegment {
        text: text.to_string(),
        kind,
    });
}

fn recent_highlight(line: &str) -> Vec<RecentCodeSegment> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
        return vec![RecentCodeSegment {
            text: line.to_string(),
            kind: RecentCodeSegmentKind::Addition,
        }];
    }
    if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        return vec![RecentCodeSegment {
            text: line.to_string(),
            kind: RecentCodeSegmentKind::Deletion,
        }];
    }
    if trimmed.starts_with("@@") {
        return vec![RecentCodeSegment {
            text: line.to_string(),
            kind: RecentCodeSegmentKind::Keyword,
        }];
    }

    let chars: Vec<char> = line.chars().collect();
    let first_non_whitespace = chars
        .iter()
        .position(|character| !character.is_whitespace());
    let mut segments = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if character == '"' || character == '\'' {
            let quote = character;
            let mut end = index + 1;
            let mut escaped = false;
            while end < chars.len() {
                let candidate = chars[end];
                if escaped {
                    escaped = false;
                } else if candidate == '\\' {
                    escaped = true;
                } else if candidate == quote {
                    end += 1;
                    break;
                }
                end += 1;
            }
            let token: String = chars[index..end].iter().collect();
            append_recent_segment(&mut segments, &token, RecentCodeSegmentKind::String);
            index = end;
        } else if (character == '#' && first_non_whitespace == Some(index))
            || (character == '/' && index + 1 < chars.len() && chars[index + 1] == '/')
        {
            let token: String = chars[index..].iter().collect();
            append_recent_segment(&mut segments, &token, RecentCodeSegmentKind::Comment);
            break;
        } else if character.is_alphabetic() || character == '_' {
            let mut end = index + 1;
            while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            let word: String = chars[index..end].iter().collect();
            let kind = if matches!(
                word.as_str(),
                "actor"
                    | "class"
                    | "const"
                    | "else"
                    | "enum"
                    | "fn"
                    | "for"
                    | "func"
                    | "if"
                    | "impl"
                    | "import"
                    | "in"
                    | "let"
                    | "match"
                    | "mut"
                    | "pub"
                    | "return"
                    | "struct"
                    | "switch"
                    | "var"
                    | "where"
                    | "while"
            ) {
                RecentCodeSegmentKind::Keyword
            } else {
                RecentCodeSegmentKind::Plain
            };
            append_recent_segment(&mut segments, &word, kind);
            index = end;
        } else {
            let token = character.to_string();
            append_recent_segment(&mut segments, &token, RecentCodeSegmentKind::Plain);
            index += 1;
        }
    }
    segments
}

fn recent_code_line(ui: &mut Ui, line: &str, number: usize) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("{number:>3} "))
                .monospace()
                .small()
                .color(theme::ui::MUTED),
        );
        for segment in recent_highlight(line) {
            ui.add(
                egui::Label::new(
                    RichText::new(segment.text)
                        .monospace()
                        .small()
                        .color(recent_code_color(segment.kind)),
                )
                .wrap(),
            );
        }
    });
}

#[allow(dead_code)]
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
        .id_salt("corral-ui-table")
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
        // The row detail reflows into a bounded column instead of consuming
        // the table's full/right-pane width.
        detail(
            ui,
            agent,
            fleet,
            allowed,
            actions,
            DetailOptions {
                show_topology: false,
                show_recent_output: true,
            },
        );
    }
    ui.separator();
}

fn header(ui: &mut Ui) {
    let _ = header_cells(ui);
}

fn header_cells(ui: &mut Ui) -> [egui::Response; 10] {
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
                        agent_row_cells(ui, agent, fleet);
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

fn agent_row_cells(ui: &mut Ui, agent: &Agent, fleet: &Fleet) -> [egui::Response; 10] {
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
            diff_cell(ui, agent, fleet),
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

/// #232: lazy diffstat cell. Filled only from the per-agent diff cache
/// (after the first read_diff); the board never prefetches diffs
/// fleet-wide — one agent at a time, explicit tap.
fn diff_cell(ui: &mut Ui, agent: &Agent, fleet: &Fleet) -> egui::Response {
    fixed_cell(ui, COL_DIFF, |ui| {
        let text = match fleet.diffs.get(&agent.agent_id) {
            Some(diff) if diff.stats.adds > 0 || diff.stats.dels > 0 => {
                format!("+{}/−{}", diff.stats.adds, diff.stats.dels)
            }
            _ => String::new(),
        };
        let color = if text.is_empty() {
            theme::ui::TEXT_MUTED
        } else {
            theme::ui::ACCENT_DIM
        };
        ui.add_sized(
            [COL_DIFF - 8.0, 18.0],
            egui::Label::new(RichText::new(text).monospace().small().color(color)),
        )
        .on_hover_text("lazy diffstat — expand the row or tap the read_diff control to load the agent worktree diff");
    })
}

/// #232: render one unified-diff stream entry with its origin-derived color.
fn diff_line(ui: &mut Ui, line: &str) {
    let color = if line.starts_with('+') {
        theme::ui::GOOD
    } else if line.starts_with('-') {
        theme::ui::BAD
    } else if line.starts_with('@') || line.starts_with("diff --git") || line.starts_with("index ")
    {
        theme::ui::ACCENT_DIM
    } else {
        theme::ui::TEXT_MUTED
    };
    ui.add(egui::Label::new(RichText::new(line).monospace().small().color(color)).wrap());
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
    /// #215 read-only web build: the control is deliberately off — the
    /// board never issues any signed drive from the browser.
    ReadOnly,
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
        DriveControlState::ReadOnly => Some(format!(
            "read-only web build — {capability} runs in the desktop client, never the browser"
        )),
    }
}

fn disabled_drive_button(ui: &mut Ui, label: &str, capability: &str, state: DriveControlState) {
    if let Some(reason) = drive_disabled_reason(capability, state) {
        crate::ui::disabled_button_with_reason(ui, label, &reason);
    }
}

fn drive_controls(
    ui: &mut Ui,
    agent: &Agent,
    allowed: &dyn Fn(&str) -> bool,
    fleet: &Fleet,
    drive: &mut dyn FnMut(DriveIntent),
    read_only: bool,
) {
    if read_only {
        disabled_drive_button(
            ui,
            "read-only (web)",
            "read_only",
            DriveControlState::ReadOnly,
        );
        return;
    }
    let rev = fleet.rev;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.spacing_mut().item_spacing.y = 2.0;

        for cap in crate::drive::CAPABILITIES_ORDER {
            let state = drive_control_state(&agent.capabilities, cap, allowed(cap));
            match cap {
                "prompt" => match state {
                    DriveControlState::Ready => prompt_widget(ui, agent, rev, drive),
                    _ => disabled_drive_button(ui, cap, cap, state),
                },
                "approve" => {
                    if agent.waiting_on.is_none() {
                        continue;
                    }
                    match state {
                        DriveControlState::Ready => approve_choices(ui, agent, rev, drive),
                        _ => disabled_drive_button(ui, cap, cap, state),
                    }
                }
                _ => match state {
                    DriveControlState::Ready => {
                        if ui.small_button(cap).clicked() {
                            let intent = match cap {
                                "interrupt" => DriveIntent::interrupt(&agent.agent_id, rev),
                                "read_tail" => DriveIntent::read_tail(&agent.agent_id, rev),
                                "read_diff" => {
                                    DriveIntent::read_diff(&agent.agent_id, 128, 0, 200, rev)
                                }
                                "kill" => DriveIntent::kill(&agent.agent_id, rev),
                                _ => DriveIntent::attach(&agent.agent_id, rev),
                            };
                            drive(intent);
                        }
                    }
                    _ => disabled_drive_button(ui, cap, cap, state),
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

#[derive(Debug, Clone, Copy)]
struct DetailOptions {
    show_topology: bool,
    show_recent_output: bool,
}

fn detail(
    ui: &mut Ui,
    agent: &Agent,
    fleet: &Fleet,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
    options: DetailOptions,
) {
    egui::Frame::group(ui.style())
        .fill(theme::ui::PANEL)
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
                    RichText::new(crate::model::relative_age(agent.ts, now_millis()))
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            });
            if options.show_topology {
                ui.label(
                    RichText::new("repo / branch / dirty / a-b / pr / ci")
                        .small()
                        .monospace()
                        .color(theme::ui::TEXT_MUTED),
                );
                ui.horizontal_wrapped(|ui| {
                    let ws = &agent.workspace;
                    topology_cell(
                        ui,
                        COL_REPO,
                        ws.repo.clone().unwrap_or_else(|| "—".into()),
                        theme::ui::TEXT_MUTED,
                    );
                    branch_cell(ui, agent);
                    topology_cell(
                        ui,
                        COL_DIRTY,
                        if ws.dirty { "●".into() } else { "".into() },
                        theme::ui::DIRTY,
                    );
                    topology_cell(
                        ui,
                        COL_AB,
                        if ws.ahead == 0 && ws.behind == 0 {
                            "".into()
                        } else {
                            format!("+{}/−{}", ws.ahead, ws.behind)
                        },
                        if ws.ahead > 0 {
                            theme::ui::WARN
                        } else {
                            theme::ui::TEXT_MUTED
                        },
                    );
                    topology_cell(
                        ui,
                        COL_PR,
                        ws.pr_number
                            .map(|n| format!("#{n}"))
                            .unwrap_or_else(|| "—".into()),
                        theme::ui::TEXT_MUTED,
                    );
                    ci_cell(ui, ws.ci_status);
                });
            }
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
                actions.read_only,
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
            if options.show_recent_output {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Recent output")
                        .strong()
                        .color(theme::ui::TEXT_STRONG),
                );
                if let Some(tail) = fleet.tails.get(&agent.agent_id) {
                    egui::Frame::NONE
                        .fill(theme::ui::PANEL2)
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            if tail.is_empty() {
                                ui.label(
                                    RichText::new("no recent output for this agent")
                                        .small()
                                        .color(theme::ui::TEXT_MUTED),
                                );
                            } else {
                                ScrollArea::vertical()
                                    .id_salt("corral-ui-recent-output")
                                    .max_height(200.0)
                                    .show(ui, |ui| {
                                        for (position, line) in tail.iter().enumerate() {
                                            recent_tail_entry(ui, line, position);
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
                        RichText::new(
                            "read_tail dispatched + audited; the daemon returned no result",
                        )
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
            }
            detail_diff(ui, agent, fleet, allowed, actions);
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

/// #232: the worktree-diff section of the agent detail. Shows the daemon
/// attribution header (repo · branch), the one-line diffstat, the
/// changed-files list, and the paged unified diff with a "Load next"
/// control. The first page is never prefetched — the section offers an
/// explicit "load diff" button (grant/capability-gated like read_tail).
fn detail_diff(
    ui: &mut Ui,
    agent: &Agent,
    fleet: &Fleet,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) {
    let diff_state = drive_control_state(&agent.capabilities, "read_diff", allowed("read_diff"));
    ui.add_space(8.0);
    if !fleet.diffs.contains_key(&agent.agent_id) {
        if !actions.read_only {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("Worktree diff")
                        .strong()
                        .color(theme::ui::TEXT_STRONG),
                );
                match diff_state {
                    DriveControlState::Ready => {
                        if ui.small_button("± load diff").clicked() {
                            (actions.drive)(crate::drive::DriveIntent::read_diff(
                                &agent.agent_id,
                                128,
                                0,
                                200,
                                fleet.rev,
                            ));
                        }
                    }
                    _ => disabled_drive_button(ui, "± load diff", "read_diff", diff_state),
                }
            });
        }
        return;
    }
    let Some(diff) = fleet.diffs.get(&agent.agent_id) else {
        return;
    };
    let header = match (&diff.repo, &diff.branch) {
        (Some(repo), Some(branch)) => format!("{repo} · {branch}"),
        (Some(repo), None) => repo.clone(),
        (None, Some(branch)) => branch.clone(),
        (None, None) => "worktree diff".to_string(),
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(header).strong().color(theme::ui::TEXT_STRONG));
        ui.label(
            RichText::new(format!(
                "+{}/−{} · {} files",
                diff.stats.adds, diff.stats.dels, diff.stats.files
            ))
            .monospace()
            .small()
            .color(theme::ui::TEXT_MUTED),
        );
    });
    if diff.files.is_empty() {
        ui.label(
            RichText::new("no changed files — the worktree is clean")
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        return;
    }
    egui::Frame::NONE
        .fill(theme::ui::PANEL2)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Changed-files list (left column, bounded).
                ui.vertical(|ui| {
                    ui.set_width(230.0);
                    ui.label(RichText::new("files").small().color(theme::ui::TEXT_MUTED));
                    for f in diff.files.iter().take(128) {
                        let color = if f.adds == 0 && f.dels == 0 {
                            theme::ui::TEXT_MUTED
                        } else {
                            theme::ui::ACCENT_DIM
                        };
                        ui.label(
                            RichText::new(format!("{} (+{}/−{})", f.path, f.adds, f.dels))
                                .monospace()
                                .small()
                                .color(color),
                        )
                        .on_hover_text(&f.path);
                    }
                    if diff.files_truncated {
                        ui.label(
                            RichText::new("… more files")
                                .small()
                                .color(theme::ui::TEXT_MUTED),
                        );
                    }
                });
                ui.separator();
                // Paged unified diff (right column, lazy pages).
                ScrollArea::vertical()
                    .id_salt(("corral-ui-diff", &agent.agent_id))
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for line in &diff.lines {
                            diff_line(ui, line);
                        }
                        if diff.has_more
                            && !actions.read_only
                            && diff_state == DriveControlState::Ready
                            && let Some(next) = diff.next_offset
                            && ui
                                .small_button(format!("Load next 200 lines (offset {next})"))
                                .clicked()
                        {
                            (actions.drive)(crate::drive::DriveIntent::read_diff(
                                &agent.agent_id,
                                128,
                                next,
                                200,
                                fleet.rev,
                            ));
                        }
                    });
            });
        });
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

    fn health_entry(
        name: &str,
        orch_alive: bool,
        workers: usize,
        heartbeat: Option<u64>,
        degraded: bool,
        paused: bool,
        warnings: &[&str],
    ) -> crate::model::FleetHealthEntry {
        crate::model::FleetHealthEntry {
            name: name.into(),
            gh_repo: "owner/repo".into(),
            paused,
            orch: format!("orch-{name}"),
            orch_alive,
            orch_state: orch_alive.then(|| "working".to_string()),
            workers,
            last_heartbeat: heartbeat,
            degraded,
            warnings: warnings.iter().map(|w| w.to_string()).collect(),
        }
    }

    #[test]
    fn health_pill_healthy_shows_live_orch_workers_and_heartbeat_age() {
        let entry = health_entry("corral", true, 2, Some(1_000_000), false, false, &[]);
        assert_eq!(
            health_pill_text(&entry, 1_004_000),
            "● corral  orch ✓  2w  ♥4s",
            "healthy pill carries orch alive, live worker count and a ticking heartbeat age"
        );
    }

    #[test]
    fn health_pill_degraded_warns_and_paused_renders_muted_marker() {
        let degraded = health_entry("sendmeter", false, 0, None, true, false, &["orch_missing"]);
        assert_eq!(
            health_pill_text(&degraded, 1_000),
            "⚠ sendmeter  orch ✗  0w  ♥—",
            "a missing orch reads as a warning pill, never as a stall accusation"
        );
        let paused = health_entry("plush", false, 0, None, false, true, &[]);
        assert_eq!(
            health_pill_text(&paused, 1_000),
            "⏸ plush  orch ✗  0w  ♥— paused",
            "paused fleets are parked by design and read muted"
        );
    }

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
    fn board_columns_are_the_ten_conformance_columns_within_limit() {
        assert_eq!(BOARD_COLUMNS.len(), 10);
        assert!(
            BOARD_COLUMNS.iter().all(|(label, _)| *label != "DRIVE"),
            "drive is no longer a table column"
        );
        let width: f32 = BOARD_COLUMNS.iter().map(|(_, width)| *width).sum();
        assert_eq!(width, 1104.0);
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

        fleet.agents.insert(
            "herdr:blocked".into(),
            agent_with_state("herdr:blocked", crate::model::AgentState::Blocked),
        );
        assert_eq!(
            available_state_filters(&fleet, ""),
            vec![
                StateFilter::Blocked,
                StateFilter::All,
                StateFilter::Working,
                StateFilter::Idle,
            ],
            "the production filter sequence puts Needs you before All"
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
        fleet.agents.insert(
            "herdr:unknown".into(),
            agent_with_state("herdr:unknown", crate::model::AgentState::Unknown),
        );
        let ids = ["herdr:working", "herdr:unknown", "herdr:idle"];
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
        assert_eq!(
            sections[1].agent_ids,
            vec!["herdr:unknown", "herdr:idle"],
            "Unknown folds into the Idle section"
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
            read_only: false,
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
    fn cards_path_click_selects_agent() {
        let ctx = row_test_context();
        let mut agent = agent_with_caps(&[]);
        agent.agent_id = "herdr:cards".into();
        agent.display_name = Some("card agent".into());
        agent.ts = 1_700_000_000_000;
        let now = agent.ts + 42 * 60_000;
        let mut fleet = Fleet::default();
        fleet.agents.insert(agent.agent_id.clone(), agent.clone());

        let (clicked, rect, mut output) = master_card_frame(
            &ctx,
            &fleet,
            &agent.agent_id,
            false,
            now,
            row_test_input(vec![]),
        );
        assert_eq!(clicked, None);
        let pos = rect.expect("master card rendered").center();
        assert!(
            text_rect(&output, "Working · 42m").is_some(),
            "master card meta shows State · relative age"
        );
        clear_textures(&mut output);

        let (clicked, _, mut output) = master_card_frame(
            &ctx,
            &fleet,
            &agent.agent_id,
            false,
            now,
            pointer_down_input(pos),
        );
        assert_eq!(clicked, None);
        clear_textures(&mut output);
        let (clicked, _, mut output) = master_card_frame(
            &ctx,
            &fleet,
            &agent.agent_id,
            false,
            now,
            pointer_up_input(pos),
        );
        let clicked = clicked.expect("master card click returns the agent id");
        fleet.select_agent(&clicked);
        clear_textures(&mut output);
        assert_eq!(
            fleet.selected_agent.as_deref(),
            Some("herdr:cards"),
            "clicking a Cards master card selects it"
        );
    }

    #[test]
    fn master_card_age_right_edge_is_stable_across_lengths() {
        let ctx = row_test_context();
        let now = 1_700_000_000_000_u64;
        let mut agent = agent_with_caps(&[]);
        agent.agent_id = "herdr:age-edge".into();
        agent.display_name = Some("age edge card".into());
        agent.ts = now;
        let mut fleet = Fleet::default();
        fleet.agents.insert(agent.agent_id.clone(), agent);

        let render_age_right = |now_ms: u64, expected: &str| {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(496.44, 800.0));
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let (_, card_rect, mut output) =
                master_card_frame(&ctx, &fleet, "herdr:age-edge", false, now_ms, input);
            let card_right = card_rect.expect("master card rendered").right();
            let rendered = rendered_text(&output, expected)
                .unwrap_or_else(|| panic!("{expected} did not render"));
            clear_textures(&mut output);
            (card_right, rendered.rect.right(), rendered.layout_right)
        };

        let short = render_age_right(now + 42 * 60_000, "Working · 42m");
        let long = render_age_right(now + 100 * 24 * 60 * 60_000, "Working · 100d 00h");
        let short_visual_inset = short.0 - short.1;
        let long_visual_inset = long.0 - long.1;
        // `visual_bounding_rect` is tight glyph ink: egui snaps each galley to
        // physical pixels, and final-glyph side bearings can therefore differ
        // by one pixel even when both labels share the same logical edge.
        assert!(
            (short_visual_inset - long_visual_inset).abs() <= 1.0,
            "short and long age ink edges must share the card-right inset within one rasterized pixel: short={short_visual_inset}, long={long_visual_inset}"
        );

        let short_layout_inset = short.0 - short.2;
        let long_layout_inset = long.0 - long.2;
        assert!(
            (short_layout_inset - MASTER_STATE_RIGHT_INSET).abs() <= 1.0,
            "short age layout edge must stay inside the themed right inset: inset={short_layout_inset}"
        );
        assert!(
            (long_layout_inset - MASTER_STATE_RIGHT_INSET).abs() <= 1.0,
            "long age layout edge must stay inside the themed right inset: inset={long_layout_inset}"
        );
    }

    #[test]
    fn master_card_clamps_left_label_sizes_at_narrow_width() {
        let ctx = row_test_context();
        let now = 1_700_000_000_000_u64;
        let mut agent = agent_with_caps(&[]);
        agent.agent_id = "herdr:narrow-card".into();
        agent.display_name = Some("narrow card".into());
        agent.ts = now;
        let mut fleet = Fleet::default();
        fleet.agents.insert(agent.agent_id.clone(), agent);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 800.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };

        let (_, card_rect, mut output) = master_card_frame(
            &ctx,
            &fleet,
            "herdr:narrow-card",
            false,
            now + 42 * 60_000,
            input,
        );
        assert_eq!(
            master_card_left_text_width(2.0),
            0.0,
            "the 200px card leaves a 2px left reserve, so its 8px text inset must clamp to zero"
        );
        let card_rect = card_rect.expect("narrow master card still renders");
        let rendered = rendered_text(&output, "Working · 42m")
            .expect("narrow master card keeps its age label");
        assert!(
            rendered.rect.width() >= 0.0
                && rendered.rect.left() >= card_rect.left()
                && rendered.rect.right() <= card_rect.right(),
            "narrow age geometry must stay non-negative and inside the card: age={:?}, card={card_rect:?}",
            rendered.rect
        );
        clear_textures(&mut output);
    }

    #[test]
    fn ordinary_needs_you_ages_render_unelided_at_master_widths() {
        let now = 1_700_000_000_000_u64;
        for width in [320.0, 496.44] {
            for (elapsed_ms, expected) in [
                ((23 * 60 + 59) * 60_000_u64, "Needs you · 23h 59m"),
                ((3 * 24 + 4) * 60 * 60_000_u64, "Needs you · 3d 04h"),
                (100 * 24 * 60 * 60_000_u64, "Needs you · 100d 00h"),
            ] {
                let ctx = row_test_context();
                let mut agent = agent_with_caps(&[]);
                agent.agent_id = "herdr:needs-you".into();
                agent.display_name = Some("needs you card".into());
                agent.state = crate::model::AgentState::Blocked;
                agent.ts = now;
                let mut fleet = Fleet::default();
                fleet.agents.insert(agent.agent_id.clone(), agent.clone());
                let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 800.0));
                let input = egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                };
                let (_, mut output) = master_list_frame(
                    &ctx,
                    &fleet,
                    &[agent.agent_id.as_str()],
                    true,
                    None,
                    now + elapsed_ms,
                    input,
                );
                let rendered = rendered_text(&output, expected)
                    .unwrap_or_else(|| panic!("{expected} did not render at {width}px"));
                assert!(
                    !rendered.elided,
                    "{expected} elided at the {width}px master-column width"
                );
                clear_textures(&mut output);
            }
        }
    }

    #[test]
    fn extreme_master_card_age_is_clipped_inside_bound() {
        for width in [320.0, 496.44] {
            let ctx = row_test_context();
            let mut agent = agent_with_caps(&[]);
            agent.agent_id = "herdr:extreme-age".into();
            agent.display_name = Some("extreme age card".into());
            agent.ts = 1;
            let now = u64::MAX;
            let mut fleet = Fleet::default();
            fleet.agents.insert(agent.agent_id.clone(), agent.clone());
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 800.0));
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let (_, card_rect, mut output) =
                master_card_frame(&ctx, &fleet, &agent.agent_id, false, now, input);
            let card_rect = card_rect.expect("master card rendered");
            let age = format!("Working · {}", crate::model::relative_age(agent.ts, now));
            let rendered =
                rendered_text(&output, &age).expect("extreme master card age label rendered");
            assert!(
                rendered.elided,
                "extreme age must elide inside its reserved slot at {width}px"
            );
            assert!(
                rendered.rect.width() <= CARD_AGE_WIDTH + 0.01,
                "extreme age width {} exceeds its {} bound",
                rendered.rect.width(),
                CARD_AGE_WIDTH
            );
            assert!(
                rendered.rect.left() >= card_rect.left()
                    && rendered.rect.right() <= card_rect.right(),
                "extreme age {rendered:?} overflows card {card_rect:?}"
            );
            clear_textures(&mut output);
        }
    }

    #[test]
    fn unknown_card_preserves_contract_state() {
        let ctx = row_test_context();
        let mut agent = agent_with_caps(&[]);
        agent.agent_id = "herdr:unknown-card".into();
        agent.state = crate::model::AgentState::Unknown;
        agent.display_name = Some("unknown card".into());
        agent.ts = 1_700_000_000_000;
        let now = agent.ts + 42 * 60_000;
        let mut fleet = Fleet::default();
        fleet.agents.insert(agent.agent_id.clone(), agent.clone());

        let (_, _, mut output) = master_card_frame(
            &ctx,
            &fleet,
            &agent.agent_id,
            false,
            now,
            row_test_input(vec![]),
        );
        assert!(
            text_rect(&output, "Unknown").is_some(),
            "Unknown card keeps the contract state label"
        );
        assert!(
            text_rect(&output, "Idle").is_none(),
            "Unknown card is not relabelled as Idle"
        );
        clear_textures(&mut output);
    }

    #[test]
    fn master_list_omits_full_output_and_collapses_idle_tail() {
        let ctx = row_test_context();
        let mut working = agent_in_repo("herdr:working", Some("corral"));
        working.state = crate::model::AgentState::Working;
        working.display_name = Some("working card".into());
        let mut idle = agent_in_repo("herdr:idle", Some("corral"));
        idle.state = crate::model::AgentState::Idle;
        idle.display_name = Some("idle card".into());
        let mut fleet = Fleet::default();
        fleet
            .agents
            .insert(working.agent_id.clone(), working.clone());
        fleet.agents.insert(idle.agent_id.clone(), idle.clone());
        let visible = ["herdr:working", "herdr:idle"];

        let (_, mut output) = master_list_frame(
            &ctx,
            &fleet,
            &visible,
            true,
            None,
            now_millis(),
            row_test_input(vec![]),
        );
        assert!(
            text_rect(&output, "working card").is_some(),
            "non-idle card renders so the output omission is observable"
        );
        assert!(
            text_rect(&output, "Idle / done (1) — expandable").is_some(),
            "idle tail renders as one collapsed expandable section"
        );
        clear_textures(&mut output);
    }

    #[test]
    fn recent_tail_classifies_terminal_semantics_into_chat_styles() {
        assert_eq!(classify_tail_line("› fix the board"), RecentBlockKind::User);
        assert_eq!(
            classify_tail_line("› Ask Codex to do anything"),
            RecentBlockKind::Agent,
            "the empty Codex prompt is not a human message"
        );
        assert_eq!(
            classify_tail_line("• Working (5m43s • esc to interrupt)"),
            RecentBlockKind::Tool
        );
        assert_eq!(
            classify_tail_line("Here is the assistant response."),
            RecentBlockKind::Agent
        );
        assert_eq!(
            classify_tail_line("The tool_call concept is explained here."),
            RecentBlockKind::Agent,
            "ordinary prose containing tool_call stays agent text"
        );
        assert_eq!(
            classify_tail_line("This tool-use phrase is prose."),
            RecentBlockKind::Agent,
            "ordinary prose containing tool-use stays agent text"
        );

        let user = recent_block_style(RecentBlockKind::User);
        assert_eq!(
            user.fill,
            theme::ui::USER_TINT,
            "user blocks use the prototype tint"
        );
        assert_eq!(
            user.inset, USER_BLOCK_INSET,
            "user blocks keep the 24px left inset"
        );
        assert!(!user.monospace, "user text remains proportional");

        let tool = recent_block_style(RecentBlockKind::Tool);
        assert_eq!(
            tool.fill,
            theme::ui::PANEL3,
            "tool blocks use the tool panel"
        );
        assert_eq!(tool.inset, 0.0, "tool blocks fill the pane width");
        assert!(
            tool.monospace,
            "tool text keeps terminal monospace treatment"
        );

        let agent = recent_block_style(RecentBlockKind::Agent);
        assert_eq!(agent.fill, Color32::TRANSPARENT);
        assert!(!agent.monospace, "agent text remains proportional");
    }

    #[test]
    fn recent_metadata_is_badged_and_not_rendered_as_prose() {
        let metadata = recent_metadata_from_texts(&[
            "assistant text\ngpt-5.6-luna max · ~/.herdr/worktrees/project-hearthwild/gauntlet-54",
        ]);
        assert_eq!(metadata.model.as_deref(), Some("gpt-5.6-luna"));
        assert_eq!(metadata.effort.as_deref(), Some("max"));
        assert_eq!(
            metadata.worktree.as_deref(),
            Some("~/.herdr/worktrees/project-hearthwild/gauntlet-54")
        );
        assert_eq!(
            recent_visible_text(
                "assistant text\ngpt-5.6-luna max · ~/.herdr/worktrees/project-hearthwild/gauntlet-54"
            )
            .as_deref(),
            Some("assistant text")
        );
        let keyed_prose = "path: src/main.rs\nmodel: tool output";
        assert_eq!(
            recent_visible_text(keyed_prose).as_deref(),
            Some(keyed_prose),
            "key-looking prose remains content"
        );
        let canonical = "gpt-5.6-luna max · ~/.herdr/worktrees/corral/session";
        assert_eq!(
            recent_visible_text(&format!("assistant text\n{canonical}")).as_deref(),
            Some("assistant text"),
            "only a trailing canonical line is lifted"
        );
        assert_eq!(
            recent_visible_text(canonical).as_deref(),
            Some(canonical),
            "a sole canonical line is never deleted"
        );
        assert!(
            parse_recent_metadata("The model is ready · no slash").is_none(),
            "ordinary prose must not be removed as metadata"
        );
    }

    #[test]
    fn recent_highlighting_is_restricted_to_code_or_diff_tool_blocks() {
        let diff = "git diff -- src/catalog.rs\n@@ -1,1 +1,2 @@\n-let old = \"plain\";\n+let new = \"highlighted\";";
        assert!(recent_is_code_or_diff(diff));
        assert!(!recent_is_code_or_diff(
            "The tool reports a model mismatch."
        ));
        assert!(!recent_is_code_or_diff("index out of bounds"));
        assert!(!recent_is_code_or_diff("---"));
        assert!(!recent_is_code_or_diff("git diff -- src/catalog.rs"));
        assert!(
            recent_highlight("+let value = \"highlighted\";")
                .iter()
                .any(|segment| segment.kind == RecentCodeSegmentKind::Addition)
        );
        assert!(
            recent_highlight("let value = \"highlighted\";")
                .iter()
                .any(|segment| segment.kind == RecentCodeSegmentKind::String)
        );
        let tick = '\u{60}';
        assert!(
            !recent_is_code_or_diff(&format!("{tick}let value = \"plain\";{tick}")),
            "single backticks are inline prose, not a fenced code block"
        );
        assert!(
            recent_is_code_or_diff(&format!(
                "{tick}{tick}{tick}rust\nlet value = \"highlighted\";\n{tick}{tick}{tick}"
            )),
            "only triple backticks form a fenced code block"
        );
        assert!(
            recent_highlight("# comment")
                .iter()
                .any(|segment| segment.kind == RecentCodeSegmentKind::Comment)
        );
        assert!(
            recent_highlight("value#hash")
                .iter()
                .all(|segment| segment.kind != RecentCodeSegmentKind::Comment),
            "a mid-line hash is not a comment marker"
        );
    }

    #[test]
    fn recent_tool_summary_preserves_the_full_trimmed_command() {
        assert_eq!(
            recent_tool_summary("  $ cargo test --workspace  \ntest result: ok"),
            "cargo test --workspace"
        );
        assert_eq!(
            recent_tool_summary("\n  npm run lint -- --strict  \noutput"),
            "npm run lint -- --strict"
        );
    }

    #[test]
    fn recent_tool_disclosure_ids_are_positional_and_history_count_is_unknown_safe() {
        assert_eq!(recent_tool_disclosure_id(4), recent_tool_disclosure_id(4));
        assert_ne!(recent_tool_disclosure_id(4), recent_tool_disclosure_id(5));
        assert_eq!(EARLIER_OUTPUT_LABEL, "Earlier output");
        assert!(!EARLIER_OUTPUT_LABEL.contains("229"));
    }

    #[test]
    fn dense_master_tool_pill_has_fixed_inner_height() {
        let ctx = row_test_context();
        let mut pill_rect = None;
        let mut output = ctx.run_ui(row_test_input(vec![]), |ui| {
            row_test_style(ui);
            pill_rect = Some(tool_pill(ui, "codex").rect);
        });
        let pill_rect = pill_rect.expect("tool pill allocated a bounded rectangle");
        assert_eq!(pill_rect.height(), TOOL_PILL_HEIGHT);
        assert!(
            pill_rect.height() < MASTER_ROW_HEIGHT,
            "the tool pill must fit inside the dense 34px master row"
        );
        clear_textures(&mut output);
    }

    #[test]
    fn long_tool_blocks_wrap_inside_the_recent_output_pane() {
        let ctx = row_test_context();
        let long_tool_line = format!("• {}", "command-output ".repeat(30));
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(220.0, 240.0),
                )),
                ..Default::default()
            },
            |ui| {
                row_test_style(ui);
                ui.set_max_width(180.0);
                recent_chat_block(ui, RecentBlockKind::Tool, &long_tool_line, 0);
            },
        );
        let rendered = rendered_text(&output, &long_tool_line)
            .expect("the complete bounded tool line remains in the rendered galley");
        assert!(
            !rendered.elided,
            "long tool output wraps instead of eliding"
        );
        assert!(
            rendered.rect.right() <= 180.0 + 0.01,
            "wrapped tool output stays inside the pane width"
        );
        clear_textures(&mut output);
    }

    #[test]
    fn master_headers_align_identity_and_state_time_with_dense_rows() {
        let ctx = row_test_context();
        let mut agent = agent_with_caps(&[]);
        agent.display_name = Some("dense agent".into());
        let fleet = Fleet {
            agents: [(agent.agent_id.clone(), agent.clone())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let width = 520.0;
        let mut output = ctx.run_ui(row_test_input(vec![]), |ui| {
            row_test_style(ui);
            ui.set_width(width);
            master_column_header(ui, width);
            master_card_with_response(ui, &fleet, &agent.agent_id, false, 0);
        });
        let header_identity = rendered_text(&output, "Agent").expect("Agent header rendered");
        let row_identity = rendered_text(&output, "dense agent").expect("agent identity rendered");
        let header_state =
            rendered_text(&output, "State · time").expect("state/time header rendered");
        let row_state = rendered_text(&output, "Working ·").expect("state/time value rendered");
        assert!(
            (header_identity.rect.left() - row_identity.rect.left()).abs() <= 0.01,
            "Agent header follows the row identity inset"
        );
        assert!(
            (header_state.layout_right - row_state.layout_right).abs() <= 0.1,
            "State · time header aligns to the row value's right edge"
        );
        assert!(
            header_state.layout_right <= width - MASTER_STATE_RIGHT_INSET + 0.1,
            "header terminal glyph stays inside the themed right inset"
        );
        assert!(
            row_state.layout_right <= width - MASTER_STATE_RIGHT_INSET + 0.1,
            "row terminal glyph stays inside the themed right inset"
        );
        clear_textures(&mut output);
    }

    #[test]
    fn board_toolbar_has_required_chips_and_detail_owns_view_actions() {
        let ctx = row_test_context();
        let mut agent = agent_in_repo("herdr:alpha", Some("corral"));
        agent.display_name = Some("alpha agent".into());
        agent.state = crate::model::AgentState::Blocked;
        agent.capabilities = ["read_tail", "interrupt", "kill"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let fleet = Fleet {
            agents: [(agent.agent_id.clone(), agent)].into_iter().collect(),
            ..Default::default()
        };

        let (view, flat, query, filter, mut output) = toolbar_frame(
            &ctx,
            &fleet,
            BoardView::Cards,
            true,
            "",
            StateFilter::All,
            row_test_input(vec![]),
        );
        assert_eq!(view, BoardView::Cards);
        assert!(flat, "Cards keeps its flat default");
        assert_eq!(query, "");
        assert_eq!(filter, StateFilter::All);
        let needs_you = text_rect(&output, "Needs you").expect("Needs you chip rendered");
        let all = text_rect(&output, "All").expect("the active All chip emits its label");
        assert!(
            needs_you.left() < all.left(),
            "production chip order must be Needs you then All: needs={needs_you:?} all={all:?}"
        );
        assert!(
            text_rect(&output, "Search repo / branch / issue…").is_some(),
            "the board search hint remains in the master toolbar"
        );
        for forbidden in ["Working", "Idle", "Review", "flat sort", "Cards", "Table"] {
            assert!(
                text_rect(&output, forbidden).is_none(),
                "master toolbar must not render the legacy {forbidden} control"
            );
        }
        clear_textures(&mut output);

        let mut view = BoardView::Cards;
        let mut actions = BoardActions {
            drive: &mut |_| {},
            read_only: false,
        };
        let mut output = ctx.run_ui(row_test_input(vec![]), |ui| {
            row_test_style(ui);
            right_pane(
                ui,
                &fleet,
                Some("herdr:alpha"),
                &mut view,
                true,
                &|_| true,
                &mut actions,
            );
        });
        for required in [
            "Cards",
            "Interrupt",
            "Kill",
            "Recent output",
            PAUSED_LABEL,
            EARLIER_OUTPUT_LABEL,
            LOAD_EARLIER_LABEL,
        ] {
            assert!(
                text_rect(&output, required).is_some(),
                "detail pane must render {required:?}"
            );
        }
        let controls = ["Cards", "Interrupt", "Kill"]
            .map(|label| text_rect(&output, label).expect("primary control rendered"));
        assert!(
            controls
                .windows(2)
                .all(|pair| pair[0].left() < pair[1].left()),
            "Cards controls stay in prototype order Cards / Interrupt / Kill"
        );
        let controls_share_row = controls
            .windows(2)
            .all(|pair| (pair[0].top() - pair[1].top()).abs() <= 1.0);
        if !controls_share_row {
            clear_textures(&mut output);
            panic!("Cards controls share one inline row rather than wrapping: {controls:?}");
        }
        assert!(
            text_rect(&output, "repo / branch / dirty / a-b / pr / ci").is_none(),
            "Cards detail must not append the legacy topology/drive card below Recent output"
        );
        assert!(
            text_rect(&output, "recent drives").is_none(),
            "Cards detail must keep Recent output as the sole selected-agent surface"
        );
        clear_textures(&mut output);
    }

    #[test]
    fn recent_output_order_honors_stick_to_bottom_setting() {
        assert_eq!(recent_output_indices(8, true), vec![7, 6, 5, 4, 3, 2, 1, 0]);
        assert_eq!(
            recent_output_indices(8, false),
            vec![0, 1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(recent_output_indices(3, true), vec![2, 1, 0]);
        assert_eq!(recent_output_indices(0, false), Vec::<usize>::new());
    }

    #[test]
    fn recent_output_filters_before_rendering_the_full_loaded_history() {
        let lines = [
            "assistant output",
            "gpt-5.6-luna max · ~/.herdr/worktrees/corral/session\nassistant output",
            "second output",
            "third output",
            "fourth output",
            "fifth output",
            "sixth output",
            "seventh output",
        ];
        assert_eq!(recent_visible_indices(lines), vec![0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(
            recent_output_indices(recent_visible_indices(lines).len(), true),
            vec![7, 6, 5, 4, 3, 2, 1, 0],
            "a loaded history longer than six rows is not clipped"
        );
        assert!(
            recent_visible_indices(["\n", " \t"]).is_empty(),
            "an all-metadata/blank pane gets the explicit empty-state path"
        );
    }

    #[test]
    fn recent_live_indicator_requires_visible_output_and_non_error_drive() {
        let sending = DriveState::Sending {
            request_id: "r1".into(),
            capability: "read_tail".into(),
        };
        let ok = DriveState::Ok {
            rev: 1,
            capability: "read_tail".into(),
        };
        let failed = DriveState::Failed {
            failure: DriveFailure::NotGranted("grant".into()),
            capability: "read_tail".into(),
        };
        assert!(recent_should_show_live(Some(&sending), true));
        assert!(recent_should_show_live(Some(&ok), true));
        assert!(!recent_should_show_live(Some(&failed), true));
        assert!(!recent_should_show_live(Some(&ok), false));
        assert!(!recent_should_show_live(None, true));
    }

    #[test]
    fn production_cards_toolbar_hides_needs_you_when_blocked_bucket_is_empty() {
        let ctx = row_test_context();
        let agent = agent_with_caps(&["read_tail"]);
        let mut fleet = Fleet {
            agents: [(agent.agent_id.clone(), agent)].into_iter().collect(),
            ..Default::default()
        };
        let mut actions = BoardActions {
            drive: &mut |_| {},
            read_only: false,
        };
        let mut output = ctx.run_ui(row_test_input(vec![]), |ui| {
            row_test_style(ui);
            show(ui, &mut fleet, &|_| true, &mut actions);
        });
        assert!(text_rect(&output, "All").is_some());
        assert!(
            text_rect(&output, "Needs you").is_none(),
            "production Cards toolbar must hide the empty Needs you bucket"
        );
        assert!(text_rect(&output, "Recent output").is_some());
        clear_textures(&mut output);
    }

    #[test]
    fn cards_surface_scrolls_at_small_viewports() {
        let ctx = row_test_context();
        let agent = agent_with_caps(&["read_tail"]);
        let mut fleet = Fleet {
            agents: [(agent.agent_id.clone(), agent)].into_iter().collect(),
            ..Default::default()
        };
        let mut actions = BoardActions {
            drive: &mut |_| {},
            read_only: false,
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 300.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            row_test_style(ui);
            show(ui, &mut fleet, &|_| true, &mut actions);
        });
        assert!(text_rect(&output, "All").is_some());
        assert!(text_rect(&output, "Recent output").is_some());
        assert!(
            text_rect(&output, LOAD_EARLIER_LABEL).is_some(),
            "the small Cards surface keeps its fetch control reachable inside ScrollArea"
        );
        clear_textures(&mut output);
    }

    #[test]
    fn public_board_entry_is_cards_only_after_legacy_table_state() {
        let ctx = row_test_context();
        ctx.memory_mut(|memory| {
            memory
                .data
                .insert_temp::<BoardView>(egui::Id::new(VIEW_MODE), BoardView::Table);
        });
        let agent = agent_with_caps(&["read_tail"]);
        let mut fleet = Fleet {
            agents: [(agent.agent_id.clone(), agent)].into_iter().collect(),
            ..Default::default()
        };
        let mut actions = BoardActions {
            drive: &mut |_| {},
            read_only: false,
        };
        let mut output = ctx.run_ui(row_test_input(vec![]), |ui| {
            row_test_style(ui);
            show(ui, &mut fleet, &|_| true, &mut actions);
        });
        assert!(
            text_rect(&output, "Recent output").is_some(),
            "the public board entry must render the Cards detail surface"
        );
        assert!(
            text_rect(&output, "AGENT").is_none(),
            "legacy Table state must not expose the removed table surface"
        );
        assert!(
            text_rect(&output, "Audit").is_none(),
            "Audit must not return as a board-local navigation tab"
        );
        clear_textures(&mut output);
    }

    #[test]
    fn cards_load_earlier_dispatches_real_read_tail() {
        let ctx = row_test_context();
        let mut agent = agent_with_caps(&["read_tail"]);
        agent.agent_id = "herdr:cards-fetch".into();
        let fleet = Fleet {
            agents: [(agent.agent_id.clone(), agent)].into_iter().collect(),
            rev: Some(42),
            selected_agent: Some("herdr:cards-fetch".into()),
            ..Default::default()
        };
        let intents = std::cell::RefCell::new(Vec::new());
        let mut actions = BoardActions {
            drive: &mut |intent| intents.borrow_mut().push(intent),
            read_only: false,
        };
        let mut view = BoardView::Cards;
        let mut output = ctx.run_ui(row_test_input(vec![]), |ui| {
            row_test_style(ui);
            right_pane(
                ui,
                &fleet,
                Some("herdr:cards-fetch"),
                &mut view,
                true,
                &|_| true,
                &mut actions,
            );
        });
        let load_pos = text_rects(&output, LOAD_EARLIER_LABEL)
            .last()
            .expect("the divider's Load earlier control is rendered")
            .center();
        clear_textures(&mut output);

        let mut output = ctx.run_ui(pointer_down_input(load_pos), |ui| {
            row_test_style(ui);
            right_pane(
                ui,
                &fleet,
                Some("herdr:cards-fetch"),
                &mut view,
                true,
                &|_| true,
                &mut actions,
            );
        });
        clear_textures(&mut output);
        let mut output = ctx.run_ui(pointer_up_input(load_pos), |ui| {
            row_test_style(ui);
            right_pane(
                ui,
                &fleet,
                Some("herdr:cards-fetch"),
                &mut view,
                true,
                &|_| true,
                &mut actions,
            );
        });
        clear_textures(&mut output);

        assert_eq!(
            intents.borrow().len(),
            1,
            "Cards click dispatches one real drive"
        );
        assert_eq!(
            intents.borrow()[0].capability,
            crate::drive::Capability::ReadTail
        );
        assert_eq!(intents.borrow()[0].target, "herdr:cards-fetch");
        assert_eq!(intents.borrow()[0].rev, Some(42));
    }

    #[test]
    fn right_pane_gates_interrupt_kill_and_requires_kill_confirmation() {
        let render = |ctx: &egui::Context,
                      fleet: &Fleet,
                      view: &mut BoardView,
                      allowed: &dyn Fn(&str) -> bool,
                      actions: &mut BoardActions,
                      input: egui::RawInput|
         -> egui::FullOutput {
            ctx.run_ui(input, |ui| {
                row_test_style(ui);
                right_pane(
                    ui,
                    fleet,
                    fleet.selected_agent.as_deref(),
                    view,
                    true,
                    allowed,
                    actions,
                );
            })
        };

        let ctx = row_test_context();
        let mut ready_agent = agent_with_caps(&["interrupt", "kill"]);
        ready_agent.agent_id = "herdr:ready-actions".into();
        let ready_fleet = Fleet {
            agents: [(ready_agent.agent_id.clone(), ready_agent)]
                .into_iter()
                .collect(),
            selected_agent: Some("herdr:ready-actions".into()),
            ..Default::default()
        };
        let intents = std::cell::RefCell::new(Vec::new());
        let mut actions = BoardActions {
            drive: &mut |intent| intents.borrow_mut().push(intent),
            read_only: false,
        };
        let mut view = BoardView::Cards;
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            row_test_input(vec![]),
        );
        let interrupt_pos = text_rect(&output, "Interrupt").unwrap().center();
        let kill_pos = text_rect(&output, "Kill").unwrap().center();
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_down_input(interrupt_pos),
        );
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_up_input(interrupt_pos),
        );
        assert_eq!(
            intents.borrow()[0].capability,
            crate::drive::Capability::Interrupt
        );
        clear_textures(&mut output);

        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_down_input(kill_pos),
        );
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_up_input(kill_pos),
        );
        assert_eq!(
            intents.borrow().len(),
            1,
            "Kill never dispatches on its first click"
        );
        clear_textures(&mut output);
        // Replay the original Kill coordinate as a double-click. The
        // pending trigger stays disabled in place and confirmation lives in a
        // separate row, so this second release cannot dispatch Kill.
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_down_input(kill_pos),
        );
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_up_input(kill_pos),
        );
        assert_eq!(
            intents.borrow().len(),
            1,
            "replaying the original Kill coordinate cannot bypass confirmation"
        );
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            row_test_input(vec![]),
        );
        let cancel_pos = text_rect(&output, "Cancel").unwrap().center();
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_down_input(cancel_pos),
        );
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_up_input(cancel_pos),
        );
        assert_eq!(
            intents.borrow().len(),
            1,
            "Cancel leaves the destructive action unissued"
        );
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            row_test_input(vec![]),
        );
        let kill_again = text_rect(&output, "Kill").unwrap().center();
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_down_input(kill_again),
        );
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_up_input(kill_again),
        );
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            row_test_input(vec![]),
        );
        let confirm_again = text_rect(&output, "Confirm kill").unwrap().center();
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_down_input(confirm_again),
        );
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_up_input(confirm_again),
        );
        assert_eq!(intents.borrow().len(), 2);
        assert_eq!(
            intents.borrow()[1].capability,
            crate::drive::Capability::Kill
        );
        clear_textures(&mut output);

        // A pending confirmation is owned by the selected agent, not by a
        // stale per-agent flag. Changing selection clears it before the new
        // agent can render a destructive confirmation.
        let mut second_agent = agent_with_caps(&["interrupt", "kill"]);
        second_agent.agent_id = "herdr:second-actions".into();
        let changed_selection_fleet = Fleet {
            agents: [
                (
                    "herdr:ready-actions".into(),
                    ready_fleet.agents["herdr:ready-actions"].clone(),
                ),
                (second_agent.agent_id.clone(), second_agent),
            ]
            .into_iter()
            .collect(),
            selected_agent: Some("herdr:second-actions".into()),
            ..Default::default()
        };
        // Create a fresh pending confirmation for the original selected
        // agent, then render the changed selection through the real pane.
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            row_test_input(vec![]),
        );
        let kill_again = text_rect(&output, "Kill").unwrap().center();
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_down_input(kill_again),
        );
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &ready_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            pointer_up_input(kill_again),
        );
        clear_textures(&mut output);
        let mut output = render(
            &ctx,
            &changed_selection_fleet,
            &mut view,
            &|_| true,
            &mut actions,
            row_test_input(vec![]),
        );
        assert!(
            text_rect(&output, "Confirm kill").is_none(),
            "changing the selected agent clears the prior kill confirmation"
        );
        clear_textures(&mut output);

        for (id, caps, allowed) in [
            ("herdr:unadvertised", Vec::new(), true),
            (
                "herdr:ungranted",
                vec!["interrupt".to_string(), "kill".to_string()],
                false,
            ),
        ] {
            let mut agent = agent_with_caps(&[]);
            agent.agent_id = id.into();
            agent.capabilities = caps;
            let fleet = Fleet {
                agents: [(agent.agent_id.clone(), agent)].into_iter().collect(),
                selected_agent: Some(id.into()),
                ..Default::default()
            };
            let mut blocked_intents = Vec::new();
            let mut blocked_actions = BoardActions {
                drive: &mut |intent| blocked_intents.push(intent),
                read_only: false,
            };
            let mut view = BoardView::Cards;
            let mut output = render(
                &ctx,
                &fleet,
                &mut view,
                &|_| allowed,
                &mut blocked_actions,
                row_test_input(vec![]),
            );
            let interrupt = text_rect(&output, "Interrupt").unwrap().center();
            clear_textures(&mut output);
            let mut output = render(
                &ctx,
                &fleet,
                &mut view,
                &|_| allowed,
                &mut blocked_actions,
                pointer_down_input(interrupt),
            );
            clear_textures(&mut output);
            let mut output = render(
                &ctx,
                &fleet,
                &mut view,
                &|_| allowed,
                &mut blocked_actions,
                pointer_up_input(interrupt),
            );
            assert!(
                blocked_intents.is_empty(),
                "{id} must not dispatch an unavailable or ungranted action"
            );
            clear_textures(&mut output);
        }
    }

    #[test]
    fn toolbar_search_accepts_real_click_and_text_events() {
        let ctx = row_test_context();
        let agent = agent_in_repo("herdr:search", Some("corral"));
        let fleet = Fleet {
            agents: [(agent.agent_id.clone(), agent)].into_iter().collect(),
            ..Default::default()
        };

        let (_, _, _, _, mut output) = toolbar_frame(
            &ctx,
            &fleet,
            BoardView::Table,
            false,
            "",
            StateFilter::All,
            row_test_input(vec![]),
        );
        let search_pos = text_rect(&output, "Search repo / branch / issue…")
            .expect("search field rendered")
            .center();
        clear_textures(&mut output);

        let (view, flat, query, filter, mut output) = toolbar_frame(
            &ctx,
            &fleet,
            BoardView::Table,
            false,
            "",
            StateFilter::All,
            pointer_down_input(search_pos),
        );
        clear_textures(&mut output);
        let (view, flat, query, filter, mut output) = toolbar_frame(
            &ctx,
            &fleet,
            view,
            flat,
            &query,
            filter,
            pointer_up_input(search_pos),
        );
        clear_textures(&mut output);
        let (view, flat, query, filter, mut output) = toolbar_frame(
            &ctx,
            &fleet,
            view,
            flat,
            &query,
            filter,
            row_test_input(vec![egui::Event::Text("alpha".into())]),
        );
        assert_eq!(
            query, "alpha",
            "the clicked TextEdit accepts the text event"
        );
        assert_eq!(search_query(&ctx), "alpha");
        assert_eq!(view, BoardView::Table);
        assert_eq!(filter, StateFilter::All);
        assert!(
            !flat,
            "Table remains grouped until its own flat sort control is used"
        );
        clear_textures(&mut output);
    }

    #[test]
    fn table_empty_state_shows_message_once_and_keeps_header() {
        let ctx = row_test_context();
        let message = no_match_message("missing");
        let mut output = ctx.run_ui(row_test_input(vec![]), |ui| {
            row_test_style(ui);
            show_empty_table_state(ui, "missing");
        });
        assert!(
            text_rects(&output, &message).len() == 1,
            "Table no-match state reports the query once"
        );
        assert!(
            text_rect(&output, "AGENT").is_some(),
            "Table no-match state keeps the column header"
        );
        clear_textures(&mut output);
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
            body: None,
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
            body: None,
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
                    agent_row_cells(ui, &agent, &Fleet::default())
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

    #[derive(Debug, Clone, Copy)]
    struct RenderedText {
        elided: bool,
        rect: egui::Rect,
        layout_right: f32,
    }

    fn rendered_texts(output: &egui::FullOutput, needle: &str) -> Vec<RenderedText> {
        fn walk(shape: &egui::epaint::Shape, needle: &str, texts: &mut Vec<RenderedText>) {
            match shape {
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, needle, texts);
                    }
                }
                egui::epaint::Shape::Text(text) if text.galley.job.text.contains(needle) => {
                    texts.push(RenderedText {
                        elided: text.galley.elided,
                        rect: text.visual_bounding_rect(),
                        layout_right: text.pos.x + text.galley.rect.right(),
                    });
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, needle, &mut texts);
        }
        texts
    }

    fn rendered_text(output: &egui::FullOutput, needle: &str) -> Option<RenderedText> {
        rendered_texts(output, needle).into_iter().next()
    }

    fn text_rect(output: &egui::FullOutput, needle: &str) -> Option<egui::Rect> {
        rendered_text(output, needle).map(|rendered| rendered.rect)
    }

    fn text_rects(output: &egui::FullOutput, needle: &str) -> Vec<egui::Rect> {
        rendered_texts(output, needle)
            .into_iter()
            .map(|rendered| rendered.rect)
            .collect()
    }

    fn master_list_frame(
        ctx: &egui::Context,
        fleet: &Fleet,
        visible: &[&str],
        flat: bool,
        selected: Option<&str>,
        now_ms: u64,
        input: egui::RawInput,
    ) -> (Option<String>, egui::FullOutput) {
        let width = input.screen_rect.unwrap_or_else(row_test_screen).width();
        let mut clicked = None;
        let output = ctx.run_ui(input, |ui| {
            row_test_style(ui);
            ui.set_max_width(width);
            clicked = master_list(ui, fleet, visible, flat, selected, now_ms);
        });
        (clicked, output)
    }

    fn master_card_frame(
        ctx: &egui::Context,
        fleet: &Fleet,
        id: &str,
        selected: bool,
        now_ms: u64,
        input: egui::RawInput,
    ) -> (Option<String>, Option<egui::Rect>, egui::FullOutput) {
        let width = input.screen_rect.unwrap_or_else(row_test_screen).width();
        let mut clicked = None;
        let mut rect = None;
        let output = ctx.run_ui(input, |ui| {
            row_test_style(ui);
            ui.set_max_width(width);
            if let Some((card_clicked, response)) =
                master_card_with_response(ui, fleet, id, selected, now_ms)
            {
                clicked = card_clicked;
                rect = Some(response.rect);
            }
        });
        (clicked, rect, output)
    }

    fn toolbar_frame(
        ctx: &egui::Context,
        fleet: &Fleet,
        view: BoardView,
        flat: bool,
        query: &str,
        filter: StateFilter,
        input: egui::RawInput,
    ) -> (BoardView, bool, String, StateFilter, egui::FullOutput) {
        let mut view = view;
        let mut flat = flat;
        let mut query = query.to_string();
        let mut filter = filter;
        let output = ctx.run_ui(input, |ui| {
            row_test_style(ui);
            toolbar(ui, fleet, &mut view, &mut flat, &mut query, &mut filter);
        });
        (view, flat, query, filter, output)
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
            read_only: false,
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
            read_only: false,
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
    fn detail_diff_load_dispatches_and_cached_page_renders() {
        let ctx = row_test_context();
        let mut agent = agent_with_caps(&["read_diff", "read_tail"]);
        agent.agent_id = "herdr:diff".into();
        agent.workspace.dirty = true;
        let mut fleet = Fleet::default();
        fleet.agents.insert(agent.agent_id.clone(), agent.clone());
        fleet.expanded.push(agent.agent_id.clone());

        let diff_intent = |fleet: &Fleet,
                           output: &mut egui::FullOutput,
                           label: &str,
                           intents: &mut Vec<DriveIntent>|
         -> Vec<String> {
            let rect = text_rect(output, label).expect("diff control rendered");
            clear_textures(output);
            let mut actions = BoardActions {
                drive: &mut |intent| intents.push(intent),
                read_only: false,
            };
            board_row_click_with_allowed(
                &ctx,
                fleet,
                &agent.agent_id,
                rect.center(),
                &|_| true,
                &mut actions,
            )
        };

        // Phase 1 — no cache yet: the section offers the explicit load
        // control and clicking it dispatches page 0 (bounded query).
        let mut intents = Vec::new();
        let (_, mut output) = {
            let mut actions = BoardActions {
                drive: &mut |intent| intents.push(intent),
                read_only: false,
            };
            board_row_frame_with_allowed(
                &ctx,
                &fleet,
                &agent.agent_id,
                row_test_input(vec![]),
                &|_| true,
                &mut actions,
            )
        };
        let header_rect = text_rect(&output, "Worktree diff").expect("section header rendered");
        let toggles = diff_intent(&fleet, &mut output, "± load diff", &mut intents);
        assert!(toggles.is_empty(), "load button must not toggle the row");
        assert_eq!(intents.len(), 1, "load diff must dispatch exactly once");
        assert_eq!(intents[0].capability, crate::drive::Capability::ReadDiff);
        assert_eq!(intents[0].target, "herdr:diff");
        assert_eq!(intents[0].payload["files"], 128);
        assert_eq!(intents[0].payload["offset"], 0);
        assert_eq!(intents[0].payload["lines"], 200);

        // Phase 2 — cached page: header (repo · branch), diffstat, files,
        // paging control; "Load next" dispatches the next offset.
        fleet.remember_diff_page(
            &agent.agent_id,
            crate::drive::DiffPage {
                repo: Some("corral".into()),
                branch: Some("g232/read-diff".into()),
                head: Some("abc1234".into()),
                stats: crate::drive::DiffStats {
                    files: 1,
                    adds: 12,
                    dels: 5,
                },
                files: vec![crate::drive::DiffFileStat {
                    path: "src/core/diff.rs".into(),
                    adds: 12,
                    dels: 5,
                }],
                files_truncated: false,
                offset: 0,
                lines: vec![
                    "diff --git a/src/core/diff.rs b/src/core/diff.rs".into(),
                    " one".into(),
                    "+two".into(),
                    "-three".into(),
                ],
                total: 9,
                has_more: true,
                next_offset: Some(4),
            },
        );
        let (_, mut output) = {
            let mut actions = BoardActions {
                drive: &mut |intent| intents.push(intent),
                read_only: false,
            };
            board_row_frame_with_allowed(
                &ctx,
                &fleet,
                &agent.agent_id,
                row_test_input(vec![]),
                &|_| true,
                &mut actions,
            )
        };
        assert!(
            rendered_text(&output, "corral · g232/read-diff").is_some(),
            "diff section must render the daemon attribution header"
        );
        assert!(
            rendered_text(&output, "+12/−5 · 1 files").is_some(),
            "diff section must render the one-line diffstat"
        );
        assert!(
            rendered_text(&output, "src/core/diff.rs (+12/−5)").is_some(),
            "diff section must render the changed-files list"
        );
        assert!(
            rendered_text(&output, "Load next 200 lines (offset 4)").is_some(),
            "diff section must offer the next lazy page"
        );
        let toggles = diff_intent(
            &fleet,
            &mut output,
            "Load next 200 lines (offset 4)",
            &mut intents,
        );
        assert!(toggles.is_empty());
        assert_eq!(intents.len(), 2, "load next must dispatch a page fetch");
        assert_eq!(intents[1].capability, crate::drive::Capability::ReadDiff);
        assert_eq!(intents[1].payload["offset"], 4);
        assert_eq!(intents[1].payload["lines"], 200);
        let _ = header_rect;
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
        assert_eq!(grant, "requires the kill grant — ask the host");
        assert_eq!(not_implemented, "kill: not implemented yet");
        assert_eq!(
            drive_disabled_reason("read_tail", DriveControlState::MissingGrant),
            Some("requires the read_tail grant — ask the host".to_string())
        );
        assert_eq!(
            drive_disabled_reason("read_tail", DriveControlState::NotImplemented),
            Some("read_tail: not implemented yet".to_string())
        );
        assert!(
            !grant.contains(LOAD_EARLIER_LABEL) && !not_implemented.contains(LOAD_EARLIER_LABEL),
            "disabled guidance names capabilities, never display labels"
        );
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
            body: None,
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
}
