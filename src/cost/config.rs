//! Per-provider cap configuration (G34).
//!
//! Real plan limits are Guy's to set — nobody in this workstream knows the
//! actual opencode-go / claude / codex subscription caps. Every cap below
//! is a **placeholder** unless overridden by an env var, and the API
//! response marks placeholder caps explicitly (`cap_is_placeholder: true`)
//! so a client can visually flag "this percentage is against a guess, not
//! your real limit" rather than presenting it as authoritative.
//!
//! Configure real caps via env vars before trusting the alert:
//!
//! ```text
//! CORRAL_COST_CAP_OPENCODE_5H_USD=<n>      CORRAL_COST_CAP_OPENCODE_WEEKLY_USD=<n>      CORRAL_COST_CAP_OPENCODE_MONTHLY_USD=<n>
//! CORRAL_COST_CAP_CLAUDE_5H_USD=<n>        CORRAL_COST_CAP_CLAUDE_WEEKLY_USD=<n>        CORRAL_COST_CAP_CLAUDE_MONTHLY_USD=<n>
//! CORRAL_COST_CAP_CODEX_5H_USD=<n>         CORRAL_COST_CAP_CODEX_WEEKLY_USD=<n>         CORRAL_COST_CAP_CODEX_MONTHLY_USD=<n>
//! CORRAL_COST_ALERT_THRESHOLD_PCT=<0-100>  # PROBLEM at/above this % of cap (default 90)
//! CORRAL_COST_WARN_THRESHOLD_PCT=<0-100>   # WARNING at/above this % of cap (default 70)
//! ```

use std::collections::HashMap;

use super::windows::Window;
use super::Provider;

/// Order-of-magnitude placeholder caps (USD), roughly shaped like a
/// $20-$200/mo-class subscription spread across the three windows. NOT
/// real plan limits — see module docs.
fn placeholder_cap_usd(window: Window) -> f64 {
    match window {
        Window::FiveHour => 5.0,
        Window::Weekly => 35.0,
        Window::Monthly => 140.0,
    }
}

const DEFAULT_ALERT_THRESHOLD_PCT: f64 = 90.0;
const DEFAULT_WARN_THRESHOLD_PCT: f64 = 70.0;

#[derive(Debug, Clone)]
pub struct CostConfig {
    caps_usd: HashMap<(Provider, Window), (f64, bool)>,
    pub alert_threshold_pct: f64,
    pub warn_threshold_pct: f64,
}

impl CostConfig {
    /// Build config from `CORRAL_COST_CAP_*` / `CORRAL_COST_*_THRESHOLD_PCT`
    /// env vars, filling in placeholders for anything unset.
    pub fn from_env() -> Self {
        let mut caps_usd = HashMap::new();
        for provider in Provider::ALL {
            for window in Window::ALL {
                let var = format!(
                    "CORRAL_COST_CAP_{}_{}_USD",
                    provider.as_str().to_ascii_uppercase(),
                    window_env_segment(window),
                );
                let (usd, is_placeholder) = match std::env::var(&var).ok().and_then(|v| v.parse::<f64>().ok()) {
                    Some(v) if v > 0.0 => (v, false),
                    _ => (placeholder_cap_usd(window), true),
                };
                caps_usd.insert((provider, window), (usd, is_placeholder));
            }
        }
        let alert_threshold_pct = env_pct("CORRAL_COST_ALERT_THRESHOLD_PCT", DEFAULT_ALERT_THRESHOLD_PCT);
        let warn_threshold_pct = env_pct("CORRAL_COST_WARN_THRESHOLD_PCT", DEFAULT_WARN_THRESHOLD_PCT);
        Self { caps_usd, alert_threshold_pct, warn_threshold_pct }
    }

    /// `(cap_usd, is_placeholder)` for a provider/window. Always `Some`-like
    /// (caps default to a documented placeholder rather than absence).
    pub fn cap_for(&self, provider: Provider, window: Window) -> (f64, bool) {
        self.caps_usd
            .get(&(provider, window))
            .copied()
            .unwrap_or_else(|| (placeholder_cap_usd(window), true))
    }
}

fn window_env_segment(window: Window) -> &'static str {
    match window {
        Window::FiveHour => "5H",
        Window::Weekly => "WEEKLY",
        Window::Monthly => "MONTHLY",
    }
}

fn env_pct(var: &str, default: f64) -> f64 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| (0.0..=100.0).contains(v))
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `cargo test` runs tests in the same binary concurrently; both tests
    /// below mutate process-wide env vars, so they must not interleave.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn defaults_are_placeholders_when_no_env_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        for provider in Provider::ALL {
            for window in Window::ALL {
                let var = format!(
                    "CORRAL_COST_CAP_{}_{}_USD",
                    provider.as_str().to_ascii_uppercase(),
                    window_env_segment(window)
                );
                unsafe { std::env::remove_var(&var) };
            }
        }
        unsafe {
            std::env::remove_var("CORRAL_COST_ALERT_THRESHOLD_PCT");
            std::env::remove_var("CORRAL_COST_WARN_THRESHOLD_PCT");
        }
        let config = CostConfig::from_env();
        let (usd, is_placeholder) = config.cap_for(Provider::Claude, Window::FiveHour);
        assert!(is_placeholder);
        assert_eq!(usd, placeholder_cap_usd(Window::FiveHour));
        assert_eq!(config.alert_threshold_pct, DEFAULT_ALERT_THRESHOLD_PCT);
        assert_eq!(config.warn_threshold_pct, DEFAULT_WARN_THRESHOLD_PCT);
    }

    #[test]
    fn configured_cap_overrides_the_placeholder() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("CORRAL_COST_CAP_CODEX_WEEKLY_USD", "42.5") };
        let config = CostConfig::from_env();
        let (usd, is_placeholder) = config.cap_for(Provider::Codex, Window::Weekly);
        assert!(!is_placeholder);
        assert_eq!(usd, 42.5);
        unsafe { std::env::remove_var("CORRAL_COST_CAP_CODEX_WEEKLY_USD") };
    }
}
