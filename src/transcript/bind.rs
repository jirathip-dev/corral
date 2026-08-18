//! #63 agent → session binding: which store/session holds the transcript
//! for an agent working in a given worktree?
//!
//! Sources (all READ-ONLY, all optional — a missing store root simply
//! contributes no candidates):
//! - opencode: sessions whose `session.directory` equals the worktree
//!   (the session cwd — same column the legacy summary tooling matched),
//!   via the sqlite3-CLI read-only discipline shared with the readers.
//! - claude: `<claude_dir>/<encoded-cwd>/*.jsonl`, where the encoding maps
//!   every non-alphanumeric byte of the cwd to `-` (verified against the
//!   real layout: `/Users/x/.herdr/…` → `-Users-x--herdr-…`).
//! - codex: rollout `*.jsonl` files whose FIRST line carries
//!   `payload.cwd` equal to the worktree (bounded read: first line only).
//!
//! Multiple candidates tie-break by recency (last message time for
//! opencode, file mtime for the JSONL stores — both epoch millis). A tie
//! AT the maximum recency is surfaced as [`BindError::Ambiguous`] carrying
//! the full candidate list — never a guess presented as fact.
//!
//! The decision core ([`choose`]) is pure — candidates in, verdict out —
//! so the rules unit-test without any store; the per-store discovery fns
//! are thin IO adapters over paths a test can point at fixtures.

use std::path::{Path, PathBuf};

use super::{StoreRef, TranscriptError};

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
}

/// One session that matched the worktree. `recency_ms` is epoch millis of
/// the session's last activity (last message time / file mtime).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub store: StoreRef,
    pub recency_ms: u64,
}

impl Candidate {
    /// Human/client-facing name for ambiguity lists: the store kind plus
    /// the session id or file name — enough to pick one, no full paths
    /// beyond what the caller already knows.
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

/// Why no single store could be bound.
#[derive(Debug)]
pub enum BindError {
    /// No session in any store matches the worktree.
    NoSession { worktree: String },
    /// More than one session shares the maximum recency — the caller gets
    /// the full candidate list (newest first), never a silent pick.
    Ambiguous {
        worktree: String,
        candidates: Vec<Candidate>,
    },
    /// A store existed but could not be read/queried.
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

/// Resolve the single store/session for `worktree`, or a typed refusal.
/// A store root that does not exist contributes nothing; a store that
/// exists but fails to read only surfaces as the error when NO candidate
/// was found elsewhere (a readable match beats an unreadable maybe —
/// stated in the docs as a deliberate choice).
pub async fn bind_worktree(
    worktree: &str,
    roots: &TranscriptRoots,
) -> Result<StoreRef, BindError> {
    let mut candidates = Vec::new();
    let mut first_error: Option<BindError> = None;

    match opencode_candidates(&roots.opencode_db, worktree).await {
        Ok(mut found) => candidates.append(&mut found),
        Err(error) => first_error = first_error.or(Some(error)),
    }
    match claude_candidates(&roots.claude_dir, worktree) {
        Ok(mut found) => candidates.append(&mut found),
        Err(error) => first_error = first_error.or(Some(error)),
    }
    match codex_candidates(&roots.codex_dir, worktree) {
        Ok(mut found) => candidates.append(&mut found),
        Err(error) => first_error = first_error.or(Some(error)),
    }

    match choose(candidates, worktree) {
        Err(BindError::NoSession { worktree }) => match first_error {
            // Nothing matched AND a store was unreadable: report the
            // failure, not a confident "no session".
            Some(error) => Err(error),
            None => Err(BindError::NoSession { worktree }),
        },
        other => other,
    }
}

/// The pure decision rule: most recent candidate wins; a tie at the top is
/// ambiguous and returns everything (newest first); none is a typed miss.
pub fn choose(mut candidates: Vec<Candidate>, worktree: &str) -> Result<StoreRef, BindError> {
    if candidates.is_empty() {
        return Err(BindError::NoSession {
            worktree: worktree.to_string(),
        });
    }
    candidates.sort_by(|a, b| b.recency_ms.cmp(&a.recency_ms).then(a.label().cmp(&b.label())));
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
/// Lossy by design, which is why matching ENCODES the worktree and
/// compares directory names rather than trying to decode.
pub fn encode_claude_project_dir(cwd: &str) -> String {
    cwd.trim_end_matches('/')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Session rows matching the worktree, with recency = the session's last
/// message time (0 for a session with no messages — still a candidate).
/// Values embed via SQL single-quote doubling, same rule as the readers.
fn opencode_session_sql(worktree: &str) -> String {
    let wt = worktree.trim_end_matches('/').replace('\'', "''");
    format!(
        "SELECT s.id AS id, \
                COALESCE((SELECT MAX(m.time_created) FROM message m \
                          WHERE m.session_id = s.id), 0) AS recency \
         FROM session s WHERE RTRIM(s.directory, '/') = '{wt}'"
    )
}

async fn opencode_candidates(
    db_path: &Path,
    worktree: &str,
) -> Result<Vec<Candidate>, BindError> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let sql = opencode_session_sql(worktree);
    let fut = tokio::process::Command::new("sqlite3")
        .args(super::opencode_sqlite_args(db_path, &sql))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(super::OPENCODE_QUERY_TIMEOUT, fut)
        .await
        .map_err(|_| BindError::Store(TranscriptError::QueryTimeout))?
        .map_err(|_| BindError::Store(TranscriptError::Sqlite3Unavailable))?;
    if !output.status.success() {
        return Err(BindError::Store(TranscriptError::Sqlite3Unavailable));
    }
    let rows: Vec<serde_json::Value> = if output.stdout.iter().all(u8::is_ascii_whitespace) {
        Vec::new()
    } else {
        serde_json::from_slice(&output.stdout)
            .map_err(|_| BindError::Store(TranscriptError::Sqlite3Unavailable))?
    };
    Ok(rows
        .iter()
        .filter_map(|row| {
            let id = row.get("id")?.as_str()?;
            let recency = row.get("recency").and_then(serde_json::Value::as_i64)?;
            Some(Candidate {
                store: StoreRef::Opencode {
                    db_path: db_path.to_path_buf(),
                    session_id: id.to_string(),
                },
                recency_ms: recency.max(0) as u64,
            })
        })
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

fn claude_candidates(claude_dir: &Path, worktree: &str) -> Result<Vec<Candidate>, BindError> {
    let project = claude_dir.join(encode_claude_project_dir(worktree));
    if !project.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&project).map_err(|source| {
        BindError::Store(TranscriptError::StoreUnreadable {
            path: project.clone(),
            source,
        })
    })?;
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jsonl") && path.is_file() {
            found.push(Candidate {
                recency_ms: mtime_ms(&path),
                store: StoreRef::Claude { jsonl_path: path },
            });
        }
    }
    Ok(found)
}

/// Codex rollouts record their cwd in the FIRST line's `payload.cwd`.
/// Discovery walks `<codex_dir>` (bounded depth — the real layout is
/// `sessions/YYYY/MM/DD/rollout-*.jsonl`) reading at most the first line
/// of each file. Individually unreadable/torn files are skipped: one bad
/// rollout must not hide every other candidate.
const CODEX_WALK_MAX_DEPTH: usize = 6;
const CODEX_FIRST_LINE_MAX: u64 = 64 * 1024;

fn codex_candidates(codex_dir: &Path, worktree: &str) -> Result<Vec<Candidate>, BindError> {
    if !codex_dir.is_dir() {
        return Ok(Vec::new());
    }
    let want = worktree.trim_end_matches('/');
    let mut found = Vec::new();
    let mut stack = vec![(codex_dir.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            // The ROOT being unreadable is an error; a subdir vanishing
            // mid-walk (session rotation) is not.
            if depth == 0 {
                return Err(BindError::Store(TranscriptError::StoreUnreadable {
                    path: dir,
                    source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "read_dir"),
                }));
            }
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < CODEX_WALK_MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
            } else if path.extension().is_some_and(|e| e == "jsonl")
                && codex_rollout_cwd(&path).is_some_and(|cwd| cwd.trim_end_matches('/') == want)
            {
                found.push(Candidate {
                    recency_ms: mtime_ms(&path),
                    store: StoreRef::Codex { rollout_path: path },
                });
            }
        }
    }
    Ok(found)
}

/// The `payload.cwd` of a rollout's first line, reading at most
/// [`CODEX_FIRST_LINE_MAX`] bytes. `None` on any shortfall (unreadable,
/// torn, not JSON, no cwd) — skip semantics, never an error.
fn codex_rollout_cwd(path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader, Read};
    let file = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file.take(CODEX_FIRST_LINE_MAX))
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
                // The FULL list, sorted newest-first then by label — the
                // stale one is included so the client sees everything.
                assert_eq!(candidates.len(), 3);
                assert_eq!(candidates[0].recency_ms, 7);
                assert_eq!(candidates[1].recency_ms, 7);
                assert_eq!(candidates[2].label(), "opencode:stale");
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn claude_encoding_matches_live_layout() {
        assert_eq!(
            encode_claude_project_dir("/Users/x/.herdr/worktrees/corral/corral-g35b"),
            "-Users-x--herdr-worktrees-corral-corral-g35b"
        );
        // Dots, underscores and any other punctuation all map to '-';
        // trailing slash is normalized before encoding.
        assert_eq!(
            encode_claude_project_dir("/a/b.c/d_e/f-g/"),
            "-a-b-c-d-e-f-g"
        );
        assert_eq!(encode_claude_project_dir("/Users/x"), "-Users-x");
    }

    #[test]
    fn opencode_session_sql_escapes_and_normalizes() {
        let sql = opencode_session_sql("/w/o'brien/");
        assert!(sql.contains("'/w/o''brien'"), "{sql}");
        assert!(sql.contains("RTRIM(s.directory, '/')"), "{sql}");
        assert!(!sql.contains("brien/'"), "trailing slash must be trimmed: {sql}");
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
    fn codex_walk_finds_nested_rollouts_and_matches_trimmed_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let day = dir.path().join("2026/08/18");
        std::fs::create_dir_all(&day).expect("mkdirs");
        std::fs::write(
            day.join("rollout-a.jsonl"),
            "{\"payload\":{\"cwd\":\"/wt/target/\"}}\n",
        )
        .expect("write");
        std::fs::write(
            day.join("rollout-b.jsonl"),
            "{\"payload\":{\"cwd\":\"/wt/other\"}}\n",
        )
        .expect("write");
        let found = codex_candidates(dir.path(), "/wt/target").expect("walk");
        assert_eq!(found.len(), 1);
        assert!(matches!(
            &found[0].store,
            StoreRef::Codex { rollout_path } if rollout_path.ends_with("rollout-a.jsonl")
        ));
    }

    #[test]
    fn claude_candidates_lists_only_jsonl_in_encoded_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join(encode_claude_project_dir("/wt/repo"));
        std::fs::create_dir_all(&project).expect("mkdir");
        std::fs::write(project.join("s1.jsonl"), "{}\n").expect("write");
        std::fs::write(project.join("s2.jsonl"), "{}\n").expect("write");
        std::fs::write(project.join("notes.txt"), "x").expect("write");
        let found = claude_candidates(dir.path(), "/wt/repo").expect("list");
        assert_eq!(found.len(), 2);

        // No project dir at all: clean empty, not an error.
        assert!(claude_candidates(dir.path(), "/wt/missing").expect("empty").is_empty());
    }
}
