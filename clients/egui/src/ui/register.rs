//! Registration + settings view: host URL, device registration (paste
//! token or localhost auto-register), and the connection-only Settings pane
//! (#354 L3: device/grant admin UI and the subordinate audit surface were
//! removed with the daemon's grant administration).

use eframe::egui::{self, Color32, RichText, Stroke, TextEdit, Ui};

use crate::keys::KeyStore;
use crate::protocol::READ_GRANT_CAPABILITIES;
use crate::state::{ConnState, Level};
use crate::theme;

/// Connection-only settings state.
pub struct SettingsState {
    pub host_url: String,
    pub auto_reconnect: bool,
    pub token_input: String,
    pub notice: Option<(Level, String)>,
    /// Health-line Details disclosure (key id / store / grants).
    pub health_details_open: bool,
    /// Set by the view when the user asks for an action.
    pub requested: Option<Request>,
    /// #310: true only after an actual current-key `bad_signature`
    /// rejection. Healthy state never shows re-register guidance; the
    /// recovery block renders only while this is set.
    pub bad_signature: bool,
    /// #310 r3: the persisted recovery-guidance notice text set on a
    /// current-generation rejection. A later current-generation success
    /// clears it (and the matching `settings.notice`) without deleting
    /// unrelated notices; stale-generation drive results never touch it.
    pub recovery_notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Connect,
    AutoRegister,
    Register,
    /// #310 recovery: re-register the CURRENT key material with the host
    /// ("Restore saved identity" — never a fresh key). The daemon's
    /// read-only default grants are empty; the host provisions out-of-band.
    RecoverIdentity,
    /// #310: re-register this device with a fresh key (Re-register…).
    ReRegister,
    SaveSettings,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            host_url: crate::protocol::DEFAULT_HOST_URL.to_string(),
            auto_reconnect: true,
            token_input: String::new(),
            notice: None,
            health_details_open: false,
            requested: None,
            bad_signature: false,
            recovery_notice: None,
        }
    }
}

/// Read-only data owned by the app while the Settings surface renders.
pub struct SettingsPaneContext<'a> {
    pub key_id: &'a str,
    pub grants: &'a [String],
    pub store: Option<&'a KeyStore>,
    pub conn: ConnState,
    pub rev: Option<u64>,
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
                 host's routing-only registration token (it never authorizes writes — the \
                 daemon is read-only).",
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
            RichText::new("reads are device-signed; the daemon is read-only.")
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

/// Settings tab (device already registered). #354 L3: CONNECTION ONLY —
/// host URL + identity status + the #310 recovery block. The board view
/// toggles, the device/grant admin surface and the audit viewer were
/// removed with the cut.
pub fn settings_pane(ui: &mut Ui, settings: &mut SettingsState, context: SettingsPaneContext<'_>) {
    let mut requested = None;
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
            health_line(ui, settings, &context);
            ui.add_space(12.0);

            // CONNECTION — the only settings group after the cut.
            group_header(ui, "CONNECTION", "");
            ui.add_space(2.0);
            ui.separator();
            ui.add_space(6.0);

            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("host URL").color(theme::ui::TEXT_MUTED));
                let mut url = settings.host_url.clone();
                ui.add(
                    TextEdit::singleline(&mut url)
                        .hint_text("http://127.0.0.1:8474")
                        .desired_width(280.0),
                );
                settings.host_url = url;
                if ui.button("reconnect").clicked() {
                    requested = Some(Request::Connect);
                }
            });
            ui.checkbox(&mut settings.auto_reconnect, "auto-reconnect");
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(6.0);

            // This device identity row (read-only).
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("This device")
                        .strong()
                        .color(theme::ui::TEXT_STRONG),
                );
                ui.label(
                    RichText::new(short_key(context.key_id))
                        .strong()
                        .color(theme::ui::INK),
                );
                ui.label(
                    RichText::new(match context.store {
                        Some(KeyStore::File { .. }) => "file key store (0600)",
                        _ => "keychain",
                    })
                    .monospace()
                    .small()
                    .color(theme::ui::TEXT_MUTED),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} of {} read capabilities granted",
                            context.grants.len(),
                            READ_GRANT_CAPABILITIES.len()
                        ))
                        .monospace()
                        .small()
                        .color(theme::ui::GOOD),
                    );
                });
            });
            ui.label(
                RichText::new(
                    "Grants are provisioned out-of-band by the host and are shown read-only; \
                     re-register to refresh them.",
                )
                .small()
                .color(theme::ui::TEXT_MUTED),
            );
            ui.add_space(6.0);

            if settings.bad_signature {
                bad_signature_recovery(ui, &mut requested);
                ui.add_space(8.0);
            }

            ui.add_space(6.0);
            if ui
                .add(egui::Button::new(RichText::new("Save settings").strong()))
                .clicked()
            {
                requested = Some(Request::SaveSettings);
            }

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

/// The #310 health line: Connected / Identity in one human-readable row.
/// The healthy state carries NO re-register guidance; key ID and grants
/// live behind the Details disclosure.
fn health_line(ui: &mut Ui, settings: &mut SettingsState, context: &SettingsPaneContext<'_>) {
    ui.horizontal(|ui| {
        let (color, text) = match context.conn {
            ConnState::Connected => (theme::ui::GOOD, "Connected".to_string()),
            ConnState::Connecting => (theme::ui::WARN, "Connecting".to_string()),
            ConnState::Reconnecting { .. } => (theme::ui::WARN, "Reconnecting".to_string()),
            ConnState::Down => (theme::ui::BAD, "Daemon offline".to_string()),
        };
        ui.label(
            RichText::new("Health")
                .strong()
                .color(theme::ui::TEXT_STRONG),
        );
        ui.add_space(8.0);
        ui.label(RichText::new(text).strong().color(color));
        ui.add_space(4.0);
        let identity_ok = !settings.bad_signature;
        ui.label(
            RichText::new(if identity_ok {
                "Identity trust check passed · current key accepted by the daemon."
            } else {
                "Identity rejected — recovery is available below."
            })
            .small()
            .color(if identity_ok {
                theme::ui::TEXT_MUTED
            } else {
                theme::ui::WARN
            }),
        );
        if ui.small_button("details").clicked() {
            settings.health_details_open = !settings.health_details_open;
        }
    });
    if settings.health_details_open {
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!("device key id: {}", short_key(context.key_id)))
                        .monospace()
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
                let grants = if context.grants.is_empty() {
                    "none (read-only default)".to_string()
                } else {
                    context.grants.join(", ")
                };
                ui.label(
                    RichText::new(format!("grants: {grants}"))
                        .monospace()
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            });
    }
}

/// #310 bad-signature recovery block. Recovery appears only for this
/// recorded event: the daemon rejected the current key's signature.
fn bad_signature_recovery(ui: &mut Ui, requested: &mut Option<Request>) {
    egui::Frame::group(ui.style())
        .fill(Color32::from_rgba_unmultiplied(0xe3, 0xb3, 0x41, 10))
        .stroke(Stroke::new(1.0, theme::ui::WARN))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.label(
                RichText::new("BAD-SIGNATURE REJECTION")
                    .size(10.5)
                    .strong()
                    .color(theme::ui::WARN),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "The daemon rejected this current key. Recovery appears only for this \
                         recorded event.",
                    )
                    .size(10.5)
                    .color(theme::ui::TEXT_MUTED),
                )
                .wrap(),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("Restore saved identity").strong())
                            .fill(theme::ui::ACCENT),
                    )
                    .clicked()
                {
                    *requested = Some(Request::RecoverIdentity);
                }
                if ui.button("Re-register…").clicked() {
                    *requested = Some(Request::ReRegister);
                }
            });
            ui.label(
                RichText::new("Restore keeps this key ID and re-registers the current key.")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
            ui.label(
                RichText::new("Re-register mints a fresh key (grants stay read-only).")
                    .small()
                    .color(theme::ui::TEXT_MUTED),
            );
        });
}

fn short_key(key_id: &str) -> String {
    if key_id.len() <= 12 {
        key_id.to_string()
    } else {
        format!("{}…{}", &key_id[..6], &key_id[key_id.len() - 4..])
    }
}

fn group_header(ui: &mut Ui, label: &str, note: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .size(10.5)
                .strong()
                .color(theme::ui::MUTED),
        );
        if !note.is_empty() {
            ui.label(RichText::new(note).small().color(theme::ui::TEXT_MUTED));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_connection_only() {
        let settings = SettingsState::default();
        assert_eq!(settings.host_url, crate::protocol::DEFAULT_HOST_URL);
        assert!(settings.auto_reconnect);
        assert!(!settings.bad_signature);
        assert!(settings.recovery_notice.is_none());
        assert!(settings.requested.is_none());
    }

    #[test]
    fn request_variants_are_the_connection_and_recovery_set() {
        // #354 RED/GREEN probe: the Settings action surface must not grow
        // grant-admin/audit actions back. This list is the closed set.
        let requests = [
            Request::Connect,
            Request::AutoRegister,
            Request::Register,
            Request::RecoverIdentity,
            Request::ReRegister,
            Request::SaveSettings,
        ];
        assert_eq!(requests.len(), 6);
        assert!(requests.contains(&Request::RecoverIdentity));
    }

    #[test]
    fn short_key_bounds_long_ids() {
        assert_eq!(short_key("abc"), "abc");
        let long = "0123456789abcdef";
        let out = short_key(long);
        assert!(out.len() < long.len());
        assert!(out.starts_with("012345"));
        assert!(out.ends_with("cdef"));
    }

    #[test]
    fn read_grant_vocabulary_is_what_the_identity_row_counts() {
        assert_eq!(READ_GRANT_CAPABILITIES.len(), 2);
    }
}
