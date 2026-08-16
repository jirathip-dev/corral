//! Fleet board: agent rows (state/reason/waiting_on kind badges), worktree
//! topology columns (repo/branch/dirty/ahead-behind), PR/CI columns, and
//! capability-driven drive controls rendered from `agent.capabilities` AND
//! the device's grant ledger.

use eframe::egui::{Color32, RichText, ScrollArea, TextEdit, Ui};

use crate::drive::{DriveIntent, DriveOutcome};
use crate::state::DriveState;
use crate::model::Agent;
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

/// Callbacks the board issues to the app (drive dispatch).
pub struct BoardActions<'a> {
    pub drive: &'a mut dyn FnMut(DriveIntent),
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

    let total_width = COL_AGENT
        + COL_STATE
        + COL_WAITING
        + COL_REPO
        + COL_BRANCH
        + COL_DIRTY
        + COL_AB
        + COL_PR
        + COL_CI
        + COL_DRIVE;

    let ids: Vec<String> = fleet.agents.keys().cloned().collect();
    ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(total_width);
            header(ui);
            ui.separator();
            let mut toggles: Vec<String> = Vec::new();
            for id in &ids {
                let Some(agent) = fleet.agents.get(id) else {
                    continue;
                };
                let is_expanded = fleet.is_expanded(id);
                let (clicked, _) =
                    agent_row(ui, agent, is_expanded, allowed, fleet, actions);
                if clicked {
                    toggles.push(id.clone());
                }
                if is_expanded {
                    detail(ui, agent, fleet);
                }
                ui.separator();
            }
            for id in &toggles {
                fleet.toggle_expanded(id);
            }
        });
}

fn header(ui: &mut Ui) {
    ui.horizontal(|ui| {
        header_cell(ui, COL_AGENT, "AGENT");
        header_cell(ui, COL_STATE, "STATE");
        header_cell(ui, COL_WAITING, "WAITING ON");
        header_cell(ui, COL_REPO, "REPO");
        header_cell(ui, COL_BRANCH, "BRANCH");
        header_cell(ui, COL_DIRTY, "DIRTY");
        header_cell(ui, COL_AB, "A/B");
        header_cell(ui, COL_PR, "PR");
        header_cell(ui, COL_CI, "CI");
        header_cell(ui, COL_DRIVE, "DRIVE");
    });
}

fn header_cell(ui: &mut Ui, width: f32, text: &str) {
    ui.add_sized(
        [width, 20.0],
        egui::Label::new(RichText::new(text).monospace().color(theme::ui::TEXT_MUTED)),
    );
}

#[allow(clippy::too_many_arguments)]
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
        .inner_margin(egui::Margin::symmetric(2, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                agent_cell(ui, agent);
                state_cell(ui, agent);
                waiting_cell(ui, agent);
                topology_cells(ui, agent);
                drive_cell(ui, agent, allowed, fleet, &mut actions.drive);
            })
        })
        .response;

    // Expand/collapse on a click anywhere in the row except on widgets
    // (widgets consume their own clicks, so a plain row click reaches here).
    if response.clicked() {
        expanded = true;
    }
    (expanded, response)
}

fn agent_cell(ui: &mut Ui, agent: &Agent) {
    ui.vertical(|ui| {
        ui.add_sized(
            [COL_AGENT - 8.0, 18.0],
            egui::Label::new(RichText::new(agent.display()).color(theme::ui::TEXT_STRONG)),
        );
        ui.add_sized(
            [COL_AGENT - 8.0, 14.0],
            egui::Label::new(
                RichText::new(format!("{} · {}", agent.source, agent.tool))
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            ),
        );
    });
}

fn state_cell(ui: &mut Ui, agent: &Agent) {
    ui.vertical(|ui| {
        badge(ui, agent.state.label(), state::of(agent.state.into()));
        if let Some(reason) = &agent.reason {
            let truncated: String = reason.chars().take(40).collect();
            ui.add_sized(
                [COL_STATE - 8.0, 30.0],
                egui::Label::new(
                    RichText::new(truncated).small().color(theme::ui::TEXT_MUTED),
                ),
            )
            .on_hover_text(reason);
        }
    });
}

fn waiting_cell(ui: &mut Ui, agent: &Agent) {
    ui.vertical(|ui| {
        let Some(w) = &agent.waiting_on else {
            ui.add_sized([COL_WAITING - 8.0, 18.0], egui::Label::new(""));
            return;
        };
        badge(ui, w.kind.label(), kind::of(w.kind.into()));
        let prompt_preview: String = w.prompt.chars().take(120).collect();
        ui.add_sized(
            [COL_WAITING - 8.0, 30.0],
            egui::Label::new(
                RichText::new(prompt_preview).small().color(theme::ui::TEXT_MUTED),
            )
            .truncate(),
        )
        .on_hover_text(w.prompt.clone());
    });
}

fn topology_cells(ui: &mut Ui, agent: &Agent) {
    let ws = &agent.workspace;
    let cell = |ui: &mut Ui, width: f32, text: String, color: Color32| {
        ui.add_sized(
            [width - 4.0, 18.0],
            egui::Label::new(RichText::new(text).monospace().small().color(color)),
        );
    };
    ui.vertical(|ui| {
        cell(ui, COL_REPO, ws.repo.clone().unwrap_or_else(|| "—".into()), theme::ui::TEXT_MUTED);
        cell(ui, COL_BRANCH, ws.branch.clone().unwrap_or_else(|| "—".into()), theme::ui::TEXT_STRONG);
        cell(ui, COL_DIRTY, if ws.dirty { "●".into() } else { "".into() }, theme::ui::DIRTY);
        let ab = if ws.ahead == 0 && ws.behind == 0 {
            "".to_string()
        } else {
            format!("+{}/−{}", ws.ahead, ws.behind)
        };
        cell(ui, COL_AB, ab, if ws.ahead > 0 { theme::ui::WARN } else { theme::ui::TEXT_MUTED });
        cell(ui, COL_PR, ws.pr_number.map(|n| format!("#{n}")).unwrap_or_else(|| "—".into()), theme::ui::TEXT_MUTED);
        match ws.ci_status {
            Some(status) => {
                ui.add_sized([COL_CI - 4.0, 18.0], egui::Label::new(RichText::new("")));
                badge(ui, status.label(), ci::of(status.into()));
            }
            None => cell(ui, COL_CI, "—".into(), theme::ui::TEXT_MUTED),
        }
    });
}

fn drive_cell(
    ui: &mut Ui,
    agent: &Agent,
    allowed: &dyn Fn(&str) -> bool,
    fleet: &Fleet,
    drive: &mut dyn FnMut(DriveIntent),
) {
    let rev = fleet.rev;
    ui.vertical(|ui| {
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
    });
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
    ui.label(RichText::new("approve:").small().color(theme::ui::TEXT_MUTED));
    for choice in &w.choices {
        let label: String = choice.chars().take(16).collect();
        if ui.small_button(label).clicked() {
            drive(DriveIntent::approve(agent, choice.clone(), rev));
        }
    }
}

/// Prompt input + Enter-to-send. The buffer lives in egui's temp memory
/// keyed by agent id (no per-frame churn in the fleet model).
fn prompt_widget(
    ui: &mut Ui,
    agent: &Agent,
    rev: Option<u64>,
    drive: &mut dyn FnMut(DriveIntent),
) {
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

fn detail(ui: &mut Ui, agent: &Agent, fleet: &Fleet) {
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
                if let Some(c) = agent.cost {
                    detail_kv(ui, "cost", &format!("${c:.2}"));
                }
                if let Some(wt) = &agent.workspace.worktree_path {
                    detail_kv(ui, "worktree", wt);
                }
            });
            if let Some(title) = &agent.title {
                ui.label(RichText::new(title).color(theme::ui::TEXT_STRONG));
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
                        ui.add(egui::Label::new(RichText::new(w.prompt.clone()).monospace()).wrap());
                    });
                ui.horizontal_wrapped(|ui| {
                    detail_kv(ui, "prompt_hash", &w.prompt_hash);
                    detail_kv(ui, "approval_id", &w.approval_id);
                });
            }
            if let Some(tail) = fleet.tails.get(&agent.agent_id) {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("read_tail output (v1 returns no tail text; wired for a future daemon result)")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
                egui::Frame::NONE
                    .fill(Color32::from_rgb(0x16, 0x1b, 0x22))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                            for line in tail {
                                ui.add(egui::Label::new(RichText::new(line).monospace().small()));
                            }
                        });
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
                // The drive dispatched + was audited, but corrald v1 never
                // returns tail text (DriveResponse.result is always None).
                ui.add_space(4.0);
                ui.label(
                    RichText::new("read_tail dispatched + audited; v1 returns no tail text (the herdr adapter fire-and-forgets agent.read)")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            }
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

/// Outcome display for a drive state (shared with tests).
pub fn drive_state_text(state: &DriveState) -> String {
    match state {
        DriveState::Sending {
            request_id,
            capability,
        } => format!("{capability} sending {request_id}"),
        DriveState::Ok { rev, capability } => format!("{capability} → ok  rev {rev}"),
        DriveState::Failed { failure, capability } => {
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
            cost: None,
            parent_id: None,
            host: None,
            workspace: Default::default(),
            attachment: None,
            display_name: None,
            title: None,
        }
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
}
