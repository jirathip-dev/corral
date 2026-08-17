//! opencode session-store reader (G34) — read-only, bounded, feature-detected.
//!
//! opencode.db is 13GB+ in steady state. This module never opens it
//! read-write and never runs an unbounded scan: every query filters by
//! `time_created`/`time_updated` in SQL, before any row reaches Rust.
//!
//! No `rusqlite` (or any sqlite crate) is in `Cargo.toml`, and the brief for
//! this workstream says a genuinely-missing crate needs orchestrator
//! approval rather than a silent add. Since the system `sqlite3` CLI is a
//! read-only-capable, busy-timeout-capable, JSON-emitting client already
//! present on the target hosts (same tool herdr-usage.py shells out to),
//! this reader drives it via `tokio::process::Command` instead of adding a
//! dependency. If corrald ever needs to run somewhere without the `sqlite3`
//! binary, that is the trigger to revisit this trade — not a reason to add
//! the crate silently now.
//!
//! Schema is feature-detected rather than assumed: current opencode installs
//! carry per-message `data` blobs (`message.data` JSON: `tokens`, `cost`,
//! `modelID`, `time.created`, `path.cwd`) which give message-level,
//! time-correct cost; if that column is absent (older/different schema),
//! this falls back to the per-session cumulative columns
//! (`session.cost`/`tokens_*`/`time_updated`) opencode has also started
//! shipping, bucketed by `time_updated` as a single event per session.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;

use super::windows::UsageEvent;

/// `.timeout` pragma (ms): the query blocks this long on a write-locked
/// page before giving up, matching herdr-usage.py's busy-timeout choice.
const BUSY_TIMEOUT_MS: u64 = 10_000;
/// Hard wall-clock cap on the whole `sqlite3` invocation — belt-and-suspenders
/// against a wedged process even under a misbehaving lock.
const QUERY_TIMEOUT: Duration = Duration::from_secs(20);

/// Read opencode usage in `[start_ms, end_ms]`. Returns an empty vec (not an
/// error) when the store is absent, unreadable, or the query fails — a
/// missing provider is "no data", never a crash.
pub async fn opencode_usage(db_path: &Path, start_ms: u64, end_ms: u64) -> Vec<UsageEvent> {
    if !db_path.exists() {
        return Vec::new();
    }
    if has_message_data_column(db_path).await
        && let Some(events) = query_message_data(db_path, start_ms, end_ms).await
    {
        return events;
    }
    query_session_fallback(db_path, start_ms, end_ms)
        .await
        .unwrap_or_default()
}

async fn has_message_data_column(db_path: &Path) -> bool {
    match run_sqlite_json(db_path, "PRAGMA table_info(message);").await {
        Some(rows) => rows
            .iter()
            .any(|r| r.get("name").and_then(Value::as_str) == Some("data")),
        None => false,
    }
}

async fn query_message_data(db_path: &Path, start_ms: u64, end_ms: u64) -> Option<Vec<UsageEvent>> {
    let sql = format!(
        "SELECT time_created, data FROM message WHERE time_created >= {start_ms} AND time_created <= {end_ms};"
    );
    let rows = run_sqlite_json(db_path, &sql).await?;
    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(data_str) = row.get("data").and_then(Value::as_str) else {
            continue;
        };
        let Ok(data) = serde_json::from_str::<Value>(data_str) else {
            continue;
        };
        let ts_ms = data
            .get("time")
            .and_then(|t| t.get("created"))
            .and_then(Value::as_u64)
            .or_else(|| row.get("time_created").and_then(Value::as_u64))
            .unwrap_or(0);
        let usd = data.get("cost").and_then(Value::as_f64);
        let workspace_path = data
            .get("path")
            .and_then(|p| p.get("cwd"))
            .and_then(Value::as_str)
            .map(str::to_string);
        events.push(UsageEvent {
            ts_ms,
            usd,
            workspace_path,
        });
    }
    Some(events)
}

async fn query_session_fallback(
    db_path: &Path,
    start_ms: u64,
    end_ms: u64,
) -> Option<Vec<UsageEvent>> {
    let sql = format!(
        "SELECT cost, time_updated, directory FROM session WHERE time_updated >= {start_ms} AND time_updated <= {end_ms};"
    );
    let rows = run_sqlite_json(db_path, &sql).await?;
    Some(
        rows.into_iter()
            .filter_map(|row| {
                let ts_ms = row.get("time_updated").and_then(Value::as_u64)?;
                let usd = row.get("cost").and_then(Value::as_f64);
                let workspace_path = row
                    .get("directory")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                Some(UsageEvent {
                    ts_ms,
                    usd,
                    workspace_path,
                })
            })
            .collect(),
    )
}

/// Run `sql` against `db_path` read-only (URI-independent `-readonly` CLI
/// flag) with a busy-timeout and JSON output; `None` on any failure
/// (missing binary, non-zero exit, timeout, unparseable output).
async fn run_sqlite_json(db_path: &Path, sql: &str) -> Option<Vec<Value>> {
    let fut = Command::new("sqlite3")
        .arg("-readonly")
        .arg("-json")
        .arg("-cmd")
        .arg(format!(".timeout {BUSY_TIMEOUT_MS}"))
        .arg(db_path)
        .arg(sql)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let output = match tokio::time::timeout(QUERY_TIMEOUT, fut).await {
        Ok(Ok(out)) if out.status.success() => out,
        _ => return None,
    };
    if output.stdout.iter().all(u8::is_ascii_whitespace) {
        return Some(Vec::new());
    }
    serde_json::from_slice::<Vec<Value>>(&output.stdout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn have_sqlite3() -> bool {
        StdCommand::new("sqlite3").arg("-version").output().is_ok()
    }

    async fn build_fixture_db(dir: &Path) -> std::path::PathBuf {
        let db_path = dir.join("opencode.db");
        let schema = r#"
            CREATE TABLE session (id TEXT, directory TEXT, time_created INTEGER, time_updated INTEGER, cost REAL);
            CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
        "#;
        StdCommand::new("sqlite3")
            .arg(&db_path)
            .arg(schema)
            .output()
            .expect("create fixture schema");
        db_path
    }

    #[tokio::test]
    async fn missing_store_returns_empty_not_error() {
        let events = opencode_usage(Path::new("/nonexistent/opencode.db"), 0, u64::MAX).await;
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn reads_message_data_bounded_by_window() {
        if !have_sqlite3() {
            eprintln!("skipping: sqlite3 CLI not available in this environment");
            return;
        }
        let tmp = tempfile::tempdir().expect("tmpdir");
        let db_path = build_fixture_db(tmp.path()).await;

        // one row inside the query window, one clearly outside it
        let in_window = serde_json::json!({
            "cost": 0.0021316,
            "modelID": "claude-opus-5",
            "time": {"created": 1_000_500u64},
            "path": {"cwd": "/Users/jirathip/Projects/foo"},
        })
        .to_string();
        let out_of_window = serde_json::json!({
            "cost": 5.0,
            "modelID": "claude-opus-5",
            "time": {"created": 500u64},
        })
        .to_string();
        let insert = format!(
            "INSERT INTO message (id, session_id, time_created, data) VALUES \
             ('m1','s1',1000500,'{}'), ('m2','s1',500,'{}');",
            in_window.replace('\'', "''"),
            out_of_window.replace('\'', "''"),
        );
        StdCommand::new("sqlite3")
            .arg(&db_path)
            .arg(insert)
            .output()
            .expect("insert fixture rows");

        let events = opencode_usage(&db_path, 1_000_000, 2_000_000).await;
        assert_eq!(events.len(), 1, "SQL WHERE clause must bound the query");
        assert_eq!(events[0].ts_ms, 1_000_500);
        assert!((events[0].usd.unwrap() - 0.0021316).abs() < 1e-9);
        assert_eq!(
            events[0].workspace_path.as_deref(),
            Some("/Users/jirathip/Projects/foo")
        );
    }

    #[tokio::test]
    async fn falls_back_to_session_table_when_message_lacks_data_column() {
        if !have_sqlite3() {
            eprintln!("skipping: sqlite3 CLI not available in this environment");
            return;
        }
        let tmp = tempfile::tempdir().expect("tmpdir");
        let db_path = tmp.path().join("legacy.db");
        // No `data` column on message — the older schema this reader must
        // defensively handle per the brief's feature-detect instruction.
        let schema = r#"
            CREATE TABLE session (id TEXT, directory TEXT, time_created INTEGER, time_updated INTEGER, cost REAL);
            CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER);
            INSERT INTO session (id, directory, time_created, time_updated, cost)
                VALUES ('s1', '/Users/jirathip/Projects/bar', 900000, 1500000, 1.25);
        "#;
        StdCommand::new("sqlite3")
            .arg(&db_path)
            .arg(schema)
            .output()
            .expect("create legacy fixture");

        let events = opencode_usage(&db_path, 1_000_000, 2_000_000).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].ts_ms, 1_500_000);
        assert!((events[0].usd.unwrap() - 1.25).abs() < 1e-9);
    }
}
