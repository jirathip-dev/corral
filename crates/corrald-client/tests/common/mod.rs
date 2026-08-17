//! Conformance harness: spawns a REAL corrald (workspace binary, scratch
//! `CORRAL_CONFIG_DIR` + port — the same recipe as `tests/auth.rs`'s
//! `live_daemon_self_test`) plus a FAKE herdr unix-socket server.
//!
//! Why a fake herdr: the daemon on main is frozen, and its agents come from
//! the herdr adapter over the herdr JSON-RPC socket. herdr itself is out of
//! repo scope (a third-party daemon), so the harness stands in for it with
//! the documented protocol (newline-delimited JSON-RPC; `agent.list`
//! bootstrap; `events.subscribe` connections; pushed `pane_*` events). The
//! corrald under test is real and unmodified — R3/R6/R8/R9's "200 ok:true,
//! exactly one dispatch" would be impossible to observe otherwise.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub const AGENT_PANE: &str = "wQ:p1";
/// The canonical agent_id the daemon derives for the fake pane (no herdr
/// session id → pane-derived fallback).
pub const AGENT_ID: &str = "herdr:pane:wQ:p1";
/// The herdr agent name the fake reports — the daemon's herdr adapter
/// resolves drive targets to the NAME when one exists (name > pane id).
pub const AGENT_NAME: &str = "w1-conformance";

/// Locate the real corrald binary. `CARGO_BIN_EXE_corrald` is only set for
/// corrald's own tests, so the workspace target dir is the fallback (the
/// gate runs `cargo build` first — the conformance suite must test the real
/// binary, not a stub).
pub fn daemon_binary() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_corrald") {
        return PathBuf::from(path);
    }
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let target = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            // crates/corrald-client -> workspace root/target
            manifest
                .parent()
                .expect("crate dir")
                .parent()
                .expect("workspace root")
                .join("target")
        });
    let bin = target.join(profile).join("corrald");
    assert!(
        bin.exists(),
        "corrald binary not found at {bin:?} — build the workspace first (`cargo build` at \
         the repo root); the conformance suite runs against the REAL daemon"
    );
    bin
}

// ---------------------------------------------------------------------------
// Fake herdr
// ---------------------------------------------------------------------------

/// One agent reported by the fake `agent.list`.
#[derive(Debug, Clone)]
pub struct FakeAgent {
    pub pane_id: String,
    pub agent: String,
    pub status: String,
    pub name: String,
    pub title: String,
    pub cwd: String,
}

impl Default for FakeAgent {
    fn default() -> Self {
        Self {
            pane_id: AGENT_PANE.to_string(),
            agent: "claude".to_string(),
            status: "idle".to_string(),
            name: AGENT_NAME.to_string(),
            title: "W1 conformance agent".to_string(),
            cwd: "/tmp/corral-w1".to_string(),
        }
    }
}

#[derive(Debug)]
struct Inner {
    agents: Mutex<Vec<FakeAgent>>,
    conns: Mutex<Vec<mpsc::Sender<String>>>,
    commands: Mutex<Vec<(String, Value)>>,
}

/// A stand-in herdr JSON-RPC server over a unix socket, implementing the
/// exact protocol the daemon's herdr adapter speaks (bootstrap
/// `agent.list`, `events.subscribe` connections that become push-only,
/// pushed `pane_*` events, `agent.*` drive RPCs).
pub struct FakeHerdr {
    inner: Arc<Inner>,
    _task: JoinHandle<()>,
}

impl FakeHerdr {
    /// Bind with caller-supplied agents (the R11 live gh test points a fake
    /// agent's cwd at a real git worktree so git+gh facts bind).
    pub async fn bind_with_agents(socket_path: PathBuf, agents: Vec<FakeAgent>) -> Self {
        let listener = UnixListener::bind(&socket_path).expect("bind fake herdr socket");
        let inner = Arc::new(Inner {
            agents: Mutex::new(agents),
            conns: Mutex::new(Vec::new()),
            commands: Mutex::new(Vec::new()),
        });
        let task = tokio::spawn(accept_loop(listener, inner.clone()));
        Self { inner, _task: task }
    }

    /// Every drive RPC received so far: (method, params).
    pub fn commands(&self) -> Vec<(String, Value)> {
        self.inner.commands.lock().unwrap().clone()
    }

    /// Count `agent.prompt` RPCs whose text equals `text` (exactly-one
    /// dispatch assertions).
    pub fn count_prompts_with(&self, text: &str) -> usize {
        self.inner
            .commands
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, params)| {
                method == "agent.prompt" && params.get("text").and_then(Value::as_str) == Some(text)
            })
            .count()
    }

    /// Count approve dispatches: an `agent.prompt` to the resolved herdr
    /// target (the agent name, since the fake reports one) whose text is
    /// the validated menu choice.
    pub fn count_approves_with(&self, choice: &str) -> usize {
        self.inner
            .commands
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, params)| {
                method == "agent.prompt"
                    && params.get("text").and_then(Value::as_str) == Some(choice)
                    && params.get("target").and_then(Value::as_str) == Some(AGENT_NAME)
            })
            .count()
    }

    /// Broadcast a `pane_*` event to every subscribed connection.
    pub async fn push(&self, event: &str, data: Value) {
        let line = json!({ "event": event, "data": data }).to_string() + "\n";
        let mut dead: Vec<usize> = Vec::new();
        {
            let mut conns = self.inner.conns.lock().unwrap();
            for (i, tx) in conns.iter().enumerate() {
                if tx.try_send(line.clone()).is_err() {
                    dead.push(i);
                }
            }
            for i in dead.into_iter().rev() {
                conns.remove(i);
            }
        }
        // Give the daemon's event loop a tick to process before returning.
        tokio::task::yield_now().await;
    }

    /// `pane.agent_status_changed` — the adapter maps this onto the agent's
    /// state (and clears waiting_on for non-blocked states).
    pub async fn set_status(&self, pane_id: &str, status: &str) {
        self.push(
            "pane_agent_status_changed",
            json!({
                "pane_id": pane_id,
                "agent_status": status,
                "agent": "claude",
                "title": "W1 conformance agent",
                "state_labels": {},
            }),
        )
        .await;
    }

    /// `pane.output_matched` — while the agent is blocked, the adapter turns
    /// the matched line into the canonical `waiting_on` record (untrimmed,
    /// redacted; `prompt_hash` over it).
    pub async fn set_output_match(&self, pane_id: &str, matched_line: &str, read_text: &str) {
        self.push(
            "pane_output_matched",
            json!({
                "pane_id": pane_id,
                "matched_line": matched_line,
                "read": { "text": read_text },
            }),
        )
        .await;
    }

    /// Set the agent blocked and attach a waiting approval with a menu.
    pub async fn wait_for_approval(&self, matched_line: &str, choices_text: &str) {
        self.set_status(AGENT_PANE, "blocked").await;
        // Small settle: the status change must land before the output match.
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.set_output_match(AGENT_PANE, matched_line, choices_text)
            .await;
    }
}

fn agent_json(a: &FakeAgent) -> Value {
    json!({
        "agent": a.agent,
        "agent_status": a.status,
        "cwd": a.cwd,
        "foreground_cwd": a.cwd,
        "focused": false,
        "interactive_ready": true,
        "name": a.name,
        "pane_id": a.pane_id,
        "revision": 1,
        "state_labels": {},
        "state_change_seq": 1,
        "title": a.title,
        "terminal_title_stripped": a.title,
        "workspace_id": "wQ",
    })
}

async fn accept_loop(listener: UnixListener, inner: Arc<Inner>) {
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(handle_conn(stream, inner.clone()));
    }
}

async fn handle_conn(stream: UnixStream, inner: Arc<Inner>) {
    let (read, mut write) = stream.into_split();
    let (tx, mut rx) = mpsc::channel::<String>(128);
    inner.conns.lock().unwrap().push(tx);
    let mut lines = BufReader::new(read).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line.ok().flatten() else { break };
                if line.trim().is_empty() { continue; }
                let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
                let id = value.get("id").and_then(Value::as_str).map(str::to_owned);
                let method = value.get("method").and_then(Value::as_str).map(str::to_owned);
                let Some((id, method)) = id.zip(method) else { continue };
                let params = value.get("params").cloned().unwrap_or(Value::Null);
                let response = match method.as_str() {
                    "agent.list" => {
                        let agents: Vec<Value> = inner
                            .agents
                            .lock()
                            .unwrap()
                            .iter()
                            .map(agent_json)
                            .collect();
                        json!({ "id": id, "result": { "agents": agents } })
                    }
                    "events.subscribe" => json!({ "id": id, "result": null }),
                    other => {
                        inner
                            .commands
                            .lock()
                            .unwrap()
                            .push((other.to_string(), params));
                        json!({ "id": id, "result": { "ok": true } })
                    }
                };
                let mut frame = response.to_string();
                frame.push('\n');
                if write.write_all(frame.as_bytes()).await.is_err() {
                    break;
                }
            }
            Some(frame) = rx.recv() => {
                if write.write_all(frame.as_bytes()).await.is_err() {
                    break;
                }
            }
        }
    }
    inner.conns.lock().unwrap().retain(|c| !c.is_closed());
}

// ---------------------------------------------------------------------------
// Real daemon spawn
// ---------------------------------------------------------------------------

/// A spawned, real corrald with a scratch config dir and the fake herdr.
pub struct LiveDaemon {
    child: Child,
    /// Owned scratch dir when the harness created it (`spawn_live_daemon`);
    /// `None` when the test supplied its own dir and keeps it alive
    /// (`spawn_live_daemon_at` — e.g. R11, whose git worktree must exist
    /// before the daemon boots).
    _dir: Option<tempfile::TempDir>,
    pub base: String,
    pub registration_token: String,
    pub admin_token: String,
    pub herdr: FakeHerdr,
    /// HEAD sha of the scratch git repo created at the config dir (the
    /// daemon's `CORRAL_REPO_ROOT`); the snapshot's `head_sha` must equal it
    /// (G21 conformance).
    pub repo_head_sha: String,
}

impl Drop for LiveDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pick_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .unwrap()
        .port()
}

/// Spawn corrald (CARGO_BIN_EXE / workspace target dir) with a FRESH scratch
/// config dir, wait for `/healthz`, and read the host-side credentials the
/// daemon minted (registration + admin tokens — the same files a host
/// operator would hand a device out of band).
pub async fn spawn_live_daemon() -> LiveDaemon {
    let dir = tempfile::tempdir().expect("scratch config dir");
    let config_dir = dir.path().to_path_buf();
    // G21: make the config dir a REAL git repo (one commit) and point the
    // fake agent's cwd at it, so the daemon's git plane probes it and the
    // snapshot carries a real head_sha/head_subject for the conformance
    // agent — proving the fields round-trip through the full pipeline, not
    // just the wire decode. The R11 _at flow pre-clones its own worktree
    // and must NOT get this staging.
    let repo_head_sha = init_scratch_repo(&config_dir);
    // The cwd must be set BEFORE the daemon boots: the herdr adapter learns
    // it from the bootstrap agent.list, and the git plane probes it on the
    // first sweep. A post-boot mutation would never reach the daemon.
    let agent = FakeAgent {
        cwd: config_dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let mut daemon = spawn_live_daemon_at(dir.path(), vec![agent]).await;
    daemon.repo_head_sha = repo_head_sha;
    daemon._dir = Some(dir);
    daemon
}

/// Spawn corrald over a caller-owned scratch dir with caller-supplied fake
/// agents. The dir must exist and stay alive for the daemon's lifetime (the
/// R11 live gh test pre-clones a git worktree into it); the retry/identity
/// probe recipe is identical to [`spawn_live_daemon`].
pub async fn spawn_live_daemon_at(dir: &std::path::Path, agents: Vec<FakeAgent>) -> LiveDaemon {
    let config_dir = dir.to_path_buf();
    let socket = config_dir.join("herdr.sock");
    let herdr = FakeHerdr::bind_with_agents(socket.clone(), agents).await;
    let bin = daemon_binary();
    let http = reqwest::Client::new();
    // The repo/worktrees roots MUST be canonical: the git plane
    // canonicalizes every fact path, and the integrator derives the repo by
    // stripping the (raw) root — a symlinked tempdir (macOS /var/folders ->
    // /private/var/folders) would otherwise leave every agent repo-less.
    let roots = std::fs::canonicalize(&config_dir).unwrap_or_else(|_| config_dir.clone());

    let mut last_failure = String::from("no spawn attempt made");
    for _ in 0..3 {
        let port = pick_port();
        let base = format!("http://127.0.0.1:{port}");
        let mut child = Command::new(&bin)
            .env("CORRAL_CONFIG_DIR", &config_dir)
            .env("CORRAL_REPO_ROOT", &roots)
            .env("CORRAL_WORKTREES_ROOT", &roots)
            .arg("--port")
            .arg(port.to_string())
            .arg("--socket")
            .arg(&socket)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn corrald");

        // Bounded readiness wait (same recipe as tests/auth.rs).
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
        if !ready {
            last_failure = format!("daemon did not come up on {base}");
            let _ = child.kill();
            let _ = child.wait();
            continue;
        }

        // Identity probe: our scratch config dir's registration token must
        // be accepted by the daemon answering on this port.
        let probe = corrald_client::DeviceKeypair::generate();
        let probe_result = http
            .post(format!("{base}/register"))
            .json(&json!({
                "token": read_token(&config_dir, "registration-token"),
                "public_key": probe.public_key_b64(),
            }))
            .send()
            .await;
        match probe_result {
            Ok(response) if response.status().is_success() => {
                let registration_token = read_token(&config_dir, "registration-token");
                let admin_token = read_token(&config_dir, "admin-token");
                return LiveDaemon {
                    child,
                    _dir: None,
                    base,
                    registration_token,
                    admin_token,
                    herdr,
                    // R11's _at flow pre-clones its own worktree; the
                    // scratch-repo HEAD is only set by the default
                    // spawn_live_daemon flow (G21 staging).
                    repo_head_sha: String::new(),
                };
            }
            Ok(response) => {
                last_failure = format!(
                    "identity probe failed on {base}: HTTP {} (port collision?)",
                    response.status()
                );
            }
            Err(e) => {
                last_failure = format!("identity probe transport error on {base}: {e}");
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    panic!("failed to bring up a real corrald after 3 attempts: {last_failure}");
}

fn read_token(config_dir: &std::path::Path, name: &str) -> String {
    std::fs::read_to_string(config_dir.join(name))
        .unwrap_or_else(|e| panic!("{name} missing from config dir: {e}"))
        .trim()
        .to_string()
}

/// `git init` the config dir + one commit, returning the HEAD sha. The
/// daemon probes this repo as its main checkout (`CORRAL_REPO_ROOT`); the
/// conformance snapshot's `head_sha` must equal the returned sha and
/// `head_subject` the message's first line, trimmed (G21 F4: the subject is
/// committed with LEADING whitespace — `git log %s` keeps leading spaces,
/// so the probe's `.trim()` is the only thing producing the pinned
/// "conformance initial commit", locking the trim against drift).
fn init_scratch_repo(config_dir: &std::path::Path) -> String {
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(config_dir)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("git {args:?} failed to spawn: {e}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "conformance@test.local"]);
    git(&["config", "user.name", "Conformance Test"]);
    std::fs::write(config_dir.join("README.md"), "corral conformance repo\n").unwrap();
    git(&["add", "README.md"]);
    // `--cleanup=verbatim` pins the exact message bytes; `%s` keeps the
    // leading spaces (and trims trailing), so the probe's trim is exercised.
    // The message file lives in THIS daemon's config dir (one tempdir per
    // spawn_live_daemon call) — a shared temp path would race across the
    // parallel conformance tests of the same binary.
    let msg = config_dir.join(".conformance-commit-msg.txt");
    std::fs::write(&msg, "  conformance initial commit  \n\nbody paragraph\n").unwrap();
    git(&[
        "commit",
        "-F",
        msg.to_str().expect("msg path"),
        "--cleanup=verbatim",
    ]);
    let _ = std::fs::remove_file(&msg);
    git(&["rev-parse", "HEAD"]).trim().to_string()
}

// ---------------------------------------------------------------------------
// Read-path helpers
// ---------------------------------------------------------------------------

/// Poll `/snapshot` until the agent is present (bounded).
pub async fn wait_for_agent(
    client: &corrald_client::CorralClient,
    agent_id: &str,
    timeout: Duration,
) -> corrald_client::Snapshot {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let snap = client.snapshot().await.expect("snapshot");
        if snap.agents.contains_key(agent_id) {
            return snap;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "agent {agent_id} never appeared in the snapshot"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Poll `/snapshot` until the agent's workspace carries a head commit
/// (G21): the git plane's boot probe must have merged before the assertions
/// that pin `head_sha`/`head_subject`.
pub async fn wait_for_head(
    client: &corrald_client::CorralClient,
    agent_id: &str,
    timeout: Duration,
) -> corrald_client::Snapshot {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let snap = client.snapshot().await.expect("snapshot");
        if snap
            .agents
            .get(agent_id)
            .is_some_and(|a| a.workspace.head_sha.is_some())
        {
            return snap;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "agent {agent_id} never carried a head commit in the snapshot"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Poll `/snapshot` until the agent carries a waiting approval (bounded).
pub async fn wait_for_waiting_on(
    client: &corrald_client::CorralClient,
    agent_id: &str,
    timeout: Duration,
) -> (corrald_client::Snapshot, corrald_client::model::WaitingOn) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let snap = client.snapshot().await.expect("snapshot");
        if let Some(agent) = snap.agents.get(agent_id).and_then(|a| a.waiting_on.clone()) {
            return (snap, agent);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "agent {agent_id} never reached a waiting approval"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Wait (bounded) until the fake herdr has seen `n` matching dispatches.
pub async fn wait_for_dispatch_count(
    herdr: &FakeHerdr,
    predicate: impl Fn(usize) -> bool + Send,
    timeout: Duration,
) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if predicate(herdr.commands().len()) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for dispatches; saw {:?}",
            herdr.commands()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Current audit log length (admin token).
pub async fn audit_len(client: &corrald_client::CorralClient, admin: &str) -> usize {
    client.audit(admin).await.expect("GET /audit").len()
}

/// Raw POST /drive, returning status + exact body bytes (byte-identical
/// replay assertions need the raw response).
pub async fn raw_drive(
    base: &str,
    signed: &corrald_client::SignedDrive,
    step_up_token: Option<&str>,
) -> (reqwest::StatusCode, bytes::Bytes) {
    let http = reqwest::Client::new();
    let mut request = http.post(format!("{base}/drive")).json(signed);
    if let Some(token) = step_up_token {
        request = request.header("X-Step-Up-Token", token);
    }
    let response = request.send().await.expect("drive request");
    let status = response.status();
    let body = response.bytes().await.expect("drive body");
    (status, body)
}
