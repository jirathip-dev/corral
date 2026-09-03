//! Read-only board + recents v1 (shared by the native desktop panes and the
//! WASM demo board).
//!
//! #354 L3 cut: this module replaces the pre-cut board surface (cards /
//! master-detail with action controls, search + filter chips, diff/terminal
//! drill-ins and the V3 Conversation/Harness recents partition). What stays
//! is exactly the v2 read-only board — repo groups with status chips,
//! blocked pinned to the top, attention-ordered rows carrying name / repo /
//! state / time-in-state / branch + small pane ref — and the recents v1
//! LIVE TAIL (≤200-line daemon cap, auto-scroll, no load-earlier).
//!
//! The pure projections mirror the iOS `BoardModel`/`StateStyle` semantics
//! (the L2 client cut) so the two clients cannot diverge again.

use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Painter, Rect, RichText, Sense, Stroke, Ui, Vec2,
};

use crate::drive::CanonicalBlock;
use crate::model::{Agent, AgentState};
use crate::state::{ConnState, DriveState, Fleet};
use crate::theme;

/// Native workspace split: master (board) vs detail (recents/settings).
pub const MASTER_DETAIL_RATIO: (f32, f32) = (0.42, 0.58);

const ROW_HEIGHT: f32 = 46.0;
const NO_REPO_LABEL: &str = "no repo";

/// Epoch millis "now" for the age labels. Native has std time; the wasm
/// build must use the JS clock (std time panics on wasm32-unknown-unknown).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn now_millis() -> u64 {
    js_sys::Date::now() as u64
}

// ---------------------------------------------------------------------------
// Pure board projections (mirror iOS BoardModel)
// ---------------------------------------------------------------------------

/// v2 attention rank for one state: blocked(0) > working(1) > idle(2) >
/// unknown(3). A wire `done` ranks WITH idle: the board treats `done` as
/// finished (ranked/rendered with idle, never active/working — the wire
/// can carry `done` per the #324 live probe).
pub fn state_rank(state: AgentState) -> u8 {
    theme::AgentStateLike::from(state).rank()
}

/// A repo section of the board. `repo: None` is the orphan bucket (agents
/// without a workspace repo); it sorts last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSection {
    pub repo: Option<String>,
    pub agent_ids: Vec<String>,
}

impl RepoSection {
    pub fn header(&self) -> String {
        let label = self.repo.as_deref().unwrap_or(NO_REPO_LABEL);
        format!("{label} ({})", self.agent_ids.len())
    }
}

/// The v2 board shape: every blocked agent pinned to the top (a PROMOTION,
/// not a filter — the same agents also appear in their repo section), then
/// one repo section per workspace repo holding every agent of that repo in
/// attention order. A finished (`done`/idle) agent therefore STAYS in its
/// repo section until the daemon replaces/deletes it: the last-done-per-repo
/// retention rule. No collapsed cross-repo bucket, no search, no filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardSections {
    pub blocked: Vec<String>,
    pub repos: Vec<RepoSection>,
}

impl BoardSections {
    pub fn total_agents(&self) -> usize {
        self.repos.iter().map(|s| s.agent_ids.len()).sum()
    }
}

/// The canonical board ordering: v2 rank, then ts desc, then agent id for
/// determinism.
pub fn ordered_agents<'a>(agents: &[&'a Agent]) -> Vec<&'a Agent> {
    let mut agents = agents.to_vec();
    agents.sort_by(|a, b| {
        let ra = state_rank(a.state);
        let rb = state_rank(b.state);
        if ra != rb {
            return ra.cmp(&rb);
        }
        if a.ts != b.ts {
            return b.ts.cmp(&a.ts);
        }
        a.agent_id.cmp(&b.agent_id)
    });
    agents
}

/// The v2 repo-grouped board as a pure function of the agent set (stable
/// and unit-testable across renders).
pub fn sections(fleet: &Fleet) -> BoardSections {
    let agents: Vec<&Agent> = fleet.agents.values().collect();
    let ordered = ordered_agents(&agents);
    let mut blocked = Vec::new();
    let mut by_repo: Vec<(Option<String>, Vec<String>)> = Vec::new();
    for agent in ordered {
        if agent.state == AgentState::Blocked {
            blocked.push(agent.agent_id.clone());
        }
        let repo = agent.repo().map(str::to_string);
        if let Some(entry) = by_repo.iter_mut().find(|(existing, _)| *existing == repo) {
            entry.1.push(agent.agent_id.clone());
        } else {
            by_repo.push((repo, vec![agent.agent_id.clone()]));
        }
    }
    // Named repos sort by name; the orphan bucket sorts last.
    by_repo.sort_by(|(a, _), (b, _)| match (a, b) {
        (Some(a), Some(b)) => a.cmp(b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    let repos = by_repo
        .into_iter()
        .map(|(repo, agent_ids)| RepoSection { repo, agent_ids })
        .collect();
    BoardSections { blocked, repos }
}

// ---------------------------------------------------------------------------
// Recents v1 tail rows (pure; mirrors iOS RecentOutputModel.tailRows)
// ---------------------------------------------------------------------------

/// One row the recents drill-in renders: the daemon's canonical blocks when
/// present, else the legacy lines honestly mapped to `unknown` (never
/// reclassified). Whitespace-only blocks are dropped and adjacent
/// tool/system blocks merge, exactly like the pre-cut renderer, so the
/// stream stays compact and stable across fetches.
pub fn tail_rows(lines: &[String], blocks: &[CanonicalBlock]) -> Vec<TailRow> {
    let raw: Vec<CanonicalBlock> = if blocks.is_empty() {
        lines
            .iter()
            .map(|line| CanonicalBlock {
                kind: crate::drive::CanonicalBlockKind::Unknown,
                text: line.clone(),
                prompt_request_id: None,
            })
            .collect()
    } else {
        blocks.to_vec()
    };
    let mut grouped: Vec<TailRow> = Vec::new();
    for block in raw {
        let visible: Vec<&str> = block
            .text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        if visible.is_empty() {
            continue;
        }
        let text = visible.join("\n");
        let kind = block.kind;
        let is_tool_system = matches!(
            kind,
            crate::drive::CanonicalBlockKind::Tool | crate::drive::CanonicalBlockKind::System
        );
        if let Some(last) = grouped.last_mut()
            && is_tool_system
            && last.kind == kind
        {
            last.text.push('\n');
            last.text.push_str(&text);
        } else {
            grouped.push(TailRow {
                kind,
                text,
                prompt_request_id: block.prompt_request_id,
            });
        }
    }
    grouped
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailRow {
    pub kind: crate::drive::CanonicalBlockKind,
    pub text: String,
    pub prompt_request_id: Option<String>,
}

/// The recents drill-in's four-state phase, derived from the caches + drive
/// bookkeeping (mirrors iOS RecentOutputModel.phase).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecentsPhase {
    /// A read_tail fetch is in flight and nothing is cached yet.
    Loading,
    /// A cached tail exists.
    Loaded,
    /// The last read_tail attempt failed and nothing is cached.
    Error(String),
    /// No fetch has been attempted yet (first open).
    Empty,
}

pub fn recents_phase(fleet: &Fleet, agent_id: &str, can_read_tail: bool) -> RecentsPhase {
    if !can_read_tail {
        return RecentsPhase::Error(
            "this agent does not advertise the read_tail capability".to_string(),
        );
    }
    let has_cached = fleet
        .tails
        .get(agent_id)
        .is_some_and(|tail| !tail.is_empty())
        || fleet
            .tail_blocks
            .get(agent_id)
            .is_some_and(|blocks| !blocks.is_empty());
    if has_cached {
        return RecentsPhase::Loaded;
    }
    if fleet.tails.contains_key(agent_id) {
        return RecentsPhase::Loaded;
    }
    let newest = fleet.latest_drive(agent_id);
    match newest {
        Some(DriveState::Failed { failure, .. }) => RecentsPhase::Error(failure.to_string()),
        Some(DriveState::Sending { .. }) => RecentsPhase::Loading,
        _ => RecentsPhase::Empty,
    }
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------

/// Board chrome notice for a connection state: `None` = fully live. The
/// offline notice keeps the last-known board honest about its freshness.
pub fn connection_notice(conn: ConnState, detail: Option<&str>) -> Option<String> {
    match conn {
        ConnState::Connected => None,
        ConnState::Connecting => Some("connecting — waiting for the daemon".to_string()),
        ConnState::Reconnecting { .. } => {
            Some("daemon offline — showing the last-known board".to_string())
        }
        ConnState::Down => Some(
            detail
                .map(|d| format!("daemon offline — showing the last-known board ({d})"))
                .unwrap_or_else(|| "daemon offline — showing the last-known board".to_string()),
        ),
    }
}

/// Raw label + color for a state (labels are the herdr tokens verbatim).
pub fn state_style(state: AgentState) -> (Color32, &'static str) {
    let like = theme::AgentStateLike::from(state);
    (theme::state::of(like), like.label())
}

/// Chip text: mark glyph + raw state token.
fn state_chip_text(state: AgentState) -> String {
    let like = theme::AgentStateLike::from(state);
    format!("{} {}", like.mark_glyph(), like.label())
}

/// Paint a state chip inside `rect` (colored outline + tinted fill).
fn paint_state_chip(painter: &Painter, rect: Rect, state: AgentState) {
    let (color, _) = state_style(state);
    let text = state_chip_text(state);
    let font = FontId::proportional(11.0);
    let bg = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 40);
    painter.rect_filled(rect, CornerRadius::same(4), bg);
    painter.rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0, color),
        egui::StrokeKind::Outside,
    );
    let galley = painter.layout_no_wrap(text, font, color);
    painter.galley(rect.min + Vec2::new(7.0, 3.0), galley, color);
}

/// Measure the chip a row would paint (for right-aligned layout).
fn state_chip_size(painter: &Painter, state: AgentState) -> Vec2 {
    let font = FontId::proportional(11.0);
    painter
        .layout_no_wrap(state_chip_text(state), font, Color32::WHITE)
        .size()
        + Vec2::new(14.0, 6.0)
}

/// Truncate `text` to `max_width` (with an ellipsis). Bounded, so a huge
/// label cannot starve a frame.
fn truncate_text(painter: &Painter, text: &str, font: FontId, max_width: f32) -> String {
    if max_width <= 0.0 {
        return String::new();
    }
    let mut candidate = text.to_string();
    let mut guard = 0;
    while painter
        .layout_no_wrap(candidate.clone(), font.clone(), Color32::WHITE)
        .size()
        .x
        > max_width
        && !candidate.is_empty()
        && guard < 64
    {
        candidate.pop();
        guard += 1;
    }
    if candidate != text {
        candidate.push('…');
    }
    candidate
}

/// Paint one two-line board row into `rect` (name + age + state chip on line
/// one; repo · branch + pane ref on line two). Shared by the native master
/// list and the WASM board so both surfaces cannot diverge. Returns the full
/// row text for the hover/accessibility tooltip.
pub fn paint_agent_row(
    painter: &Painter,
    rect: Rect,
    agent: &Agent,
    selected: bool,
    now: u64,
    show_repo: bool,
) -> String {
    let fill = if selected {
        theme::ui::PANEL3
    } else {
        theme::ui::PANEL
    };
    painter.rect_filled(rect, CornerRadius::same(6), fill);

    let label = agent.row_label();
    let mut full = label.clone();
    let inner_left = rect.left() + 8.0;
    let inner_right = rect.right() - 8.0;

    // Line one, right cluster (right→left): state chip, then age.
    let chip_size = state_chip_size(painter, agent.state);
    let age_text = crate::model::relative_age(agent.ts, now);
    let age_font = FontId::proportional(10.0);
    let age_size = painter
        .layout_no_wrap(age_text.clone(), age_font.clone(), theme::ui::TEXT_MUTED)
        .size();
    let chip_rect = Rect::from_min_size(
        egui::pos2(inner_right - chip_size.x, rect.top() + 5.0),
        chip_size,
    );
    let age_min = egui::pos2(chip_rect.left() - 6.0 - age_size.x, rect.top() + 7.0);
    paint_state_chip(painter, chip_rect, agent.state);
    painter.galley(
        age_min,
        painter.layout_no_wrap(age_text, age_font, theme::ui::TEXT_MUTED),
        theme::ui::TEXT_MUTED,
    );

    // Line one, name: everything left of the age.
    let name_max = age_min.x - 6.0 - inner_left;
    let name_font = FontId::proportional(13.0);
    let name_color = if selected {
        theme::ui::INK
    } else {
        theme::ui::TEXT_STRONG
    };
    let name = truncate_text(painter, &label, name_font.clone(), name_max);
    painter.galley(
        egui::pos2(inner_left, rect.top() + 6.0),
        painter.layout_no_wrap(name, name_font, name_color),
        name_color,
    );

    // Line two: repo · branch, with the pane ref trailing right.
    let mut body = Vec::new();
    if show_repo && let Some(repo) = agent.repo() {
        body.push(repo.to_string());
        full.push_str(&format!(" · {repo}"));
    }
    if let Some(branch) = agent.workspace.branch.as_deref().filter(|b| !b.is_empty()) {
        body.push(branch.to_string());
        full.push_str(&format!(" · {branch}"));
    }
    let body_text = if body.is_empty() {
        "no workspace".to_string()
    } else {
        body.join(" · ")
    };
    let body_font = FontId::monospace(10.0);
    let pane = agent.pane_reference();
    let pane_width = pane.as_ref().map_or(0.0, |p| {
        painter
            .layout_no_wrap(
                format!("pane {p}"),
                body_font.clone(),
                theme::ui::TEXT_MUTED,
            )
            .size()
            .x
    });
    let body_max = (inner_right - pane_width - 6.0 - inner_left).max(0.0);
    let body_text = truncate_text(painter, &body_text, body_font.clone(), body_max);
    painter.galley(
        egui::pos2(inner_left, rect.bottom() - 15.0),
        painter.layout_no_wrap(body_text, body_font.clone(), theme::ui::TEXT_MUTED),
        theme::ui::TEXT_MUTED,
    );
    if let Some(pane) = pane {
        let pane_text = format!("pane {pane}");
        painter.galley(
            egui::pos2(inner_right - pane_width, rect.bottom() - 15.0),
            painter.layout_no_wrap(pane_text, body_font, theme::ui::TEXT_MUTED),
            theme::ui::TEXT_MUTED,
        );
        full.push_str(&format!(" · pane {pane}"));
    }
    full
}

/// Render one section's rows (used by native master and web board). Returns
/// the clicked agent id, if any.
fn section_rows(
    ui: &mut Ui,
    fleet: &Fleet,
    agent_ids: &[String],
    selected: Option<&str>,
    now: u64,
    show_repo: bool,
    row_id_salt: &str,
) -> Option<String> {
    let mut clicked = None;
    let full_width = ui.available_width();
    for agent_id in agent_ids {
        let Some(agent) = fleet.agents.get(agent_id) else {
            continue;
        };
        let row_id = ui.id().with((row_id_salt, agent_id));
        let (rect, _) = ui.allocate_exact_size(Vec2::new(full_width, ROW_HEIGHT), Sense::hover());
        let response = ui.interact(rect, row_id, Sense::click());
        let is_selected = selected == Some(agent_id.as_str());
        let was_clicked = response.clicked();
        if was_clicked {
            clicked = Some(agent_id.clone());
        }
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(6), theme::ui::PANEL2);
        }
        let full = paint_agent_row(ui.painter(), rect, agent, is_selected, now, show_repo);
        if response.hovered() {
            response.on_hover_text(full);
        }
    }
    clicked
}

fn section_header(ui: &mut Ui, text: &str, accent: Option<Color32>) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let color = accent.unwrap_or(theme::ui::TEXT_MUTED);
        ui.label(RichText::new(text).small().strong().color(color));
        ui.add_space(2.0);
        ui.separator();
    });
    ui.add_space(1.0);
}

/// Render the read-only board inside `ui` (native master pane or the WASM
/// board surface). Returns the agent id the user clicked, if any.
pub fn show_board(
    ui: &mut Ui,
    fleet: &Fleet,
    conn: ConnState,
    conn_detail: Option<&str>,
    selected: Option<&str>,
    show_repo: bool,
    row_id_salt: &str,
) -> Option<String> {
    let now = now_millis();
    let mut clicked = None;

    // Chrome: title + count + connection state.
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("fleet board")
                .heading()
                .strong()
                .color(theme::ui::TEXT_STRONG),
        );
        if !fleet.agents.is_empty() {
            ui.label(
                RichText::new(format!("{} agents", fleet.agents.len()))
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            crate::ui::connection_pill(ui, conn);
        });
    });
    if let Some(notice) = connection_notice(conn, conn_detail) {
        ui.add_space(2.0);
        ui.label(
            RichText::new(format!("⚠ {notice}"))
                .small()
                .color(theme::ui::WARN),
        );
    }
    ui.add_space(4.0);

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

    let board = sections(fleet);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !board.blocked.is_empty() {
                section_header(
                    ui,
                    &format!("blocked ({})", board.blocked.len()),
                    Some(theme::state::BLOCKED),
                );
                if let Some(id) = section_rows(
                    ui,
                    fleet,
                    &board.blocked,
                    selected,
                    now,
                    show_repo,
                    &format!("{row_id_salt}-blocked"),
                ) {
                    clicked = Some(id);
                }
            }
            for section in &board.repos {
                section_header(ui, &section.header(), None);
                if let Some(id) = section_rows(
                    ui,
                    fleet,
                    &section.agent_ids,
                    selected,
                    now,
                    show_repo,
                    row_id_salt,
                ) {
                    clicked = Some(id);
                }
            }
        });
    clicked
}

// ---------------------------------------------------------------------------
// Recents v1 drill-in (live tail)
// ---------------------------------------------------------------------------

fn tail_row_color(kind: crate::drive::CanonicalBlockKind) -> Color32 {
    match kind {
        crate::drive::CanonicalBlockKind::User => theme::ui::USER_BLUE,
        crate::drive::CanonicalBlockKind::Agent => theme::ui::INK,
        crate::drive::CanonicalBlockKind::Tool
        | crate::drive::CanonicalBlockKind::System
        | crate::drive::CanonicalBlockKind::Unknown => theme::ui::TEXT_MUTED,
    }
}

/// Recents v1: LIVE TAIL ONLY. Renders the cached tail (canonical blocks
/// when the daemon sent them, else the legacy lines) with auto-scroll; the
/// caller owns the refresh pacing (single-flight + cooldown) and passes
/// `retry` for the error state's Retry action. No partition, no
/// load-earlier, no composer.
pub fn show_recents(
    ui: &mut Ui,
    agent: &Agent,
    rows: &[TailRow],
    phase: RecentsPhase,
    live: bool,
    retry: &mut dyn FnMut(),
) {
    let now = now_millis();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(agent.row_label())
                .strong()
                .color(theme::ui::TEXT_STRONG),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Right→left cluster: age, state chip, live dot.
            let age_text = crate::model::relative_age(agent.ts, now);
            ui.label(RichText::new(age_text).small().color(theme::ui::TEXT_MUTED));
            let (color, _) = state_style(agent.state);
            let chip_text = state_chip_text(agent.state);
            let font = FontId::proportional(11.0);
            let galley = ui.fonts_mut(|fonts| fonts.layout_no_wrap(chip_text, font, color));
            let size = galley.size() + Vec2::new(14.0, 6.0);
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
            painter.galley(rect.min + Vec2::new(7.0, 3.0), galley, color);
            if live {
                ui.label(RichText::new("● live").small().color(theme::ui::GOOD));
            }
        });
    });
    // repo · branch · pane context line above the tail.
    let mut body: Vec<String> = Vec::new();
    if let Some(repo) = agent.repo() {
        body.push(repo.to_string());
    }
    if let Some(branch) = agent.workspace.branch.as_deref().filter(|b| !b.is_empty()) {
        body.push(branch.to_string());
    }
    if let Some(pane) = agent.pane_reference() {
        body.push(format!("pane {pane}"));
    }
    let context = if body.is_empty() {
        agent.agent_id.clone()
    } else {
        body.join(" · ")
    };
    ui.label(
        RichText::new(context)
            .monospace()
            .small()
            .color(theme::ui::TEXT_MUTED),
    );
    ui.separator();

    match phase {
        RecentsPhase::Loading => {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("reading the tail…").color(theme::ui::TEXT_MUTED));
            });
        }
        RecentsPhase::Empty => {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("no output yet").color(theme::ui::TEXT_MUTED));
            });
        }
        RecentsPhase::Error(message) => {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(format!("⚠ {message}"))
                        .small()
                        .color(theme::ui::WARN),
                );
                if ui.button("retry").clicked() {
                    retry();
                }
            });
        }
        RecentsPhase::Loaded => {
            let available_height = ui.available_height().max(120.0);
            egui::ScrollArea::vertical()
                .id_salt("corral-ui-recents-tail")
                .max_height(available_height)
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for row in rows {
                        let color = tail_row_color(row.kind);
                        for line in row.text.lines() {
                            ui.label(RichText::new(line).monospace().size(12.0).color(color));
                        }
                        ui.add_space(2.0);
                    }
                });
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::CanonicalBlockKind;
    use crate::model::{Attachment, Workspace};

    fn agent(id: &str, state: AgentState, repo: Option<&str>, ts: u64) -> Agent {
        Agent {
            agent_id: id.into(),
            source: "herdr".into(),
            tool: "claude".into(),
            state,
            reason: None,
            seq: 0,
            ts,
            capabilities: vec!["read_tail".into()],
            workspace: Workspace {
                repo: repo.map(str::to_string),
                branch: Some("g-main".into()),
                ..Default::default()
            },
            attachment: Some(Attachment {
                kind: "pane".into(),
                reference: format!("herdr:pane:{id}:p1"),
            }),
            display_name: Some(id.into()),
            title: None,
        }
    }

    fn fleet_of(agents: Vec<Agent>) -> Fleet {
        let mut fleet = Fleet::default();
        for agent in agents {
            fleet.agents.insert(agent.agent_id.clone(), agent);
        }
        fleet
    }

    #[test]
    fn attention_rank_orders_blocked_working_idle_done_with_idle_and_unknown() {
        assert_eq!(state_rank(AgentState::Blocked), 0);
        assert_eq!(state_rank(AgentState::Working), 1);
        assert_eq!(state_rank(AgentState::Idle), 2);
        assert_eq!(
            state_rank(AgentState::Done),
            2,
            "wire done ranks with idle (treated as finished)"
        );
        assert_eq!(state_rank(AgentState::Unknown), 3);
    }

    #[test]
    fn sections_group_by_repo_pin_blocked_and_keep_last_done_in_repo() {
        let fleet = fleet_of(vec![
            agent("herdr:a", AgentState::Blocked, Some("zeta"), 5),
            agent("herdr:b", AgentState::Working, Some("zeta"), 4),
            agent("herdr:c", AgentState::Idle, Some("zeta"), 3),
            agent("herdr:d", AgentState::Working, Some("alpha"), 6),
            agent("herdr:e", AgentState::Unknown, None, 2),
        ]);
        let board = sections(&fleet);
        assert_eq!(
            board.blocked,
            vec!["herdr:a".to_string()],
            "blocked pinned on top"
        );
        assert_eq!(
            board
                .repos
                .iter()
                .map(|s| s.repo.clone())
                .collect::<Vec<_>>(),
            vec![Some("alpha".to_string()), Some("zeta".to_string()), None],
            "named repos sort first by name; orphan bucket last"
        );
        assert_eq!(board.repos[0].header(), "alpha (1)");
        assert_eq!(board.repos[1].header(), "zeta (3)");
        assert_eq!(board.repos[2].header(), "no repo (1)");
        // zeta's attention order: blocked first, then working, then the
        // idle (finished-rank) agent stays in its repo — retention.
        assert_eq!(
            board.repos[1].agent_ids,
            vec![
                "herdr:a".to_string(),
                "herdr:b".to_string(),
                "herdr:c".to_string()
            ]
        );
        assert_eq!(board.total_agents(), 5);
    }

    #[test]
    fn ordering_ties_break_on_ts_desc_then_agent_id() {
        let fleet = fleet_of(vec![
            agent("herdr:z", AgentState::Idle, Some("r"), 10),
            agent("herdr:a", AgentState::Idle, Some("r"), 10),
            agent("herdr:m", AgentState::Idle, Some("r"), 20),
        ]);
        let board = sections(&fleet);
        assert_eq!(
            board.repos[0].agent_ids,
            vec![
                "herdr:m".to_string(),
                "herdr:a".to_string(),
                "herdr:z".to_string()
            ],
            "newer ts first; equal ts sorted by agent id"
        );
    }

    #[test]
    fn empty_fleet_produces_empty_sections() {
        let board = sections(&Fleet::default());
        assert!(board.blocked.is_empty());
        assert!(board.repos.is_empty());
    }

    #[test]
    fn tail_rows_fall_back_to_unknown_lines_and_merge_adjacent_tool_system() {
        let lines = vec!["alpha".to_string(), "  ".to_string(), "beta".to_string()];
        let rows = tail_rows(&lines, &[]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, CanonicalBlockKind::Unknown);
        assert_eq!(rows[0].text, "alpha");
        assert_eq!(rows[1].text, "beta");

        let blocks = vec![
            CanonicalBlock {
                kind: CanonicalBlockKind::Tool,
                text: "tool a\n\n  ".into(),
                prompt_request_id: None,
            },
            CanonicalBlock {
                kind: CanonicalBlockKind::Tool,
                text: "tool b".into(),
                prompt_request_id: None,
            },
            CanonicalBlock {
                kind: CanonicalBlockKind::Agent,
                text: "agent line".into(),
                prompt_request_id: None,
            },
            CanonicalBlock {
                kind: CanonicalBlockKind::System,
                text: "system".into(),
                prompt_request_id: None,
            },
        ];
        let rows = tail_rows(&[], &blocks);
        assert_eq!(rows.len(), 3, "adjacent tool blocks merge into one row");
        assert_eq!(rows[0].text, "tool a\ntool b");
        assert_eq!(rows[1].kind, CanonicalBlockKind::Agent);
        assert_eq!(rows[2].kind, CanonicalBlockKind::System);
    }

    #[test]
    fn recents_phase_covers_loading_loaded_error_empty() {
        let mut fleet = fleet_of(vec![agent("herdr:a", AgentState::Working, Some("r"), 1)]);
        assert_eq!(
            recents_phase(&fleet, "herdr:a", true),
            RecentsPhase::Empty,
            "nothing cached and nothing in flight = first open"
        );
        fleet.remember_drive(
            "herdr:a",
            DriveState::Sending {
                request_id: "r".into(),
                capability: "read_tail".into(),
            },
        );
        assert_eq!(
            recents_phase(&fleet, "herdr:a", true),
            RecentsPhase::Loading
        );
        fleet.remember_tail_full("herdr:a", vec!["x".into()], Vec::new(), Some(1));
        assert_eq!(recents_phase(&fleet, "herdr:a", true), RecentsPhase::Loaded);
        let mut no_cap = fleet_of(vec![agent("herdr:a", AgentState::Working, Some("r"), 1)]);
        no_cap
            .agents
            .get_mut("herdr:a")
            .unwrap()
            .capabilities
            .clear();
        assert!(matches!(
            recents_phase(&no_cap, "herdr:a", false),
            RecentsPhase::Error(_)
        ));
    }

    #[test]
    fn recents_phase_error_surfaces_the_latest_failure() {
        let mut fleet = fleet_of(vec![agent("herdr:a", AgentState::Working, Some("r"), 1)]);
        fleet.remember_drive(
            "herdr:a",
            DriveState::Failed {
                failure: crate::drive::DriveFailure::NotGranted(
                    "capability not granted: read_tail".into(),
                ),
                capability: "read_tail".into(),
            },
        );
        assert!(matches!(
            recents_phase(&fleet, "herdr:a", true),
            RecentsPhase::Error(message) if message.contains("not granted")
        ));
    }

    #[test]
    fn connection_notice_keeps_last_known_board_honest() {
        assert!(connection_notice(ConnState::Connected, None).is_none());
        let text = connection_notice(ConnState::Down, None).unwrap();
        assert!(text.contains("last-known"));
        let text = connection_notice(ConnState::Reconnecting { backoff_ms: 500 }, None).unwrap();
        assert!(text.contains("last-known"));
        assert!(
            connection_notice(ConnState::Connecting, None)
                .unwrap()
                .contains("connecting")
        );
    }

    #[test]
    fn state_labels_are_raw_tokens_and_colors_follow_the_contract() {
        let (color, label) = state_style(AgentState::Blocked);
        assert_eq!(label, "blocked");
        assert_eq!(color, theme::state::BLOCKED);
        let (_, label) = state_style(AgentState::Done);
        assert_eq!(label, "done");
        assert_eq!(state_chip_text(AgentState::Working), "○ working");
    }
}
