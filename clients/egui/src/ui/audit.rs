//! Audit view: the host's hash-chained audit log (`GET /audit`, admin
//! bearer token). The device cannot read it by default — the view shows
//! how to provide the host admin token (auto-read on localhost, or paste),
//! then renders entries + the chain-validity verdict.

use eframe::egui::{Color32, RichText, ScrollArea, Ui};

use crate::protocol::{AuditEntry, AuditView};
use crate::theme;

/// Renders the audit view. `entries` is the last fetched view.
pub fn show(
    ui: &mut Ui,
    view: &Option<Result<AuditView, String>>,
    admin_token_configured: bool,
    loading: bool,
    request_refresh: &mut dyn FnMut(),
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("AUDIT LOG")
                .strong()
                .color(theme::ui::TEXT_STRONG),
        );
        if loading {
            ui.spinner();
        }
        if ui.button("refresh").clicked() {
            request_refresh();
        }
    });
    ui.label(
        RichText::new("host-admin endpoint: grows only on drive writes (auth failures and GETs are never logged).")
            .small()
            .color(theme::ui::TEXT_MUTED),
    );

    if !admin_token_configured {
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "The audit log is host-admin (bearer token). On localhost the host's \
                 admin-token file is used automatically; paste it here to view the log \
                 for a remote host.",
            )
            .color(theme::ui::TEXT_MUTED),
        );
        ui.label(
            RichText::new(
                "Set the admin token in Settings (Audit section); it is stored in the OS \
                 keychain, never on disk in plaintext.",
            )
            .color(theme::ui::TEXT_MUTED),
        );
        return;
    }

    match view {
        None => {
            ui.add_space(12.0);
            ui.label(
                RichText::new("no audit data yet — press refresh.").color(theme::ui::TEXT_MUTED),
            );
        }
        Some(Err(e)) => {
            ui.label(RichText::new(format!("audit error: {e}")).color(theme::ui::BAD));
        }
        Some(Ok(audit)) => {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let (text, color) = if audit.valid {
                    ("chain valid", theme::ui::GOOD)
                } else {
                    ("CHAIN INVALID", theme::ui::BAD)
                };
                ui.label(RichText::new(text).monospace().color(color));
                ui.label(
                    RichText::new(format!("head {}", short_hash(&audit.head)))
                        .monospace()
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
                ui.label(
                    RichText::new(format!("{} entries", audit.entries.len()))
                        .small()
                        .color(theme::ui::TEXT_MUTED),
                );
            });
            ui.separator();
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for entry in &audit.entries {
                        entry_row(ui, entry);
                    }
                });
        }
    }
}

fn entry_row(ui: &mut Ui, entry: &AuditEntry) {
    ui.horizontal_wrapped(|ui| {
        let (outcome_text, color) = outcome_of(&entry.outcome);
        ui.label(
            RichText::new(format!("#{:>4}", entry.seq))
                .monospace()
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        ui.label(
            RichText::new(crate::model::clock_of(entry.ts))
                .monospace()
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        ui.label(
            RichText::new(&entry.capability)
                .monospace()
                .small()
                .color(theme::ui::TEXT_STRONG),
        );
        ui.label(
            RichText::new(&entry.target)
                .monospace()
                .small()
                .color(theme::ui::TEXT_STRONG),
        );
        ui.label(
            RichText::new(short_key(&entry.key_id))
                .monospace()
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        ui.label(
            RichText::new(short_hash(&entry.request_id))
                .monospace()
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
        ui.label(RichText::new(outcome_text).monospace().small().color(color));
        ui.label(
            RichText::new(format!("h {}", short_hash(&entry.hash)))
                .monospace()
                .small()
                .color(theme::ui::TEXT_MUTED),
        );
    });
}

fn outcome_of(outcome: &serde_json::Value) -> (String, Color32) {
    if outcome == "executed" {
        ("executed".to_string(), theme::ui::GOOD)
    } else if let Some(detail) = outcome.get("refused").and_then(|v| v.as_str()) {
        (format!("refused: {detail}"), theme::ui::WARN)
    } else if let Some(detail) = outcome.get("failed").and_then(|v| v.as_str()) {
        (format!("failed: {detail}"), theme::ui::BAD)
    } else {
        (outcome.to_string(), theme::ui::TEXT_MUTED)
    }
}

fn short_hash(hash: &str) -> String {
    let chars: String = hash.chars().take(10).collect();
    format!("{chars}…")
}

fn short_key(key: &str) -> String {
    let chars: String = key.chars().take(14).collect();
    format!("{chars}…")
}

/// Pure helpers exposed for tests.
pub fn outcome_summary(entry: &AuditEntry) -> String {
    outcome_of(&entry.outcome).0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(outcome: serde_json::Value) -> AuditEntry {
        AuditEntry {
            seq: 1,
            ts: 0,
            key_id: "dev_abcdef".into(),
            request_id: "req_1".into(),
            capability: "interrupt".into(),
            target: "herdr:a".into(),
            outcome,
            prev: "prev".into(),
            hash: "hash".into(),
        }
    }

    #[test]
    fn outcome_parsing_covers_all_shapes() {
        assert!(outcome_summary(&entry(serde_json::json!("executed"))).starts_with("executed"));
        assert!(
            outcome_summary(&entry(serde_json::json!({"refused": "not implemented"})))
                .contains("refused")
        );
        assert!(outcome_summary(&entry(serde_json::json!({"failed": "rpc"}))).contains("failed"));
    }

    #[test]
    fn shorts_are_bounded() {
        assert_eq!(short_hash("corral-audit-genesis-v1"), "corral-aud…");
        assert_eq!(short_key("dev_0123456789abcdef"), "dev_0123456789…");
    }
}
