//! Dark-dashboard theme pass: a custom `egui::Visuals` palette (not the
//! default flat dark) plus the shared badge/color vocabulary the board
//! uses so state/kind/CI colors live in exactly one place.
//!
//! Palette: charcoal canvas (`#0d1117`-family), one accent (teal), and
//! distinct hues for the four agent states and the four waiting-on kinds
//! (P4: ApproveTool / AnswerQuestion / Menu / Crash must render DISTINCT).
//!
//! Agent-state colors/labels/ranks/marks are authoritative in
//! `contracts/state-tokens.json` (shared with the iOS notifier). This file
//! keeps native `Color32` consts for the egui dark board surface but must
//! never diverge from that contract: the `state_tokens_match_contract`
//! test below reads the JSON and fails on any hex/label/rank/mark drift.

use eframe::egui::{Color32, Visuals};

/// State colors — each of the four (plus unknown) gets a distinct hue.
pub mod state {
    use super::Color32;

    // Hexes mirror the `dark` column of contracts/state-tokens.json
    // (egui board is a dark-theme surface).
    pub const IDLE: Color32 = Color32::from_rgb(0x8b, 0x94, 0x9e);
    pub const WORKING: Color32 = Color32::from_rgb(0x58, 0xa6, 0xff);
    pub const BLOCKED: Color32 = Color32::from_rgb(0xf8, 0x51, 0x49);
    pub const DONE: Color32 = Color32::from_rgb(0xd2, 0x99, 0x22);
    pub const UNKNOWN: Color32 = Color32::from_rgb(0x6e, 0x76, 0x81);

    pub fn of(kind: super::AgentStateLike) -> Color32 {
        match kind {
            super::AgentStateLike::Idle => IDLE,
            super::AgentStateLike::Working => WORKING,
            super::AgentStateLike::Blocked => BLOCKED,
            super::AgentStateLike::Done => DONE,
            super::AgentStateLike::Unknown => UNKNOWN,
        }
    }
}

/// Accent + semantic colors shared across views.
pub mod ui {
    use super::Color32;

    /// Exact surface tokens from `docs/design/corral-ux-prototype-spec.md`.
    /// Keep these in one place so the board cannot drift by inventing a
    /// near-miss gray for one pane or recent-output block.
    pub const BG: Color32 = Color32::from_rgb(0x0d, 0x11, 0x17);
    pub const PANEL: Color32 = Color32::from_rgb(0x10, 0x15, 0x1c);
    pub const PANEL2: Color32 = Color32::from_rgb(0x16, 0x1b, 0x22);
    pub const PANEL3: Color32 = Color32::from_rgb(0x1c, 0x21, 0x28);
    pub const LINE: Color32 = Color32::from_rgb(0x30, 0x36, 0x3d);
    pub const INK: Color32 = Color32::from_rgb(0xe6, 0xed, 0xf3);
    pub const MUTED: Color32 = Color32::from_rgb(0x8b, 0x94, 0x9e);
    pub const ACCENT: Color32 = Color32::from_rgb(0x2d, 0xd4, 0xbf);
    /// Dark foreground for enabled accent controls, including Send.
    pub const SEND_INK: Color32 = Color32::from_rgb(0x05, 0x24, 0x20);
    /// Role blue is shared by the recent-output user-message label and the
    /// approved prototype's `--working`-family role cue.
    pub const USER_BLUE: Color32 = Color32::from_rgb(0x6e, 0xa8, 0xff);
    pub const USER_TINT: Color32 = Color32::from_rgb(0x12, 0x26, 0x3f);
    /// The approved prototype uses this slightly darker outer frame border.
    pub const FRAME_BORDER: Color32 = Color32::from_rgb(0x2a, 0x2f, 0x37);
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x14, 0x8f, 0x84);
    pub const GOOD: Color32 = Color32::from_rgb(0x3f, 0xb9, 0x50);
    pub const BAD: Color32 = Color32::from_rgb(0xf8, 0x51, 0x49);
    pub const WARN: Color32 = Color32::from_rgb(0xe3, 0xb3, 0x41);
    pub const TEXT_STRONG: Color32 = INK;
    pub const TEXT_MUTED: Color32 = MUTED;
    pub const DIRTY: Color32 = Color32::from_rgb(0xff, 0xa6, 0x57);
}

/// Trait-seam enums so `theme` never depends on `model` (pure color map).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStateLike {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

impl From<crate::model::AgentState> for AgentStateLike {
    fn from(s: crate::model::AgentState) -> Self {
        match s {
            crate::model::AgentState::Idle => Self::Idle,
            crate::model::AgentState::Working => Self::Working,
            crate::model::AgentState::Blocked => Self::Blocked,
            crate::model::AgentState::Done => Self::Done,
            crate::model::AgentState::Unknown => Self::Unknown,
        }
    }
}

impl AgentStateLike {
    /// Raw herdr state token label (`contracts/state-tokens.json` "label",
    /// #354 v2: no Corral-invented wording).
    pub fn label(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Unknown => "unknown",
        }
    }

    /// Attention-ordered rank (`contracts/state-tokens.json` "rank",
    /// 0 = highest priority). v2 board order: blocked → working → idle →
    /// unknown; a wire `done` ranks with idle (its herdr fallback).
    pub fn rank(self) -> u8 {
        match self {
            Self::Blocked => 0,
            Self::Working => 1,
            Self::Idle => 2,
            Self::Done => 2,
            Self::Unknown => 3,
        }
    }

    /// Contract mark token (`contracts/state-tokens.json` "mark").
    pub fn mark(self) -> &'static str {
        match self {
            Self::Blocked => "alert",
            Self::Done => "check",
            Self::Working => "ring",
            Self::Idle => "dot",
            Self::Unknown => "query",
        }
    }

    /// Display glyph for the mark (color is never the only channel).
    pub fn mark_glyph(self) -> &'static str {
        match self {
            Self::Blocked => "!",
            Self::Done => "\u{2713}",
            Self::Working => "\u{25CB}",
            Self::Idle => "\u{25E6}",
            Self::Unknown => "?",
        }
    }
}

/// Build the dark-dashboard `Visuals` from the palette above.
pub fn dark_dashboard() -> Visuals {
    use eframe::egui::{CornerRadius, Stroke, style::Widgets};

    let mut v = Visuals::dark();
    v.panel_fill = ui::BG;
    v.window_fill = ui::PANEL;
    v.extreme_bg_color = ui::PANEL2;
    v.faint_bg_color = ui::PANEL3;
    v.code_bg_color = ui::PANEL2;
    v.window_stroke = Stroke::new(1.0, ui::LINE);
    v.selection.bg_fill = Color32::from_rgb(0x1f, 0x3a, 0x3d);
    v.selection.stroke = Stroke::new(1.0, ui::ACCENT_DIM);
    v.hyperlink_color = ui::ACCENT;
    v.override_text_color = None;
    v.warn_fg_color = ui::WARN;
    v.error_fg_color = ui::BAD;

    v.widgets = Widgets {
        noninteractive: egui::style::WidgetVisuals {
            bg_fill: ui::PANEL2,
            weak_bg_fill: ui::PANEL3,
            bg_stroke: Stroke::new(1.0, ui::LINE),
            fg_stroke: Stroke::new(1.0, ui::INK),
            corner_radius: CornerRadius::same(4),
            expansion: 0.0,
        },
        inactive: egui::style::WidgetVisuals {
            bg_fill: ui::PANEL2,
            weak_bg_fill: ui::PANEL3,
            bg_stroke: Stroke::new(1.0, ui::LINE),
            fg_stroke: Stroke::new(1.0, ui::INK),
            corner_radius: CornerRadius::same(4),
            expansion: 0.0,
        },
        hovered: egui::style::WidgetVisuals {
            bg_fill: ui::PANEL3,
            weak_bg_fill: ui::PANEL3,
            bg_stroke: Stroke::new(1.0, ui::LINE),
            fg_stroke: Stroke::new(1.0, ui::INK),
            corner_radius: CornerRadius::same(4),
            expansion: 1.0,
        },
        active: egui::style::WidgetVisuals {
            bg_fill: ui::PANEL3,
            weak_bg_fill: ui::ACCENT_DIM,
            bg_stroke: Stroke::new(1.0, ui::ACCENT_DIM),
            fg_stroke: Stroke::new(1.0, ui::INK),
            corner_radius: CornerRadius::same(4),
            expansion: 0.0,
        },
        open: egui::style::WidgetVisuals {
            bg_fill: ui::PANEL3,
            weak_bg_fill: ui::ACCENT_DIM,
            bg_stroke: Stroke::new(1.0, ui::ACCENT_DIM),
            fg_stroke: Stroke::new(1.0, ui::INK),
            corner_radius: CornerRadius::same(4),
            expansion: 0.0,
        },
    };

    v.slider_trailing_fill = true;
    v.button_frame = true;
    v.collapsing_header_frame = false;
    v.window_shadow = eframe::egui::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(0, 0, 0, 120),
    };
    v
}

/// Fonts the board renders with on BOTH surfaces (the desktop app's
/// `configure_fonts` in app.rs and the wasm app's `WebCorralApp::new` in
/// web.rs install these): the egui defaults plus the toolkit's built-in
/// monospace default (Hack) appended to the proportional chain. Mark glyphs
/// the default proportional trio (Ubuntu-Light/NotoEmoji-Regular/
/// emoji-icon-font) lacks then resolve from Hack instead of epaint's tofu
/// replacement U+25A1 — the #358 idle marker U+25E6 is the live case (the
/// wire never emits `done`, and its U+2713 mark exists in no
/// toolkit-default font, so it stays out of scope). No bundled font assets:
/// Hack ships inside egui/epaint itself (epaint_default_fonts), so nothing
/// from the retired #347 bundle machinery is re-added.
pub(crate) fn board_font_definitions() -> eframe::egui::FontDefinitions {
    let mut fonts = eframe::egui::FontDefinitions::default();
    fonts
        .families
        .entry(eframe::egui::FontFamily::Proportional)
        .or_default()
        .push("Hack".to_owned());
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgentState;

    #[test]
    fn every_state_has_a_distinct_color() {
        let states = [
            AgentState::Idle,
            AgentState::Working,
            AgentState::Blocked,
            AgentState::Done,
            AgentState::Unknown,
        ];
        let colors: Vec<_> = states.iter().map(|s| state::of((*s).into())).collect();
        let distinct: std::collections::HashSet<_> = colors.iter().collect();
        assert_eq!(distinct.len(), states.len(), "states must render distinct");
    }

    #[test]
    fn dark_dashboard_is_not_default_dark() {
        let ours = dark_dashboard();
        let default = Visuals::dark();
        assert_ne!(ours.panel_fill, default.panel_fill, "custom panel fill");
        assert_ne!(
            ours.widgets.inactive.bg_fill,
            default.widgets.inactive.bg_fill
        );
        assert_ne!(ours.window_fill, default.window_fill);
    }

    #[test]
    fn role_blue_matches_approved_prototype_token() {
        assert_eq!(ui::USER_BLUE, Color32::from_rgb(0x6e, 0xa8, 0xff));
    }

    /// Drift guard for the shared state token contract
    /// (`contracts/state-tokens.json`). If the checked-in JSON or these egui
    /// consts/accessors move, this test fails loudly rather than letting the
    /// two client surfaces diverge again.
    #[derive(serde::Deserialize)]
    struct StateToken {
        state: String,
        rank: u8,
        label: String,
        dark: String,
        light: String,
        mark: String,
    }

    fn state_like_from_str(s: &str) -> Option<super::AgentStateLike> {
        match s {
            "idle" => Some(super::AgentStateLike::Idle),
            "working" => Some(super::AgentStateLike::Working),
            "blocked" => Some(super::AgentStateLike::Blocked),
            "done" => Some(super::AgentStateLike::Done),
            "unknown" => Some(super::AgentStateLike::Unknown),
            _ => None,
        }
    }

    fn parse_hex(s: &str) -> (u8, u8, u8) {
        let h = s.strip_prefix('#').expect("token hex starts with #");
        assert_eq!(h.len(), 6, "token hex is six digits");
        (
            u8::from_str_radix(&h[0..2], 16).unwrap(),
            u8::from_str_radix(&h[2..4], 16).unwrap(),
            u8::from_str_radix(&h[4..6], 16).unwrap(),
        )
    }

    #[test]
    fn state_tokens_match_contract() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../contracts/state-tokens.json"
        );
        let raw = std::fs::read_to_string(path).expect("read contracts/state-tokens.json");
        let tokens: Vec<StateToken> =
            serde_json::from_str(&raw).expect("parse contracts/state-tokens.json");

        assert_eq!(tokens.len(), 5, "contract has exactly five states");

        let mut seen = std::collections::HashSet::new();
        for token in &tokens {
            let like = state_like_from_str(&token.state)
                .unwrap_or_else(|| panic!("unknown state in contract: {}", token.state));
            let (r, g, b) = parse_hex(&token.dark);
            let _light = parse_hex(&token.light);
            assert_eq!(
                state::of(like),
                Color32::from_rgb(r, g, b),
                "egui dark const diverged from contract for {}",
                token.state
            );
            assert_eq!(
                like.label(),
                token.label,
                "label drifted for {}",
                token.state
            );
            assert_eq!(like.rank(), token.rank, "rank drifted for {}", token.state);
            assert_eq!(
                like.mark(),
                token.mark,
                "mark token drifted for {}",
                token.state
            );
            // Distinct mark + label per state (AC5).
            assert!(
                seen.insert((token.label.clone(), token.mark.clone())),
                "duplicate label/mark pair in contract for {}",
                token.state
            );
        }
    }
}
