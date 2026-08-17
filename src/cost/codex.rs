//! Codex rollout reader (G34) — read-only, bounded, D-083-safe.
//!
//! `~/.codex/sessions/**/rollout-*.jsonl` interleaves `turn_context` lines
//! (carrying the active `model`) with `event_msg` / `token_count` lines
//! whose `info.last_token_usage` is the *incremental* usage for that turn
//! (as opposed to `total_token_usage`, which is a cumulative running total
//! for the whole session — summing that would double-count). Each line's
//! top-level `timestamp` is the event time. Only `type`, `payload.model`,
//! `payload.info`, `payload.cwd`, and `timestamp` are ever read off a
//! line — never `response_item`/message content — so this satisfies D-083
//! the same way the Claude reader does: by construction.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::Value;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::pricing::codex_model_rate;
use super::windows::UsageEvent;

const DAY_MS: u64 = 24 * 3600 * 1000;

/// Read codex usage in `[start_ms, end_ms]` from every `rollout-*.jsonl`
/// under `dir`. Empty vec (not an error) when the directory is absent.
pub async fn codex_usage(dir: &Path, start_ms: u64, end_ms: u64) -> Vec<UsageEvent> {
    if !is_dir(dir).await {
        return Vec::new();
    }
    let margin_ms = start_ms.saturating_sub(DAY_MS);
    let mut events = Vec::new();
    for path in rollout_files_since(dir, margin_ms).await {
        scan_rollout_file(&path, start_ms, end_ms, &mut events).await;
    }
    events
}

async fn is_dir(path: &Path) -> bool {
    fs::metadata(path).await.map(|m| m.is_dir()).unwrap_or(false)
}

async fn rollout_files_since(root: &Path, margin_ms: u64) -> Vec<PathBuf> {
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
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.starts_with("rollout-") && name.ends_with(".jsonl")) {
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

async fn scan_rollout_file(path: &Path, start_ms: u64, end_ms: u64, out: &mut Vec<UsageEvent>) {
    let Ok(file) = fs::File::open(path).await else { return };
    let mut lines = BufReader::new(file).lines();
    let mut model: Option<String> = None;
    let mut workspace_path: Option<String> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else { continue };
        let Some(kind) = record.get("type").and_then(Value::as_str) else { continue };
        let payload = record.get("payload");

        if kind == "turn_context" {
            if let Some(m) = payload.and_then(|p| p.get("model")).and_then(Value::as_str) {
                model = Some(m.to_string());
            }
            continue;
        }
        if kind == "session_meta"
            && workspace_path.is_none()
            && let Some(cwd) = payload.and_then(|p| p.get("cwd")).and_then(Value::as_str)
        {
            workspace_path = Some(cwd.to_string());
        }
        if kind != "event_msg" {
            continue;
        }
        let Some(payload) = payload else { continue };
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            continue;
        }
        let Some(last) = payload.get("info").and_then(|i| i.get("last_token_usage")) else {
            continue;
        };
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
        out.push(UsageEvent {
            ts_ms,
            usd: price_delta(model.as_deref().unwrap_or(""), last),
            workspace_path: workspace_path.clone(),
        });
    }
}

fn price_delta(model: &str, last_token_usage: &Value) -> Option<f64> {
    let rate = codex_model_rate(model)?;
    let tok = |key: &str| last_token_usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cached = tok("cached_input_tokens");
    let total_input = tok("input_tokens");
    let uncached_input = total_input.saturating_sub(cached) as f64;
    let output = tok("output_tokens") as f64;
    Some(uncached_input * rate.input + cached as f64 * rate.cache_read + output * rate.output)
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

    fn write_jsonl(path: &Path, lines: &[Value]) {
        let mut f = std::fs::File::create(path).expect("create fixture");
        for line in lines {
            writeln!(f, "{line}").expect("write line");
        }
    }

    fn turn_context(ts: &str, model: &str) -> Value {
        serde_json::json!({"type": "turn_context", "timestamp": ts, "payload": {"model": model}})
    }

    fn session_meta(ts: &str, cwd: &str) -> Value {
        serde_json::json!({"type": "session_meta", "timestamp": ts, "payload": {"cwd": cwd, "id": "s1"}})
    }

    fn token_count(ts: &str, input: u64, cached: u64, output: u64) -> Value {
        serde_json::json!({
            "type": "event_msg",
            "timestamp": ts,
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {"input_tokens": input * 3, "cached_input_tokens": cached * 3, "output_tokens": output * 3},
                    "last_token_usage": {"input_tokens": input, "cached_input_tokens": cached, "output_tokens": output},
                },
            },
        })
    }

    #[tokio::test]
    async fn missing_dir_returns_empty() {
        let events = codex_usage(Path::new("/nonexistent/codex/sessions"), 0, u64::MAX).await;
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn uses_last_token_usage_delta_not_cumulative_total() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let path = tmp.path().join("rollout-2026-08-17T00-00-00-abc.jsonl");
        write_jsonl(
            &path,
            &[
                session_meta("2026-08-17T00:00:00Z", "/repo"),
                turn_context("2026-08-17T00:00:01Z", "gpt-5.6-sol"),
                token_count("2026-08-17T00:05:00Z", 19010, 11008, 246),
            ],
        );

        let events = codex_usage(tmp.path(), 0, u64::MAX).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].workspace_path.as_deref(), Some("/repo"));
        let rate = codex_model_rate("gpt-5.6-sol").unwrap();
        let expected = (19010.0 - 11008.0) * rate.input + 11008.0 * rate.cache_read + 246.0 * rate.output;
        assert!((events[0].usd.unwrap() - expected).abs() < 1e-9);
    }

    #[tokio::test]
    async fn no_turn_context_yet_leaves_usd_unpriced() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let path = tmp.path().join("rollout-2026-08-17T00-00-00-def.jsonl");
        write_jsonl(&path, &[token_count("2026-08-17T00:05:00Z", 100, 0, 10)]);

        let events = codex_usage(tmp.path(), 0, u64::MAX).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].usd, None);
    }
}
