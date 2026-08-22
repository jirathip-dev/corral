//! D23+D33 test suite: history ring (bounds, ordering, restart survival,
//! torn lines), the store choke point, GET /history, the digest CLI, a
//! stress flood against the disk bound, and a live smoke (real daemon +
//! fake herdr + real digest binary, `--ignored`).

use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use corrald::api::{AppState, router};
use corrald::core::model::{Agent, AgentState, Change};
use corrald::core::store::Store;
use corrald::history::{HistoryEvent, HistoryRing, RotationPolicy, should_rotate};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tower::ServiceExt;

const MIB: u64 = 1024 * 1024;

fn event(ts: u64, agent: &str, old: Option<AgentState>, new: AgentState) -> HistoryEvent {
    HistoryEvent {
        ts,
        pane_id: Some(format!("pane-{agent}")),
        agent_id: Some(agent.to_string()),
        old_status: old,
        new_status: new,
        source: "herdr".to_string(),
        repo: Some("corral".to_string()),
    }
}

fn agent(id: &str, state: AgentState) -> Agent {
    Agent {
        agent_id: id.to_string(),
        source: "herdr".to_string(),
        tool: "opencode".to_string(),
        state,
        reason: None,
        seq: 1,
        ts: 0,
        capabilities: vec!["prompt".to_string()],
        waiting_on: None,
        parent_id: None,
        host: None,
        workspace: Default::default(),
        attachment: None,
        display_name: None,
        title: None,
    }
}

fn segment_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("history dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("seg-"))
        })
        .collect();
    files.sort();
    files
}

fn dir_bytes(dir: &std::path::Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().metadata().map(|m| m.len()).unwrap_or(0))
                .sum()
        })
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Ring: ordering, bounds, restart survival, torn lines, rotation
// ---------------------------------------------------------------------------

#[test]
fn in_memory_ring_ordered_and_filtered() {
    let ring = HistoryRing::in_memory(RotationPolicy::default());
    for i in 0..5 {
        let state = if i % 2 == 0 {
            AgentState::Working
        } else {
            AgentState::Blocked
        };
        ring.push(event(1000 + i, "a", None, state));
    }
    let all = ring.events();
    assert_eq!(all.len(), 5);
    let ts: Vec<u64> = all.iter().map(|e| e.ts).collect();
    assert_eq!(
        ts,
        vec![1000, 1001, 1002, 1003, 1004],
        "insertion order preserved"
    );

    let since = ring.query(Some(1002), None);
    let ts: Vec<u64> = since.iter().map(|e| e.ts).collect();
    assert_eq!(ts, vec![1002, 1003, 1004], "since filter inclusive");

    let limited = ring.query(None, Some(2));
    assert_eq!(limited.len(), 2);
    assert_eq!(limited[0].ts, 1000);
}

#[test]
fn oldest_drop_past_cap() {
    let policy = RotationPolicy {
        max_events: 32,
        max_events_per_segment: 16,
        ..Default::default()
    };
    let ring = HistoryRing::in_memory(policy);
    for i in 0..100 {
        let state = if i % 2 == 0 {
            AgentState::Working
        } else {
            AgentState::Blocked
        };
        ring.push(event(1000 + i, "a", None, state));
    }
    assert_eq!(ring.len(), 32, "ring capped at max_events");
    let all = ring.events();
    let first = all.first().expect("first event");
    assert_eq!(first.ts, 1000 + 100 - 32, "oldest events dropped");
}

#[test]
fn persistent_ring_survives_restart() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ring = HistoryRing::open(dir.path().to_path_buf(), RotationPolicy::default());
    for i in 0..10 {
        let state = if i % 2 == 0 {
            AgentState::Working
        } else {
            AgentState::Blocked
        };
        ring.push(event(1000 + i, "a", None, state));
    }
    drop(ring);

    let reopened = HistoryRing::open(dir.path().to_path_buf(), RotationPolicy::default());
    let events = reopened.events();
    assert_eq!(events.len(), 10, "history survives a restart");
    let ts: Vec<u64> = events.iter().map(|e| e.ts).collect();
    assert_eq!(
        ts,
        (1000..1010).collect::<Vec<_>>(),
        "order preserved after reload"
    );
}

#[test]
fn torn_tail_line_skipped_on_load() {
    let dir = tempfile::tempdir().expect("temp dir");
    {
        let ring = HistoryRing::open(dir.path().to_path_buf(), RotationPolicy::default());
        for i in 0..5 {
            let state = if i % 2 == 0 {
                AgentState::Working
            } else {
                AgentState::Blocked
            };
            ring.push(event(1000 + i, "a", None, state));
        }
    }
    // Simulate a crash mid-write: a torn line appended to the active segment.
    let seg = segment_files(dir.path()).pop().expect("segment file");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&seg)
        .unwrap()
        .write_all(b"{truncated json\n")
        .unwrap();

    let reopened = HistoryRing::open(dir.path().to_path_buf(), RotationPolicy::default());
    let events = reopened.events();
    assert_eq!(events.len(), 5, "torn line skipped, intact events survive");
    assert_eq!(events.first().unwrap().ts, 1000);
}

#[test]
fn segments_rotate_and_prune_oldest() {
    let dir = tempfile::tempdir().expect("temp dir");
    // Byte cap deliberately non-binding (~120B/event x 64 = ~7.7 KiB < 64 KiB)
    // so the event-count cap governs rotation deterministically.
    let policy = RotationPolicy {
        max_events: 192,
        max_events_per_segment: 64,
        max_segments: 3,
        max_bytes_per_segment: 64 * 1024,
        max_total_bytes: MIB,
        max_segment_age: Duration::from_secs(24 * 3600),
    };
    let ring = HistoryRing::open(dir.path().to_path_buf(), policy);
    for i in 0..400 {
        let state = if i % 2 == 0 {
            AgentState::Working
        } else {
            AgentState::Blocked
        };
        ring.push(event(1000 + i, "a", None, state));
    }
    let files = segment_files(dir.path());
    assert!(files.len() <= 3, "pruned to max_segments: {}", files.len());
    assert!(dir_bytes(dir.path()) <= policy.max_total_bytes);
    // 400 events = 6 full + 1 partial (16) segment; pruning keeps the last
    // 3: 64 + 64 + 16 = 144 events. Memory mirrors the disk exactly.
    assert_eq!(ring.len(), 144, "memory mirrors disk capacity");
    assert_eq!(ring.events().first().unwrap().ts, 1000 + 400 - 144);
    // Restart keeps exactly the same view.
    drop(ring);
    let reopened = HistoryRing::open(dir.path().to_path_buf(), policy);
    assert_eq!(reopened.events().len(), 144);
    assert_eq!(reopened.events().first().unwrap().ts, 1000 + 400 - 144);
}

#[test]
fn should_rotate_pure_caps() {
    let policy = RotationPolicy::default();
    assert!(!should_rotate(&policy, 255, 0, 0, 1000));
    assert!(should_rotate(&policy, 256, 0, 0, 1000), "event cap");
    assert!(!should_rotate(&policy, 0, 255 * 1024, 0, 1000));
    assert!(should_rotate(&policy, 0, 256 * 1024, 0, 1000), "byte cap");
    let age_ms = policy.max_segment_age.as_millis() as u64;
    assert!(!should_rotate(&policy, 0, 0, 1000, 1000 + age_ms - 1));
    assert!(should_rotate(&policy, 0, 0, 1000, 1000 + age_ms), "age cap");
}

// ---------------------------------------------------------------------------
// Store choke point: transitions recorded at Change::Upsert
// ---------------------------------------------------------------------------

#[tokio::test]
async fn store_records_only_actual_transitions() {
    let store = Store::new();
    store
        .apply(Change::upsert(agent("a", AgentState::Working)))
        .await;
    store
        .apply(Change::upsert(agent("a", AgentState::Working)))
        .await; // no-op
    store
        .apply(Change::upsert(agent("a", AgentState::Blocked)))
        .await;
    store
        .apply(Change::upsert(agent("a", AgentState::Working)))
        .await;
    store
        .apply(Change::upsert(agent("a", AgentState::Blocked)))
        .await;
    store
        .apply(Change::upsert(agent("a", AgentState::Done)))
        .await;

    let events = store.history().events();
    assert_eq!(events.len(), 5, "same-state upserts record nothing");
    let pairs: Vec<(Option<AgentState>, AgentState)> = events
        .iter()
        .map(|e| (e.old_status, e.new_status))
        .collect();
    assert_eq!(
        pairs,
        vec![
            (None, AgentState::Working),
            (Some(AgentState::Working), AgentState::Blocked),
            (Some(AgentState::Blocked), AgentState::Working),
            (Some(AgentState::Working), AgentState::Blocked),
            (Some(AgentState::Blocked), AgentState::Done),
        ],
        "blocked -> working -> blocked -> done sequence, in order"
    );
    let ts: Vec<u64> = events.iter().map(|e| e.ts).collect();
    let mut sorted = ts.clone();
    sorted.sort();
    assert_eq!(ts, sorted, "timestamps non-decreasing");
}

#[tokio::test]
async fn store_remove_emits_no_event() {
    let store = Store::new();
    store
        .apply(Change::upsert(agent("a", AgentState::Working)))
        .await;
    store.apply(Change::Remove("a".to_string())).await;
    assert_eq!(
        store.history().len(),
        1,
        "removal is not a status transition"
    );
}

#[tokio::test]
async fn store_with_history_dir_persists() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Store::with_history_dir(dir.path().join("history"));
    store
        .apply(Change::upsert(agent("a", AgentState::Working)))
        .await;
    store
        .apply(Change::upsert(agent("a", AgentState::Blocked)))
        .await;
    drop(store);

    let reopened = HistoryRing::open(dir.path().join("history"), RotationPolicy::default());
    assert_eq!(reopened.len(), 2);
    assert_eq!(reopened.events()[1].old_status, Some(AgentState::Working));
    assert_eq!(reopened.events()[1].new_status, AgentState::Blocked);
}

// ---------------------------------------------------------------------------
// GET /history
// ---------------------------------------------------------------------------

async fn app() -> (Store, axum::Router) {
    let store = Store::new();
    let coalescer = store.clone();
    std::mem::drop(tokio::spawn(async move { coalescer.run_coalescer().await }));
    let app = router(AppState {
        store: store.clone(),
        ..Default::default()
    });
    (store, app)
}

#[tokio::test]
async fn history_endpoint_returns_ordered_events() {
    let (store, app) = app().await;
    store
        .apply(Change::upsert(agent("a", AgentState::Working)))
        .await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    store
        .apply(Change::upsert(agent("a", AgentState::Blocked)))
        .await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    store
        .apply(Change::upsert(agent("a", AgentState::Working)))
        .await;
    let events = store.history().events();
    let mid = events[1].ts;
    assert!(
        events[0].ts < events[1].ts && events[1].ts < events[2].ts,
        "distinct ts"
    );

    let res = app
        .clone()
        .oneshot(Request::get("/history").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&body).unwrap();
    let list = v["events"].as_array().expect("events array");
    assert_eq!(list.len(), 3);
    assert_eq!(list[0]["new_status"], "working");
    assert_eq!(list[1]["new_status"], "blocked");
    assert_eq!(list[2]["new_status"], "working");
    assert_eq!(
        list[0]["old_status"],
        Value::Null,
        "first-seen has null old"
    );
    assert_eq!(list[1]["old_status"], "working");
    assert!(list[0]["ts"].as_u64().unwrap() <= list[1]["ts"].as_u64().unwrap());
    assert_eq!(list[0]["agent_id"], "a");
    assert_eq!(list[0]["source"], "herdr");

    // ?since= filters inclusive.
    let res = app
        .clone()
        .oneshot(
            Request::get(format!("/history?since={mid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["events"].as_array().unwrap().len(), 2);

    // ?limit= caps the page.
    let res = app
        .clone()
        .oneshot(
            Request::get("/history?limit=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["events"].as_array().unwrap().len(), 2);

    // Malformed since is rejected.
    let res = app
        .clone()
        .oneshot(
            Request::get("/history?since=garbage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Digest CLI (offline, real binary, no daemon)
// ---------------------------------------------------------------------------

#[test]
fn digest_cli_offline_against_ring() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ring = HistoryRing::open(dir.path().join("history"), RotationPolicy::default());
    let states = [
        AgentState::Working,
        AgentState::Blocked,
        AgentState::Working,
        AgentState::Blocked,
        AgentState::Done,
    ];
    for (i, state) in states.iter().enumerate() {
        let old = if i == 0 { None } else { Some(states[i - 1]) };
        let mut e = event(1000 + i as u64 * 60_000, "herdr:pane:p1", old, *state);
        e.agent_id = Some("herdr:pane:p1".to_string());
        e.pane_id = Some("p1".to_string());
        ring.push(e);
    }
    drop(ring);

    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["digest", "--config-dir"])
        .arg(dir.path())
        .arg("--since")
        .arg("0")
        .output()
        .expect("run corrald digest");
    assert!(output.status.success(), "digest exit: {:?}", output.status);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("herdr:pane:p1"), "agent section: {text}");
    assert!(
        text.contains("transitions (5): working -> blocked -> working -> blocked -> done"),
        "{text}"
    );
    assert!(text.contains("blocked: 2 spans"), "{text}");
    assert!(text.contains("work by repo: corral 5"), "{text}");
}

// ---------------------------------------------------------------------------
// Stress: flood transitions, assert bounded ring + disk
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stress_flood_stays_bounded_under_2_mib() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Store::with_history_dir(dir.path().join("history"));
    let policy = RotationPolicy::default();
    let flood = 50_000usize;
    for i in 0..flood {
        // Every apply is a real transition for its agent: state toggles per
        // agent on every cycle.
        let id = format!("agent-{}", i % 40);
        let state = if (i / 40) % 2 == 0 {
            AgentState::Working
        } else {
            AgentState::Blocked
        };
        let mut a = agent(&id, state);
        a.workspace.repo = Some("corral".to_string());
        store.apply(Change::upsert(a)).await;
    }
    let ring = store.history();
    let ring_len_before = ring.len();
    assert!(
        ring_len_before <= policy.max_events,
        "ring bounded by max_events: {ring_len_before}"
    );
    assert!(
        ring.len() >= policy.max_events - policy.max_events_per_segment,
        "retention near the cap"
    );

    let bytes = dir_bytes(&dir.path().join("history"));
    assert!(
        bytes <= policy.max_total_bytes,
        "disk bounded: {bytes} bytes (budget {})",
        policy.max_total_bytes
    );
    assert!(
        bytes < 2 * MIB,
        "acceptance: disk under ~2 MiB, got {bytes} bytes"
    );
    let files = segment_files(&dir.path().join("history"));
    assert!(
        files.len() <= policy.max_segments,
        "segments pruned: {}",
        files.len()
    );
    let disk_events: usize = files
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap().lines().count())
        .sum();
    assert_eq!(ring.len(), disk_events, "memory mirrors the disk exactly");

    let retained = ring.events();
    let first = retained.first().expect("first retained event");
    assert!(first.ts >= 1000, "oldest events rotated out");

    // Restart coherence: a fresh ring over the same dir sees the same view.
    drop(store);
    let reopened = HistoryRing::open(dir.path().join("history"), policy);
    assert_eq!(reopened.len(), ring_len_before);
    assert_eq!(
        reopened.events()[0],
        retained[0].clone(),
        "restart view matches memory"
    );
}

// ---------------------------------------------------------------------------
// Live smoke (ignored): real daemon + fake herdr + real digest binary
// ---------------------------------------------------------------------------

/// Minimal fake herdr JSON-RPC server: agent.list bootstrap, push-only
/// events.subscribe connections, pane_agent_status_changed pushes.
struct FakeHerdr {
    inner: Arc<FakeInner>,
    _task: JoinHandle<()>,
}

#[derive(Default)]
struct FakeInner {
    conns: Mutex<Vec<mpsc::Sender<String>>>,
}

impl FakeHerdr {
    async fn bind(socket: PathBuf) -> Self {
        let listener = UnixListener::bind(&socket).expect("bind fake herdr socket");
        let inner = Arc::new(FakeInner::default());
        let task = tokio::spawn(accept_loop(listener, inner.clone()));
        Self { inner, _task: task }
    }

    async fn set_status(&self, pane_id: &str, status: &str) {
        let frame = json!({
            "event": "pane_agent_status_changed",
            "data": {
                "pane_id": pane_id,
                "agent_status": status,
                "agent": "opencode",
                "title": "smoke agent",
                "state_labels": {},
            },
        })
        .to_string()
            + "\n";
        let mut dead = Vec::new();
        {
            let conns = self.inner.conns.lock().unwrap();
            for (i, tx) in conns.iter().enumerate() {
                if tx.try_send(frame.clone()).is_err() {
                    dead.push(i);
                }
            }
        }
        if !dead.is_empty() {
            self.inner.conns.lock().unwrap().retain(|c| !c.is_closed());
        }
        tokio::task::yield_now().await;
    }
}

async fn accept_loop(listener: UnixListener, inner: Arc<FakeInner>) {
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(handle_conn(stream, inner.clone()));
    }
}

async fn handle_conn(stream: UnixStream, inner: Arc<FakeInner>) {
    let (read, mut write) = stream.into_split();
    let (tx, mut rx) = mpsc::channel::<String>(64);
    inner.conns.lock().unwrap().push(tx);
    let mut lines = BufReader::new(read).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line.ok().flatten() else { break };
                if line.trim().is_empty() { continue; }
                let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
                let Some(id) = value.get("id").and_then(Value::as_str).map(str::to_owned) else { continue };
                let Some(method) = value.get("method").and_then(Value::as_str).map(str::to_owned) else { continue };
                let response = match method.as_str() {
                    "agent.list" => json!({
                        "id": id,
                        "result": { "agents": [{
                            "agent": "opencode",
                            "agent_status": "idle",
                            "cwd": "/tmp/smoke",
                            "name": "smoke-agent",
                            "pane_id": "p1",
                            "state_labels": {},
                            "title": "smoke agent",
                            "terminal_title_stripped": "smoke agent",
                            "workspace_id": "w1",
                        }] },
                    }),
                    "events.subscribe" => json!({ "id": id, "result": null }),
                    _ => json!({ "id": id, "result": { "ok": true } }),
                };
                let mut frame = response.to_string();
                frame.push('\n');
                if write.write_all(frame.as_bytes()).await.is_err() { break; }
            }
            Some(frame) = rx.recv() => {
                if write.write_all(frame.as_bytes()).await.is_err() { break; }
            }
        }
    }
    inner.conns.lock().unwrap().retain(|c| !c.is_closed());
}

fn pick_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .unwrap()
        .port()
}

struct LiveDaemon {
    child: Child,
    _dir: tempfile::TempDir,
    base: String,
    config_dir: PathBuf,
    herdr: FakeHerdr,
}

impl Drop for LiveDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn spawn_live_daemon() -> LiveDaemon {
    let dir = tempfile::tempdir().expect("scratch dir");
    let config_dir = dir.path().to_path_buf();
    let socket = config_dir.join("herdr.sock");
    let herdr = FakeHerdr::bind(socket.clone()).await;
    let port = pick_port();
    let base = format!("http://127.0.0.1:{port}");
    let mut child = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .env("CORRAL_CONFIG_DIR", &config_dir)
        .env("CORRAL_REPO_ROOT", &config_dir)
        .env("CORRAL_WORKTREES_ROOT", &config_dir)
        .arg("--port")
        .arg(port.to_string())
        .arg("--socket")
        .arg(&socket)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn corrald");
    let http = reqwest::Client::new();
    let mut ready = false;
    for _ in 0..200 {
        if let Ok(Some(status)) = child.try_wait() {
            eprintln!("[spawn_live_daemon] daemon exited early: {status}");
            panic!("daemon exited during healthz wait: {status}");
        }
        if http
            .get(format!("{base}/healthz"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "daemon did not come up on {base}");
    LiveDaemon {
        child,
        _dir: dir,
        base,
        config_dir,
        herdr,
    }
}

/// Poll /history?since=0 until at least `n` events exist, then return them.
async fn wait_history(base: &str, n: usize, timeout: Duration) -> Vec<Value> {
    let http = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let resp = match http.get(format!("{base}/history?since=0")).send().await {
            Ok(r) => r,
            Err(e) => {
                // The daemon may still be settling the listener right after
                // a spawn; retry instead of failing the smoke on a race.
                eprintln!("[wait_history] GET failed (retrying): {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let body = resp.json::<Value>().await.expect("history json");
        let events = body["events"].as_array().cloned().unwrap_or_default();
        if events.len() >= n {
            return events;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {n} history events; saw {}",
            events.len()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Live smoke (acceptance 1, 3, 5): a REAL corrald daemon ingests status
/// changes from herdr (fake socket, real adapter over the documented
/// protocol), /history reflects the blocked->working->blocked->done
/// sequence in order, and the real `corrald digest` binary reports the
/// transitions with blocked durations.
#[ignore = "needs the corrald binary + a fake herdr socket; run with --ignored"]
#[tokio::test]
async fn live_smoke_daemon_herdr_digest() {
    let mut daemon = spawn_live_daemon().await;
    let config_dir = daemon.config_dir.clone();

    // Wait until the daemon bootstrapped the agent (initial idle event
    // lands via the real adapter over the fake socket).
    let _ = wait_history(&daemon.base, 1, Duration::from_secs(15)).await;

    // Drive the acceptance sequence on a REAL daemon via the herdr adapter.
    for status in ["blocked", "working", "blocked", "done"] {
        daemon.herdr.set_status("p1", status).await;
    }

    // /history reflects the full sequence in order.
    let pre_restart_history = wait_history(&daemon.base, 5, Duration::from_secs(15)).await;
    let statuses: Vec<&str> = pre_restart_history
        .iter()
        .map(|e| e["new_status"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        statuses,
        vec!["idle", "blocked", "working", "blocked", "done"],
        "real adapter sequence: idle(bootstrap) -> blocked -> working -> blocked -> done"
    );
    assert_eq!(pre_restart_history[1]["old_status"], "idle");
    assert_eq!(pre_restart_history[2]["old_status"], "blocked");
    assert_eq!(pre_restart_history[3]["old_status"], "working");
    assert_eq!(pre_restart_history[4]["old_status"], "blocked");
    let ts: Vec<u64> = pre_restart_history
        .iter()
        .map(|e| e["ts"].as_u64().unwrap_or(0))
        .collect();
    let mut sorted = ts.clone();
    sorted.sort();
    assert_eq!(ts, sorted, "timestamps ordered");

    // The real digest CLI (offline, same config dir) reflects the events.
    let output = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .args(["digest", "--config-dir"])
        .arg(&config_dir)
        .arg("--since")
        .arg("0")
        .output()
        .expect("run corrald digest");
    assert!(output.status.success(), "digest exit: {:?}", output.status);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("herdr:pane:p1"),
        "digest has the agent: {text}"
    );
    assert!(
        text.contains("transitions (5): idle -> blocked -> working -> blocked -> done"),
        "digest sequence: {text}"
    );
    assert!(
        text.contains("blocked: 2 spans"),
        "blocked durations: {text}"
    );
    assert!(text.contains("work by repo"), "work per repo: {text}");

    // Restart survival against the real daemon: kill it, restart with the
    // SAME config dir (fresh port), and /history must still serve the
    // events from before the restart (plus the restart's own first-seen
    // re-observation of the live agent).
    let (restarted, restarted_base) = spawn_with_config(&config_dir).await;
    let mut old_child = std::mem::replace(&mut daemon.child, restarted);
    let _ = old_child.kill();
    let _ = old_child.wait();
    drop(old_child);
    daemon.base = restarted_base;
    let history = wait_history(&daemon.base, 5, Duration::from_secs(15)).await;
    let statuses: Vec<&str> = history
        .iter()
        .map(|e| e["new_status"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        &statuses[..5],
        &["idle", "blocked", "working", "blocked", "done"],
        "history survives a daemon restart, order intact"
    );
    assert_eq!(
        history[..5].to_vec(),
        pre_restart_history[..5].to_vec(),
        "pre-restart events byte-identical after restart"
    );
}

/// Spawn another real corrald against the same config dir (the fake herdr
/// socket and history segments live there) on a fresh port. Waits until the
/// restarted daemon answers `/healthz`, so the caller can query it
/// immediately.
async fn spawn_with_config(config_dir: &std::path::Path) -> (Child, String) {
    let port = pick_port();
    let child = Command::new(env!("CARGO_BIN_EXE_corrald"))
        .env("CORRAL_CONFIG_DIR", config_dir)
        .env("CORRAL_REPO_ROOT", config_dir)
        .env("CORRAL_WORKTREES_ROOT", config_dir)
        .arg("--port")
        .arg(port.to_string())
        .arg("--socket")
        .arg(config_dir.join("herdr.sock"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn restarted corrald");
    let base = format!("http://127.0.0.1:{port}");
    let http = reqwest::Client::new();
    let mut ready = false;
    for _ in 0..200 {
        if http
            .get(format!("{base}/healthz"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "restarted daemon did not come up on {base}");
    (child, base)
}
