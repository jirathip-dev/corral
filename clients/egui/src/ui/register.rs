//! Registration + settings view: host URL, device registration (paste
//! token or localhost auto-register), device identity status (key store
//! + warnings), and host-side administration (audit + device grants).

use std::collections::BTreeSet;

use eframe::egui::{RichText, TextEdit, Ui};

use crate::keys::KeyStore;
use crate::protocol::{GRANT_CAPABILITIES, GrantDevice};
use crate::state::{ConnState, Level};
use crate::theme;

pub struct SettingsState {
    pub host_url: String,
    pub token_input: String,
    pub admin_token_input: String,
    pub notice: Option<(Level, String)>,
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
    ApplyGrantSet,
    RevokeGrantDevice,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            host_url: crate::protocol::DEFAULT_HOST_URL.to_string(),
            token_input: String::new(),
            admin_token_input: String::new(),
            notice: None,
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
    let admin_token_configured = settings.admin_token_configured;
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
                RichText::new(
                    "Host-side credential for the audit view and device grant management. It \
                     is sent only to host admin endpoints, stored in the OS keychain (never on \
                     disk in plaintext), and never embedded in a device-signed drive flow. On \
                     localhost the host's own admin-token file is used automatically when \
                     available.",
                )
                .small()
                .color(theme::ui::TEXT_MUTED),
            );
            grant_management_block(ui, settings, key_id, admin_token_configured, &mut requested);
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
        RichText::new("device grants (host admin)")
            .strong()
            .color(theme::ui::TEXT_STRONG),
    );
    ui.label(
        RichText::new(
            "Select a registered device key and edit capabilities. Applying replaces its \
             full grant set through the same admin-token POST /grants path as \
             scripts/corrald-grant.sh; unchecking every capability is read-only.",
        )
        .small()
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
                RichText::new("applying grants…")
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
        } else {
            let selected_text = devices
                .iter()
                .find(|d| d.key_id == state.draft.selected_key)
                .map(|d| device_option_label(d, own_key_id))
                .unwrap_or_else(|| "select device".to_string());
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("device key")
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
                egui::ComboBox::from_id_salt("corral-ui-grant-device")
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        for device in &devices {
                            let label = device_option_label(device, own_key_id);
                            let selected = device.key_id == state.draft.selected_key;
                            if ui.selectable_label(selected, label).clicked() {
                                *requested =
                                    Some(Request::SelectGrantDevice(device.key_id.clone()));
                            }
                        }
                    });
            });

            let selected_device = state.selected_device().cloned();
            match selected_device {
                None => {
                    ui.label(
                        RichText::new("select a registered key above.")
                            .color(theme::ui::TEXT_MUTED),
                    );
                }
                Some(device) => {
                    let current_grants = device.grants.clone();
                    let revoked = device.revoked;
                    let busy = state.loading || state.saving;
                    ui.horizontal_wrapped(|ui| {
                        detail_kv(ui, "selected", &device.key_id);
                        detail_kv(
                            ui,
                            "host grants",
                            &if current_grants.is_empty() {
                                "read-only".to_string()
                            } else {
                                current_grants.join(", ")
                            },
                        );
                        detail_kv(ui, "revoked", &revoked.to_string());
                    });
                    if revoked {
                        ui.label(
                            RichText::new(
                                "DEVICE REVOKED — it cannot drive until re-enabled by the host.",
                            )
                            .color(theme::ui::BAD),
                        );
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.add_enabled_ui(!busy && !revoked, |ui| {
                            for capability in GRANT_CAPABILITIES {
                                let mut checked = state.draft.caps.contains(capability);
                                if ui
                                    .checkbox(&mut checked, capability_label(capability))
                                    .changed()
                                {
                                    state.draft.toggle(capability);
                                }
                            }
                        });
                    });
                    let dirty = !grant_set_matches(&current_grants, &state.draft.caps);
                    let can_apply = admin_token_configured && !busy && !revoked && dirty;
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(can_apply, egui::Button::new("apply grants (replace set)"))
                            .clicked()
                        {
                            *requested = Some(Request::ApplyGrantSet);
                        }
                        if ui
                            .add_enabled(
                                admin_token_configured && !busy && !revoked,
                                egui::Button::new("revoke device key (--revoke)"),
                            )
                            .clicked()
                        {
                            *requested = Some(Request::RevokeGrantDevice);
                        }
                    });
                    let hint = if revoked {
                        "revoked device: no drive capability is usable until unrevoked."
                    } else if !admin_token_configured {
                        "admin token required (save/paste above)."
                    } else if !dirty {
                        "grant set unchanged."
                    } else {
                        "apply replaces the full set: unchecking all grants returns the device to read-only."
                    };
                    ui.label(RichText::new(hint).small().color(theme::ui::TEXT_MUTED));
                }
            }
        }
    }

    if let Some((level, text)) = &state.notice {
        let color = match level {
            Level::Info => theme::ui::GOOD,
            Level::Warn => theme::ui::WARN,
            Level::Error => theme::ui::BAD,
        };
        ui.label(RichText::new(text).color(color));
    }
}

fn grant_set_matches(grants: &[String], caps: &BTreeSet<String>) -> bool {
    grants.len() == caps.len() && grants.iter().all(|g| caps.contains(g))
}

fn device_option_label(device: &GrantDevice, own_key_id: &str) -> String {
    let own = if device.key_id == own_key_id {
        " (this device)"
    } else {
        ""
    };
    let grants = if device.grants.is_empty() {
        "read-only".to_string()
    } else {
        device.grants.join(",")
    };
    format!("{}{own} — {grants}", device.key_id)
}

fn capability_label(capability: &str) -> String {
    if capability == "start_worktree" {
        "start_worktree (fleet-level)".to_string()
    } else {
        capability.to_string()
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
            grants: grants.iter().map(|g| g.to_string()).collect(),
            revoked: false,
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
    fn grant_dirty_check_ignores_server_grant_order() {
        let caps = ["prompt", "read_tail"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert!(grant_set_matches(
            &["read_tail".to_string(), "prompt".to_string()],
            &caps
        ));
        assert!(!grant_set_matches(&["read_tail".to_string()], &caps));
        assert!(grant_set_matches(&[], &BTreeSet::new()));
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
    fn start_worktree_gets_a_distinct_fleet_level_label() {
        assert!(capability_label("start_worktree").contains("fleet-level"));
        assert_eq!(capability_label("read_tail"), "read_tail");
    }
}
