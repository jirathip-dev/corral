//! #62 (D35 slice 1): transcript read-path core — per-store paged readers.
//!
//! Given an explicit store reference, return one page of transcript
//! entries, NEWEST-FIRST, with every entry redacted (D-083 rules,
//! [`crate::core::redact`]) BEFORE it leaves this module — no unredacted
//! text crosses the boundary. No HTTP surface, no UI, no agent→session
//! discovery here (those are #63/#64).
//!
//! Store disciplines:
//! - **opencode** (`opencode.db`, 13GB+ in steady state): the same
//!   sqlite3-CLI pattern as [`crate::cost::opencode`] — `-readonly`, busy
//!   timeout, JSON output, every query bounded in SQL by session id and a
//!   `(time_created, id)` cursor. Never a full scan, never a write, and no
//!   sqlite crate (the system `sqlite3` binary is the documented trade;
//!   its absence is a typed error, not a panic). The schema coded against
//!   is the one the cost reader feature-detects (`message` rows with a
//!   `data` JSON blob; `part` rows carrying text); slice 2 revalidates
//!   against live stores before anything user-facing depends on it.
//! - **claude / codex** (JSONL transcripts, 100MB+): read BACKWARDS from a
//!   byte-offset cursor in bounded chunks, parse only the requested page.
//!   Opening the newest page of a huge file reads at most a few chunks
//!   from the tail — pinned by test via the returned cursor offset, not
//!   wall-clock.
//!
//! Malformed lines are skipped and counted ([`TranscriptPage::skipped`]),
//! never a panic: session stores are written by external tools mid-crash,
//! so torn tails are normal, and silent drops would make a page look
//! complete when it is not.

use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::core::redact::redact;

/// Hard page caps: at most this many entries AND at most this much entry
/// text per page, whichever bites first. Callers may ask for less.
pub const MAX_PAGE_ENTRIES: usize = 50;
pub const MAX_PAGE_TEXT_BYTES: usize = 256 * 1024;

/// How much of a JSONL file tail is read per disk round-trip while
/// assembling a page. Bounded-read guarantee: a page consumes at most
/// enough chunks to fill its caps, regardless of file size.
const JSONL_CHUNK_BYTES: u64 = 64 * 1024;

/// The explicit store to read. Slice 1 takes fully-resolved references —
/// binding an agent to its store is #63's job. (The brief sketched
/// `Opencode { session_id }` alone; the db path is a parameter here so the
/// reader is testable against fixtures and multi-store hosts — noted in
/// the report.)
#[derive(Debug, Clone, PartialEq)]
pub enum StoreRef {
    Opencode {
        db_path: PathBuf,
        session_id: String,
    },
    Claude {
        jsonl_path: PathBuf,
    },
    Codex {
        rollout_path: PathBuf,
    },
}

/// One transcript entry, already redacted. `ts` is epoch millis when the
/// store carries a numeric timestamp; stores with string-only timestamps
/// yield `None` in slice 1 (normalising them is #63 polish, not worth a
/// date-parsing dependency here).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub role: String,
    pub text: String,
    pub ts: Option<u64>,
}

/// Where the NEXT (older) page starts. Opaque to callers; stable across
/// store growth because both variants key on positions that appends never
/// move (older rows / earlier byte offsets).
#[derive(Debug, Clone, PartialEq)]
pub enum Cursor {
    /// Read opencode rows strictly older than `(time_created, id)`.
    Opencode { time_created: i64, id: String },
    /// Read JSONL lines that END at or before this byte offset.
    Bytes { offset: u64 },
}

/// One page, newest-first. `next_cursor: None` means the store is
/// exhausted. `skipped` counts malformed lines/rows encountered while
/// building THIS page (honesty counter — a nonzero value means the page
/// is complete but the store had torn data in this range).
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptPage {
    pub entries: Vec<Entry>,
    pub next_cursor: Option<Cursor>,
    pub skipped: usize,
}

/// Why a page could not be read.
#[derive(Debug)]
pub enum TranscriptError {
    /// The store file could not be opened/read.
    StoreUnreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The system `sqlite3` binary is missing or failed — the opencode
    /// reader shells out to it by design (no sqlite crate).
    Sqlite3Unavailable,
    /// The cursor does not belong to this store kind or is out of range.
    BadCursor,
}

impl std::fmt::Display for TranscriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranscriptError::StoreUnreadable { path, source } => {
                write!(f, "cannot read session store {}: {source}", path.display())
            }
            TranscriptError::Sqlite3Unavailable => {
                write!(
                    f,
                    "the sqlite3 binary is unavailable (opencode stores are read via the sqlite3 CLI)"
                )
            }
            TranscriptError::BadCursor => write!(f, "cursor does not match this store"),
        }
    }
}

impl std::error::Error for TranscriptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TranscriptError::StoreUnreadable { source, .. } => Some(source),
            TranscriptError::Sqlite3Unavailable | TranscriptError::BadCursor => None,
        }
    }
}

/// Read one newest-first page from `store`, starting at `cursor` (or the
/// newest content when `None`). `limit` is clamped to
/// [`MAX_PAGE_ENTRIES`]; the [`MAX_PAGE_TEXT_BYTES`] budget applies on
/// top, truncating the page early (with a cursor that resumes exactly
/// where it stopped) rather than dropping entries silently.
pub async fn read_page(
    store: &StoreRef,
    cursor: Option<&Cursor>,
    limit: usize,
) -> Result<TranscriptPage, TranscriptError> {
    let limit = limit.clamp(1, MAX_PAGE_ENTRIES);
    match store {
        StoreRef::Opencode {
            db_path,
            session_id,
        } => {
            let cur = match cursor {
                None => None,
                Some(Cursor::Opencode { time_created, id }) => Some((*time_created, id.as_str())),
                Some(Cursor::Bytes { .. }) => return Err(TranscriptError::BadCursor),
            };
            read_opencode_page(db_path, session_id, cur, limit).await
        }
        StoreRef::Claude { jsonl_path } => read_jsonl_page(jsonl_path, cursor, limit).await,
        StoreRef::Codex { rollout_path } => read_jsonl_page(rollout_path, cursor, limit).await,
    }
}

/// The exact sqlite3 argv for one opencode page query — factored out so a
/// test can pin the read-only discipline (`-readonly`, busy timeout,
/// bounded SQL) without needing the binary.
fn opencode_sqlite_args(db_path: &Path, sql: &str) -> Vec<String> {
    vec![
        "-readonly".to_string(),
        "-json".to_string(),
        "-cmd".to_string(),
        ".timeout 2000".to_string(),
        db_path.display().to_string(),
        sql.to_string(),
    ]
}

/// The page SQL: newest-first within one session, strictly older than the
/// cursor, joined to text parts, LIMIT-bounded in SQL (never in Rust).
/// `session_id` and cursor id are embedded via SQL single-quote escaping —
/// sqlite3-CLI has no bind parameters; the values come from our own
/// cursor/store structs, and doubling `'` is the complete quoting rule for
/// SQLite string literals.
fn opencode_page_sql(session_id: &str, cursor: Option<(i64, &str)>, limit: usize) -> String {
    let sid = session_id.replace('\'', "''");
    let cursor_clause = match cursor {
        Some((t, id)) => format!(
            "AND (m.time_created < {t} OR (m.time_created = {t} AND m.id < '{}'))",
            id.replace('\'', "''")
        ),
        None => String::new(),
    };
    format!(
        "SELECT m.id AS id, m.role AS role, m.time_created AS time_created, \
                p.data AS part_data, m.data AS msg_data \
         FROM message m LEFT JOIN part p ON p.message_id = m.id \
         WHERE m.session_id = '{sid}' {cursor_clause} \
         ORDER BY m.time_created DESC, m.id DESC LIMIT {limit}"
    )
}

async fn read_opencode_page(
    db_path: &Path,
    session_id: &str,
    cursor: Option<(i64, &str)>,
    limit: usize,
) -> Result<TranscriptPage, TranscriptError> {
    if !db_path.exists() {
        return Err(TranscriptError::StoreUnreadable {
            path: db_path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such store"),
        });
    }
    let sql = opencode_page_sql(session_id, cursor, limit);
    let output = tokio::process::Command::new("sqlite3")
        .args(opencode_sqlite_args(db_path, &sql))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .map_err(|_| TranscriptError::Sqlite3Unavailable)?;
    if !output.status.success() {
        return Err(TranscriptError::Sqlite3Unavailable);
    }
    let rows: Vec<Value> = if output.stdout.iter().all(u8::is_ascii_whitespace) {
        Vec::new()
    } else {
        serde_json::from_slice(&output.stdout).map_err(|_| TranscriptError::Sqlite3Unavailable)?
    };

    let mut entries = Vec::new();
    let mut skipped = 0usize;
    let mut text_budget = MAX_PAGE_TEXT_BYTES;
    let mut last_key: Option<(i64, String)> = None;
    let full_rows = rows.len();
    for row in rows {
        let (Some(id), Some(t)) = (
            row.get("id").and_then(Value::as_str),
            row.get("time_created").and_then(Value::as_i64),
        ) else {
            skipped += 1;
            continue;
        };
        let role = row
            .get("role")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                row.get("msg_data")
                    .and_then(Value::as_str)
                    .and_then(|d| serde_json::from_str::<Value>(d).ok())
                    .and_then(|d| d.get("role").and_then(Value::as_str).map(str::to_string))
            })
            .unwrap_or_else(|| "unknown".to_string());
        let text = row
            .get("part_data")
            .and_then(Value::as_str)
            .and_then(|d| serde_json::from_str::<Value>(d).ok())
            .and_then(|d| d.get("text").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default();
        let text = redact(&text).into_owned();
        if text.len() > text_budget && !entries.is_empty() {
            // Budget hit: stop BEFORE this entry; the cursor resumes at it.
            break;
        }
        text_budget = text_budget.saturating_sub(text.len());
        last_key = Some((t, id.to_string()));
        entries.push(Entry {
            role,
            text,
            ts: Some(t as u64),
        });
        if entries.len() >= limit {
            break;
        }
    }
    // More rows may exist iff the query returned a full LIMIT worth or we
    // stopped early on budget; exhaustion = the query underfilled AND we
    // consumed every row.
    let consumed_all = entries.len() + skipped == full_rows;
    let next_cursor = match (&last_key, full_rows == limit || !consumed_all) {
        (Some((t, id)), true) => Some(Cursor::Opencode {
            time_created: *t,
            id: id.clone(),
        }),
        _ => None,
    };
    Ok(TranscriptPage {
        entries,
        next_cursor,
        skipped,
    })
}

/// Backwards-paged JSONL reader shared by the claude and codex stores.
///
/// Bounded-read contract: each call reads ONE tail range of at most
/// [`MAX_PAGE_TEXT_BYTES`] + 4×[`JSONL_CHUNK_BYTES`] bytes ending at the
/// cursor (EOF when `None`), regardless of file size — a 100MB transcript
/// opens its newest page by reading only that tail slice. If the range
/// cannot fill the page (many skipped lines), the call returns a SHORT
/// page whose cursor resumes exactly below the range — every call stays
/// bounded and the walk stays complete.
///
/// The cursor is the byte offset lines must END strictly before; `None`
/// starts at EOF.
async fn read_jsonl_page(
    path: &Path,
    cursor: Option<&Cursor>,
    limit: usize,
) -> Result<TranscriptPage, TranscriptError> {
    let unreadable = |source: std::io::Error| TranscriptError::StoreUnreadable {
        path: path.to_path_buf(),
        source,
    };
    let mut file = tokio::fs::File::open(path).await.map_err(unreadable)?;
    let len = file.metadata().await.map_err(unreadable)?.len();
    let end = match cursor {
        None => len,
        Some(Cursor::Bytes { offset }) if *offset <= len => *offset,
        Some(_) => return Err(TranscriptError::BadCursor),
    };
    if end == 0 {
        return Ok(TranscriptPage {
            entries: Vec::new(),
            next_cursor: None,
            skipped: 0,
        });
    }

    let scan_cap = MAX_PAGE_TEXT_BYTES as u64 + 4 * JSONL_CHUNK_BYTES;
    let lower = end.saturating_sub(scan_cap);
    let mut buf = vec![0u8; (end - lower) as usize];
    file.seek(std::io::SeekFrom::Start(lower))
        .await
        .map_err(unreadable)?;
    file.read_exact(&mut buf).await.map_err(unreadable)?;

    // Split into lines with ABSOLUTE start offsets. When the range does
    // not begin at the file start, the first segment is (potentially) a
    // partial line and is never consumed — the resume cursor lands on the
    // boundary after it instead.
    let mut lines: Vec<(u64, &[u8])> = Vec::new();
    let mut pos = lower;
    for seg in buf.split(|b| *b == b'\n') {
        lines.push((pos, seg));
        pos += seg.len() as u64 + 1;
    }
    let first_consumable = if lower > 0 { 1 } else { 0 };

    let mut entries = Vec::new();
    let mut skipped = 0usize;
    let mut text_budget = MAX_PAGE_TEXT_BYTES;
    let mut oldest_consumed_start: Option<u64> = None;
    let mut stopped_early = false;

    for (line_start, raw) in lines.iter().skip(first_consumable).rev() {
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        if line.iter().all(u8::is_ascii_whitespace) {
            oldest_consumed_start = Some(*line_start);
            continue;
        }
        let parsed: Option<Entry> = serde_json::from_slice::<Value>(line)
            .ok()
            .and_then(|v| jsonl_entry(&v));
        let Some(entry) = parsed else {
            skipped += 1;
            oldest_consumed_start = Some(*line_start);
            continue;
        };
        if entry.text.len() > text_budget && !entries.is_empty() {
            stopped_early = true;
            break;
        }
        text_budget = text_budget.saturating_sub(entry.text.len());
        entries.push(entry);
        oldest_consumed_start = Some(*line_start);
        if entries.len() >= limit {
            stopped_early = true;
            break;
        }
    }

    // Resume point: the start of the oldest line this call consumed; when
    // we consumed the whole range, resume below it (the partial-line
    // boundary), and only a range that reached byte 0 is exhausted.
    let resume = if stopped_early {
        oldest_consumed_start
    } else if lower > 0 {
        Some(lines.get(first_consumable).map_or(lower, |(s, _)| *s))
    } else {
        None
    };
    let next_cursor = resume
        .filter(|r| *r > 0)
        .map(|offset| Cursor::Bytes { offset });
    Ok(TranscriptPage {
        entries,
        next_cursor,
        skipped,
    })
}

/// Decode one JSONL object into an [`Entry`], tolerating the claude
/// (`{type, message:{role, content:[{type:"text",text}...]}}`) and codex
/// rollout (`{role?, text?/content?}`) shapes. `None` = not a
/// transcript-bearing line (metadata records are normal, counted as
/// skipped by the caller only when unparseable — a parseable non-message
/// line is silently ignored... no: counted skipped too, so pages stay
/// honest about what they passed over).
fn jsonl_entry(value: &Value) -> Option<Entry> {
    let msg = value.get("message").unwrap_or(value);
    let role = msg
        .get("role")
        .and_then(Value::as_str)
        .or_else(|| value.get("type").and_then(Value::as_str))?
        .to_string();
    let text = match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => {
            let mut joined = String::new();
            for part in parts {
                if let Some(t) = part.get("text").and_then(Value::as_str) {
                    if !joined.is_empty() {
                        joined.push('\n');
                    }
                    joined.push_str(t);
                }
            }
            joined
        }
        _ => msg
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    };
    if text.is_empty() {
        return None;
    }
    let ts = value
        .get("ts")
        .or_else(|| value.get("timestamp"))
        .and_then(Value::as_u64);
    Some(Entry {
        role,
        text: redact(&text).into_owned(),
        ts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn claude_line(role: &str, text: &str, ts: u64) -> String {
        serde_json::json!({
            "type": role,
            "message": {"role": role, "content": [{"type": "text", "text": text}]},
            "ts": ts,
        })
        .to_string()
    }

    fn write_jsonl(lines: &[String]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("temp jsonl");
        for l in lines {
            writeln!(f, "{l}").expect("write line");
        }
        f.flush().expect("flush");
        f
    }

    async fn walk(store: &StoreRef, limit: usize) -> (Vec<Entry>, usize, usize) {
        let mut all = Vec::new();
        let mut cursor: Option<Cursor> = None;
        let mut pages = 0;
        let mut skipped = 0;
        loop {
            let page = read_page(store, cursor.as_ref(), limit)
                .await
                .expect("page");
            pages += 1;
            skipped += page.skipped;
            all.extend(page.entries);
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
            assert!(pages < 1000, "cursor walk must terminate");
        }
        (all, pages, skipped)
    }

    /// Newest-first, no gaps, no duplicates, across a multi-page walk.
    #[tokio::test]
    async fn jsonl_walk_is_newest_first_complete_and_duplicate_free() {
        let lines: Vec<String> = (0..120)
            .map(|i| claude_line("assistant", &format!("message number {i}"), i))
            .collect();
        let f = write_jsonl(&lines);
        let store = StoreRef::Claude {
            jsonl_path: f.path().to_path_buf(),
        };
        let (all, pages, skipped) = walk(&store, 50).await;
        assert_eq!(all.len(), 120, "no gaps");
        assert!(pages >= 3, "walk actually paged");
        assert_eq!(skipped, 0);
        let ts: Vec<u64> = all.iter().map(|e| e.ts.expect("ts")).collect();
        let mut expected: Vec<u64> = (0..120).rev().collect();
        assert_eq!(ts, expected.as_mut_slice(), "newest-first, no duplicates");
    }

    /// The text budget truncates a page WITHOUT losing entries: the cursor
    /// resumes exactly at the next entry.
    #[tokio::test]
    async fn jsonl_text_budget_truncates_but_the_walk_stays_complete() {
        // 9 entries of ~64KiB each: a 256KiB page fits at most 4.
        let big = "x".repeat(64 * 1024);
        let lines: Vec<String> = (0..9).map(|i| claude_line("user", &big, i)).collect();
        let f = write_jsonl(&lines);
        let store = StoreRef::Claude {
            jsonl_path: f.path().to_path_buf(),
        };
        let first = read_page(&store, None, 50).await.expect("page");
        assert!(
            first.entries.len() < 9,
            "budget must truncate: got {}",
            first.entries.len()
        );
        assert!(first.next_cursor.is_some(), "truncation leaves a cursor");
        let (all, _, skipped) = walk(&store, 50).await;
        assert_eq!(all.len(), 9, "nothing dropped across the walk");
        assert_eq!(skipped, 0);
    }

    /// Seeded secrets are redacted for every JSONL store kind.
    #[tokio::test]
    async fn jsonl_pages_are_redacted_for_claude_and_codex_shapes() {
        let secrets = "key sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghij-AA and ghp_abcdefghijklmnopqrstuvwxyz0123456789 and AKIAIOSFODNN7EXAMPLE";
        let claude = write_jsonl(&[claude_line("assistant", secrets, 1)]);
        // codex rollout shape: flat role/text.
        let codex_line =
            serde_json::json!({"role": "assistant", "text": secrets, "ts": 2}).to_string();
        let codex = write_jsonl(&[codex_line]);

        for store in [
            StoreRef::Claude {
                jsonl_path: claude.path().to_path_buf(),
            },
            StoreRef::Codex {
                rollout_path: codex.path().to_path_buf(),
            },
        ] {
            let page = read_page(&store, None, 10).await.expect("page");
            assert_eq!(page.entries.len(), 1);
            let text = &page.entries[0].text;
            assert!(
                !text.contains("sk-ant-api03-abcdefgh"),
                "anthropic key redacted: {text}"
            );
            assert!(
                !text.contains("ghp_abcdefgh"),
                "github token redacted: {text}"
            );
            assert!(!text.contains("AKIAIOSFODNN7"), "aws key redacted: {text}");
        }
    }

    /// Malformed lines are skipped and counted; parsing never panics.
    #[tokio::test]
    async fn jsonl_malformed_lines_are_skipped_and_counted() {
        let lines = vec![
            claude_line("user", "good one", 1),
            "{not json at all".to_string(),
            "\u{7f}\u{7f}garbage\\".to_string(),
            claude_line("assistant", "good two", 2),
        ];
        let f = write_jsonl(&lines);
        let store = StoreRef::Claude {
            jsonl_path: f.path().to_path_buf(),
        };
        let (all, _, skipped) = walk(&store, 50).await;
        assert_eq!(all.len(), 2, "good entries survive");
        assert_eq!(skipped, 2, "torn lines counted, not hidden");
    }

    /// Bounded-read pin: the first page of a ~20MB file must leave its
    /// cursor within the capped tail range — proof the call never scanned
    /// toward the file start. (Cursor offset is an exact observable of
    /// bytes considered; wall-clock would prove nothing.)
    #[tokio::test]
    async fn jsonl_first_page_of_a_large_file_reads_only_the_tail() {
        let mut f = tempfile::NamedTempFile::new().expect("temp jsonl");
        let filler = claude_line("user", &"y".repeat(200), 0);
        for _ in 0..80_000 {
            writeln!(f, "{filler}").expect("write");
        }
        f.flush().expect("flush");
        let len = f.as_file().metadata().expect("meta").len();
        assert!(len > 20_000_000, "fixture is actually large: {len}");

        let store = StoreRef::Claude {
            jsonl_path: f.path().to_path_buf(),
        };
        let page = read_page(&store, None, 50).await.expect("page");
        assert_eq!(page.entries.len(), 50);
        let Some(Cursor::Bytes { offset }) = page.next_cursor else {
            panic!("large file must leave a cursor");
        };
        let scan_cap = MAX_PAGE_TEXT_BYTES as u64 + 4 * JSONL_CHUNK_BYTES;
        assert!(
            offset >= len - scan_cap,
            "cursor {offset} must stay within the tail scan cap ({} of {len})",
            len - scan_cap
        );
    }

    /// The read-only discipline for opencode is pinned at the argv level —
    /// no sqlite3 binary needed to enforce it in CI.
    #[test]
    fn opencode_invocation_is_readonly_json_and_time_bounded() {
        let args = opencode_sqlite_args(Path::new("/tmp/x.db"), "SELECT 1");
        assert_eq!(args[0], "-readonly", "read-only is the FIRST flag");
        assert!(args.contains(&"-json".to_string()));
        assert!(args.iter().any(|a| a.starts_with(".timeout")));
    }

    /// SQL quoting: a session id carrying a single quote cannot break out
    /// of the literal.
    #[test]
    fn opencode_sql_escapes_quotes_and_bounds_the_query() {
        let sql = opencode_page_sql("ses'--x", Some((42, "id'y")), 7);
        assert!(sql.contains("'ses''--x'"), "session id escaped: {sql}");
        assert!(sql.contains("'id''y'"), "cursor id escaped: {sql}");
        assert!(sql.contains("LIMIT 7"), "bounded in SQL: {sql}");
        assert!(sql.contains("ORDER BY m.time_created DESC, m.id DESC"));
    }

    fn have_sqlite3() -> bool {
        std::process::Command::new("sqlite3")
            .arg("-version")
            .output()
            .is_ok()
    }

    /// Fixture-driven opencode paging + redaction, skipped cleanly when the
    /// sqlite3 binary is absent (same convention as the cost tests).
    #[tokio::test]
    async fn opencode_fixture_pages_newest_first_and_redacts() {
        if !have_sqlite3() {
            eprintln!("skipping: sqlite3 binary not available");
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("opencode.db");
        let seed = r#"
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, role TEXT, time_created INTEGER, data TEXT);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, type TEXT, data TEXT);
            INSERT INTO message VALUES ('m1','ses1','user',100,'{}');
            INSERT INTO part VALUES ('p1','m1','text','{"text":"first question"}');
            INSERT INTO message VALUES ('m2','ses1','assistant',200,'{}');
            INSERT INTO part VALUES ('p2','m2','text','{"text":"token ghp_abcdefghijklmnopqrstuvwxyz0123456789 leaked"}');
            INSERT INTO message VALUES ('m3','ses2','user',300,'{}');
            INSERT INTO part VALUES ('p3','m3','text','{"text":"other session"}');
        "#;
        let status = std::process::Command::new("sqlite3")
            .arg(&db)
            .arg(seed)
            .status()
            .expect("seed fixture");
        assert!(status.success(), "fixture seeded");

        let store = StoreRef::Opencode {
            db_path: db.clone(),
            session_id: "ses1".to_string(),
        };
        // Page size 1: newest first, then the cursor walks older.
        let p1 = read_page(&store, None, 1).await.expect("page 1");
        assert_eq!(p1.entries.len(), 1);
        assert_eq!(p1.entries[0].role, "assistant");
        assert!(
            !p1.entries[0].text.contains("ghp_abcdefgh"),
            "opencode pages are redacted: {}",
            p1.entries[0].text
        );
        let p2 = read_page(&store, p1.next_cursor.as_ref(), 1)
            .await
            .expect("page 2");
        assert_eq!(p2.entries.len(), 1);
        assert_eq!(p2.entries[0].text, "first question");
        assert_eq!(
            p2.entries[0].role, "user",
            "other sessions never leak into the page"
        );
    }
}
