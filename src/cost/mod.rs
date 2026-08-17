//! Cost / usage-% meter (G34, issue #34) — per-provider USD spend and % of
//! configured cap over rolling 5h / weekly / monthly windows, for opencode,
//! claude, and codex.
//!
//! ## Data flow
//!
//! Each provider has its own read-only, bounded session-store reader
//! ([`opencode`], [`claude`], [`codex`]) producing a flat `Vec<UsageEvent>`
//! `{ts_ms, usd, workspace_path}` over the trailing 30 days (the widest
//! rolling window this meter tracks). [`windows::sum_usd_in_window`] then
//! slices that same list into the 5h/weekly/monthly sums — one query per
//! provider, not one per window. [`compute_all`] is the entry point `GET
//! /cost` and the alert watchdog both call.
//!
//! ## D-083 (redaction / no leaked chat content)
//!
//! No reader ever extracts message/response text — see each reader's own
//! module docs for exactly which fields it touches. A `UsageEvent` has no
//! field capable of carrying prose, so nothing downstream (the API
//! response, logs, the D30 per-agent cache) can leak chat content through
//! this path even by accident.
//!
//! ## Alert before exhaustion
//!
//! [`spawn_alert_watchdog`] recomputes usage on an interval and logs a
//! `tracing::warn!` the moment a window crosses [`config::CostConfig`]'s
//! alert threshold — mirroring fleet-watch's "flag before agents idle"
//! shape, but for spend instead of agent liveness. The same threshold
//! computation rides on every `GET /cost` response as `status`, so a
//! client polling the snapshot sees the same signal without needing to
//! also read daemon logs.

pub mod agent_cache;
pub mod claude;
pub mod codex;
pub mod config;
pub mod opencode;
pub mod pricing;
pub mod windows;

pub use config::CostConfig;
pub use windows::{UsageEvent, Window};

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Opencode,
    Claude,
    Codex,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::Opencode, Provider::Claude, Provider::Codex];

    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Opencode => "opencode",
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }
}

/// fleet-watch-style severity: `Problem` means "flag before exhaustion",
/// not "already exhausted" — the alert threshold is meant to fire early.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertStatus {
    Ok,
    Warning,
    Problem,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowUsage {
    pub window: Window,
    pub usd: f64,
    pub cap_usd: f64,
    /// `true` when `cap_usd` is the built-in placeholder, not a value the
    /// operator configured via `CORRAL_COST_CAP_*` — see [`config`] docs.
    pub cap_is_placeholder: bool,
    pub pct_of_cap: f64,
    pub status: AlertStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderUsage {
    pub provider: Provider,
    /// Whether the provider's session store was found at all (a fresh
    /// install, or a provider Guy doesn't use, is "no data", not an error).
    pub store_found: bool,
    pub windows: Vec<WindowUsage>,
}

/// $CORRAL_OPENCODE_DB, or the opencode default install path.
pub fn opencode_db_path() -> PathBuf {
    std::env::var("CORRAL_OPENCODE_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".local/share/opencode/opencode.db"))
}

/// $CORRAL_CLAUDE_DIR, or `~/.claude/projects`.
pub fn claude_dir_path() -> PathBuf {
    std::env::var("CORRAL_CLAUDE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".claude/projects"))
}

/// $CORRAL_CODEX_DIR, or `~/.codex/sessions`.
pub fn codex_dir_path() -> PathBuf {
    std::env::var("CORRAL_CODEX_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".codex/sessions"))
}

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

/// Compute per-provider windowed USD + % of cap for all three providers,
/// reading from the default env-configured store paths. Each provider is
/// read once over the trailing 30 days; the 5h/weekly windows are cheap
/// slices of that same result.
pub async fn compute_all(config: &CostConfig, now_ms: u64) -> Vec<ProviderUsage> {
    compute_all_with_paths(
        config,
        now_ms,
        &opencode_db_path(),
        &claude_dir_path(),
        &codex_dir_path(),
    )
    .await
}

/// [`compute_all`]'s core, parameterized on store paths — lets tests point
/// at fixtures without mutating process-wide env vars (which would race
/// against other tests in this binary).
pub async fn compute_all_with_paths(
    config: &CostConfig,
    now_ms: u64,
    opencode_path: &std::path::Path,
    claude_path: &std::path::Path,
    codex_path: &std::path::Path,
) -> Vec<ProviderUsage> {
    let start_ms = now_ms.saturating_sub(Window::Monthly.duration_ms());

    let opencode_found = opencode_path.exists();
    let claude_found = tokio::fs::metadata(claude_path)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false);
    let codex_found = tokio::fs::metadata(codex_path)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false);

    let (opencode_events, claude_events, codex_events) = tokio::join!(
        opencode::opencode_usage(opencode_path, start_ms, now_ms),
        claude::claude_usage(claude_path, start_ms, now_ms),
        codex::codex_usage(codex_path, start_ms, now_ms),
    );

    vec![
        provider_usage(
            Provider::Opencode,
            &opencode_events,
            opencode_found,
            config,
            now_ms,
        ),
        provider_usage(
            Provider::Claude,
            &claude_events,
            claude_found,
            config,
            now_ms,
        ),
        provider_usage(Provider::Codex, &codex_events, codex_found, config, now_ms),
    ]
}

fn provider_usage(
    provider: Provider,
    events: &[UsageEvent],
    store_found: bool,
    config: &CostConfig,
    now_ms: u64,
) -> ProviderUsage {
    let windows = Window::ALL
        .iter()
        .map(|&window| {
            let usd = windows::sum_usd_in_window(events, window, now_ms);
            let (cap_usd, cap_is_placeholder) = config.cap_for(provider, window);
            let pct_of_cap = if cap_usd > 0.0 {
                usd / cap_usd * 100.0
            } else {
                0.0
            };
            let status = if pct_of_cap >= config.alert_threshold_pct {
                AlertStatus::Problem
            } else if pct_of_cap >= config.warn_threshold_pct {
                AlertStatus::Warning
            } else {
                AlertStatus::Ok
            };
            WindowUsage {
                window,
                usd,
                cap_usd,
                cap_is_placeholder,
                pct_of_cap,
                status,
            }
        })
        .collect();
    ProviderUsage {
        provider,
        store_found,
        windows,
    }
}

/// Recompute usage on `interval` and `tracing::warn!` the moment any
/// provider/window is at or above [`CostConfig::alert_threshold_pct`] — the
/// "flag exhaustion before agents idle" watchdog. Call once from `main`.
pub fn spawn_alert_watchdog(interval: Duration) {
    tokio::spawn(async move {
        loop {
            let config = CostConfig::from_env();
            let now_ms = crate::core::util::now_millis();
            for provider in compute_all(&config, now_ms).await {
                for w in &provider.windows {
                    match w.status {
                        AlertStatus::Problem => tracing::warn!(
                            provider = provider.provider.as_str(),
                            window = ?w.window,
                            usd = w.usd,
                            cap_usd = w.cap_usd,
                            pct_of_cap = w.pct_of_cap,
                            cap_is_placeholder = w.cap_is_placeholder,
                            "cost meter: window at/above alert threshold — exhaustion risk"
                        ),
                        AlertStatus::Warning => tracing::info!(
                            provider = provider.provider.as_str(),
                            window = ?w.window,
                            pct_of_cap = w.pct_of_cap,
                            "cost meter: window approaching cap"
                        ),
                        AlertStatus::Ok => {}
                    }
                }
            }
            tokio::time::sleep(interval).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn provider_usage_reports_ok_below_threshold_and_problem_above() {
        let mut config = CostConfig::from_env();
        config.alert_threshold_pct = 90.0;
        config.warn_threshold_pct = 70.0;
        let now_ms = 1_000_000_000u64;

        let cheap = vec![UsageEvent {
            ts_ms: now_ms,
            usd: Some(0.01),
            workspace_path: None,
        }];
        let usage = provider_usage(Provider::Claude, &cheap, true, &config, now_ms);
        let five_h = usage
            .windows
            .iter()
            .find(|w| w.window == Window::FiveHour)
            .unwrap();
        assert_eq!(five_h.status, AlertStatus::Ok);

        let (cap, _) = config.cap_for(Provider::Claude, Window::FiveHour);
        let expensive = vec![UsageEvent {
            ts_ms: now_ms,
            usd: Some(cap * 0.95),
            workspace_path: None,
        }];
        let usage = provider_usage(Provider::Claude, &expensive, true, &config, now_ms);
        let five_h = usage
            .windows
            .iter()
            .find(|w| w.window == Window::FiveHour)
            .unwrap();
        assert_eq!(five_h.status, AlertStatus::Problem);
    }

    #[tokio::test]
    async fn store_not_found_reports_false_not_an_error() {
        let config = CostConfig::from_env();
        let nonexistent = std::path::Path::new("/nonexistent/corral-g34-cost-test");
        let usage = compute_all_with_paths(
            &config,
            crate::core::util::now_millis(),
            nonexistent,
            nonexistent,
            nonexistent,
        )
        .await;
        assert!(usage.iter().all(|p| !p.store_found));
        assert!(usage.iter().all(|p| p.windows.iter().all(|w| w.usd == 0.0)));
    }
}
