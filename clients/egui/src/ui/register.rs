//! Registration + settings view: host URL, device registration (paste
//! token or localhost auto-register), device identity status (key store
//! + warnings), and the host admin token for the audit view.

use eframe::egui::{RichText, TextEdit, Ui};

use crate::keys::KeyStore;
use crate::state::{ConnState, Level};
use crate::theme;

pub struct SettingsState {
    pub host_url: String,
    pub token_input: String,
    pub admin_token_input: String,
    pub notice: Option<(Level, String)>,
    /// Set by the view when the user asks for an action.
    pub requested: Option<Request>,
}

pub enum Request {
    Connect,
    AutoRegister,
    Register,
    ReRegister,
    RefreshGrants,
    SaveAdminToken,
    ClearAdminToken,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            host_url: crate::protocol::DEFAULT_HOST_URL.to_string(),
            token_input: String::new(),
            admin_token_input: String::new(),
            notice: None,
            requested: None,
        }
    }
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
pub fn settings_pane(
    ui: &mut Ui,
    settings: &mut SettingsState,
    key_id: &str,
    grants: &[String],
    store: Option<&KeyStore>,
    conn: ConnState,
    rev: Option<u64>,
) {
    let mut requested = None;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("SETTINGS")
                .strong()
                .color(theme::ui::TEXT_STRONG),
        );
        crate::ui::connection_pill(ui, conn);
        if let Some(rev) = rev {
            ui.label(
                RichText::new(format!("rev {rev}"))
                    .monospace()
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        }
    });
    ui.separator();

    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_max_width(620.0);
            ui.label(
                RichText::new("connection")
                    .strong()
                    .color(theme::ui::TEXT_STRONG),
            );
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
            ui.add_space(8.0);
            ui.label(
                RichText::new("device identity")
                    .strong()
                    .color(theme::ui::TEXT_STRONG),
            );
            ui.horizontal_wrapped(|ui| {
                detail_kv(ui, "key_id", key_id);
                let store_text = store
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "uninitialized".to_string());
                detail_kv(ui, "key store", &store_text);
                detail_kv(
                    ui,
                    "grants",
                    &if grants.is_empty() {
                        "read-only".to_string()
                    } else {
                        grants.join(", ")
                    },
                );
            });
            if let Some(KeyStore::File { path }) = store {
                ui.label(
                    RichText::new(format!(
                        "WARNING: OS keychain unavailable — device key stored in plaintext \
                         file (0600) at {}. Consider a keychain-enabled desktop session.",
                        path.display()
                    ))
                    .color(theme::ui::WARN),
                );
            } else {
                ui.label(
                    RichText::new(
                        "device key lives in the OS keychain (0600 fallback never used).",
                    )
                    .small()
                    .color(theme::ui::TEXT_MUTED),
                );
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button("re-register (new device key)").clicked() {
                    requested = Some(Request::ReRegister);
                }
                if ui.button("refresh grants (re-fetch from host)").clicked() {
                    requested = Some(Request::RefreshGrants);
                }
            });
            ui.label(
                RichText::new(
                    "refresh grants re-registers the SAME device key to re-learn the host's \
                     current grant set and clears any locally-demoted capability (re-enables \
                     buttons the host re-granted).",
                )
                .small()
                .color(theme::ui::TEXT_MUTED),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new("audit (host admin)")
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
                RichText::new(
                    "Used only for the audit view; stored in the OS keychain (never on disk \
                     in plaintext). On localhost the host's own admin-token file is used \
                     automatically when available.",
                )
                .small()
                .color(theme::ui::TEXT_MUTED),
            );
        });

    if let Some(request) = requested {
        settings.requested = Some(request);
    }

    if let Some((level, text)) = &settings.notice {
        let color = match level {
            Level::Info => theme::ui::GOOD,
            Level::Warn => theme::ui::WARN,
            Level::Error => theme::ui::BAD,
        };
        ui.label(RichText::new(text).color(color));
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
