//! Registration + settings view: host URL, device registration (paste
//! token or localhost auto-register), device identity status (key store
//! + warnings), and host-side administration (device grants plus a
//!   subordinate audit surface).

use std::collections::BTreeSet;

use eframe::egui::{
    self, Color32, CornerRadius, FontId, RichText, Sense, Stroke, StrokeKind, TextEdit, Ui,
};

use crate::keys::KeyStore;
use crate::protocol::{GRANT_CAPABILITIES, GrantDevice};
use crate::state::{ConnState, Level};
use crate::theme;

pub struct SettingsState {
    pub host_url: String,
    pub auto_reconnect: bool,
    pub group_by_repo: bool,
    pub show_idle_collapsed: bool,
    pub stick_to_bottom: bool,
    pub theme: String,
    pub token_input: String,
    pub admin_token_input: String,
    pub notice: Option<(Level, String)>,
    /// Audit is reachable only below Advanced device access, never as a
    /// top-level workspace tab.
    pub audit_open: bool,
    /// Set by the view when the user asks for an action.
    pub requested: Option<Request>,
    /// Host-admin credential availability for the grant editor, refreshed
    /// by the app from the keychain/local daemon before rendering.
    pub admin_token_configured: bool,
    pub grant_admin: GrantAdminState,
}

pub enum Request {
    Connect,
    AutoRegister,
    Register,
    ReRegister,
    RefreshGrants,
    SaveAdminToken,
    ClearAdminToken,
    LoadGrantDevices,
    SelectGrantDevice(String),
    /// Toggle a capability on the selected device — applies immediately
    /// (the mockup's switch flips; no separate Apply button).
    ToggleGrantCap(String),
    ApplyGrantSet,
    RevokeGrantDevice,
    /// Re-grant a revoked remote device (revoke=false), per the mockup's
    /// switched "Re-grant device" action.
    ReGrantDevice,
    /// Re-register this device with a fresh key, keeping the previous
    /// grant set available for the one-tap Restore (mockup's THIS-device
    /// primary action; #249 recovery path).
    ReRegisterFromGrants,
    /// Re-apply the recorded previous grant set to the freshly
    /// re-registered key (mockup's Restore strip).
    RestoreGrantSet,
    OpenAudit,
    CloseAudit,
    RefreshAudit,
    SaveSettings,
}

/// Read-only data owned by the app while the Settings surface renders.
/// Grouping it keeps the immediate-mode entry point small while making the
/// subordinate audit path explicit rather than hiding it in global state.
pub struct SettingsPaneContext<'a> {
    pub key_id: &'a str,
    pub grants: &'a [String],
    pub store: Option<&'a KeyStore>,
    pub conn: ConnState,
    pub rev: Option<u64>,
    pub audit: &'a Option<Result<crate::protocol::AuditView, String>>,
    pub audit_loading: bool,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            host_url: crate::protocol::DEFAULT_HOST_URL.to_string(),
            auto_reconnect: true,
            group_by_repo: true,
            show_idle_collapsed: true,
            stick_to_bottom: true,
            theme: "dark".to_string(),
            token_input: String::new(),
            admin_token_input: String::new(),
            notice: None,
            audit_open: false,
            requested: None,
            admin_token_configured: false,
            grant_admin: GrantAdminState::default(),
        }
    }
}

/// The selected device's editable grant set. Selection is by `key_id`;
/// grants are kept as a set but always serialized in the daemon's
/// canonical order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrantDraft {
    pub selected_key: String,
    pub caps: BTreeSet<String>,
}

impl GrantDraft {
    pub fn for_device(device: &GrantDevice) -> Self {
        Self {
            selected_key: device.key_id.clone(),
            caps: device.grants.iter().cloned().collect(),
        }
    }

    pub fn toggle(&mut self, capability: &str) {
        if !self.caps.remove(capability) {
            self.caps.insert(capability.to_string());
        }
    }

    /// Current selection in the stable canonical order.
    pub fn granted(&self) -> Vec<String> {
        GRANT_CAPABILITIES
            .iter()
            .filter(|cap| self.caps.contains(**cap))
            .map(|cap| (*cap).to_string())
            .collect()
    }
}

/// Host-admin grant management state. The UI never falls back to reading
/// the audit log to infer device keys.
#[derive(Debug, Default)]
pub struct GrantAdminState {
    pub view: Option<Result<Vec<GrantDevice>, String>>,
    pub loading: bool,
    pub saving: bool,
    pub draft: GrantDraft,
    pub notice: Option<(Level, String)>,
    /// #250/#209: after a THIS-device re-register (fresh key) the previous
    /// grant set is kept for the one-tap Restore strip. `reregistered` is
    /// true from the moment the fresh key registers until Restore applies
    /// (or the user leaves the screen).
    pub reregistered: bool,
    pub restore_grants: Vec<String>,
    /// Previous key id shown in the detail meta while `reregistered`.
    pub previous_key: Option<String>,
    /// Captured before a THIS-device re-register: `(key_id, grants)` of the
    /// registration being replaced, consumed by the RegisterResult path.
    /// Kept here because the request and its async result are separate
    /// events.
    pub pending_restore: Option<(String, Vec<String>)>,
}

impl GrantAdminState {
    pub fn selected_device(&self) -> Option<&GrantDevice> {
        let Ok(devices) = self.view.as_ref()? else {
            return None;
        };
        devices.iter().find(|d| d.key_id == self.draft.selected_key)
    }

    pub fn set_view(&mut self, devices: Vec<GrantDevice>, preferred_key: &str) {
        self.view = Some(Ok(devices));
        self.loading = false;
        let selected = choose_grant_key(
            self.view
                .as_ref()
                .and_then(|v| v.as_ref().ok())
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            &self.draft.selected_key,
            preferred_key,
        );
        self.draft = selected
            .and_then(|key| {
                self.view
                    .as_ref()
                    .and_then(|v| v.as_ref().ok())
                    .and_then(|devices| devices.iter().find(|d| d.key_id == key))
            })
            .map(GrantDraft::for_device)
            .unwrap_or_default();
    }

    pub fn set_error(&mut self, error: String) {
        self.view = Some(Err(error));
        self.loading = false;
        self.draft = GrantDraft::default();
    }

    /// Record a fresh this-device registration: keep `previous_grants`
    /// for the Restore strip; the new key's grants are empty (read-only).
    /// `previous_key` is shown in the detail meta (the old record stays in
    /// the ledger until it expires).
    pub fn mark_reregistered(
        &mut self,
        previous_grants: Vec<String>,
        previous_key: &str,
        new_key: &str,
    ) {
        self.reregistered = true;
        self.restore_grants = previous_grants;
        self.previous_key = if previous_key.is_empty() {
            None
        } else {
            Some(previous_key.to_string())
        };
        self.draft = GrantDraft {
            selected_key: new_key.to_string(),
            caps: BTreeSet::new(),
        };
        self.notice = Some((
            Level::Warn,
            format!(
                "Re-registered: fresh key {new_key} — grants are EMPTY. Use Restore to re-apply the previous set."
            ),
        ));
    }

    pub fn mark_restored(&mut self) {
        self.reregistered = false;
        self.restore_grants.clear();
        self.notice = Some((
            Level::Info,
            "Restored the previous grant set to this device.".to_string(),
        ));
    }
}

/// Keep an existing selection when it is still registered; otherwise prefer
/// this board's own key, then the first registered device.
pub(crate) fn choose_grant_key(
    devices: &[GrantDevice],
    current: &str,
    preferred: &str,
) -> Option<String> {
    if let Some(device) = devices.iter().find(|d| d.key_id == current) {
        return Some(device.key_id.clone());
    }
    if let Some(device) = devices.iter().find(|d| d.key_id == preferred) {
        return Some(device.key_id.clone());
    }
    devices.first().map(|d| d.key_id.clone())
}

/// Registration screen (no device registered for this host yet).
pub fn register_screen(ui: &mut Ui, settings: &mut SettingsState, conn: ConnState) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("corral fleet — device registration")
                .heading()
                .color(theme::ui::TEXT_STRONG),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "This device is not yet registered with the host. Registration needs the \
                 host's routing-only registration token (it never authorizes drive writes).",
            )
            .color(theme::ui::TEXT_MUTED),
        );
    });
    ui.add_space(16.0);

    let mut requested = None;
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(24, 16))
        .show(ui, |ui| {
            ui.set_max_width(560.0);
            ui.horizontal(|ui| {
                ui.label("host URL");
                let mut url = settings.host_url.clone();
                let response = ui.add(
                    TextEdit::singleline(&mut url)
                        .hint_text("http://127.0.0.1:8474")
                        .desired_width(360.0),
                );
                settings.host_url = url;
                if ui.button("connect").clicked() {
                    requested = Some(Request::Connect);
                }
                let _ = response;
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("registration token");
                let mut token = settings.token_input.clone();
                ui.add(
                    TextEdit::singleline(&mut token)
                        .password(true)
                        .desired_width(360.0),
                );
                settings.token_input = token;
                if ui.button("register").clicked() {
                    requested = Some(Request::Register);
                }
            });
            ui.add_space(8.0);
            if ui.button("auto-register (localhost)").clicked() {
                requested = Some(Request::AutoRegister);
            }
            ui.label(
                RichText::new(
                    "auto-register reads the token from ~/.config/corral/registration-token \
                     (same machine, same user) and registers with a fresh device key.",
                )
                .small()
                .color(theme::ui::TEXT_MUTED),
            );
        });

    if let Some(request) = requested {
        settings.requested = Some(request);
    }

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        crate::ui::connection_pill(ui, conn);
        ui.label(
            RichText::new("reads are open (snapshot/SSE); writes are device-signed.")
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
    });
    if let Some((level, text)) = &settings.notice {
        let color = match level {
            Level::Info => theme::ui::GOOD,
            Level::Warn => theme::ui::WARN,
            Level::Error => theme::ui::BAD,
        };
        ui.label(RichText::new(text).color(color));
    }
}

/// Settings tab (device already registered).
pub fn settings_pane(ui: &mut Ui, settings: &mut SettingsState, context: SettingsPaneContext<'_>) {
    let mut requested = None;
    let admin_token_configured = settings.admin_token_configured;
    egui::ScrollArea::vertical()
        .id_salt("corral-ui-settings-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Settings")
                        .heading()
                        .color(theme::ui::TEXT_STRONG),
                );
                crate::ui::connection_pill(ui, context.conn);
                if let Some(rev) = context.rev {
                    ui.label(
                        RichText::new(format!("rev {rev}"))
                            .monospace()
                            .small()
                            .color(theme::ui::TEXT_MUTED),
                    );
                }
            });
            ui.add_space(8.0);

            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Connection")
                            .strong()
                            .color(theme::ui::TEXT_STRONG),
                    );
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.label("host URL");
                        let mut url = settings.host_url.clone();
                        ui.add(
                            TextEdit::singleline(&mut url)
                                .hint_text("http://127.0.0.1:8474")
                                .desired_width(360.0),
                        );
                        settings.host_url = url;
                        if ui.button("reconnect").clicked() {
                            requested = Some(Request::Connect);
                        }
                    });
                    ui.checkbox(&mut settings.auto_reconnect, "auto-reconnect");
                    ui.label(
                        RichText::new("Reconnect the live read path after a dropped SSE connection.")
                            .small()
                            .color(theme::ui::TEXT_MUTED),
                    );
                });
            ui.add_space(10.0);

            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Board")
                            .strong()
                            .color(theme::ui::TEXT_STRONG),
                    );
                    ui.checkbox(&mut settings.group_by_repo, "group agents by repo");
                    ui.checkbox(&mut settings.show_idle_collapsed, "show idle / done collapsed");
                    ui.checkbox(&mut settings.stick_to_bottom, "stick output to bottom");
                    ui.label(
                        RichText::new("Cards is the only board view; repo grouping stays on the master bar.")
                            .small()
                            .color(theme::ui::TEXT_MUTED),
                    );
                });
            ui.add_space(10.0);

            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Display")
                            .strong()
                            .color(theme::ui::TEXT_STRONG),
                    );
                    ui.horizontal(|ui| {
                        ui.label("theme");
                        ui.label(
                            RichText::new("dark dashboard (approved prototype)")
                                .monospace()
                                .color(theme::ui::TEXT_STRONG),
                        );
                    });
                    ui.label(
                        RichText::new("Theme selection is intentionally fixed to the approved dark dashboard.")
                            .small()
                            .color(theme::ui::TEXT_MUTED),
                    );
                });
            ui.add_space(10.0);
            if ui
                .add(egui::Button::new(RichText::new("Save settings").strong()))
                .clicked()
            {
                requested = Some(Request::SaveSettings);
            }

            ui.add_space(12.0);
            egui::CollapsingHeader::new(
                RichText::new("Advanced device access")
                    .strong()
                    .color(theme::ui::TEXT_STRONG),
            )
            .default_open(settings.audit_open)
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Device identity, host-admin credentials, grants, and the subordinate audit view live here.")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("device identity")
                        .strong()
                        .color(theme::ui::TEXT_STRONG),
                );
                ui.horizontal_wrapped(|ui| {
                    detail_kv(ui, "key_id", context.key_id);
                    let store_text = context
                        .store
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "uninitialized".to_string());
                    detail_kv(ui, "key store", &store_text);
                    detail_kv(
                        ui,
                        "grants",
                        &if context.grants.is_empty() {
                            "read-only".to_string()
                        } else {
                            context.grants.join(", ")
                        },
                    );
                });
                if let Some(KeyStore::File { path }) = context.store {
                    ui.label(
                        RichText::new(format!(
                            "WARNING: OS keychain unavailable — device key stored in plaintext file (0600) at {}.",
                            path.display()
                        ))
                        .color(theme::ui::WARN),
                    );
                } else {
                    ui.label(
                        RichText::new("device key lives in the OS keychain (0600 fallback never used).")
                            .small()
                            .color(theme::ui::TEXT_MUTED),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("re-register (new device key)").clicked() {
                        requested = Some(Request::ReRegister);
                    }
                    if ui.button("refresh grants").clicked() {
                        requested = Some(Request::RefreshGrants);
                    }
                });
                ui.add_space(8.0);
                ui.label(
                    RichText::new("host administration (admin token)")
                        .strong()
                        .color(theme::ui::TEXT_STRONG),
                );
                ui.horizontal(|ui| {
                    ui.label("admin token");
                    let mut token = settings.admin_token_input.clone();
                    ui.add(
                        TextEdit::singleline(&mut token)
                            .password(true)
                            .desired_width(300.0),
                    );
                    settings.admin_token_input = token;
                    if ui.button("save (keychain)").clicked() {
                        requested = Some(Request::SaveAdminToken);
                    }
                    if ui.button("clear").clicked() {
                        requested = Some(Request::ClearAdminToken);
                    }
                });
                ui.label(
                    RichText::new("Host-side credentials are kept out of the normal settings flow and are never sent in a device-signed drive request.")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
                grant_management_block(
                    ui,
                    settings,
                    context.key_id,
                    admin_token_configured,
                    &mut requested,
                );
                ui.add_space(12.0);
                if settings.audit_open {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Audit — subordinate Settings surface")
                                .strong()
                                .color(theme::ui::TEXT_STRONG),
                        );
                        if ui.small_button("hide audit").clicked() {
                            requested = Some(Request::CloseAudit);
                        }
                    });
                    crate::ui::audit::show(
                        ui,
                        context.audit,
                        admin_token_configured,
                        context.audit_loading,
                        &mut || requested = Some(Request::RefreshAudit),
                    );
                } else {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("open audit log").clicked() {
                            requested = Some(Request::OpenAudit);
                        }
                        ui.label(
                            RichText::new(
                                "Host-admin audit is available here when needed; it is not a top-level tab.",
                            )
                            .small()
                            .color(theme::ui::TEXT_MUTED),
                        );
                    });
                }
            });

            if let Some((level, text)) = &settings.notice {
                let color = match level {
                    Level::Info => theme::ui::GOOD,
                    Level::Warn => theme::ui::WARN,
                    Level::Error => theme::ui::BAD,
                };
                ui.add_space(8.0);
                ui.label(RichText::new(text).color(color));
            }
        });

    if let Some(request) = requested {
        settings.requested = Some(request);
    }
}

/// Plain-language capability descriptions for the Devices & Grants
/// master/detail surface (#209/#250).
fn capability_description(capability: &str) -> &'static str {
    match capability {
        "read_tail" => "Read live agent output",
        "read_diff" => "Read the agent's worktree diff (files, diffstat, paged diff)",
        "prompt" => "Send prompts / steer the agent",
        "interrupt" => "Interrupt a running task",
        "approve" => "Approve tool calls & awaiting decisions",
        "kill" => "Terminate a task",
        "attach" => "Attach to a session & stream events",
        "start_worktree" => "Start a worktree from a bound issue",
        _ => "Drive capability",
    }
}

/// Compact device label: the registered display name when present,
/// otherwise the short key fingerprint (pre-#209 devices have no name).
fn device_title(device: &GrantDevice) -> String {
    match device.name.as_deref().filter(|n| !n.is_empty()) {
        Some(name) => name.to_string(),
        None => short_key(&device.key_id),
    }
}

/// `dev_1a2b3c4d…9c41`-style short fingerprint for list rows.
fn short_key(key_id: &str) -> String {
    let bare = key_id.strip_prefix("dev_").unwrap_or(key_id);
    if bare.len() > 12 {
        let (head, tail) = bare.split_at(8);
        format!("dev_{head}…{}", &tail[tail.len() - 4..])
    } else {
        format!("dev_{bare}")
    }
}

/// "granted/registered N ago" subline for device list rows.
fn age_label(seconds: u64, now_secs: u64) -> String {
    if seconds == 0 {
        return "—".to_string();
    }
    let days = now_secs.saturating_sub(seconds) / 86_400;
    match days {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        d if d < 7 => format!("{d}d ago"),
        d if d < 30 => format!("{}w ago", d / 7),
        d if d < 365 => format!("{}mo ago", d / 30),
        d => format!("{}y ago", d / 365),
    }
}

/// Subline for a revoked device. With a `revoked_ts` (#257) the true
/// revocation age is shown; without one (devices revoked before #257)
/// just "revoked" — never the creation age, which is the reported bug.
fn revoked_subline(revoked_ts: Option<u64>, now_secs: u64) -> String {
    match revoked_ts {
        Some(ts) => format!("revoked {}", age_label(ts, now_secs)),
        None => "revoked".to_string(),
    }
}

/// Split registered devices into THIS device (the board's own key) and
/// REMOTE DEVICES (other machines) — the #250 grouping rule.
fn split_devices<'a>(
    devices: &'a [GrantDevice],
    own_key_id: &str,
) -> (Vec<&'a GrantDevice>, Vec<&'a GrantDevice>) {
    let (mut self_devices, mut remote_devices) = (Vec::new(), Vec::new());
    for device in devices {
        if device.key_id == own_key_id {
            self_devices.push(device);
        } else {
            remote_devices.push(device);
        }
    }
    (self_devices, remote_devices)
}

fn group_header(ui: &mut Ui, label: &str, note: &str) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .size(10.0)
                .strong()
                .color(theme::ui::TEXT_MUTED),
        );
        ui.label(
            RichText::new(note)
                .size(10.0)
                .color(theme::ui::TEXT_MUTED.gamma_multiply(0.65)),
        );
    });
}

fn small_chip(ui: &mut Ui, text: &str, color: Color32) {
    let font = FontId::monospace(9.0);
    let galley = ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_string(), font, color));
    let size = galley.size() + egui::vec2(8.0, 3.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius::same(3),
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 26),
    );
    painter.rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0, color),
        StrokeKind::Outside,
    );
    painter.galley(rect.min + egui::vec2(4.0, 1.0), galley, color);
}

/// One master-list device row (the left column of the #250 master/detail).
fn device_row(ui: &mut Ui, device: &GrantDevice, is_self: bool, selected: bool) -> egui::Response {
    let width = ui.available_width().max(120.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 44.0), Sense::click());
    let painter = ui.painter();
    let bg = if selected {
        theme::ui::PANEL2
    } else {
        theme::ui::PANEL
    };
    painter.rect_filled(rect, CornerRadius::same(6), bg);
    if selected {
        painter.rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(1.0, theme::ui::ACCENT),
            egui::StrokeKind::Outside,
        );
    }
    painter.rect_filled(
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 3.0, rect.bottom())),
        CornerRadius::ZERO,
        if device.revoked {
            theme::ui::BAD
        } else if is_self {
            theme::ui::ACCENT
        } else {
            theme::ui::MUTED
        },
    );
    // Right rail: "N caps" + subline (fixed width).
    let right_width = 92.0;
    let text_rect = rect.shrink2(egui::vec2(12.0, 4.0));
    let right_rect = egui::Rect::from_min_max(
        egui::pos2(
            (text_rect.right() - right_width).max(text_rect.left()),
            text_rect.top(),
        ),
        text_rect.right_bottom(),
    );
    let left_rect = egui::Rect::from_min_max(
        text_rect.min,
        egui::pos2(right_rect.left() - 6.0, text_rect.bottom()),
    );
    let mut left_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(left_rect)
            .id(egui::Id::new(("corral-ui-grant-row-left", &device.key_id)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    left_ui.horizontal(|ui| {
        ui.add(
            egui::Label::new(
                RichText::new(device_title(device))
                    .size(12.5)
                    .strong()
                    .color(if device.revoked {
                        theme::ui::TEXT_MUTED
                    } else {
                        theme::ui::INK
                    }),
            )
            .truncate(),
        );
        if is_self {
            small_chip(ui, "THIS DEVICE", theme::ui::ACCENT);
        }
        if device.revoked {
            small_chip(ui, "REVOKED", theme::ui::BAD);
        }
    });
    left_ui.add(
        egui::Label::new(
            RichText::new(short_key(&device.key_id))
                .monospace()
                .size(10.0)
                .color(theme::ui::TEXT_MUTED),
        )
        .truncate(),
    );
    let mut right_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(right_rect)
            .id(egui::Id::new(("corral-ui-grant-row-right", &device.key_id)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    right_ui.vertical(|ui| {
        let caps = device.grants.len();
        ui.label(
            RichText::new(format!("{caps} {}", if caps == 1 { "cap" } else { "caps" }))
                .size(10.5)
                .strong()
                .color(theme::ui::TEXT_MUTED),
        );
        let subline = if device.revoked {
            revoked_subline(device.revoked_ts, now_secs())
        } else if is_self {
            "this computer".to_string()
        } else {
            format!("registered {}", age_label(device.created_ts, now_secs()))
        };
        ui.label(
            RichText::new(subline)
                .size(9.5)
                .color(theme::ui::TEXT_MUTED),
        );
    });
    response
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A mockup-style pill switch (egui has no built-in Switch in 0.36).
fn toggle_switch(ui: &mut Ui, on: bool, enabled: bool) -> egui::Response {
    let size = egui::vec2(34.0, 18.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius::same(9),
        if enabled && on {
            theme::ui::ACCENT
        } else {
            theme::ui::PANEL3
        },
    );
    painter.rect_stroke(
        rect,
        CornerRadius::same(9),
        Stroke::new(1.0, theme::ui::LINE),
        egui::StrokeKind::Outside,
    );
    let center = egui::pos2(
        if on {
            rect.right() - 11.0
        } else {
            rect.left() + 11.0
        },
        rect.center().y,
    );
    painter.circle_filled(
        center,
        6.0,
        if enabled && on {
            theme::ui::SEND_INK
        } else {
            theme::ui::MUTED
        },
    );
    response
}

/// Capability rows of the detail pane: name + description left, toggle
/// right. A flip applies immediately (Request::ToggleGrantCap).
fn capability_row(
    ui: &mut Ui,
    capability: &str,
    on: bool,
    enabled: bool,
    requested: &mut Option<Request>,
) {
    let width = ui.available_width().max(200.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 34.0), Sense::hover());
    let painter = ui.painter();
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.bottom()),
            egui::pos2(rect.right(), rect.bottom()),
        ],
        Stroke::new(1.0, theme::ui::LINE),
    );
    let mut row_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(4.0, 3.0)))
            .id(egui::Id::new(("corral-ui-grant-cap-row", capability)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    row_ui.vertical(|ui| {
        ui.label(
            RichText::new(capability)
                .monospace()
                .size(12.0)
                .strong()
                .color(theme::ui::INK),
        );
        ui.label(
            RichText::new(capability_description(capability))
                .size(10.0)
                .color(theme::ui::TEXT_MUTED),
        );
    });
    row_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let resp = toggle_switch(ui, on, enabled);
        if resp.changed() && enabled {
            *requested = Some(Request::ToggleGrantCap(capability.to_string()));
        }
    });
}

/// The right detail pane of the Devices & Grants surface.
fn device_detail(
    ui: &mut Ui,
    state: &mut GrantAdminState,
    own_key_id: &str,
    admin_token_configured: bool,
    requested: &mut Option<Request>,
) {
    let Some(device) = state.selected_device().cloned() else {
        ui.label(RichText::new("select a registered device.").color(theme::ui::TEXT_MUTED));
        return;
    };
    let is_self = device.key_id == own_key_id;
    let busy = state.loading || state.saving;

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(device_title(&device))
                .size(15.0)
                .strong()
                .color(theme::ui::INK),
        );
        if is_self {
            small_chip(ui, "THIS DEVICE", theme::ui::ACCENT);
        }
        if device.revoked {
            small_chip(ui, "REVOKED", theme::ui::BAD);
        }
    });
    let mut meta = format!(
        "key {} · created {} · {} · {}",
        short_key(&device.key_id),
        crate::model::relative_age(
            device.created_ts.saturating_mul(1000),
            now_secs().saturating_mul(1000)
        ),
        if device.expiry_ts == 0 || now_secs() >= device.expiry_ts {
            "expired".to_string()
        } else {
            format!(
                "expires in {}",
                age_label(device.expiry_ts, now_secs()).replace("ago", "")
            )
        },
        if device.revoked {
            "revoked".to_string()
        } else {
            "active".to_string()
        },
    );
    if is_self && let Some(previous) = &state.previous_key {
        meta = format!(
            "key {} (previous {}) ·{}",
            short_key(&device.key_id),
            short_key(previous),
            meta.split_once('·')
                .map(|(_, rest)| format!("·{rest}"))
                .unwrap_or_default()
        );
    }
    ui.label(
        RichText::new(meta)
            .monospace()
            .size(10.0)
            .color(theme::ui::TEXT_MUTED),
    );
    ui.add_space(6.0);

    let granted = state.draft.caps.len();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("CAPABILITIES")
                .size(10.5)
                .strong()
                .color(theme::ui::TEXT_MUTED),
        );
        ui.label(
            RichText::new(format!("{granted} of {} granted", GRANT_CAPABILITIES.len()))
                .size(10.0)
                .monospace()
                .color(theme::ui::TEXT_MUTED),
        );
    });
    let caps_enabled = admin_token_configured && !busy && !device.revoked && !state.reregistered;
    if !admin_token_configured {
        ui.label(
            RichText::new("admin token required to edit grants (save/paste it above).")
                .size(10.0)
                .color(theme::ui::WARN),
        );
    }
    for capability in GRANT_CAPABILITIES {
        let on = state.draft.caps.contains(capability);
        capability_row(ui, capability, on, caps_enabled, requested);
    }
    ui.add_space(6.0);

    if is_self {
        let rebutton =
            egui::Button::new(RichText::new("Re-register").strong()).fill(theme::ui::ACCENT);
        if ui.add_enabled(!busy, rebutton).clicked() {
            *requested = Some(Request::ReRegisterFromGrants);
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Refresh grants"))
            .clicked()
        {
            *requested = Some(Request::RefreshGrants);
        }
        if state.reregistered {
            // Restore strip: the previous grant set is one tap away.
            egui::Frame::group(ui.style())
                .fill(theme::ui::PANEL2)
                .stroke(Stroke::new(1.0, theme::ui::ACCENT))
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(
                                    "New key runs with zero grants until restored or re-granted:",
                                )
                                .size(11.0)
                                .color(theme::ui::TEXT_MUTED),
                            )
                            .wrap(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    !busy
                                        && admin_token_configured
                                        && !state.restore_grants.is_empty(),
                                    egui::Button::new("Restore grant set"),
                                )
                                .clicked()
                            {
                                *requested = Some(Request::RestoreGrantSet);
                            }
                        });
                    });
                });
        }
        // #249 trust-check note — only on the THIS-device card.
        egui::Frame::group(ui.style())
            .fill(Color32::from_rgba_unmultiplied(0xe3, 0xb3, 0x41, 10))
            .stroke(Stroke::new(1.0, theme::ui::WARN))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "Bad-signature trust check (#249). If the daemon rejects this device with a bad-signature error, the stored key is stale — Re-register above to mint a fresh key, then Restore the grants.",
                    )
                    .size(10.5)
                    .color(theme::ui::WARN),
                );
            });
    } else {
        if !device.revoked {
            let revoke = egui::Button::new(RichText::new("Revoke device").color(theme::ui::BAD))
                .fill(Color32::TRANSPARENT);
            if ui
                .add_enabled(!busy && admin_token_configured, revoke)
                .clicked()
            {
                *requested = Some(Request::RevokeGrantDevice);
            }
        } else if ui
            .add_enabled(
                !busy && admin_token_configured,
                egui::Button::new(RichText::new("Re-grant device").strong())
                    .fill(theme::ui::ACCENT),
            )
            .clicked()
        {
            *requested = Some(Request::ReGrantDevice);
        }
    }
}

/// The #250 V2 master/detail Device access surface: THIS DEVICE vs REMOTE
/// DEVICES groups on the left, capabilities + actions on the right. Every
/// grant change goes through the same admin-token `POST /grants` path as
/// `scripts/corrald-grant.sh`.
fn grant_management_block(
    ui: &mut Ui,
    settings: &mut SettingsState,
    own_key_id: &str,
    admin_token_configured: bool,
    requested: &mut Option<Request>,
) {
    ui.add_space(10.0);
    let state = &mut settings.grant_admin;
    ui.label(
        RichText::new("DEVICE ACCESS")
            .size(10.0)
            .strong()
            .color(theme::ui::TEXT_MUTED),
    );
    ui.horizontal(|ui| {
        if state.loading {
            ui.spinner();
            ui.label(
                RichText::new("loading registered devices…")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        }
        if state.saving {
            ui.spinner();
            ui.label(
                RichText::new("applying…")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        }
        if ui
            .add_enabled(
                !state.loading && !state.saving,
                egui::Button::new("refresh device list"),
            )
            .clicked()
        {
            *requested = Some(Request::LoadGrantDevices);
        }
        ui.label(
            RichText::new(if admin_token_configured {
                "admin token ✓"
            } else {
                "admin token ✗"
            })
            .monospace()
            .size(10.0)
            .color(if admin_token_configured {
                theme::ui::GOOD
            } else {
                theme::ui::WARN
            }),
        );
    });

    let devices = match state.view.clone() {
        Some(Ok(devices)) => Some(devices),
        Some(Err(error)) => {
            ui.label(RichText::new(format!("grants view error: {error}")).color(theme::ui::BAD));
            None
        }
        None => {
            if !state.loading {
                ui.label(
                    RichText::new(
                        if admin_token_configured {
                            "no device/grants view loaded — press refresh."
                        } else {
                            "no admin token available — save/paste it above before loading device grants."
                        },
                    )
                    .color(theme::ui::TEXT_MUTED),
                );
            }
            None
        }
    };

    if let Some(devices) = devices {
        if devices.is_empty() {
            ui.label(
                RichText::new(
                    "no registered device keys on this host — register one before granting.",
                )
                .color(theme::ui::TEXT_MUTED),
            );
            return;
        }
        let (self_devices, remote_devices) = split_devices(&devices, own_key_id);
        let selected = state.draft.selected_key.clone();
        let notice = state.notice.clone();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(300.0, ui.available_height().max(1.0)),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(300.0);
                    group_header(ui, "THIS DEVICE", "(this computer)");
                    if self_devices.is_empty() {
                        ui.label(RichText::new("—").color(theme::ui::TEXT_MUTED));
                    }
                    for device in &self_devices {
                        let is_selected = device.key_id == selected;
                        if device_row(ui, device, true, is_selected).clicked() && !is_selected {
                            *requested = Some(Request::SelectGrantDevice(device.key_id.clone()));
                        }
                    }
                    group_header(ui, "REMOTE DEVICES", "(other machines)");
                    if remote_devices.is_empty() {
                        ui.label(
                            RichText::new("no other registered devices.")
                                .color(theme::ui::TEXT_MUTED),
                        );
                    }
                    for device in &remote_devices {
                        let is_selected = device.key_id == selected;
                        if device_row(ui, device, false, is_selected).clicked() && !is_selected {
                            *requested = Some(Request::SelectGrantDevice(device.key_id.clone()));
                        }
                    }
                },
            );
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.set_max_width(ui.available_width());
                device_detail(ui, state, own_key_id, admin_token_configured, requested);
                if let Some((level, text)) = &notice {
                    let color = match level {
                        Level::Info => theme::ui::GOOD,
                        Level::Warn => theme::ui::WARN,
                        Level::Error => theme::ui::BAD,
                    };
                    ui.add_space(6.0);
                    ui.label(RichText::new(text).color(color));
                }
            });
        });
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
            .monospace()
            .small()
            .color(theme::ui::TEXT_STRONG),
    );
    ui.add_space(8.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(key_id: &str, grants: &[&str]) -> GrantDevice {
        GrantDevice {
            key_id: key_id.to_string(),
            name: None,
            grants: grants.iter().map(|g| g.to_string()).collect(),
            revoked: false,
            revoked_ts: None,
            expiry_ts: 1_000,
            created_ts: 500,
        }
    }

    #[test]
    fn grant_draft_toggles_and_keeps_canonical_order() {
        let mut draft =
            GrantDraft::for_device(&device("dev_a", &["start_worktree", "prompt", "attach"]));
        assert_eq!(draft.granted(), ["prompt", "attach", "start_worktree"]);
        draft.toggle("kill");
        draft.toggle("start_worktree");
        assert_eq!(draft.granted(), ["prompt", "kill", "attach"]);
        draft.toggle("prompt");
        assert_eq!(draft.granted(), ["kill", "attach"]);
    }

    #[test]
    fn grant_key_selection_prefers_current_then_own_then_first() {
        let devices = vec![device("dev_a", &[]), device("dev_b", &["prompt"])];
        assert_eq!(
            choose_grant_key(&devices, "dev_b", "dev_a"),
            Some("dev_b".to_string())
        );
        assert_eq!(
            choose_grant_key(&devices, "gone", "dev_a"),
            Some("dev_a".to_string())
        );
        assert_eq!(
            choose_grant_key(&devices, "gone", "gone2"),
            Some("dev_a".to_string())
        );
        assert_eq!(choose_grant_key(&[], "gone", "own"), None);
    }

    #[test]
    fn grant_admin_state_loads_selection_and_fails_closed() {
        let mut state = GrantAdminState::default();
        state.set_view(
            vec![device("dev_a", &[]), device("dev_b", &["read_tail"])],
            "dev_b",
        );
        assert_eq!(state.draft.selected_key, "dev_b");
        assert_eq!(state.draft.granted(), ["read_tail"]);
        state.draft.toggle("start_worktree");
        assert_eq!(state.draft.granted(), ["read_tail", "start_worktree"]);

        state.set_error("transport failed".to_string());
        assert!(state.view.as_ref().unwrap().is_err());
        assert!(state.selected_device().is_none());
        assert!(state.draft.selected_key.is_empty());
    }

    #[test]
    fn devgrants_group_self_before_renote_and_label_with_name_or_fingerprint() {
        let mut self_device = device("dev_5b6e0e...", &["read_tail"]);
        self_device.name = Some("midnight".to_string());
        let mut iphone = device("dev_6afc68bb...", &["prompt"]);
        iphone.name = Some("iPhone 15 Pro".to_string());
        let unnamed = device("dev_9c41d7e0...", &[]);
        let devices = vec![self_device.clone(), iphone.clone(), unnamed.clone()];

        let (self_devices, remote_devices) = split_devices(&devices, "dev_5b6e0e...");
        assert_eq!(self_devices.len(), 1);
        assert_eq!(self_devices[0].key_id, "dev_5b6e0e...");
        assert_eq!(remote_devices.len(), 2);
        assert!(
            remote_devices.iter().all(|d| d.key_id != "dev_5b6e0e..."),
            "the own key must never land in REMOTE DEVICES"
        );

        // Named devices show the name; un-named ones fall back to the
        // short fingerprint.
        assert_eq!(device_title(&self_device), "midnight");
        assert_eq!(device_title(&iphone), "iPhone 15 Pro");
        assert_eq!(device_title(&unnamed), short_key(&unnamed.key_id));
        assert!(short_key("dev_0123456789abcdef").starts_with("dev_01234567"));
        assert!(short_key("dev_0123456789abcdef").ends_with("…cdef"));
        assert_eq!(age_label(0, 100), "—");
        assert_eq!(age_label(1_000_000, 1_000_000 + 86_400 * 3), "3d ago");
        assert_eq!(age_label(1_000_000, 1_000_000 + 86_400 * 14), "2w ago");
    }

    /// #257: the revoked subline shows the TRUE revocation age when the
    /// ledger has one — never the creation age (the reported bug).
    #[test]
    fn revoked_subline_shows_true_revocation_age_when_known() {
        assert_eq!(
            revoked_subline(Some(1_000_000), 1_000_000 + 86_400 * 3),
            "revoked 3d ago"
        );
        assert_eq!(
            revoked_subline(Some(1_000_000), 1_000_000 + 86_400 * 14),
            "revoked 2w ago"
        );
        assert_eq!(
            revoked_subline(Some(1_000_000), 1_000_000 + 86_400 * 100),
            "revoked 3mo ago"
        );
        assert_eq!(revoked_subline(Some(1_000_000), 1_000_000), "revoked today");
    }

    /// #257: pre-#257 devices (no `revoked_ts`) fall back to a plain
    /// "revoked" — the creation age must never masquerade as the
    /// revocation age.
    #[test]
    fn revoked_subline_falls_back_without_age_when_unknown() {
        assert_eq!(revoked_subline(None, 1_000_000 + 86_400 * 3), "revoked");
        assert_eq!(revoked_subline(None, 1_000_000), "revoked");
    }

    #[test]
    fn devgrants_reregister_keeps_restore_set_and_restored_clears_it() {
        let mut state = GrantAdminState::default();
        state.mark_reregistered(
            vec!["read_tail".to_string(), "prompt".to_string()],
            "dev_old",
            "dev_new",
        );
        assert!(state.reregistered);
        assert_eq!(state.restore_grants.len(), 2);
        assert_eq!(state.draft.selected_key, "dev_new");
        assert!(state.draft.caps.is_empty(), "fresh key starts read-only");
        assert!(
            state
                .notice
                .as_ref()
                .unwrap()
                .1
                .contains("grants are EMPTY")
        );

        state.mark_restored();
        assert!(!state.reregistered);
        assert!(state.restore_grants.is_empty());
        assert!(state.notice.as_ref().unwrap().1.contains("Restored"));
    }

    #[test]
    fn devgrants_every_capability_has_a_description() {
        for capability in GRANT_CAPABILITIES {
            let description = capability_description(capability);
            assert!(!description.is_empty());
            assert_ne!(
                description, "Drive capability",
                "explicit text for {capability}"
            );
        }
    }

    fn rendered_text(shape: &egui::epaint::Shape, text: &mut String) {
        match shape {
            egui::epaint::Shape::Text(shape) => {
                text.push_str(shape.galley.text());
                text.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    rendered_text(shape, text);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn audit_renders_only_as_a_reachable_settings_subordinate_surface() {
        let ctx = egui::Context::default();
        let mut settings = SettingsState {
            audit_open: true,
            ..Default::default()
        };
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 900.0),
                )),
                ..Default::default()
            },
            |ui| {
                settings_pane(
                    ui,
                    &mut settings,
                    SettingsPaneContext {
                        key_id: "dev_test",
                        grants: &[],
                        store: None,
                        conn: ConnState::Connected,
                        rev: None,
                        audit: &None,
                        audit_loading: false,
                    },
                );
            },
        );
        let mut text = String::new();
        for clipped in &output.shapes {
            rendered_text(&clipped.shape, &mut text);
        }
        output.textures_delta.clear();
        assert!(text.contains("Audit — subordinate Settings surface"));
        assert!(text.contains("AUDIT LOG"));
        assert!(!text.contains("Board / Issues / Registry / Settings / Audit"));
    }
}
