//! Restricted tmux transport for interactive shell panes.
//!
//! A transport session is created by Corral and is the only target accepted by
//! input, resize, capture, and close operations.  tmux supplies cursor
//! coordinates separately from the ANSI capture, preserving terminal fidelity.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::process::Command;
use tokio::sync::Mutex;

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
const MAX_CAPTURE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalFrame {
    pub ansi: String,
    pub cursor_x: u16,
    pub cursor_y: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxSession {
    pub id: String,
    pub workspace_id: String,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone)]
pub struct TmuxTransport {
    root: PathBuf,
    sessions: Arc<Mutex<HashMap<String, TmuxSession>>>,
}

impl TmuxTransport {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn open(&self, workspace_id: &str, cwd: &Path) -> io::Result<TmuxSession> {
        let cwd = self.checked_cwd(cwd)?;
        if workspace_id.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace id is empty",
            ));
        }
        let id = format!(
            "corral-{}-{}",
            safe_name(workspace_id),
            NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
        );
        run_tmux(["new-session", "-d", "-s", &id, "-c", cwd.to_str().unwrap()]).await?;
        let session = TmuxSession {
            id: id.clone(),
            workspace_id: workspace_id.to_string(),
            cwd,
        };
        self.sessions.lock().await.insert(id, session.clone());
        Ok(session)
    }

    pub async fn capture(&self, session_id: &str) -> io::Result<TerminalFrame> {
        self.require_session(session_id).await?;
        let ansi =
            bounded_capture(&run_tmux(["capture-pane", "-p", "-e", "-t", session_id]).await?);
        let cursor = run_tmux([
            "display-message",
            "-p",
            "-t",
            session_id,
            "#{cursor_x},#{cursor_y}",
        ])
        .await?;
        let (cursor_x, cursor_y) = parse_cursor(&cursor)?;
        Ok(TerminalFrame {
            ansi,
            cursor_x,
            cursor_y,
        })
    }

    pub async fn send_input(&self, session_id: &str, input: &str) -> io::Result<()> {
        self.require_session(session_id).await?;
        if input.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "input contains NUL",
            ));
        }
        run_tmux(["send-keys", "-t", session_id, "-l", input])
            .await
            .map(|_| ())
    }

    pub async fn resize(&self, session_id: &str, cols: u16, rows: u16) -> io::Result<()> {
        self.require_session(session_id).await?;
        if cols == 0 || rows == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal dimensions must be non-zero",
            ));
        }
        run_tmux([
            "resize-window",
            "-t",
            session_id,
            "-x",
            &cols.to_string(),
            "-y",
            &rows.to_string(),
        ])
        .await
        .map(|_| ())
    }

    pub async fn close(&self, session_id: &str) -> io::Result<()> {
        self.require_session(session_id).await?;
        let result = run_tmux(["kill-session", "-t", session_id]).await;
        self.sessions.lock().await.remove(session_id);
        result.map(|_| ())
    }

    pub async fn sessions(&self) -> Vec<TmuxSession> {
        self.sessions.lock().await.values().cloned().collect()
    }

    fn checked_cwd(&self, cwd: &Path) -> io::Result<PathBuf> {
        let cwd = cwd.canonicalize()?;
        let root = self.root.canonicalize()?;
        if cwd != root && !cwd.starts_with(&root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cwd is outside the tmux root",
            ));
        }
        Ok(cwd)
    }

    async fn require_session(&self, id: &str) -> io::Result<()> {
        if self.sessions.lock().await.contains_key(id) {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "tmux session was not created by corral",
            ))
        }
    }
}

fn safe_name(value: &str) -> String {
    let name: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if name.is_empty() {
        "workspace".to_string()
    } else {
        name
    }
}

fn bounded_capture(value: &str) -> String {
    if value.len() <= MAX_CAPTURE_BYTES {
        return value.to_string();
    }
    let mut end = MAX_CAPTURE_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn parse_cursor(value: &str) -> io::Result<(u16, u16)> {
    let mut parts = value.trim().split(',');
    let x = parts.next().and_then(|v| v.parse().ok());
    let y = parts.next().and_then(|v| v.parse().ok());
    match (x, y, parts.next()) {
        (Some(x), Some(y), None) => Ok((x, y)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "tmux returned an invalid cursor",
        )),
    }
}

async fn run_tmux<const N: usize>(args: [&str; N]) -> io::Result<String> {
    let output = Command::new("tmux").args(args).output().await?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_position_is_decoded_from_tmux_metadata() {
        assert_eq!(parse_cursor("23,6\n").unwrap(), (23, 6));
    }

    #[test]
    fn malformed_cursor_metadata_is_rejected() {
        assert!(parse_cursor("23\n").is_err());
        assert!(parse_cursor("23,nope\n").is_err());
    }

    #[test]
    fn session_names_cannot_escape_tmux_target_namespace() {
        assert_eq!(safe_name("repo/feature one"), "repo-feature-one");
    }

    #[tokio::test]
    async fn tmux_capture_reports_cursor_position() {
        let root = tempfile::tempdir().unwrap();
        let transport = TmuxTransport::new(root.path().to_path_buf());
        let session = transport.open("cursor-test", root.path()).await.unwrap();
        transport.resize(&session.id, 100, 30).await.unwrap();
        transport
            .send_input(
                &session.id,
                &format!("printf '{}033[24;81H'; sleep 60\n", '\\'),
            )
            .await
            .unwrap();
        let mut frame = transport.capture(&session.id).await.unwrap();
        for _ in 0..100 {
            if (frame.cursor_x, frame.cursor_y) == (80, 23) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            frame = transport.capture(&session.id).await.unwrap();
        }
        assert_eq!((frame.cursor_x, frame.cursor_y), (80, 23));
        transport.close(&session.id).await.unwrap();
    }
}
