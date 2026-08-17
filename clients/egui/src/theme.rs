//! Dark-dashboard theme pass: a custom `egui::Visuals` palette (not the
//! default flat dark) plus the shared badge/color vocabulary the board
//! uses so state/kind/CI colors live in exactly one place.
//!
//! Palette: charcoal canvas (`#0d1117`-family), one accent (teal), and
//! distinct hues for the four agent states and the four waiting-on kinds
//! (P4: ApproveTool / AnswerQuestion / Menu / Crash must render DISTINCT).

use eframe::egui::{Color32, Visuals};

/// State colors — each of the four (plus unknown) gets a distinct hue.
pub mod state {
    use super::Color32;

    pub const IDLE: Color32 = Color32::from_rgb(0x8b, 0x94, 0x9e);
    pub const WORKING: Color32 = Color32::from_rgb(0xd2, 0x99, 0x22);
    pub const BLOCKED: Color32 = Color32::from_rgb(0xf8, 0x51, 0x49);
    pub const DONE: Color32 = Color32::from_rgb(0x3f, 0xb9, 0x50);
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

/// Waiting-on kind badge colors — four distinct hues.
pub mod kind {
    use super::Color32;

    pub const APPROVE_TOOL: Color32 = Color32::from_rgb(0xbc, 0x8c, 0xff);
    pub const ANSWER_QUESTION: Color32 = Color32::from_rgb(0xe3, 0xb3, 0x41);
    pub const MENU: Color32 = Color32::from_rgb(0x39, 0xc5, 0xcf);
    pub const CRASH: Color32 = Color32::from_rgb(0xff, 0x7b, 0x72);

    pub fn of(kind: super::WaitingOnKindLike) -> Color32 {
        match kind {
            super::WaitingOnKindLike::ApproveTool => APPROVE_TOOL,
            super::WaitingOnKindLike::AnswerQuestion => ANSWER_QUESTION,
            super::WaitingOnKindLike::Menu => MENU,
            super::WaitingOnKindLike::Crash => CRASH,
        }
    }
}

/// CI verdict colors.
pub mod ci {
    use super::Color32;

    pub const SUCCESS: Color32 = Color32::from_rgb(0x3f, 0xb9, 0x50);
    pub const FAILURE: Color32 = Color32::from_rgb(0xf8, 0x51, 0x49);
    pub const PENDING: Color32 = Color32::from_rgb(0xe3, 0xb3, 0x41);
    pub const UNKNOWN: Color32 = Color32::from_rgb(0x6e, 0x76, 0x81);

    pub fn of(kind: super::CiStatusLike) -> Color32 {
        match kind {
            super::CiStatusLike::Success => SUCCESS,
            super::CiStatusLike::Failure => FAILURE,
            super::CiStatusLike::Pending => PENDING,
            super::CiStatusLike::Unknown => UNKNOWN,
        }
    }
}

/// Accent + semantic colors shared across views.
pub mod ui {
    use super::Color32;

    pub const ACCENT: Color32 = Color32::from_rgb(0x2d, 0xd4, 0xbf);
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x14, 0x8f, 0x84);
    pub const GOOD: Color32 = Color32::from_rgb(0x3f, 0xb9, 0x50);
    pub const BAD: Color32 = Color32::from_rgb(0xf8, 0x51, 0x49);
    pub const WARN: Color32 = Color32::from_rgb(0xe3, 0xb3, 0x41);
    pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xe6, 0xed, 0xf3);
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x8b, 0x94, 0x9e);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitingOnKindLike {
    ApproveTool,
    AnswerQuestion,
    Menu,
    Crash,
}

impl From<crate::model::WaitingOnKind> for WaitingOnKindLike {
    fn from(k: crate::model::WaitingOnKind) -> Self {
        match k {
            crate::model::WaitingOnKind::ApproveTool => Self::ApproveTool,
            crate::model::WaitingOnKind::AnswerQuestion => Self::AnswerQuestion,
            crate::model::WaitingOnKind::Menu => Self::Menu,
            crate::model::WaitingOnKind::Crash => Self::Crash,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiStatusLike {
    Success,
    Failure,
    Pending,
    Unknown,
}

impl From<crate::model::CiStatus> for CiStatusLike {
    fn from(s: crate::model::CiStatus) -> Self {
        match s {
            crate::model::CiStatus::Success => Self::Success,
            crate::model::CiStatus::Failure => Self::Failure,
            crate::model::CiStatus::Pending => Self::Pending,
            crate::model::CiStatus::Unknown => Self::Unknown,
        }
    }
}

/// Build the dark-dashboard `Visuals` from the palette above.
pub fn dark_dashboard() -> Visuals {
    use eframe::egui::{style::Widgets, CornerRadius, Stroke};

    let mut v = Visuals::dark();
    v.panel_fill = Color32::from_rgb(0x0d, 0x11, 0x17);
    v.window_fill = Color32::from_rgb(0x10, 0x15, 0x1c);
    v.extreme_bg_color = Color32::from_rgb(0x16, 0x1b, 0x22);
    v.faint_bg_color = Color32::from_rgb(0x1c, 0x21, 0x28);
    v.code_bg_color = Color32::from_rgb(0x16, 0x1b, 0x22);
    v.selection.bg_fill = Color32::from_rgb(0x1f, 0x3a, 0x3d);
    v.selection.stroke = Stroke::new(1.0, ui::ACCENT_DIM);
    v.hyperlink_color = ui::ACCENT;
    v.override_text_color = None;
    v.warn_fg_color = ui::WARN;
    v.error_fg_color = ui::BAD;

    v.widgets = Widgets {
        noninteractive: egui::style::WidgetVisuals {
            bg_fill: Color32::from_rgb(0x16, 0x1b, 0x22),
            weak_bg_fill: Color32::from_rgb(0x21, 0x26, 0x2d),
            bg_stroke: Stroke::new(1.0, Color32::from_rgb(0x30, 0x36, 0x3d)),
            fg_stroke: Stroke::new(1.0, ui::TEXT_STRONG),
            corner_radius: CornerRadius::same(4),
            expansion: 0.0,
        },
        inactive: egui::style::WidgetVisuals {
            bg_fill: Color32::from_rgb(0x21, 0x26, 0x2d),
            weak_bg_fill: Color32::from_rgb(0x1c, 0x21, 0x28),
            bg_stroke: Stroke::new(1.0, Color32::from_rgb(0x30, 0x36, 0x3d)),
            fg_stroke: Stroke::new(1.0, ui::TEXT_STRONG),
            corner_radius: CornerRadius::same(4),
            expansion: 0.0,
        },
        hovered: egui::style::WidgetVisuals {
            bg_fill: Color32::from_rgb(0x2b, 0x32, 0x3b),
            weak_bg_fill: Color32::from_rgb(0x2b, 0x32, 0x3b),
            bg_stroke: Stroke::new(1.0, Color32::from_rgb(0x4a, 0x53, 0x5f)),
            fg_stroke: Stroke::new(1.0, ui::TEXT_STRONG),
            corner_radius: CornerRadius::same(4),
            expansion: 1.0,
        },
        active: egui::style::WidgetVisuals {
            bg_fill: Color32::from_rgb(0x1a, 0x3f, 0x3d),
            weak_bg_fill: Color32::from_rgb(0x14, 0x8f, 0x84),
            bg_stroke: Stroke::new(1.0, ui::ACCENT_DIM),
            fg_stroke: Stroke::new(1.0, ui::TEXT_STRONG),
            corner_radius: CornerRadius::same(4),
            expansion: 0.0,
        },
        open: egui::style::WidgetVisuals {
            bg_fill: Color32::from_rgb(0x1a, 0x3f, 0x3d),
            weak_bg_fill: Color32::from_rgb(0x14, 0x8f, 0x84),
            bg_stroke: Stroke::new(1.0, ui::ACCENT_DIM),
            fg_stroke: Stroke::new(1.0, ui::TEXT_STRONG),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentState, CiStatus, WaitingOnKind};

    #[test]
    fn every_state_kind_and_ci_has_a_color() {
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

        let kinds = [
            WaitingOnKind::ApproveTool,
            WaitingOnKind::AnswerQuestion,
            WaitingOnKind::Menu,
            WaitingOnKind::Crash,
        ];
        let colors: Vec<_> = kinds.iter().map(|k| kind::of((*k).into())).collect();
        let distinct: std::collections::HashSet<_> = colors.iter().collect();
        assert_eq!(
            distinct.len(),
            kinds.len(),
            "waiting-on kinds must render distinct"
        );

        let cis = [
            CiStatus::Success,
            CiStatus::Failure,
            CiStatus::Pending,
            CiStatus::Unknown,
        ];
        let colors: Vec<_> = cis.iter().map(|c| ci::of((*c).into())).collect();
        let distinct: std::collections::HashSet<_> = colors.iter().collect();
        assert_eq!(
            distinct.len(),
            cis.len(),
            "CI statuses must render distinct"
        );
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
}
