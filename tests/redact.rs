//! D9 integration: redaction holds on the LIVE ingest path, end to end.
//!
//! A fake herdr JSON-RPC server (unix socket) feeds secret-bearing pane data
//! through the real `HerdrAdapter` bootstrap + event streams. The daemon's
//! store — and therefore every serialized output (snapshot, SSE, drive
//! responses) — must never hold secret-shaped text.
//!
//! The adapter itself does zero polling (P1 rule): the wait loop below is
//! test-side only, waiting for the push-driven converge.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use corrald::adapters::herdr::HerdrAdapter;
use corrald::adapters::Adapter;
use corrald::core::store::Store;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

/// Fake herdr socket: answers `agent.list` and `events.subscribe`, then
/// pushes a status change + matched output carrying fake secrets.
async fn fake_herdr(path: PathBuf) {
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("bind fake herdr socket");
    loop {
        let Ok((sock, _)) = listener.accept().await else {
            continue;
        };
        let (read, mut write) = sock.into_split();
        let mut lines = BufReader::new(read).lines();
        let Ok(Some(line)) = lines.next_line().await else {
            continue;
        };
        let Ok(frame) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let id = frame.get("id").cloned().unwrap_or(json!("0"));
        let method = frame.get("method").and_then(|m| m.as_str()).unwrap_or("");
        match method {
            "agent.list" => {
                let reply = json!({
                    "id": id,
                    "result": {"agents": [{
                        "agent": "opencode",
                        "agent_status": "working",
                        "cwd": "/tmp/corral-redact-test",
                        "name": "agent-secret-test",
                        "pane_id": "wI:p1",
                        "terminal_title": "deploy key ghp_AbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
                        "terminal_title_stripped": "deploy key ghp_AbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
                        "state_labels": {"focus_lost": "user switched pane"},
                        "workspace_id": "wI"
                    }]}
                });
                let mut out = reply.to_string();
                out.push('\n');
                let _ = write.write_all(out.as_bytes()).await;
                let _ = write.flush().await;
            }
            "events.subscribe" => {
                let mut out = json!({"id": id, "result": {"ok": true}}).to_string();
                out.push('\n');
                let _ = write.write_all(out.as_bytes()).await;
                for ev in [
                    json!({
                        "event": "pane_agent_status_changed",
                        "data": {
                            "pane_id": "wI:p1",
                            "agent_status": "blocked",
                            "agent": "opencode",
                            "title": "needs token sk-ant-api03-AB12cdEF34ghIJ56klMN78op",
                            "state_labels": {"waiting_for_input": ""}
                        }
                    }),
                    json!({
                        "event": "pane_output_matched",
                        "data": {
                            "pane_id": "wI:p1",
                            "matched_line": "  Please approve AKIA1234567890ABCDEF deploy?",
                            "read": {"text": "1. Approve\n2. Reject\n"}
                        }
                    }),
                ] {
                    let mut s = ev.to_string();
                    s.push('\n');
                    let _ = write.write_all(s.as_bytes()).await;
                }
                let _ = write.flush().await;
                // Hold the push connection briefly, then let it close (the
                // adapter re-bootstraps; the retry loop keeps the test
                // running until the store converges).
                tokio::time::sleep(Duration::from_millis(300)).await;
                let _ = write.shutdown().await;
            }
            _ => {
                let mut out = json!({"id": id, "error": {"code": "method_not_found", "message": method}}).to_string();
                out.push('\n');
                let _ = write.write_all(out.as_bytes()).await;
            }
        }
    }
}

fn agent_id() -> &'static str {
    "herdr:pane:wI:p1"
}

#[tokio::test]
async fn ingest_redacts_secrets_before_the_store_can_serialize() {
    let dir = std::env::temp_dir().join(format!("corral-redact-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test dir");
    let socket = dir.join("herdr.sock");
    tokio::spawn(fake_herdr(socket.clone()));

    let store = Store::new();
    let adapter: Arc<dyn Adapter> = Arc::new(HerdrAdapter::new(socket));
    adapter.start(store.clone());

    // Test-side wait for the push-driven converge (the adapter never polls).
    let snap = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snap = store.snapshot().await;
            if snap.agents.contains_key(agent_id())
                && snap.agents[agent_id()].waiting_on.is_some()
            {
                break snap;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("store converges within 10s");

    let agent = &snap.agents[agent_id()];
    assert_eq!(agent.state, corrald::core::model::AgentState::Blocked);

    // Every pane-derived text field is redacted in the canonical record.
    assert_eq!(
        agent.title.as_deref(),
        Some("needs token [REDACTED]"),
        "title: sk-ant token stripped"
    );
    let w = agent.waiting_on.as_ref().expect("waiting_on set while blocked");
    assert_eq!(w.prompt, "Please approve [REDACTED] deploy?", "prompt: AKIA key stripped");
    assert!(!w.prompt_hash.contains("AKIA"), "hash covers only the redacted prompt");

    // The serialized output (what SSE / snapshot bytes look like) must not
    // contain any of the seeded secret shapes.
    let serialized = serde_json::to_string(&snap).expect("snapshot serializes");
    for leaked in [
        "ghp_AbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
        "sk-ant-api03-AB12cdEF34ghIJ56klMN78op",
        "AKIA1234567890ABCDEF",
    ] {
        assert!(
            !serialized.contains(leaked),
            "serialized snapshot leaked {leaked}"
        );
    }
    assert!(serialized.contains("[REDACTED]"), "redaction marker present");
}

#[tokio::test]
async fn redacted_text_embeds_into_json_without_leaking_or_corrupting() {
    // Correct egress contract: redact the RAW TEXT first, then serialize it
    // into the response payload (a future W1 read_tail/SSE egress path does
    // exactly this). Redaction never runs over already-serialized JSON — the
    // text must be clean before it is embedded.
    let raw_tail = "deploy token ghp_AbCdEfGhIjKlMnOpQrStUvWxYz1234567890 and AWS AKIA1234567890ABCDEF set";
    let redacted = corrald::core::redact::redact(raw_tail).into_owned();
    assert_eq!(
        redacted,
        "deploy token [REDACTED] and AWS [REDACTED] set"
    );
    let payload = json!({
        "rev": 3,
        "text": redacted,
        "reason": "waiting_for_input",
    });
    let wire = serde_json::to_string(&payload).expect("payload serializes");
    let parsed: serde_json::Value = serde_json::from_str(&wire).expect("wire is valid JSON");
    assert_eq!(parsed["text"], json!("deploy token [REDACTED] and AWS [REDACTED] set"));
    for leaked in ["ghp_AbCdEfGhIjKlMnOpQrStUvWxYz1234567890", "AKIA1234567890ABCDEF"] {
        assert!(!wire.contains(leaked), "wire leaked {leaked}");
    }
}

#[tokio::test]
async fn read_tail_response_shape_passes_redaction_untouched_when_clean() {
    // The read_tail contract shape (D5): ordinary pane tail text must survive
    // byte-identical — the redaction is display-safe for clean prose.
    let tail = "  1. Continue\n  2. Abort\n  → Waiting on your decision…\n";
    let out = corrald::core::redact::redact(tail);
    assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
    assert_eq!(out.as_ref(), tail);
}
