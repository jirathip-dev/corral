use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use corrald::api::drive::NoopAdapter;
use corrald::api::{AppState, router};
use corrald::auth::AuthPlane;
use corrald::auth::test_support::{keypair, sign};
use corrald::core::store::Store;
use corrald::drive::{Capability, DriveEnvelope, SignedDrive};
use futures::{SinkExt, StreamExt};
use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

struct EnvRestore {
    name: &'static str,
    previous: Option<OsString>,
}

impl EnvRestore {
    fn set(name: &'static str, value: &Path) -> Self {
        let previous = std::env::var_os(name);
        // Rust 2024 marks process environment mutation unsafe because other
        // threads may observe it concurrently. This test is the only terminal
        // route user in its integration binary and restores the value on drop.
        unsafe { std::env::set_var(name, value) };
        Self { name, previous }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var(self.name, value) },
            None => unsafe { std::env::remove_var(self.name) },
        }
    }
}

async fn next_json<S>(socket: &mut WebSocketStream<S>) -> serde_json::Value
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    tokio::time::timeout(Duration::from_secs(3), next_json_unbounded(socket))
        .await
        .expect("terminal WebSocket response within timeout")
}

async fn next_json_unbounded<S>(socket: &mut WebSocketStream<S>) -> serde_json::Value
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let message = socket
        .next()
        .await
        .expect("terminal WebSocket remains open while reading")
        .expect("terminal WebSocket frame is valid");
    match message {
        Message::Text(text) => serde_json::from_str(&text).expect("terminal response is JSON"),
        other => panic!("unexpected terminal WebSocket message: {other:?}"),
    }
}

async fn closes<S>(socket: &mut WebSocketStream<S>) -> bool
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match socket.next().await {
                None | Some(Err(_)) => return true,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await
    .expect("terminal WebSocket closes within timeout")
}

fn open_message(
    signing: &ed25519_dalek::SigningKey,
    key_id: &str,
    request_id: &str,
    workspace_id: &str,
    cwd: &Path,
) -> Message {
    let cwd = cwd.to_string_lossy().into_owned();
    let envelope = DriveEnvelope {
        request_id: request_id.to_string(),
        capability: Capability::Attach,
        target: workspace_id.to_string(),
        payload: serde_json::json!({ "cwd": cwd, "workspace_id": workspace_id }),
        rev: None,
    };
    let signed = SignedDrive {
        key_id: key_id.to_string(),
        signature: sign(signing, &envelope),
        envelope,
    };
    Message::Text(
        serde_json::json!({ "auth": signed, "cwd": cwd })
            .to_string()
            .into(),
    )
}

fn app_state(auth: Arc<AuthPlane>) -> AppState {
    AppState {
        store: Store::new(),
        auth,
        adapter: Arc::new(NoopAdapter),
        replay: Arc::new(corrald::api::drive::ReplayTable::default()),
        issues: Arc::new(corrald::api::issues::IssuesCache::default()),
        provenance: Arc::new(corrald::core::provenance::PromptProvenance::new()),
        cors_origins: Vec::new(),
    }
}

#[tokio::test]
async fn authenticated_terminal_websocket_runs_lifecycle_and_rejects_bad_frames() {
    let fixture = TempDir::new().expect("terminal fixture directory");
    let root = fixture.path().join("worktrees");
    let cwd = root.join("corral").join("terminal");
    std::fs::create_dir_all(&cwd).expect("terminal worktree fixture");
    let _root = EnvRestore::set("CORRAL_WORKTREES_ROOT", &root);

    let auth = Arc::new(
        AuthPlane::load_or_create(fixture.path().join("config"))
            .expect("auth plane creates fixture credentials"),
    );
    let (signing, public_key) = keypair();
    let token = auth.registry.registration_token();
    let record = auth
        .registry
        .register(&token, public_key, Duration::from_secs(300))
        .expect("device registers");
    auth.registry
        .set_grants(&record.key_id, vec![Capability::Attach])
        .expect("attach grant is explicit");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let server = tokio::spawn(async move {
        axum::serve(listener, router(app_state(auth)))
            .await
            .expect("terminal test server stays alive");
    });
    let url = format!("ws://{address}/v1/terminal");

    let (mut socket, _) = connect_async(&url)
        .await
        .expect("authenticated terminal WebSocket opens");
    socket
        .send(open_message(
            &signing,
            &record.key_id,
            "terminal-lifecycle",
            "herdr:terminal",
            &cwd,
        ))
        .await
        .expect("send signed terminal open");
    let opened = next_json(&mut socket).await;
    assert_eq!(opened["type"], "opened");
    assert_eq!(opened["workspace_id"], "herdr:terminal");

    socket
        .send(Message::Text(
            r#"{"type":"resize","cols":80,"rows":24}"#.to_string().into(),
        ))
        .await
        .expect("send terminal resize");
    socket
        .send(Message::Text(
            r#"{"type":"input","text":"printf terminal-lifecycle\\n"}"#
                .to_string()
                .into(),
        ))
        .await
        .expect("send terminal input");
    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "frame");
    assert!(frame["cursor_x"].is_number());
    assert!(frame["cursor_y"].is_number());

    socket
        .send(Message::Text(r#"{"type":"close"}"#.to_string().into()))
        .await
        .expect("send terminal close");
    assert!(closes(&mut socket).await, "close command closes the socket");

    let outside = fixture.path().join("outside");
    std::fs::create_dir_all(&outside).expect("outside worktree fixture");
    let (mut unavailable, _) = connect_async(&url)
        .await
        .expect("unavailable terminal WebSocket opens");
    unavailable
        .send(open_message(
            &signing,
            &record.key_id,
            "terminal-unavailable",
            "herdr:terminal-unavailable",
            &outside,
        ))
        .await
        .expect("send unavailable signed terminal open");
    let unavailable_error = next_json(&mut unavailable).await;
    assert_eq!(unavailable_error["type"], "error");
    assert_eq!(unavailable_error["kind"], "unavailable");
    assert!(
        closes(&mut unavailable).await,
        "unavailable worktree closes cleanly"
    );

    let (mut malformed, _) = connect_async(&url)
        .await
        .expect("second terminal WebSocket opens");
    malformed
        .send(open_message(
            &signing,
            &record.key_id,
            "terminal-malformed",
            "herdr:terminal-malformed",
            &cwd,
        ))
        .await
        .expect("send second signed terminal open");
    assert_eq!(next_json(&mut malformed).await["type"], "opened");
    malformed
        .send(Message::Text("not-json".to_string().into()))
        .await
        .expect("send malformed terminal command");
    let error = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let value = next_json_unbounded(&mut malformed).await;
            if value["type"] == "error" {
                return value;
            }
        }
    })
    .await
    .expect("malformed command receives an error within timeout");
    assert_eq!(error["type"], "error");
    assert_eq!(error["kind"], "malformed_command");
    assert!(
        closes(&mut malformed).await,
        "malformed command closes cleanly"
    );

    server.abort();
}
