//! Fleet board: repo sections (CollapsingHeader, default open) with agent
//! rows beneath — state/reason/waiting_on kind badges, worktree topology
//! columns (repo/branch/dirty/ahead-behind), PR/CI columns, and
//! capability-driven drive controls
//! rendered from `agent.capabilities` AND the device's grant ledger.

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
const COL_WAITING: f32 = 320.0;
const COL_REPO: f32 = 130.0;
const COL_BRANCH: f32 = 160.0;
const COL_DIRTY: f32 = 46.0;
const COL_AB: f32 = 64.0;
const COL_PR: f32 = 56.0;
const COL_CI: f32 = 76.0;
const COL_DRIVE: f32 = 400.0;

/// Board columns in render order. Both the header and every agent row draw
/// from this one width source so labels and values start at identical x
/// positions.
const BOARD_COLUMNS: [(&str, f32); 10] = [
    ("AGENT", COL_AGENT),
    ("STATE", COL_STATE),
    ("WAITING ON", COL_WAITING),
    ("REPO", COL_REPO),
    ("BRANCH", COL_BRANCH),
    ("DIRTY", COL_DIRTY),
    ("A/B", COL_AB),
    ("PR", COL_PR),
    ("CI", COL_CI),
    ("DRIVE", COL_DRIVE),
];

/// Keep at least this much branch text even when the inferred marker is
/// unusually long; the marker segment is bounded to the remaining width.
const BRANCH_MIN_TEXT_WIDTH: f32 = 36.0;

/// Header for the bucket of agents without `workspace.repo` (sorts last).
const NO_REPO_LABEL: &str = "(no repo)";

/// egui temp-memory key for the flat-list toggle (default: grouped).
const FLAT_VIEW: &str = "corral-ui-board-flat";

/// Callbacks the board issues to the app (drive dispatch + #64
/// transcript page fetches). Both are the deferred-action pattern: the
/// board renders against `&Fleet`, so the app collects intents and acts
/// after `show` returns.
pub struct BoardActions<'a> {
    pub drive: &'a mut dyn FnMut(DriveIntent),
    pub transcript: &'a mut dyn FnMut(crate::transcript::TranscriptRequest),
    /// #113: ask the app to re-fetch the repo-level issue view.
    pub refresh_issues: &'a mut dyn FnMut(),
}

/// Render the fleet board.
pub fn show(
    ui: &mut Ui,
    fleet: &mut Fleet,
    allowed: &dyn Fn(&str) -> bool,
    actions: &mut BoardActions,
) {
    // #113: repo-level issue browser. It is independent of the agent rows —
    // it renders even when the fleet has no agents, so a just-connected
    // board can still show issues before any worktree exists.
    crate::ui::issues::show(
        ui,
        fleet,
        allowed,
        &mut *actions.drive,
        &mut *actions.refresh_issues,
    );

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

    let total_width: f32 = BOARD_COLUMNS.iter().map(|(_, width)| *width).sum();

    let ids: Vec<String> = fleet.agents.keys().cloned().collect();
    ui.horizontal(|ui| {
        let mut flat = flat_view(ui);
        if ui
            .checkbox(&mut flat, "flat sort")
            .on_hover_text("one flat list of every agent, instead of repo groups")
            .changed()
        {
            ui.ctx()
                .memory_mut(|m| m.data.insert_temp::<bool>(egui::Id::new(FLAT_VIEW), flat));
        }
    });
    ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(total_width);
            header(ui);
            ui.separator();
            let mut toggles: Vec<String> = Vec::new();
            if flat_view(ui) {
                for id in &ids {
                    board_row(ui, id, fleet, allowed, actions, &mut toggles);
                }
            } else {
                for group in group_by_repo(fleet) {
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
                            board_row(ui, id, fleet, allowed, actions, &mut toggles);
                        }
                    });
                    ui.separator();
                }
            }
            for id in &toggles {
                fleet.toggle_expanded(id);
            }
        });
}

fn flat_view(ui: &Ui) -> bool {
    ui.ctx().memory(|m| {
        m.data
            .get_temp::<bool>(egui::Id::new(FLAT_VIEW))
            .unwrap_or(false)
    })
}

/// One board section: a repo (or the "(no repo)" orphan bucket) and the
/// agent ids in it, in the fleet's stable ordering.
#[derive(Debug, PartialEq, Eq)]
pub struct RepoGroup<'a> {
    /// `None` = the orphan bucket (agents without `workspace.repo`).
    pub repo: Option<&'a str>,
    pub agent_ids: Vec<&'a str>,
}

/// Group agent ids by `workspace.repo`: named repos sorted by name, the
/// "(no repo)" bucket last. Within a group, ids keep the fleet's BTreeMap
/// ordering (stable across frames; unchanged by grouping).
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
) {
    let Some(agent) = fleet.agents.get(id) else {
        return;
    };
    let is_expanded = fleet.is_expanded(id);
    let (clicked, _) = agent_row(ui, agent, is_expanded, allowed, fleet, actions);
    if clicked {
        toggles.push(id.to_string());
    }
    if is_expanded {
        detail(ui, agent, fleet, allowed, actions);
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
    allowed: &dyn Fn(&str) -> bool,
    fleet: &Fleet,
    actions: &mut BoardActions,
) -> (bool, egui::Response) {
    let bg = if is_expanded {
        theme::ui::ACCENT_DIM.gamma_multiply(0.10)
    } else {
        Color32::TRANSPARENT
    };
    let mut expanded = false;
    let response = egui::Frame::NONE
        .fill(bg)
        // Keep only vertical padding: any left/right margin would shift row
        // cells against the header, which has none.
        .inner_margin(egui::Margin::symmetric(0, 4))
        .show(ui, |ui| {
            agent_row_cells(ui, agent, allowed, fleet, actions);
        })
        .response;

    // Expand/collapse on a click anywhere in the row except on widgets
    // (widgets consume their own clicks, so a plain row click reaches here).
    if response.clicked() {
        expanded = true;
    }
    (expanded, response)
}

fn agent_row_cells(
    ui: &mut Ui,
    agent: &Agent,
    allowed: &dyn Fn(&str) -> bool,
    fleet: &Fleet,
    actions: &mut BoardActions,
) -> [egui::Response; 10] {
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
            drive_cell(ui, agent, allowed, fleet, &mut actions.drive),
        ]
    })
    .inner
}

fn agent_cell(ui: &mut Ui, agent: &Agent) -> egui::Response {
    fixed_cell(ui, COL_AGENT, |ui| {
        ui.add_sized(
            [COL_AGENT - 8.0, 18.0],
            egui::Label::new(RichText::new(agent.display()).color(theme::ui::TEXT_STRONG))
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
        badge(ui, agent.state.label(), state::of(agent.state.into()));
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

fn drive_cell(
    ui: &mut Ui,
    agent: &Agent,
    allowed: &dyn Fn(&str) -> bool,
    fleet: &Fleet,
    drive: &mut dyn FnMut(DriveIntent),
) -> egui::Response {
    let rev = fleet.rev;
    fixed_cell(ui, COL_DRIVE, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.spacing_mut().item_spacing.y = 2.0;

            for cap in crate::drive::CAPABILITIES_ORDER {
                if !agent.capabilities.iter().any(|c| c == cap) {
                    continue;
                }
                match cap {
                    "prompt" => {
                        if allowed(cap) {
                            prompt_widget(ui, agent, rev, drive);
                        } else {
                            // F4: every agent-advertised capability renders
                            // SOMETHING — disabled with the reason, whether
                            // the ledger denies it or simply lacks it.
                            crate::ui::disabled_button_with_reason(
                                ui,
                                cap,
                                "not granted by host (read-only default) — refresh grants in Settings",
                            );
                        }
                    }
                    "approve" => {
                        if agent.waiting_on.is_none() {
                            continue;
                        }
                        if allowed(cap) {
                            approve_choices(ui, agent, rev, drive);
                        } else {
                            crate::ui::disabled_button_with_reason(
                                ui,
                                cap,
                                "not granted by host (read-only default) — refresh grants in Settings",
                            );
                        }
                    }
                    _ => {
                        if allowed(cap) {
                            if ui.small_button(cap).clicked() {
                                let intent = match cap {
                                    "interrupt" => DriveIntent::interrupt(&agent.agent_id, rev),
                                    "read_tail" => DriveIntent::read_tail(&agent.agent_id, rev),
                                    "kill" => DriveIntent::kill(&agent.agent_id, rev),
                                    _ => DriveIntent::attach(&agent.agent_id, rev),
                                };
                                drive(intent);
                            }
                        } else {
                            crate::ui::disabled_button_with_reason(
                                ui,
                                cap,
                                "not granted by host (read-only default) — refresh grants in Settings",
                            );
                        }
                    }
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
    })
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
            if let Some(tail) = fleet.tails.get(&agent.agent_id) {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("read_tail output (daemon-redacted, latest tap)")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
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
/// follows the cursor. Gated on the read_tail grant like every other
/// capability surface (review F5). Rows are virtualized (`show_rows`
/// with a pitch measured from what is actually drawn — review F3);
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
    if !allowed("read_tail") {
        ui.label(
            RichText::new("transcript needs the read_tail grant")
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        return;
    }
    let pane = fleet.transcripts.get(&agent.agent_id);
    let title = match pane {
        Some(p) if !p.session.is_empty() => format!("transcript — {}", p.session),
        _ => "transcript".to_string(),
    };
    let header = egui::CollapsingHeader::new(RichText::new(title).small())
        .id_salt(("corral-ui-transcript", &agent.agent_id))
        .default_open(false)
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

/// Pure decision: which capabilities render for an agent given the grant
/// ledger (tested in isolation, used by the row renderer).
pub fn renderable_capabilities(
    agent_caps: &[String],
    allowed: &dyn Fn(&str) -> bool,
) -> Vec<&'static str> {
    let mut out = Vec::new();
    for cap in crate::drive::CAPABILITIES_ORDER {
        if agent_caps.iter().any(|c| c == cap) && allowed(cap) {
            out.push(cap);
        }
    }
    out
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
        let groups = group_by_repo(&fleet);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].repo, Some("alpha"));
        assert_eq!(groups[0].agent_ids, vec!["herdr:a", "herdr:b"]);
        assert_eq!(groups[1].repo, Some("zeta"));
        assert_eq!(groups[1].agent_ids, vec!["herdr:z"]);
        assert_eq!(groups[2].repo, None, "orphan bucket is last");
        assert_eq!(groups[2].agent_ids, vec!["herdr:o"]);
    }

    #[test]
    fn group_by_repo_keeps_fleet_ordering_within_groups() {
        let mut fleet = Fleet::default();
        for (id, repo) in [
            ("herdr:c", Some("one")),
            ("herdr:a", Some("one")),
            ("herdr:b", Some("one")),
        ] {
            fleet.agents.insert(id.into(), agent_in_repo(id, repo));
        }
        let groups = group_by_repo(&fleet);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0].agent_ids,
            vec!["herdr:a", "herdr:b", "herdr:c"],
            "group order follows the fleet's BTreeMap order, not insertion"
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
            let mut actions = BoardActions {
                drive: &mut |_| {},
                transcript: &mut |_| {},
                refresh_issues: &mut || {},
            };
            let row = egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(0, 4))
                .show(ui, |ui| {
                    agent_row_cells(ui, &agent, &|_| false, &Fleet::default(), &mut actions)
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
    fn renderable_capabilities_intersects_agent_and_grants() {
        let agent = agent_with_caps(&["prompt", "interrupt", "kill"]);
        let allowed = |c: &str| c == "prompt" || c == "kill";
        let rendered = renderable_capabilities(&agent.capabilities, &allowed);
        assert_eq!(rendered, vec!["prompt", "kill"]);
    }

    #[test]
    fn renderable_capabilities_obeys_canonical_order() {
        let agent = agent_with_caps(&["attach", "prompt", "read_tail"]);
        let rendered = renderable_capabilities(&agent.capabilities, &|_| true);
        assert_eq!(rendered, vec!["prompt", "read_tail", "attach"]);
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
