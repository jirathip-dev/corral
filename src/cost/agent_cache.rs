//! D30: per-agent cumulative cost, background-refreshed (G34).
//!
//! The herdr adapter builds/rebuilds `Agent` records on every pane event —
//! far too often to shell out to `sqlite3` or walk JSONL transcripts
//! synchronously. Instead a background loop ([`spawn_refresh_loop`])
//! recomputes a `(tool, worktree_path) -> USD` map on an interval and
//! swaps it in atomically; `cumulative_cost_for` is then a plain map read,
//! safe to call from the adapter's hot path.
//!
//! "Cumulative" here means "summed over the trailing 30 days" (the same
//! window the monthly meter uses), not true all-time — an unbounded scan
//! of a 13GB+ opencode.db on every refresh tick would violate the "never
//! full-scan unbounded" constraint this whole workstream is built around.
//! Documented as a P1 scoping trade in `docs/corral/DECISIONS.md` (D34).
//!
//! A process-global cache (rather than threading a handle through
//! `HerdrAdapter::new`) is a deliberate, minimal-footprint choice: it
//! keeps this additive to the adapter's existing constructor and every
//! test that builds one directly, at the cost of being harder to inject a
//! fake cache in a test — acceptable here since [`cumulative_cost_for`]
//! degrades to `None` (today's `cost: None` behavior) until the loop has
//! run at least once, which is exactly what every existing test already
//! expects.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use super::windows::{UsageEvent, Window};
use super::{claude, codex, opencode};

type Cache = Arc<RwLock<HashMap<String, f64>>>;

static CACHE: OnceLock<Cache> = OnceLock::new();

fn cache() -> &'static Cache {
    CACHE.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
}

fn key(tool: &str, worktree_path: &str) -> String {
    format!("{tool}:{worktree_path}")
}

/// Best-effort cumulative cost for an agent, keyed by `(tool,
/// worktree_path)`. `None` until the refresh loop has populated a match —
/// same as the pre-G34 hardcoded `cost: None`, never an error.
pub fn cumulative_cost_for(tool: &str, worktree_path: &str) -> Option<f64> {
    cache().read().ok()?.get(&key(tool, worktree_path)).copied()
}

/// Recompute the whole cache from the env-configured store paths.
pub async fn refresh(now_ms: u64) {
    refresh_with_paths(
        now_ms,
        &super::opencode_db_path(),
        &super::claude_dir_path(),
        &super::codex_dir_path(),
    )
    .await;
}

/// [`refresh`]'s core, parameterized on store paths so tests can point at
/// fixtures without mutating process-wide env vars.
pub async fn refresh_with_paths(
    now_ms: u64,
    opencode_path: &Path,
    claude_path: &Path,
    codex_path: &Path,
) {
    let start_ms = now_ms.saturating_sub(Window::Monthly.duration_ms());
    let (opencode_events, claude_events, codex_events) = tokio::join!(
        opencode::opencode_usage(opencode_path, start_ms, now_ms),
        claude::claude_usage(claude_path, start_ms, now_ms),
        codex::codex_usage(codex_path, start_ms, now_ms),
    );

    let mut totals: HashMap<String, f64> = HashMap::new();
    accumulate(&mut totals, "opencode", &opencode_events);
    accumulate(&mut totals, "claude", &claude_events);
    accumulate(&mut totals, "codex", &codex_events);

    if let Ok(mut guard) = cache().write() {
        *guard = totals;
    }
}

fn accumulate(totals: &mut HashMap<String, f64>, tool: &str, events: &[UsageEvent]) {
    for e in events {
        let (Some(path), Some(usd)) = (&e.workspace_path, e.usd) else {
            continue;
        };
        *totals.entry(key(tool, path)).or_insert(0.0) += usd;
    }
}

/// Spawn the periodic refresh loop. Call once from `main`.
pub fn spawn_refresh_loop(interval: Duration) {
    tokio::spawn(async move {
        loop {
            refresh(crate::core::util::now_millis()).await;
            tokio::time::sleep(interval).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// `cumulative_cost_for` reads a process-global static shared by every
    /// test in this binary — serialize the tests that populate it so they
    /// don't observe each other's writes mid-assertion. `tokio::sync::Mutex`
    /// (not `std::sync::Mutex`): the guard is held across `.await` below.
    static CACHE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn unrefreshed_lookup_is_none_not_an_error() {
        let _guard = CACHE_TEST_LOCK.lock().await;
        // A key nothing has ever populated must stay None regardless of
        // what other tests in this module have refreshed.
        assert_eq!(
            cumulative_cost_for("opencode", "/no/such/workspace/ever"),
            None
        );
    }

    #[tokio::test]
    async fn refresh_sums_events_by_tool_and_workspace() {
        let _guard = CACHE_TEST_LOCK.lock().await;
        let tmp = tempfile::tempdir().expect("tmpdir");
        let claude_dir = tmp.path().join("claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        let line = serde_json::json!({
            "type": "assistant",
            "cwd": "/Users/jirathip/Projects/agent-cache-fixture",
            "timestamp": "2026-08-17T05:48:59.202Z",
            "message": {
                "model": "claude-opus-5",
                "usage": {"input_tokens": 1000, "output_tokens": 500},
            },
        });
        let mut f = std::fs::File::create(claude_dir.join("s.jsonl")).unwrap();
        writeln!(f, "{line}").unwrap();
        drop(f);

        let nonexistent = tmp.path().join("nope");
        let now_ms = chrono::DateTime::parse_from_rfc3339("2026-08-17T06:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        refresh_with_paths(now_ms, &nonexistent, &claude_dir, &nonexistent).await;

        let cost = cumulative_cost_for("claude", "/Users/jirathip/Projects/agent-cache-fixture")
            .expect("populated by refresh");
        let expected = 1000.0 * (5.00 / 1_000_000.0) + 500.0 * (25.00 / 1_000_000.0);
        assert!((cost - expected).abs() < 1e-9);
    }
}
