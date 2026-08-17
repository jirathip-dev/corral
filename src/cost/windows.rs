//! Rolling window definitions for the cost meter (G34).
//!
//! Windows are rolling from "now", not calendar-aligned: `Weekly` is the
//! trailing 7 days and `Monthly` the trailing 30 days, not "since the 1st".
//! This matches the issue's "rolling 5h / weekly / monthly" language and
//! avoids a discontinuity at midnight/month-boundary.

use serde::{Deserialize, Serialize};

/// A single usage event with a wall-clock timestamp and its priced cost
/// (`None` when the model/provider has no pricing entry — the event still
/// existed, but contributes $0 to any window sum, so totals are a lower
/// bound rather than a fabricated number).
#[derive(Debug, Clone)]
pub struct UsageEvent {
    pub ts_ms: u64,
    pub usd: Option<f64>,
    /// Workspace path the event was recorded against, when the source
    /// carries one (used only for D30 per-agent cost attribution — ignored
    /// by the provider-level windowed meter).
    pub workspace_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    FiveHour,
    Weekly,
    Monthly,
}

impl Window {
    pub const ALL: [Window; 3] = [Window::FiveHour, Window::Weekly, Window::Monthly];

    pub fn duration_ms(self) -> u64 {
        match self {
            Window::FiveHour => 5 * 3600 * 1000,
            Window::Weekly => 7 * 24 * 3600 * 1000,
            Window::Monthly => 30 * 24 * 3600 * 1000,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Window::FiveHour => "5h",
            Window::Weekly => "weekly",
            Window::Monthly => "monthly",
        }
    }
}

/// Sum priced cost for events falling inside `[now_ms - window, now_ms]`.
/// Time-correct by construction: callers pass store timestamps, never rely
/// on all-time totals.
pub fn sum_usd_in_window(events: &[UsageEvent], window: Window, now_ms: u64) -> f64 {
    let start = now_ms.saturating_sub(window.duration_ms());
    events
        .iter()
        .filter(|e| e.ts_ms >= start && e.ts_ms <= now_ms)
        .filter_map(|e| e.usd)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ts_ms: u64, usd: Option<f64>) -> UsageEvent {
        UsageEvent { ts_ms, usd, workspace_path: None }
    }

    #[test]
    fn sums_only_events_inside_the_window() {
        let now = 1_000_000_000u64;
        let events = vec![
            ev(now - Window::FiveHour.duration_ms() - 1, Some(9.0)), // just outside
            ev(now - Window::FiveHour.duration_ms() + 1, Some(1.0)), // just inside
            ev(now - 1000, Some(2.0)),
            ev(now + 1, Some(5.0)), // future: excluded
        ];
        assert_eq!(sum_usd_in_window(&events, Window::FiveHour, now), 3.0);
    }

    #[test]
    fn unpriced_events_contribute_zero_not_error() {
        let now = 10_000u64;
        let events = vec![ev(now, None), ev(now, Some(1.5))];
        assert_eq!(sum_usd_in_window(&events, Window::Monthly, now), 1.5);
    }

    #[test]
    fn windows_are_nested_5h_within_weekly_within_monthly() {
        assert!(Window::FiveHour.duration_ms() < Window::Weekly.duration_ms());
        assert!(Window::Weekly.duration_ms() < Window::Monthly.duration_ms());
    }
}
