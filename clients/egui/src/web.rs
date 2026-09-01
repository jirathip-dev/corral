//! Read-only web (wasm) board — the #215 web build of the corral fleet
//! board. It deliberately serves ONLY the daemon's credential-free read
//! plane (`GET /snapshot`, `GET /events` SSE; plus the read-only
//! `GET /issues` projection) and can also render a
//! bundled demo fixture with no daemon at all.
//!
//! Boundary (never narrowed, see the issue):
//!
//! - No `/drive`, no `/host-key`/`/step-up`, no `keyring`, no
//!   registration, no grant editor. Every write control is replaced by the
//!   disabled `read-only (web)` indicator (`BoardActions::read_only`), and
//!   the drive closure is a no-op.
//! - The base URL is user-configured (persisted to browser storage,
//!   default `http://127.0.0.1:8474`) — nothing is compiled into the wasm.
//! - Setup state (mode + URL) survives a refresh via `localStorage`.
//!
//! The live loop is a wasm-local task (`spawn_local`) — there is no tokio
//! runtime on wasm; the reconnect policy mirrors the desktop
//! `protocol::spawn_read_loop` (doubling backoff, capped; `Last-Event-ID`
//! resume via the same `SseParser`).

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use eframe::egui::{self, RichText};

use crate::demo::{self, DemoData};
use crate::model::{Delta, GhIssueRef, Snapshot};
use crate::protocol::{
    self, DEFAULT_HOST_URL, SSE_BACKOFF_BASE_MS, SSE_BACKOFF_MAX_MS, SseEvent, SseParser,
};
use crate::state::{ConnState, Fleet, Level};
use crate::theme;
use crate::ui::board::{self, BoardActions};

/// localStorage key for the setup state (#215 AC3: state survives refresh).
const STORAGE_KEY: &str = "corral_web_setup_v1";
/// Demo tick: one canned SSE frame every this many seconds.
const DEMO_STEP_INTERVAL: f64 = 3.0;
/// Issues/fleet-identity refresh cadence while live (mirrors the client's
/// ISSUES_REFRESH_INTERVAL).
const REFRESH_INTERVAL_SECS: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebMode {
    /// Bundled fixture — works on a static Pages deploy, no daemon.
    Demo,
    /// Live daemon on the user-configured base URL (loopback local).
    Live,
}

#[derive(Debug, Clone)]
struct WebSetup {
    mode: WebMode,
    base_url: String,
}

impl WebSetup {
    fn load() -> Option<Self> {
        let storage = storage()?;
        let raw = storage.get_item(STORAGE_KEY).ok()??;
        let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let mode = match value.get("mode").and_then(|m| m.as_str()) {
            Some("live") => WebMode::Live,
            _ => WebMode::Demo,
        };
        let base_url = value
            .get("base_url")
            .and_then(|u| u.as_str())
            .unwrap_or(DEFAULT_HOST_URL)
            .to_string();
        Some(WebSetup { mode, base_url })
    }

    fn save(&self) {
        let Some(storage) = storage() else {
            return;
        };
        let mode = match self.mode {
            WebMode::Demo => "demo",
            WebMode::Live => "live",
        };
        let value = serde_json::json!({ "mode": mode, "base_url": self.base_url });
        // localStorage quota failures are non-fatal: the app still runs,
        // it just forgets the setup on the next refresh.
        let _ = storage.set_item(STORAGE_KEY, &value.to_string());
    }
}

/// A transient UI message on the web board. `std::time::Instant` is NOT
/// available on wasm32-unknown-unknown (`time not implemented on this
/// platform` — Rust 1.97), so ages ride `performance.now()` millis.
struct WebToast {
    text: String,
    level: Level,
    at_ms: f64,
}

/// Millis since page load (the JS monotonic clock; same clock egui uses).
fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map(|performance| performance.now())
        .unwrap_or(0.0)
}

/// Render queued toasts in the top-right corner (mirrors
/// `crate::ui::toast_area` but with the web's own clock).
fn toast_area(ctx: &egui::Context, toasts: &mut VecDeque<WebToast>) {
    const LIFETIME_MS: f64 = 10_000.0;
    let now = now_ms();
    toasts.retain(|toast| now - toast.at_ms < LIFETIME_MS);
    if toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("corral-web-toasts"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 48.0))
        .show(ctx, |ui| {
            ui.set_max_width(420.0);
            for toast in toasts.iter() {
                let color = match toast.level {
                    Level::Info => theme::ui::GOOD,
                    Level::Warn => theme::ui::WARN,
                    Level::Error => theme::ui::BAD,
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
    Issues(Result<BTreeMap<String, Vec<GhIssueRef>>, String>),

    Conn(ConnState),
    Toast(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Board,
    Issues,
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
    tab: Tab,
    /// Latest live-loop generation; loops check it to yield after a
    /// host/mode switch.
    live_generation: u64,
    current_generation: Rc<std::cell::Cell<u64>>,
    demo: Option<DemoData>,
    demo_step: usize,
    demo_last: f64,
    issues_last_refresh: f64,
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
            tab: Tab::Board,
            live_generation: 0,
            current_generation: Rc::new(std::cell::Cell::new(0)),
            demo: None,
            demo_step: 0,
            demo_last: now_ms(),
            issues_last_refresh: now_ms(),
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
            self.settings.host_identity = data.snapshot.build_identity.clone();
            self.fleet.apply_snapshot(&data.snapshot);
            self.fleet.set_issues(Ok(data.issues.clone()));

            for agent in data.snapshot.agents.values() {
                if agent
                    .capabilities
                    .iter()
                    .any(|capability| capability == "read_tail")
                {
                    // #316 V3: the demo feeds the canonical blocks (when the
                    // fixture carries them) through the exact same cache as
                    // live results, so the Recent-output surface renders the
                    // real Conversation / Harness activity split.
                    self.fleet.remember_tail_full(
                        &agent.agent_id,
                        demo::recent_tail(),
                        demo::recent_tail_blocks(),
                        Some(data.snapshot.rev),
                    );
                }
            }
            // Select the fixture's first read_tail agent so the demo board
            // opens straight onto the V3 Recent-output detail (deterministic:
            // fixture order, not insertion order).
            if self.fleet.selected_agent.is_none() {
                if let Some(first) = data
                    .snapshot
                    .agents
                    .values()
                    .find(|agent| agent.capabilities.iter().any(|c| c == "read_tail"))
                {
                    self.fleet.select_agent(&first.agent_id);
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
        self.conn = ConnState::Connecting;
        self.live_generation += 1;
        let generation = self.live_generation;
        self.current_generation.set(generation);

        let client = self.client.clone();
        let base_url = self.setup.base_url.clone();
        let inbox = self.inbox.clone();
        let ctx = self.ctx.clone();
        let current = self.current_generation.clone();
        let mut backoff_ms = SSE_BACKOFF_BASE_MS;
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
                        backoff_ms = SSE_BACKOFF_BASE_MS;
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
                // reqwest's wasm backend streams via `bytes_stream()`
                // (no `chunk()` there, unlike native).
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
                                            SseEvent::Snapshot(snapshot) => {
                                                last_rev = Some(snapshot.rev);
                                                let _ = inbox
                                                    .borrow_mut()
                                                    .push_back(InboxMsg::Snapshot(snapshot));
                                                changed = true;
                                            }
                                            SseEvent::Delta(delta) => {
                                                last_rev = Some(delta.rev);
                                                let _ = inbox
                                                    .borrow_mut()
                                                    .push_back(InboxMsg::Delta(delta));
                                                changed = true;
                                            }
                                            // Forward-compatible: ignore event
                                            // types the read model has no
                                            // opinion on (same as desktop).
                                            SseEvent::Unknown { .. } => {}
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
                backoff_ms = (backoff_ms * 2).min(SSE_BACKOFF_MAX_MS);
            }
            // A superseded loop (host/mode switch) exits quietly.
        });
        self.ctx.request_repaint();
    }

    /// Periodic refresh of the read-only issues projection while live,
    /// mirroring the desktop client's cadence. The fleet-identity catalog is
    /// refreshed from the Issues tab too: it is the only remaining consumer
    /// (the #269 Fleets tab is gone) and the Issues view needs it to resolve
    /// repo categories into exact fleet-name drive targets.
    fn live_ticks(&mut self) {
        let now = now_ms();
        if self.tab == Tab::Issues
            && now - self.issues_last_refresh >= REFRESH_INTERVAL_SECS * 1000.0
        {
            self.issues_last_refresh = now;
            self.request_issues();
        }
    }

    fn request_issues(&mut self) {
        if self.fleet.issues_loading {
            return;
        }
        self.fleet.issues_loading = true;
        let client = self.client.clone();
        let base_url = self.setup.base_url.clone();
        let inbox = self.inbox.clone();
        let ctx = self.ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = protocol::fetch_issues(&client, &base_url).await;
            let _ = inbox.borrow_mut().push_back(InboxMsg::Issues(result));
            ctx.request_repaint();
        });
    }

    fn push_toast(&mut self, text: impl Into<String>, level: Level) {
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
                    self.settings.host_identity = snapshot.build_identity.clone();
                    self.fleet.apply_snapshot(&snapshot);
                    self.conn = ConnState::Connected;
                }
                Some(InboxMsg::Delta(delta)) => {
                    self.fleet.apply_delta(&delta);
                    self.conn = ConnState::Connected;
                }
                Some(InboxMsg::Issues(result)) => self.fleet.set_issues(result),

                Some(InboxMsg::Conn(conn)) => self.conn = conn,
                Some(InboxMsg::Toast(text)) => self.push_toast(text, Level::Warn),
                None => break,
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
            crate::ui::badge(ui, "read-only (web)", theme::ui::WARN);
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
                ui.selectable_value(&mut self.tab, Tab::Board, "Board");
                ui.selectable_value(&mut self.tab, Tab::Issues, "Issues");
            });
        });
        // The read-only boundary is stated where the action would be:
        // always visible, no hover required.
        ui.label(
            RichText::new(
                "This build serves corrald's read plane only — no /drive, no host-key/step-up, \
                 no keyring. Writes stay in the desktop client.",
            )
            .small()
            .color(theme::ui::TEXT_MUTED),
        );
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        let allowed = |_capability: &str| false;
        match self.tab {
            Tab::Board => {
                let mut actions = BoardActions {
                    drive: &mut |_intent| {
                        // Unreachable: read_only=true short-circuits every
                        // control before the drive callback.
                    },
                    read_only: true,
                };
                board::show(
                    ui,
                    &mut self.fleet,
                    crate::state::CompletedMode::Collapsed,
                    &allowed,
                    &mut actions,
                );
            }
            Tab::Issues => {
                let mut refresh_requested = false;
                crate::ui::issues::show(ui, &self.fleet, &allowed, &mut |_intent| {}, &mut || {
                    refresh_requested = true
                });
                if refresh_requested {
                    if self.setup.mode == WebMode::Live {
                        self.issues_last_refresh = now_ms();
                        self.request_issues();
                    } else {
                        self.push_toast(
                            "demo data — there is no daemon to refresh from",
                            Level::Info,
                        );
                    }
                }
            }
        }
    }

    /// First-open / reopen setup panel: mode + daemon base URL.
    fn setup_panel(&mut self, ctx: &egui::Context) {
        let mut mode = self.setup.mode;
        let mut url = self.setup.base_url.clone();
        let mut apply = false;
        egui::Window::new("corral fleet — read-only (web)")
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
                    // Keep whatever mode the stored setup last configured.
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
            WebMode::Live => self.live_ticks(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_inbox();
        match self.setup.mode {
            WebMode::Demo => self.demo_tick(),
            WebMode::Live => self.live_ticks(),
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
        // Keep the live board ticking even while the tab is idle.
        if self.setup.mode == WebMode::Live {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
}
