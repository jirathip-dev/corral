//! The read-only WASM board (#215, #354 L3, #304): the v2 board renders
//! `/snapshot` + `/events` SSE (and demo data out of the box). There is NO
//! signed drive from the browser — the demo board feeds its recents through
//! the same cache shape the desktop client uses, and live mode explains the
//! desktop-only read path.
//!
//! #354 L3 cut: the Issues tab is gone and the board is the repo-grouped
//! read-only surface shared with the native client. #304: phone layout uses
//! the full-width drill-in pattern (board list primary; tapping a row opens
//! that agent's recents full-width with a Back action) — no permanent empty
//! detail column.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use eframe::egui::{self, RichText};

use crate::demo;
use crate::drive::CanonicalBlock;
use crate::model::{Delta, Snapshot};
use crate::protocol::{self, DEFAULT_HOST_URL, SseParser};
use crate::state::{ConnState, Fleet};
use crate::theme;

const STORAGE_KEY: &str = "corral_web_setup_v1";
const DEMO_STEP_INTERVAL: f64 = 4.0;

/// Where the board gets its data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebMode {
    Demo,
    Live,
}

/// Persisted setup: mode + daemon base URL (localStorage).
#[derive(Debug, Clone, PartialEq, Eq)]
struct WebSetup {
    mode: WebMode,
    base_url: String,
}

impl WebSetup {
    fn load() -> Option<Self> {
        let raw = storage()?.get_item(STORAGE_KEY).ok()??;
        let value = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
        let mode = match value.get("mode")?.as_str()? {
            "demo" => WebMode::Demo,
            "live" => WebMode::Live,
            _ => return None,
        };
        Some(Self {
            mode,
            base_url: value.get("base_url")?.as_str()?.to_string(),
        })
    }

    fn save(&self) {
        let value = serde_json::json!({
            "mode": match self.mode { WebMode::Demo => "demo", WebMode::Live => "live" },
            "base_url": self.base_url,
        });
        if let Some(storage) = storage() {
            let _ = storage.set_item(STORAGE_KEY, &value.to_string());
        }
    }
}

/// A transient web toast.
struct WebToast {
    text: String,
    level: crate::state::Level,
    at_ms: f64,
}

/// Milliseconds since the epoch (web clock; std time panics on wasm).
fn now_ms() -> f64 {
    js_sys::Date::now()
}

fn toast_area(ctx: &egui::Context, toasts: &mut VecDeque<WebToast>) {
    toasts.retain(|t| now_ms() - t.at_ms < 10_000.0);
    if toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("corral-web-toasts"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 48.0))
        .show(ctx, |ui| {
            ui.set_max_width(360.0);
            for toast in toasts.iter() {
                let color = match toast.level {
                    crate::state::Level::Info => theme::ui::GOOD,
                    crate::state::Level::Warn => theme::ui::WARN,
                    crate::state::Level::Error => theme::ui::BAD,
                };
                egui::Frame::popup(ui.style())
                    .fill(theme::ui::PANEL3)
                    .stroke(egui::Stroke::new(1.0, color))
                    .corner_radius(egui::CornerRadius::same(6))
                    .show(ui, |ui| {
                        ui.label(RichText::new(&toast.text).color(theme::ui::TEXT_STRONG));
                    });
                ui.add_space(4.0);
            }
        });
}

/// localStorage is unavailable on exotic setups; the app falls back to
/// defaults silently (nothing about the read view requires persistence).
fn storage() -> Option<web_sys::Storage> {
    let window = web_sys::window()?;
    window.local_storage().ok().flatten()
}

/// Messages from the wasm-local background tasks to the UI (the UI is the
/// only owner of the view model; everything is plain wasm single-thread).
enum InboxMsg {
    Snapshot(Snapshot),
    Delta(Delta),
    Conn(ConnState),
    Toast(String),
}

/// The read-only web board.
pub struct WebCorralApp {
    ctx: egui::Context,
    fleet: Fleet,
    conn: ConnState,
    toasts: VecDeque<WebToast>,
    client: reqwest::Client,
    setup: WebSetup,
    /// First-open shows the setup panel; the header keeps a "setup" button
    /// to reopen it (mode/URL changes take effect immediately on apply).
    setup_open: bool,
    inbox: Rc<RefCell<VecDeque<InboxMsg>>>,
    /// The agent whose recents drill-in is open (`None` = the board).
    selected: Option<String>,
    /// Latest live-loop generation; loops check it to yield after a
    /// host/mode switch.
    live_generation: u64,
    current_generation: Rc<std::cell::Cell<u64>>,
    demo: Option<demo::DemoData>,
    demo_step: usize,
    demo_last: f64,
}

impl WebCorralApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(theme::dark_dashboard());
        let mut style = (*cc.egui_ctx.style_of(egui::Theme::Dark)).clone();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 3.0);
        cc.egui_ctx.set_style_of(egui::Theme::Dark, style);

        let setup = WebSetup::load().unwrap_or(WebSetup {
            mode: WebMode::Demo,
            base_url: DEFAULT_HOST_URL.to_string(),
        });
        let setup_open = WebSetup::load().is_none();
        let client = reqwest::Client::new();

        let mut app = WebCorralApp {
            ctx: cc.egui_ctx.clone(),
            fleet: Fleet::default(),
            conn: ConnState::Connecting,
            toasts: VecDeque::new(),
            client,
            setup,
            setup_open,
            inbox: Rc::new(RefCell::new(VecDeque::new())),
            selected: None,
            live_generation: 0,
            current_generation: Rc::new(std::cell::Cell::new(0)),
            demo: None,
            demo_step: 0,
            demo_last: now_ms(),
        };
        match app.setup.mode {
            WebMode::Demo => app.load_demo(),
            WebMode::Live => app.start_live(),
        }
        app
    }

    /// Load the bundled fixture into the view model (demo mode).
    fn load_demo(&mut self) {
        self.demo = Some(demo::load());
        if let Some(data) = &self.demo {
            self.fleet = Fleet::default();
            self.fleet.apply_snapshot(&data.snapshot);
            // The fixture's recents feed the SAME cache shape as live
            // read_tail results, so the drill-in renders identically.
            for agent in data.snapshot.agents.values() {
                if agent.can_read_tail() {
                    self.fleet.remember_tail_full(
                        &agent.agent_id,
                        demo::recent_tail(),
                        demo::recent_tail_blocks(),
                        Some(data.snapshot.rev),
                    );
                }
            }
        }
        self.demo_step = 0;
        self.demo_last = now_ms();
        self.conn = ConnState::Connected;
    }

    /// Apply the setup panel: persist and switch modes.
    fn apply_setup(&mut self, mode: WebMode, base_url: String) {
        self.setup.mode = mode;
        self.setup.base_url = base_url;
        self.setup.save();
        self.setup_open = false;
        self.live_generation += 1;
        self.current_generation.set(self.live_generation);
        match mode {
            WebMode::Demo => {
                self.fleet = Fleet::default();
                self.toasts.clear();
                self.load_demo();
            }
            WebMode::Live => self.start_live(),
        }
        self.ctx.request_repaint();
    }

    /// Start (or restart) the live read loop on a wasm local executor.
    fn start_live(&mut self) {
        self.fleet = Fleet::default();
        self.selected = None;
        self.conn = ConnState::Connecting;
        self.live_generation += 1;
        let generation = self.live_generation;
        self.current_generation.set(generation);

        let client = self.client.clone();
        let base_url = self.setup.base_url.clone();
        let inbox = self.inbox.clone();
        let ctx = self.ctx.clone();
        let current = self.current_generation.clone();
        let mut backoff_ms = protocol::SSE_BACKOFF_BASE_MS;
        // Last-Event-ID cursor: persists across reconnects so the daemon
        // resumes from the newest rev instead of re-sending the world.
        let mut last_rev: Option<u64> = None;

        wasm_bindgen_futures::spawn_local(async move {
            loop {
                if current.get() != generation {
                    return;
                }
                // 1) Fresh snapshot first (also the reconnect recovery).
                match protocol::fetch_snapshot(&client, &base_url).await {
                    Ok(snapshot) => {
                        backoff_ms = protocol::SSE_BACKOFF_BASE_MS;
                        let _ = inbox.borrow_mut().push_back(InboxMsg::Snapshot(snapshot));
                        let _ = inbox
                            .borrow_mut()
                            .push_back(InboxMsg::Conn(ConnState::Connected));
                    }
                    Err(error) => {
                        let _ = inbox
                            .borrow_mut()
                            .push_back(InboxMsg::Conn(ConnState::Reconnecting { backoff_ms }));
                        let _ = inbox
                            .borrow_mut()
                            .push_back(InboxMsg::Toast(format!("connect: {error}")));
                    }
                }
                ctx.request_repaint();

                // 2) Long-lived SSE stream with Last-Event-ID resume.
                match protocol::open_events(&client, &base_url, last_rev).await {
                    Ok(response) => {
                        use futures_util::StreamExt;
                        let mut stream = response.bytes_stream();
                        let mut parser = SseParser::default();
                        loop {
                            match stream.next().await {
                                Some(Ok(chunk)) => {
                                    let mut changed = false;
                                    for raw in parser.push(&chunk) {
                                        match protocol::parse_frame(&raw) {
                                            protocol::SseEvent::Snapshot(snapshot) => {
                                                last_rev = Some(snapshot.rev);
                                                let _ = inbox
                                                    .borrow_mut()
                                                    .push_back(InboxMsg::Snapshot(snapshot));
                                                changed = true;
                                            }
                                            protocol::SseEvent::Delta(delta) => {
                                                last_rev = Some(delta.rev);
                                                let _ = inbox
                                                    .borrow_mut()
                                                    .push_back(InboxMsg::Delta(delta));
                                                changed = true;
                                            }
                                            protocol::SseEvent::Unknown { .. } => {}
                                        }
                                    }
                                    if changed {
                                        ctx.request_repaint();
                                    }
                                }
                                Some(Err(error)) => {
                                    let _ = inbox.borrow_mut().push_back(InboxMsg::Toast(format!(
                                        "stream ended: {error}"
                                    )));
                                    break;
                                }
                                None => break,
                            }
                        }
                    }
                    Err(error) => {
                        let _ = inbox
                            .borrow_mut()
                            .push_back(InboxMsg::Toast(format!("SSE: {error}")));
                    }
                }

                // 3) Reconnect policy: doubling backoff, capped; the
                // generation check stops a superseded loop (mode switch).
                let _ = inbox
                    .borrow_mut()
                    .push_back(InboxMsg::Conn(ConnState::Reconnecting { backoff_ms }));
                ctx.request_repaint();
                gloo_timers::future::TimeoutFuture::new(backoff_ms as u32).await;
                backoff_ms = (backoff_ms * 2).min(protocol::SSE_BACKOFF_MAX_MS);
            }
        });
        self.ctx.request_repaint();
    }

    fn push_toast(&mut self, text: impl Into<String>, level: crate::state::Level) {
        self.toasts.push_back(WebToast {
            text: text.into(),
            level,
            at_ms: now_ms(),
        });
        if self.toasts.len() > 8 {
            self.toasts.pop_front();
        }
    }

    /// Fold background-task messages into the view model.
    fn drain_inbox(&mut self) {
        loop {
            let message = self.inbox.borrow_mut().pop_front();
            match message {
                Some(InboxMsg::Snapshot(snapshot)) => {
                    self.fleet.apply_snapshot(&snapshot);
                    self.conn = ConnState::Connected;
                }
                Some(InboxMsg::Delta(delta)) => {
                    self.fleet.apply_delta(&delta);
                    self.conn = ConnState::Connected;
                }
                Some(InboxMsg::Conn(conn)) => self.conn = conn,
                Some(InboxMsg::Toast(text)) => self.push_toast(text, crate::state::Level::Warn),
                None => break,
            }
        }
        // A dropped/unknown selection must not strand the drill-in.
        if let Some(selected) = self.selected.clone() {
            if !self.fleet.agents.contains_key(&selected) {
                self.selected = None;
            }
        }
    }

    /// Demo mode: apply one canned SSE frame every few seconds, wrapped.
    fn demo_tick(&mut self) {
        let now = now_ms();
        if now - self.demo_last < DEMO_STEP_INTERVAL * 1000.0 {
            return;
        }
        self.demo_last = now;
        let Some(demo) = &self.demo else {
            return;
        };
        if demo.deltas.is_empty() {
            return;
        }
        let delta = demo.deltas[self.demo_step % demo.deltas.len()].clone();
        self.demo_step += 1;
        self.fleet.apply_delta(&delta);
        self.ctx.request_repaint();
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("corral fleet")
                    .heading()
                    .strong()
                    .color(theme::ui::TEXT_STRONG),
            );
            crate::ui::badge(ui, "read-only", theme::ui::WARN);
            match self.setup.mode {
                WebMode::Demo => {
                    crate::ui::badge(ui, "demo data", theme::ui::ACCENT);
                }
                WebMode::Live => {
                    crate::ui::badge(ui, "live daemon", theme::ui::ACCENT);
                    crate::ui::connection_pill(ui, self.conn);
                    ui.label(
                        RichText::new(&self.setup.base_url)
                            .monospace()
                            .small()
                            .color(theme::ui::TEXT_MUTED),
                    );
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("setup").clicked() {
                    self.setup_open = true;
                }
            });
        });
        // The read-only boundary is stated where the action would be:
        // always visible, no hover required.
        ui.label(
            RichText::new(
                "This build serves corrald's read plane only — no signed drive from the \
                 browser. Recents on the desktop client read through the device-signed \
                 read_tail path.",
            )
            .small()
            .color(theme::ui::TEXT_MUTED),
        );
    }

    /// The board view (primary) and the recents drill-in (#304 full-width
    /// pattern: no permanent detail column; Back returns to the board).
    fn content(&mut self, ui: &mut egui::Ui) {
        let open = self.selected.clone();
        if let Some(agent_id) = open {
            self.recents_drill_in(ui, &agent_id);
        } else {
            self.board_view(ui);
        }
    }

    fn board_view(&mut self, ui: &mut egui::Ui) {
        let clicked =
            crate::ui::board::show_board(ui, &self.fleet, self.conn, None, None, true, "web-row");
        if let Some(agent_id) = clicked {
            // Demo mode feeds the recents from the fixture; live recents
            // explain the desktop-only read path inside the drill-in.
            self.selected = Some(agent_id);
        }
        if self.selected.is_none() && !self.fleet.agents.is_empty() {
            ui.add_space(6.0);
            ui.label(
                RichText::new("tap an agent to read its recent output")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        }
    }

    fn recents_drill_in(&mut self, ui: &mut egui::Ui, agent_id: &str) {
        if ui.button("← fleet board").clicked() {
            self.selected = None;
            return;
        }
        ui.separator();
        let Some(agent) = self.fleet.agents.get(agent_id).cloned() else {
            self.selected = None;
            return;
        };
        match self.setup.mode {
            WebMode::Demo => {
                // The fixture's cached tail renders through the exact same
                // read-model cache as the desktop client's live results.
                let lines: &[String] = self
                    .fleet
                    .tails
                    .get(agent_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let blocks: &[CanonicalBlock] = self
                    .fleet
                    .tail_blocks
                    .get(agent_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let rows = crate::ui::board::tail_rows(lines, blocks);
                let phase =
                    crate::ui::board::recents_phase(&self.fleet, agent_id, agent.can_read_tail());
                let mut noop = || {};
                crate::ui::board::show_recents(ui, &agent, &rows, phase, true, &mut noop);
            }
            WebMode::Live => {
                // No signed drive exists in the browser build (#215): the
                // live drill-in explains the boundary instead of pretending.
                crate::ui::board::show_recents(
                    ui,
                    &agent,
                    &[],
                    crate::ui::board::RecentsPhase::Error(
                        "live recent output needs the signed desktop client (read_tail)".into(),
                    ),
                    false,
                    &mut || {},
                );
            }
        }
    }

    /// First-open / reopen setup panel: mode + daemon base URL.
    fn setup_panel(&mut self, ctx: &egui::Context) {
        let mut mode = self.setup.mode;
        let mut url = self.setup.base_url.clone();
        let mut apply = false;
        egui::Window::new("corral fleet — read-only")
            .id(egui::Id::new("corral-web-setup"))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("Choose how the board gets its data:")
                        .color(theme::ui::TEXT_STRONG),
                );
                ui.add_space(6.0);
                ui.radio_value(&mut mode, WebMode::Demo, "Demo data (no daemon needed)");
                ui.radio_value(
                    &mut mode,
                    WebMode::Live,
                    "Live daemon (a local corrald on THIS machine)",
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("daemon base URL")
                            .small()
                            .color(theme::ui::TEXT_MUTED),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut url)
                            .hint_text(DEFAULT_HOST_URL)
                            .desired_width(260.0),
                    );
                });
                ui.label(
                    RichText::new(
                        "Live mode reads /snapshot + /events SSE from the configured URL. The \
                         browser page runs on a different origin, so corrald must be started \
                         with --cors-origin <this site's origin> to allow the read; the demo \
                         mode needs nothing.",
                    )
                    .small()
                    .color(theme::ui::TEXT_MUTED),
                );
                ui.add_space(8.0);
                if ui.button("apply").clicked() {
                    if mode == WebMode::Live && !url_valid(&url) {
                        ui.label(
                            RichText::new("invalid URL — expected http://host:port")
                                .color(theme::ui::BAD),
                        );
                    } else {
                        apply = true;
                    }
                }
                if ui.small_button("cancel").clicked() {
                    self.setup_open = false;
                }
            });
        if apply {
            self.apply_setup(mode, url.trim_end_matches('/').to_string());
        }
    }
}

fn url_valid(url: &str) -> bool {
    let trimmed = url.trim();
    trimmed.starts_with("http://") || trimmed.starts_with("https://")
}

// eframe 0.36's `App` trait calls `ui` with the root `Ui` (plus an
// optional `logic` for hidden windows) — there is no `update`.
impl eframe::App for WebCorralApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Called while the window is hidden or occluded: keep the inbox
        // drained and the demo/live ticks alive so the next visible frame
        // shows fresh state (mirrors the desktop app's `update_logic`).
        let _ = ctx;
        self.drain_inbox();
        match self.setup.mode {
            WebMode::Demo => self.demo_tick(),
            WebMode::Live => {}
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_inbox();
        if self.setup.mode == WebMode::Demo {
            self.demo_tick();
        }
        // The board always renders (the demo board is populated out of the
        // box); the setup panel only floats above it on first open.
        egui::CentralPanel::default_margins().show(ui, |ui| {
            self.header(ui);
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| self.content(ui));
        });
        toast_area(&ctx, &mut self.toasts);
        if self.setup_open {
            self.setup_panel(&ctx);
        }
        // Keep the live board ticking even while no input arrives.
        if self.setup.mode == WebMode::Live {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
}
