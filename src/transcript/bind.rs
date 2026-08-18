//! #63 agent → session binding: which store/session holds the transcript
//! for an agent?
//!
//! ## The binding ladder (review F1: exactness before heuristics)
//!
//! 1. **Direct session-id binding.** herdr agent ids are
//!    `herdr:<session-id>` where the id is the TOOL'S OWN session id for
//!    every agent herdr resolved one for (claude → the jsonl's uuid
//!    filename, opencode → `session.id`, codex → the uuid in the rollout
//!    filename); only `herdr:pane:<id>` fallbacks lack one. When the hint
//!    names a session in a store, that session IS the transcript — no
//!    heuristic, no ambiguity, and no way to land on a co-resident
//!    agent's session. Tried against the agent's own tool's store first.
//! 2. **Worktree fallback**, for pane-derived ids or a hint no store
//!    knows: sessions whose recorded cwd matches the agent's
//!    `workspace.worktree_path`, RESTRICTED to the agent's tool's store
//!    when the tool maps to one (a codex implementer can never bind a
//!    claude reviewer's file — review F1). Multiple matches tie-break by
//!    recency; a tie AT the maximum is a typed [`BindError::Ambiguous`]
//!    carrying the full candidate list — never a guess.
//!
//! ## Store disciplines
//!
//! - opencode: sqlite3-CLI, read-only, sargable `IN` lookups with LIMIT
//!   (never a function-wrapped scan — review F10); non-zero sqlite3 exits
//!   classify as [`TranscriptError::StoreShape`], not "binary missing"
//!   (review F13).
//! - claude: `<claude_dir>/<encoded-cwd>/*.jsonl` (the encoding maps every
//!   non-alphanumeric byte to `-`; verified against the live layout).
//!   The encoding is LOSSY (`/a.b` and `/a/b` collide), so fallback
//!   candidates are verified against the `cwd` recorded INSIDE the file
//!   before admission (review F7) — same treatment as codex.
//! - codex: bounded walk (depth, file count), reading at most the first
//!   line of each rollout for `payload.cwd`.
//!
//! Path comparisons go through the integrator's `paths_match` (raw, then
//! canonical — review F8): a symlinked `$HOME` or `/tmp` must not split
//! herdr's raw cwd from the store's recorded spelling. All filesystem
//! scanning runs under `spawn_blocking` with a wall-clock cap (review F6)
//! — a huge `~/.codex/sessions` cannot pin the async runtime.
//!
//! A store that fails mid-bind is carried in
//! [`BindOutcome::unavailable`] even when another store matched (review
//! F9): the client can tell a complete answer from a partial one, the
//! same honesty `TranscriptPage::skipped` provides one level down.

use std::path::{Path, PathBuf};

use super::{StoreRef, TranscriptError};
use crate::integrate::paths_match;

/// Wall-clock cap on each filesystem discovery pass (review F6). The
/// opencode SQL side has its own [`super::OPENCODE_QUERY_TIMEOUT`].
const FS_SCAN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Upper bound on files visited by the codex walk (review F6) — past this
/// the store is reported unavailable rather than ground through.
const CODEX_WALK_MAX_FILES: usize = 20_000;
const CODEX_WALK_MAX_DEPTH: usize = 6;
/// At most this much of a JSONL file is read while looking for its
/// recorded cwd (first line for codex; first few lines for claude).
const FIRST_LINE_MAX: u64 = 64 * 1024;
const CLAUDE_CWD_PROBE_LINES: usize = 8;
/// Fallback candidate cap in SQL (review F10) — plenty above any real
/// per-worktree session count, and it bounds the row set by construction.
const OPENCODE_FALLBACK_LIMIT: usize = 16;

/// Where the three session stores live. Built from the same env-var
/// overrides the cost meter uses (`$CORRAL_OPENCODE_DB`,
/// `$CORRAL_CLAUDE_DIR`, `$CORRAL_CODEX_DIR`), so tests point the whole
/// read path at fixtures without touching live stores.
#[derive(Debug, Clone)]
pub struct TranscriptRoots {
    pub opencode_db: PathBuf,
    pub claude_dir: PathBuf,
    pub codex_dir: PathBuf,
}

impl TranscriptRoots {
    pub fn from_env() -> Self {
        Self {
            opencode_db: crate::cost::opencode_db_path(),
            claude_dir: crate::cost::claude_dir_path(),
            codex_dir: crate::cost::codex_dir_path(),
        }
    }

    /// Nonexistent paths under a per-process unique name — NOTHING is
    /// created (review F14: hermetic without dragging a whole
    /// `AppState::default()` auth plane into being). A state built with
    /// these can never read a live store.
    pub fn hermetic() -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "corral-hermetic-roots-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        Self {
            opencode_db: dir.join("opencode.db"),
            claude_dir: dir.join("claude-projects"),
            codex_dir: dir.join("codex-sessions"),
        }
    }
}

/// One session that matched. `recency_ms` is epoch millis of the
/// session's last activity (last message time / file mtime).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub store: StoreRef,
    pub recency_ms: u64,
}

impl Candidate {
    /// Client-facing name: the store kind plus the session id or file
    /// name — enough to pick one, no paths beyond what the caller knows.
    pub fn label(&self) -> String {
        match &self.store {
            StoreRef::Opencode { session_id, .. } => format!("opencode:{session_id}"),
            StoreRef::Claude { jsonl_path } => format!(
                "claude:{}",
                jsonl_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            StoreRef::Codex { rollout_path } => format!(
                "codex:{}",
                rollout_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
        }
    }
}

/// A successful bind, with honesty about what could not be consulted:
/// `unavailable` names store kinds that errored during this pass (review
/// F9 — a readable match must not silently hide that the agent's REAL
/// store might have been the unreadable one).
#[derive(Debug, Clone, PartialEq)]
pub struct BindOutcome {
    pub store: StoreRef,
    pub unavailable: Vec<String>,
}

/// Why no single store could be bound.
#[derive(Debug)]
pub enum BindError {
    /// No session in any consulted store matches.
    NoSession { worktree: String },
    /// More than one fallback session shares the maximum recency — the
    /// caller gets the full candidate list (newest first), never a pick.
    Ambiguous {
        worktree: String,
        candidates: Vec<Candidate>,
    },
    /// A store existed but could not be read/queried, and no other store
    /// produced a match.
    Store(TranscriptError),
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindError::NoSession { worktree } => {
                write!(f, "no session found for worktree {worktree}")
            }
            BindError::Ambiguous {
                worktree,
                candidates,
            } => write!(
                f,
                "ambiguous session for worktree {worktree}: {}",
                candidates
                    .iter()
                    .map(Candidate::label)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            BindError::Store(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for BindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BindError::Store(error) => Some(error),
            _ => None,
        }
    }
}

/// Which store kind an agent's tool writes its sessions to. `None` for
/// tools without a supported transcript store (their fallback consults
/// every store, the pre-F1 behavior).
fn tool_store_kind(tool: &str) -> Option<&'static str> {
    match tool {
        "opencode" => Some("opencode"),
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        _ => None,
    }
}

/// The tool's own session id inside a herdr agent id, when it carries
/// one: `herdr:<session-id>`. Pane fallbacks (`herdr:pane:<id>`) and
/// foreign shapes yield `None`.
fn session_hint(agent_id: &str) -> Option<&str> {
    let rest = agent_id.strip_prefix("herdr:")?;
    if rest.is_empty() || rest.starts_with("pane:") {
        return None;
    }
    Some(rest)
}

/// Resolve the store/session for one agent. `agent_id`/`tool` drive the
/// direct-id ladder rung; `worktree` drives the fallback.
pub async fn bind_agent(
    agent_id: &str,
    tool: &str,
    worktree: &str,
    roots: &TranscriptRoots,
) -> Result<BindOutcome, BindError> {
    let mut unavailable: Vec<String> = Vec::new();
    let mut first_error: Option<TranscriptError> = None;
    let note_failure = |kind: &str,
                        error: TranscriptError,
                        unavailable: &mut Vec<String>,
                        first_error: &mut Option<TranscriptError>| {
        if !unavailable.iter().any(|k| k == kind) {
            unavailable.push(kind.to_string());
        }
        if first_error.is_none() {
            *first_error = Some(error);
        }
    };

    // Rung 1: exact session-id binding (review F1). Tool's own store
    // first, then the others — ids are globally distinctive (uuid /
    // `ses_…`), so a cross-store hit is still exact, never a guess.
    if let Some(hint) = session_hint(agent_id) {
        let mut kinds = ["opencode", "claude", "codex"];
        if let Some(first) = tool_store_kind(tool)
            && let Some(pos) = kinds.iter().position(|k| *k == first)
        {
            kinds.swap(0, pos);
        }
        for kind in kinds {
            let found = match kind {
                "opencode" => opencode_by_id(&roots.opencode_db, hint).await,
                "claude" => {
                    run_fs_scan(roots.claude_dir.clone(), {
                        let hint = hint.to_string();
                        move |dir| claude_by_id(&dir, &hint)
                    })
                    .await
                }
                _ => {
                    run_fs_scan(roots.codex_dir.clone(), {
                        let hint = hint.to_string();
                        move |dir| Ok(codex_by_id(&dir, &hint))
                    })
                    .await
                }
            };
            match found {
                Ok(Some(store)) => return Ok(BindOutcome { store, unavailable }),
                Ok(None) => {}
                Err(error) => note_failure(kind, error, &mut unavailable, &mut first_error),
            }
        }
    }

    // Rung 2: worktree fallback, tool-restricted where possible.
    let kinds: &[&str] = match tool_store_kind(tool) {
        Some(kind) => match kind {
            "opencode" => &["opencode"],
            "claude" => &["claude"],
            _ => &["codex"],
        },
        None => &["opencode", "claude", "codex"],
    };
    let mut candidates = Vec::new();
    for kind in kinds {
        let found = match *kind {
            "opencode" => opencode_candidates(&roots.opencode_db, worktree).await,
            "claude" => {
                run_fs_scan(roots.claude_dir.clone(), {
                    let worktree = worktree.to_string();
                    move |dir| claude_candidates(&dir, &worktree)
                })
                .await
            }
            _ => {
                run_fs_scan(roots.codex_dir.clone(), {
                    let worktree = worktree.to_string();
                    move |dir| codex_candidates(&dir, &worktree)
                })
                .await
            }
        };
        match found {
            Ok(mut found) => candidates.append(&mut found),
            Err(error) => note_failure(kind, error, &mut unavailable, &mut first_error),
        }
    }

    match choose(candidates, worktree) {
        Ok(store) => Ok(BindOutcome { store, unavailable }),
        Err(BindError::NoSession { worktree }) => match first_error {
            // Nothing matched AND a store was unreadable: report the
            // failure, not a confident "no session".
            Some(error) => Err(BindError::Store(error)),
            None => Err(BindError::NoSession { worktree }),
        },
        Err(other) => Err(other),
    }
}

/// Run one blocking filesystem discovery pass off the async runtime
/// (review F6), bounded by [`FS_SCAN_TIMEOUT`]. Timeout or a lost worker
/// reports the store unreadable — never a hang, never a runtime stall.
async fn run_fs_scan<T, F>(root: PathBuf, scan: F) -> Result<T, TranscriptError>
where
    T: Send + 'static,
    F: FnOnce(PathBuf) -> Result<T, TranscriptError> + Send + 'static,
{
    let path_for_error = root.clone();
    let task = tokio::task::spawn_blocking(move || scan(root));
    match tokio::time::timeout(FS_SCAN_TIMEOUT, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) | Err(_) => Err(TranscriptError::StoreUnreadable {
            path: path_for_error,
            source: std::io::Error::new(std::io::ErrorKind::TimedOut, "store scan timed out"),
        }),
    }
}

/// The pure decision rule for the fallback rung: most recent candidate
/// wins; a tie at the top is ambiguous and returns everything (newest
/// first); none is a typed miss.
pub fn choose(mut candidates: Vec<Candidate>, worktree: &str) -> Result<StoreRef, BindError> {
    if candidates.is_empty() {
        return Err(BindError::NoSession {
            worktree: worktree.to_string(),
        });
    }
    candidates.sort_by(|a, b| {
        b.recency_ms
            .cmp(&a.recency_ms)
            .then(a.label().cmp(&b.label()))
    });
    if candidates.len() > 1 && candidates[1].recency_ms == candidates[0].recency_ms {
        return Err(BindError::Ambiguous {
            worktree: worktree.to_string(),
            candidates,
        });
    }
    Ok(candidates.remove(0).store)
}

/// The Claude Code project-directory encoding: every byte of the cwd that
/// is not ASCII alphanumeric becomes `-` (so `/` → `-`, `.` → `-`, `_` →
/// `-`; a real `-` stays `-`). Verified against the live layout — e.g.
/// `/Users/x/.herdr/worktrees/r/wt` → `-Users-x--herdr-worktrees-r-wt`.
/// LOSSY by design (`/a.b` and `/a/b` collide), which is why fallback
/// candidates are content-verified (review F7) and why matching ENCODES
/// the worktree rather than trying to decode directory names.
pub fn encode_claude_project_dir(cwd: &str) -> String {
    cwd.trim_end_matches('/')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Run one bind-side sqlite3 query. Distinguishes the three failure
/// classes (review F13): spawn failure = the binary is missing
/// (`Sqlite3Unavailable`); a non-zero exit = the query/schema is wrong
/// (`StoreShape` — e.g. opencode renamed a column); timeout is its own.
async fn run_bind_sql(
    db_path: &Path,
    sql: &str,
) -> Result<Vec<serde_json::Value>, TranscriptError> {
    let fut = tokio::process::Command::new("sqlite3")
        .args(super::opencode_sqlite_args(db_path, sql))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(super::OPENCODE_QUERY_TIMEOUT, fut)
        .await
        .map_err(|_| TranscriptError::QueryTimeout)?
        .map_err(|_| TranscriptError::Sqlite3Unavailable)?;
    if !output.status.success() {
        return Err(TranscriptError::StoreShape);
    }
    if output.stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| TranscriptError::StoreShape)
}

fn opencode_row_candidate(db_path: &Path, row: &serde_json::Value) -> Option<Candidate> {
    let id = row.get("id")?.as_str()?;
    let recency = row.get("recency").and_then(serde_json::Value::as_i64)?;
    Some(Candidate {
        store: StoreRef::Opencode {
            db_path: db_path.to_path_buf(),
            session_id: id.to_string(),
        },
        recency_ms: recency.max(0) as u64,
    })
}

/// Direct rung: `session.id` primary-key lookup.
async fn opencode_by_id(
    db_path: &Path,
    session_id: &str,
) -> Result<Option<StoreRef>, TranscriptError> {
    if !db_path.exists() {
        return Ok(None);
    }
    let sid = session_id.replace('\'', "''");
    let sql =
        format!("SELECT s.id AS id, 0 AS recency FROM session s WHERE s.id = '{sid}' LIMIT 1");
    let rows = run_bind_sql(db_path, &sql).await?;
    Ok(rows
        .first()
        .and_then(|row| opencode_row_candidate(db_path, row))
        .map(|c| c.store))
}

/// Fallback rung: sessions whose `session.directory` matches the
/// worktree. Sargable `IN` over the realistic spellings (raw + canonical,
/// each with and without a trailing slash) so an index on `directory`
/// stays usable, plus LIMIT — never a function-wrapped full scan (review
/// F10). Recency = the session's last message time (0 for message-less
/// sessions — still a candidate).
async fn opencode_candidates(
    db_path: &Path,
    worktree: &str,
) -> Result<Vec<Candidate>, TranscriptError> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let mut spellings: Vec<String> = Vec::new();
    let trimmed = worktree.trim_end_matches('/');
    for base in [
        trimmed.to_string(),
        crate::integrate::canon_best_effort(Path::new(trimmed))
            .to_string_lossy()
            .into_owned(),
    ] {
        for s in [base.clone(), format!("{base}/")] {
            if !spellings.contains(&s) {
                spellings.push(s);
            }
        }
    }
    let in_list = spellings
        .iter()
        .map(|s| format!("'{}'", s.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT s.id AS id, \
                COALESCE((SELECT MAX(m.time_created) FROM message m \
                          WHERE m.session_id = s.id), 0) AS recency \
         FROM session s WHERE s.directory IN ({in_list}) \
         LIMIT {OPENCODE_FALLBACK_LIMIT}"
    );
    let rows = run_bind_sql(db_path, &sql).await?;
    Ok(rows
        .iter()
        .filter_map(|row| opencode_row_candidate(db_path, row))
        .collect())
}

fn mtime_ms(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Direct rung: a claude session id names its jsonl file. Scanned across
/// ALL project dirs (one `read_dir` + one stat per project) — the id is
/// unique, so this sidesteps the lossy dir encoding entirely.
fn claude_by_id(claude_dir: &Path, session_id: &str) -> Result<Option<StoreRef>, TranscriptError> {
    if session_id.len() < 8 || !claude_dir.is_dir() {
        // Too-short hints (never a claude uuid) must not stat around.
        return Ok(None);
    }
    let projects =
        std::fs::read_dir(claude_dir).map_err(|source| TranscriptError::StoreUnreadable {
            path: claude_dir.to_path_buf(),
            source,
        })?;
    let file_name = format!("{session_id}.jsonl");
    for project in projects.flatten() {
        let candidate = project.path().join(&file_name);
        if candidate.is_file() {
            return Ok(Some(StoreRef::Claude {
                jsonl_path: candidate,
            }));
        }
    }
    Ok(None)
}

/// Direct rung: codex rollout filenames embed the session uuid
/// (`rollout-<timestamp>-<uuid>.jsonl`) — matched on the NAME only, no
/// file opens. Short hints never match (guard against matching
/// everything).
fn codex_by_id(codex_dir: &Path, session_id: &str) -> Option<StoreRef> {
    if session_id.len() < 8 {
        return None;
    }
    let mut newest: Option<(u64, PathBuf)> = None;
    walk_codex(codex_dir, &mut |path| {
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.contains(session_id))
        {
            let mtime = mtime_ms(path);
            if newest.as_ref().is_none_or(|(t, _)| mtime > *t) {
                newest = Some((mtime, path.to_path_buf()));
            }
        }
    });
    newest.map(|(_, rollout_path)| StoreRef::Codex { rollout_path })
}

/// Fallback rung: every jsonl in the ENCODED project dir whose recorded
/// in-file cwd matches the worktree (review F7 — the dir name alone is
/// lossy and can collide across repos). Files recording a DIFFERENT cwd
/// are excluded; files recording none in their probe window are admitted
/// (torn/summary-only tails — availability over a collision this narrow).
/// Both the raw and canonical worktree spellings are probed for the dir
/// name (review F8).
fn claude_candidates(claude_dir: &Path, worktree: &str) -> Result<Vec<Candidate>, TranscriptError> {
    let mut dirs: Vec<String> = vec![encode_claude_project_dir(worktree)];
    let canon = crate::integrate::canon_best_effort(Path::new(worktree.trim_end_matches('/')));
    let canon_encoded = encode_claude_project_dir(&canon.to_string_lossy());
    if !dirs.contains(&canon_encoded) {
        dirs.push(canon_encoded);
    }
    let mut found = Vec::new();
    for dir in dirs {
        let project = claude_dir.join(&dir);
        if !project.is_dir() {
            continue;
        }
        let entries =
            std::fs::read_dir(&project).map_err(|source| TranscriptError::StoreUnreadable {
                path: project.clone(),
                source,
            })?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "jsonl")
                && path.is_file()
                && claude_file_matches_cwd(&path, worktree)
            {
                found.push(Candidate {
                    recency_ms: mtime_ms(&path),
                    store: StoreRef::Claude { jsonl_path: path },
                });
            }
        }
    }
    Ok(found)
}

/// Probe the first few lines for a top-level `"cwd"` and compare it to
/// the worktree (raw-then-canonical). No `cwd` in the probe window →
/// admit; a recorded cwd that matches → admit; a recorded cwd that does
/// NOT match → exclude (this is the collision case F7 exists for).
fn claude_file_matches_cwd(path: &Path, worktree: &str) -> bool {
    use std::io::{BufRead, BufReader, Read};
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut reader = BufReader::new(file.take(FIRST_LINE_MAX * CLAUDE_CWD_PROBE_LINES as u64));
    let mut line = String::new();
    for _ in 0..CLAUDE_CWD_PROBE_LINES {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim_end()) else {
            continue;
        };
        if let Some(cwd) = value.get("cwd").and_then(|c| c.as_str()) {
            return paths_match(
                Path::new(cwd.trim_end_matches('/')),
                Path::new(worktree.trim_end_matches('/')),
            );
        }
    }
    true
}

/// Bounded walk over the codex sessions tree: depth-capped, file-count
/// capped, dirs identified WITHOUT following symlinks (a symlink cycle
/// cannot multiply the walk — review F6/answers).
fn walk_codex(codex_dir: &Path, visit: &mut dyn FnMut(&Path)) {
    let mut visited_files = 0usize;
    let mut stack = vec![(codex_dir.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                if depth < CODEX_WALK_MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
            } else if file_type.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                visited_files += 1;
                if visited_files > CODEX_WALK_MAX_FILES {
                    return;
                }
                visit(&path);
            }
        }
    }
}

/// Fallback rung: rollouts whose FIRST line records `payload.cwd`
/// matching the worktree (raw-then-canonical — review F8). The root
/// being unreadable is an error; individually unreadable/torn files are
/// skipped — one bad rollout must not hide every other candidate.
fn codex_candidates(codex_dir: &Path, worktree: &str) -> Result<Vec<Candidate>, TranscriptError> {
    if !codex_dir.is_dir() {
        return Ok(Vec::new());
    }
    if std::fs::read_dir(codex_dir).is_err() {
        return Err(TranscriptError::StoreUnreadable {
            path: codex_dir.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "read_dir"),
        });
    }
    let want = worktree.trim_end_matches('/');
    let mut found = Vec::new();
    walk_codex(codex_dir, &mut |path| {
        if codex_rollout_cwd(path)
            .is_some_and(|cwd| paths_match(Path::new(cwd.trim_end_matches('/')), Path::new(want)))
        {
            found.push(Candidate {
                recency_ms: mtime_ms(path),
                store: StoreRef::Codex {
                    rollout_path: path.to_path_buf(),
                },
            });
        }
    });
    Ok(found)
}

/// The `payload.cwd` of a rollout's first line, reading at most
/// [`FIRST_LINE_MAX`] bytes. `None` on any shortfall (unreadable, torn,
/// not JSON, no cwd) — skip semantics, never an error.
fn codex_rollout_cwd(path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader, Read};
    let file = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file.take(FIRST_LINE_MAX))
        .read_line(&mut line)
        .ok()?;
    let value: serde_json::Value = serde_json::from_str(line.trim_end()).ok()?;
    value
        .get("payload")?
        .get("cwd")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oc(session: &str, recency_ms: u64) -> Candidate {
        Candidate {
            store: StoreRef::Opencode {
                db_path: PathBuf::from("/tmp/db"),
                session_id: session.to_string(),
            },
            recency_ms,
        }
    }

    fn cl(name: &str, recency_ms: u64) -> Candidate {
        Candidate {
            store: StoreRef::Claude {
                jsonl_path: PathBuf::from(format!("/p/{name}.jsonl")),
            },
            recency_ms,
        }
    }

    fn claude_record(cwd: Option<&str>, text: &str) -> String {
        let mut v = serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}
        });
        if let Some(cwd) = cwd {
            v["cwd"] = serde_json::Value::String(cwd.to_string());
        }
        v.to_string()
    }

    #[test]
    fn choose_empty_is_typed_no_session() {
        match choose(Vec::new(), "/wt") {
            Err(BindError::NoSession { worktree }) => assert_eq!(worktree, "/wt"),
            other => panic!("expected NoSession, got {other:?}"),
        }
    }

    #[test]
    fn choose_single_wins_and_recency_breaks_ties_across_stores() {
        let store = choose(vec![oc("s1", 5)], "/wt").expect("single");
        assert!(matches!(store, StoreRef::Opencode { ref session_id, .. } if session_id == "s1"));

        let store = choose(vec![oc("old", 5), cl("new", 9)], "/wt").expect("recency");
        assert!(matches!(store, StoreRef::Claude { .. }));
    }

    #[test]
    fn choose_tie_at_max_returns_full_candidate_list_newest_first() {
        match choose(vec![oc("a", 7), cl("b", 7), oc("stale", 3)], "/wt") {
            Err(BindError::Ambiguous {
                worktree,
                candidates,
            }) => {
                assert_eq!(worktree, "/wt");
                assert_eq!(candidates.len(), 3);
                assert_eq!(candidates[0].recency_ms, 7);
                assert_eq!(candidates[1].recency_ms, 7);
                assert_eq!(candidates[2].label(), "opencode:stale");
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn session_hint_extraction() {
        assert_eq!(
            session_hint("herdr:2d5e5911-b103-4a92-adc3-a8bdc03fd784"),
            Some("2d5e5911-b103-4a92-adc3-a8bdc03fd784")
        );
        assert_eq!(session_hint("herdr:ses_abc123"), Some("ses_abc123"));
        assert_eq!(session_hint("herdr:pane:w1:p2"), None, "pane fallback");
        assert_eq!(session_hint("herdr:"), None);
        assert_eq!(session_hint("other:xyz"), None);
    }

    #[test]
    fn claude_encoding_matches_live_layout() {
        assert_eq!(
            encode_claude_project_dir("/Users/x/.herdr/worktrees/corral/corral-g35b"),
            "-Users-x--herdr-worktrees-corral-corral-g35b"
        );
        assert_eq!(
            encode_claude_project_dir("/a/b.c/d_e/f-g/"),
            "-a-b-c-d-e-f-g"
        );
        assert_eq!(encode_claude_project_dir("/Users/x"), "-Users-x");
    }

    /// F1: the direct rung finds a claude session BY ID across project
    /// dirs — even one whose encoded dir name has nothing to do with the
    /// agent's current worktree.
    #[tokio::test]
    async fn direct_claude_id_binding_beats_the_worktree_heuristic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let roots = TranscriptRoots {
            opencode_db: dir.path().join("none.db"),
            claude_dir: dir.path().join("claude"),
            codex_dir: dir.path().join("codex"),
        };
        let sid = "2d5e5911-b103-4a92-adc3-a8bdc03fd784";
        let elsewhere = roots.claude_dir.join("-some-other-project");
        std::fs::create_dir_all(&elsewhere).expect("mkdir");
        std::fs::write(
            elsewhere.join(format!("{sid}.jsonl")),
            claude_record(None, "mine") + "\n",
        )
        .expect("write");
        // A DIFFERENT session sits in the worktree's encoded dir — the
        // heuristic would bind it; the direct rung must not.
        let wt = "/wt/repo";
        let in_wt = roots.claude_dir.join(encode_claude_project_dir(wt));
        std::fs::create_dir_all(&in_wt).expect("mkdir");
        std::fs::write(
            in_wt.join("cafebabe-0000-0000-0000-000000000000.jsonl"),
            claude_record(Some(wt), "not mine") + "\n",
        )
        .expect("write");

        let outcome = bind_agent(&format!("herdr:{sid}"), "claude", wt, &roots)
            .await
            .expect("binds");
        assert!(outcome.unavailable.is_empty());
        assert!(matches!(
            outcome.store,
            StoreRef::Claude { ref jsonl_path } if jsonl_path.ends_with(format!("{sid}.jsonl"))
        ));
    }

    /// F1: with a pane-derived id the fallback is restricted to the
    /// agent's own tool's store — a codex agent can never bind the claude
    /// reviewer's file sharing its worktree.
    #[tokio::test]
    async fn fallback_is_tool_restricted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let roots = TranscriptRoots {
            opencode_db: dir.path().join("none.db"),
            claude_dir: dir.path().join("claude"),
            codex_dir: dir.path().join("codex"),
        };
        let wt = "/wt/repo";
        let project = roots.claude_dir.join(encode_claude_project_dir(wt));
        std::fs::create_dir_all(&project).expect("mkdir");
        std::fs::write(
            project.join("aaaa1111-0000-0000-0000-000000000000.jsonl"),
            claude_record(Some(wt), "reviewer transcript") + "\n",
        )
        .expect("write");

        // codex tool: the claude session must NOT be considered.
        match bind_agent("herdr:pane:w1:p1", "codex", wt, &roots).await {
            Err(BindError::NoSession { .. }) => {}
            other => panic!("codex agent bound a claude session: {other:?}"),
        }
        // claude tool: found.
        let outcome = bind_agent("herdr:pane:w1:p1", "claude", wt, &roots)
            .await
            .expect("claude fallback binds");
        assert!(matches!(outcome.store, StoreRef::Claude { .. }));
    }

    /// F7: a session file in a COLLIDING encoded dir (recording a
    /// different cwd) is excluded by the content check; files recording
    /// no cwd are admitted.
    #[test]
    fn claude_candidates_verify_recorded_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let claude_dir = dir.path().join("claude");
        // "/wt/a.b" and "/wt/a/b" collide into the same encoded dir.
        let project = claude_dir.join(encode_claude_project_dir("/wt/a/b"));
        std::fs::create_dir_all(&project).expect("mkdir");
        std::fs::write(
            project.join("11111111-0000-0000-0000-000000000000.jsonl"),
            claude_record(Some("/wt/a/b"), "ours") + "\n",
        )
        .expect("write");
        std::fs::write(
            project.join("22222222-0000-0000-0000-000000000000.jsonl"),
            claude_record(Some("/wt/a.b"), "collision") + "\n",
        )
        .expect("write");
        std::fs::write(
            project.join("33333333-0000-0000-0000-000000000000.jsonl"),
            claude_record(None, "no cwd recorded") + "\n",
        )
        .expect("write");

        let found = claude_candidates(&claude_dir, "/wt/a/b").expect("scan");
        let labels: Vec<String> = found.iter().map(Candidate::label).collect();
        assert_eq!(found.len(), 2, "{labels:?}");
        assert!(!labels.iter().any(|l| l.contains("22222222")), "{labels:?}");
    }

    /// F9: an unreadable store alongside a readable match surfaces in
    /// `unavailable` instead of being silently dropped; with NO match it
    /// is the error itself.
    #[tokio::test]
    async fn unreadable_store_is_reported_not_swallowed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let roots = TranscriptRoots {
            // A garbage file at the db path: exists, sqlite3 exits
            // non-zero → StoreShape (F13), store kind "opencode".
            opencode_db: dir.path().join("garbage.db"),
            claude_dir: dir.path().join("claude"),
            codex_dir: dir.path().join("codex"),
        };
        std::fs::write(&roots.opencode_db, "this is not a database").expect("write");
        let wt = "/wt/repo";
        let project = roots.claude_dir.join(encode_claude_project_dir(wt));
        std::fs::create_dir_all(&project).expect("mkdir");
        std::fs::write(
            project.join("aaaa1111-0000-0000-0000-000000000000.jsonl"),
            claude_record(Some(wt), "hello") + "\n",
        )
        .expect("write");

        if !have_sqlite3() {
            eprintln!("sqlite3 not on PATH; skipping");
            return;
        }
        // Unknown tool → all stores consulted; claude matches, opencode
        // fails: Ok WITH the failure named.
        let outcome = bind_agent("herdr:pane:w1:p1", "mystery", wt, &roots)
            .await
            .expect("claude still binds");
        assert!(matches!(outcome.store, StoreRef::Claude { .. }));
        assert_eq!(outcome.unavailable, vec!["opencode".to_string()]);

        // No candidate anywhere → the store failure IS the error.
        match bind_agent("herdr:pane:w1:p1", "mystery", "/wt/empty", &roots).await {
            Err(BindError::Store(TranscriptError::StoreShape)) => {}
            other => panic!("expected Store(StoreShape), got {other:?}"),
        }
    }

    fn have_sqlite3() -> bool {
        std::process::Command::new("sqlite3")
            .arg("--version")
            .output()
            .is_ok()
    }

    /// F10: the fallback SQL is sargable (bare `s.directory IN`) and
    /// LIMIT-bounded; the direct SQL is a primary-key lookup. Neither
    /// wraps the column in a function.
    #[tokio::test]
    async fn opencode_direct_and_fallback_against_a_fixture_db() {
        if !have_sqlite3() {
            eprintln!("sqlite3 not on PATH; skipping");
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("oc.db");
        let seed = r#"
CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT);
CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, role TEXT, data TEXT);
INSERT INTO session VALUES ('ses_wt', '/wt/repo/');
INSERT INTO session VALUES ('ses_other', '/elsewhere');
INSERT INTO message VALUES ('m1', 'ses_wt', 111, 'assistant', '{}');
"#;
        let status = std::process::Command::new("sqlite3")
            .arg(&db)
            .arg(seed)
            .status()
            .expect("sqlite3 runs");
        assert!(status.success());

        // Direct rung: PK hit.
        let store = opencode_by_id(&db, "ses_wt").await.expect("query");
        assert!(
            matches!(store, Some(StoreRef::Opencode { ref session_id, .. }) if session_id == "ses_wt")
        );
        assert!(
            opencode_by_id(&db, "ses_missing")
                .await
                .expect("query")
                .is_none()
        );

        // Fallback rung: trailing-slash spelling in the store still
        // matches the trimmed worktree, recency from message time.
        let found = opencode_candidates(&db, "/wt/repo").await.expect("query");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].recency_ms, 111);
        assert_eq!(found[0].label(), "opencode:ses_wt");
    }

    #[test]
    fn codex_first_line_cwd_extraction_and_skip_semantics() {
        let dir = tempfile::tempdir().expect("tempdir");
        let good = dir.path().join("rollout-1.jsonl");
        std::fs::write(
            &good,
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/wt/x\"}}\n{\"later\":true}\n",
        )
        .expect("write");
        assert_eq!(codex_rollout_cwd(&good).as_deref(), Some("/wt/x"));

        let torn = dir.path().join("rollout-2.jsonl");
        std::fs::write(&torn, "not json at all\n").expect("write");
        assert_eq!(codex_rollout_cwd(&torn), None);

        let no_cwd = dir.path().join("rollout-3.jsonl");
        std::fs::write(&no_cwd, "{\"payload\":{}}\n").expect("write");
        assert_eq!(codex_rollout_cwd(&no_cwd), None);
    }

    #[test]
    fn codex_walk_finds_nested_rollouts_and_direct_id_matches_filenames() {
        let dir = tempfile::tempdir().expect("tempdir");
        let day = dir.path().join("2026/08/18");
        std::fs::create_dir_all(&day).expect("mkdirs");
        std::fs::write(
            day.join("rollout-2026-08-18T10-11-12-deadbeef-1111.jsonl"),
            "{\"payload\":{\"cwd\":\"/wt/target/\"}}\n",
        )
        .expect("write");
        std::fs::write(
            day.join("rollout-2026-08-18T10-11-13-cafebabe-2222.jsonl"),
            "{\"payload\":{\"cwd\":\"/wt/other\"}}\n",
        )
        .expect("write");
        let found = codex_candidates(dir.path(), "/wt/target").expect("walk");
        assert_eq!(found.len(), 1);
        assert!(matches!(
            &found[0].store,
            StoreRef::Codex { rollout_path } if rollout_path.to_string_lossy().contains("deadbeef")
        ));

        // Direct rung matches by filename, and short hints never match.
        let direct = codex_by_id(dir.path(), "cafebabe-2222").expect("found");
        assert!(matches!(
            direct,
            StoreRef::Codex { ref rollout_path } if rollout_path.to_string_lossy().contains("cafebabe")
        ));
        assert!(
            codex_by_id(dir.path(), "cafe").is_none(),
            "short hint guarded"
        );
    }

    #[test]
    fn claude_by_id_scans_all_project_dirs_and_guards_short_hints() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("-any-project");
        std::fs::create_dir_all(&p).expect("mkdir");
        std::fs::write(p.join("abcd1234-x.jsonl"), "{}\n").expect("write");
        assert!(
            claude_by_id(dir.path(), "abcd1234-x")
                .expect("scan")
                .is_some()
        );
        assert!(
            claude_by_id(dir.path(), "missing-uuid")
                .expect("scan")
                .is_none()
        );
        assert!(claude_by_id(dir.path(), "ab").expect("scan").is_none());
    }
}
