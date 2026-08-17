//! Claude Code transcript reader (G34) — read-only, bounded, D-083-safe.
//!
//! `~/.claude/projects/**/*.jsonl` is one file per session; usage tokens
//! live on `message.usage` of assistant-turn lines, timestamped by the
//! line's own `timestamp`. This reader only ever reads `usage`, `model`,
//! `cwd`, and `timestamp` off each parsed line — never `message.content` —
//! so chat text and secrets never enter a `UsageEvent`, satisfying D-083 by
//! construction rather than by a redaction pass afterward.
//!
//! Bounded: files whose mtime predates the window (minus a one-day margin,
//! matching herdr-usage.py) are skipped without being opened; files are
//! streamed line-by-line rather than loaded whole.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::Value;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::pricing::claude_model_rate;
use super::windows::UsageEvent;

const DAY_MS: u64 = 24 * 3600 * 1000;

/// Read Claude usage in `[start_ms, end_ms]` from every `*.jsonl` transcript
/// under `dir`. Empty vec (not an error) when the directory is absent.
pub async fn claude_usage(dir: &Path, start_ms: u64, end_ms: u64) -> Vec<UsageEvent> {
    if !is_dir(dir).await {
        return Vec::new();
    }
    let margin_ms = start_ms.saturating_sub(DAY_MS);
    let mut events = Vec::new();
    for path in jsonl_files_since(dir, margin_ms).await {
        scan_jsonl_file(&path, start_ms, end_ms, &mut events).await;
    }
    events
}

async fn is_dir(path: &Path) -> bool {
    fs::metadata(path).await.map(|m| m.is_dir()).unwrap_or(false)
}

/// Iterative breadth-first walk (no recursion crate, no async recursion
/// footgun) collecting `.jsonl` files whose mtime is at/after `margin_ms`.
async fn jsonl_files_since(root: &Path, margin_ms: u64) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut queue = VecDeque::new();
    queue.push_back(root.to_path_buf());
    while let Some(dir) = queue.pop_front() {
        let Ok(mut rd) = fs::read_dir(&dir).await else { continue };
        while let Ok(Some(entry)) = rd.next_entry().await {
            let path = entry.path();
            let Ok(meta) = entry.metadata().await else { continue };
            if meta.is_dir() {
                queue.push_back(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(u64::MAX);
            if mtime_ms < margin_ms {
                continue;
            }
            out.push(path);
        }
    }
    out
}

async fn scan_jsonl_file(path: &Path, start_ms: u64, end_ms: u64, out: &mut Vec<UsageEvent>) {
    let Ok(file) = fs::File::open(path).await else { return };
    let mut lines = BufReader::new(file).lines();
    let mut workspace_path: Option<String> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else { continue };
        if workspace_path.is_none()
            && let Some(cwd) = record.get("cwd").and_then(Value::as_str)
        {
            workspace_path = Some(cwd.to_string());
        }
        let Some(usage) = record.get("message").and_then(|m| m.get("usage")) else { continue };
        let Some(ts_ms) = record
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_iso_ms)
        else {
            continue;
        };
        if ts_ms < start_ms || ts_ms > end_ms {
            continue;
        }
        let model = record
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(Value::as_str)
            .unwrap_or("");
        out.push(UsageEvent {
            ts_ms,
            usd: price_usage(model, usage),
            workspace_path: workspace_path.clone(),
        });
    }
}

fn price_usage(model: &str, usage: &Value) -> Option<f64> {
    let rate = claude_model_rate(model)?;
    let tok = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0) as f64;
    Some(
        tok("input_tokens") * rate.input
            + tok("output_tokens") * rate.output
            + tok("cache_creation_input_tokens") * rate.cache_write_5m
            + tok("cache_read_input_tokens") * rate.cache_read,
    )
}

fn parse_iso_ms(s: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis().max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_jsonl(dir: &Path, name: &str, lines: &[Value]) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).expect("create fixture");
        for line in lines {
            writeln!(f, "{line}").expect("write line");
        }
        path
    }

    fn assistant_line(ts: &str, model: &str, input: u64, output: u64, cwd: &str) -> Value {
        serde_json::json!({
            "type": "assistant",
            "cwd": cwd,
            "timestamp": ts,
            "message": {
                "model": model,
                "role": "assistant",
                "usage": {"input_tokens": input, "output_tokens": output},
                "content": [{"type": "text", "text": "super secret chain of thought"}],
            },
        })
    }

    #[tokio::test]
    async fn missing_dir_returns_empty() {
        let events = claude_usage(Path::new("/nonexistent/claude/projects"), 0, u64::MAX).await;
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn prices_known_model_and_bounds_by_timestamp() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        write_jsonl(
            &proj,
            "session.jsonl",
            &[
                assistant_line("2026-08-17T05:48:59.202Z", "claude-opus-5", 1000, 500, "/repo"),
                assistant_line("2020-01-01T00:00:00.000Z", "claude-opus-5", 999, 999, "/repo"),
            ],
        );

        let start = chrono::DateTime::parse_from_rfc3339("2026-08-17T00:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        let end = chrono::DateTime::parse_from_rfc3339("2026-08-18T00:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        let events = claude_usage(tmp.path(), start, end).await;

        assert_eq!(events.len(), 1, "the 2020 line is outside the window");
        let expected = 1000.0 * (5.00 / 1_000_000.0) + 500.0 * (25.00 / 1_000_000.0);
        assert!((events[0].usd.unwrap() - expected).abs() < 1e-9);
        assert_eq!(events[0].workspace_path.as_deref(), Some("/repo"));
    }

    #[tokio::test]
    async fn unknown_model_contributes_zero_not_error() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        write_jsonl(
            tmp.path(),
            "session.jsonl",
            &[assistant_line("2026-08-17T05:48:59.202Z", "some-future-model", 10, 10, "/repo")],
        );
        let events = claude_usage(tmp.path(), 0, u64::MAX).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].usd, None);
    }

    #[tokio::test]
    async fn d083_message_content_never_reaches_a_usage_event() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        write_jsonl(
            tmp.path(),
            "session.jsonl",
            &[assistant_line(
                "2026-08-17T05:48:59.202Z",
                "claude-opus-5",
                10,
                10,
                "/repo",
            )],
        );
        let events = claude_usage(tmp.path(), 0, u64::MAX).await;
        assert_eq!(events.len(), 1);
        let serialized = format!("{events:?}");
        assert!(!serialized.contains("super secret chain of thought"));
    }
}
