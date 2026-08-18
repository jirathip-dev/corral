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
//!   timeout, JSON output, every query bounded in SQL by session id, a
//!   `(time_created, id)` cursor, LIMIT, and a per-message `substr` text
//!   cap (fresh-review R2). Never a write, and no sqlite crate (the
//!   system `sqlite3` binary is the documented trade; its absence is a
//!   typed error, not a panic). Honesty (fresh-review R3/R5): the schema
//!   facts used here (`message.{id,session_id,time_created,data}`,
//!   `part.{id,message_id,type,data}`) go BEYOND the single column the
//!   cost reader probes, and role deliberately comes from the `data`
//!   JSON only (no `m.role` — its existence is unprobed). Bounded row
//!   EXAMINATION additionally depends on the live store carrying indexes
//!   on `message(session_id, time_created)` and `part(message_id)` —
//!   without them the plan degrades to a per-page table scan; the
//!   sargable shape is pinned by test, the index presence cannot be.
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

pub mod bind;

use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::core::redact::redact;

/// Hard page caps: at most this many entries AND at most this much entry
/// text per page, whichever bites first — including a page's FIRST
/// entry, which is truncated to the budget with [`TRUNCATED_MARKER`]
/// rather than admitted whole (fresh-review R2; the marker itself rides
/// ON TOP of the budget, so a truncated page can exceed it by the
/// marker's length — S8). Callers may ask for less.
pub const MAX_PAGE_ENTRIES: usize = 50;
pub const MAX_PAGE_TEXT_BYTES: usize = 256 * 1024;

/// Sizing unit for the bounded tail read: one `read_exact` of at most
/// `MAX_PAGE_TEXT_BYTES + 4 * JSONL_CHUNK_BYTES` covers a page (there is
/// no per-chunk loop — fresh-review R8: the read is bounded, not
/// chunked). Regardless of file size, a page never reads more than that
/// window from the tail.
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

impl Cursor {
    /// Opaque wire form for HTTP clients (#63): `oc.<time>.<hex-id>` /
    /// `b.<offset>`. The opencode id is hex-encoded so arbitrary id bytes
    /// survive the dot-delimited framing; clients must treat the whole
    /// string as opaque — the format may change with the store schemas.
    pub fn encode(&self) -> String {
        match self {
            Cursor::Opencode { time_created, id } => {
                let hex: String = id.bytes().map(|b| format!("{b:02x}")).collect();
                format!("oc.{time_created}.{hex}")
            }
            Cursor::Bytes { offset } => format!("b.{offset}"),
        }
    }

    /// Inverse of [`Cursor::encode`]. Any malformed input is
    /// [`TranscriptError::BadCursor`] — never a panic, never a guess.
    pub fn decode(wire: &str) -> Result<Self, TranscriptError> {
        if let Some(rest) = wire.strip_prefix("b.") {
            let offset = rest.parse().map_err(|_| TranscriptError::BadCursor)?;
            return Ok(Cursor::Bytes { offset });
        }
        let rest = wire.strip_prefix("oc.").ok_or(TranscriptError::BadCursor)?;
        let (time, hex) = rest.split_once('.').ok_or(TranscriptError::BadCursor)?;
        let time_created = time.parse().map_err(|_| TranscriptError::BadCursor)?;
        // ASCII-hex only, checked up front: slicing below is byte-indexed
        // and a multi-byte char would otherwise panic on a char boundary.
        if hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(TranscriptError::BadCursor);
        }
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(|_| TranscriptError::BadCursor)?;
        let id = String::from_utf8(bytes).map_err(|_| TranscriptError::BadCursor)?;
        Ok(Cursor::Opencode { time_created, id })
    }
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

/// Fresh-review R2: appended to an entry whose text was truncated to the
/// page budget — the cap is a CONTRACT ("whichever bites first"), so an
/// oversized entry (even the page's first) is truncated with an explicit
/// marker rather than admitted whole.
pub const TRUNCATED_MARKER: &str = "\n… [truncated: entry exceeded the page text budget]";

/// Truncate `text` to `budget` bytes on a char boundary and append the
/// marker. Only called when `text.len() > budget`.
fn truncate_to_budget(text: &str, budget: usize) -> String {
    let mut end = budget;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATED_MARKER}", &text[..end])
}

/// Wall-clock cap on one sqlite3 invocation (mirrors the cost reader's
/// discipline — the busy timeout only covers lock waits, not a scan).
const OPENCODE_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

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
    /// The opencode query exceeded the wall-clock cap.
    QueryTimeout,
    /// The store answered, but every row in a full page failed extraction —
    /// the schema does not match what this reader codes against. Surfaced
    /// as an error (never as a silent empty/exhausted transcript).
    StoreShape,
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
            TranscriptError::QueryTimeout => {
                write!(f, "opencode query exceeded its wall-clock timeout")
            }
            TranscriptError::StoreShape => {
                write!(
                    f,
                    "the store's rows do not match the expected schema (no usable rows in a full page)"
                )
            }
        }
    }
}

impl std::error::Error for TranscriptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TranscriptError::StoreUnreadable { source, .. } => Some(source),
            TranscriptError::Sqlite3Unavailable
            | TranscriptError::BadCursor
            | TranscriptError::QueryTimeout
            | TranscriptError::StoreShape => None,
        }
    }
}

/// Read one newest-first page from `store`, starting at `cursor` (or the
/// newest content when `None`). `limit` is clamped into
/// `1..=MAX_PAGE_ENTRIES` (a `limit` of 0 reads one entry — a zero-entry
/// page would be indistinguishable from exhaustion); the
/// [`MAX_PAGE_TEXT_BYTES`] budget applies on top, truncating the page
/// early (with a cursor that resumes exactly where it stopped) rather
/// than dropping entries silently.
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
fn opencode_sqlite_args(db_path: &Path, sql: &str) -> Vec<std::ffi::OsString> {
    vec![
        "-readonly".into(),
        "-json".into(),
        "-cmd".into(),
        ".timeout 2000".into(),
        db_path.as_os_str().to_os_string(),
        sql.into(),
    ]
}

/// The page SQL: newest-first within one session, strictly older than the
/// cursor, ONE ROW PER MESSAGE — text parts are aggregated in SQL with a
/// deterministic order (`p.id`) via an ordered subselect, so the keyset
/// key `(m.time_created, m.id)` is genuinely unique over the result rows
/// (a LEFT JOIN would emit one row per part and a page boundary inside a
/// multi-part message would silently drop its tail — review F2). Non-text
/// parts never contribute (F7). LIMIT-bounded in SQL, never in Rust.
/// `session_id` and cursor id are embedded via SQL single-quote escaping —
/// sqlite3-CLI has no bind parameters; the values come from our own
/// cursor/store structs, and doubling `'` is the complete quoting rule for
/// SQLite string literals.
fn opencode_page_sql(
    session_id: &str,
    cursor: Option<(i64, &str)>,
    limit: usize,
    has_role_column: bool,
) -> String {
    let sid = session_id.replace('\'', "''");
    let cursor_clause = match cursor {
        Some((t, id)) => format!(
            "AND (m.time_created < {t} OR (m.time_created = {t} AND m.id < '{}'))",
            id.replace('\'', "''")
        ),
        None => String::new(),
    };
    // R2: substr caps a message's assembled text IN SQL (budget + one
    // byte so the Rust side can detect and mark the truncation) — one
    // giant message can no longer make a page unbounded (the old ceiling
    // was SQLITE_MAX_LENGTH, ~1GB). R3: role comes from msg_data's JSON
    // only — `m.role` was an unprobed schema assumption (the cost
    // reader's fixtures declare `message` without it; an absent column
    // would hard-fail EVERY page, and the old NULL-fallback protected
    // against the wrong failure).
    // S6: substr counts CHARS, so the intermediate can overshoot the
    // byte budget by up to 4x on multi-byte text (and group_concat still
    // materialises the full concatenation inside the sqlite3 child,
    // bounded only by SQLITE_MAX_LENGTH); the final entry is byte-capped
    // in Rust. The +1 makes the cap detectable for S1's seam handling.
    let cap = MAX_PAGE_TEXT_BYTES + 1;
    // S3: `m.role` appears only when the probe found the column.
    let role_select = if has_role_column {
        "m.role AS role, "
    } else {
        ""
    };
    format!(
        "SELECT m.id AS id, {role_select}m.time_created AS time_created, \
                m.data AS msg_data, \
                substr((SELECT group_concat(t, char(10)) FROM \
                   (SELECT CASE WHEN json_valid(p.data) \
                           THEN json_extract(p.data, '$.text') END AS t \
                    FROM part p \
                    WHERE p.message_id = m.id AND p.type = 'text' \
                    ORDER BY p.id) \
                 WHERE t IS NOT NULL), 1, {cap}) AS text \
         FROM message m \
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
    // S3: probe whether `message.role` exists (the same PRAGMA shape
    // cost::opencode uses for `data`) — when it does, the page SQL
    // selects it and role resolution prefers the column over the
    // data-JSON fallback; when it doesn't, the column never appears in
    // the SQL. Both schema shapes are real (the cost fixtures lack the
    // column); neither is assumed any more.
    let has_role_column = {
        let probe = "SELECT count(*) AS n FROM pragma_table_info('message') WHERE name = 'role'";
        let out = tokio::process::Command::new("sqlite3")
            .args(opencode_sqlite_args(db_path, probe))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        match tokio::time::timeout(OPENCODE_QUERY_TIMEOUT, out).await {
            Ok(Ok(o)) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout).contains("\"n\":1")
            }
            _ => false,
        }
    };
    let sql = opencode_page_sql(session_id, cursor, limit, has_role_column);
    let fut = tokio::process::Command::new("sqlite3")
        .args(opencode_sqlite_args(db_path, &sql))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        // R4: keep sqlite's own diagnostic — "no such column: …" must
        // not be reported as "the binary is unavailable".
        .stderr(std::process::Stdio::piped())
        // A timed-out scanner must DIE, not keep grinding the 13GB store
        // after we returned QueryTimeout (round-2 N5).
        .kill_on_drop(true)
        .output();
    // Wall-clock cap, same discipline as the cost reader: the busy timeout
    // only covers lock waits, not a long scan (review F5).
    let output = tokio::time::timeout(OPENCODE_QUERY_TIMEOUT, fut)
        .await
        .map_err(|_| TranscriptError::QueryTimeout)?
        .map_err(|_| TranscriptError::Sqlite3Unavailable)?;
    if !output.status.success() {
        // R4: a non-zero sqlite3 exit is a STORE problem (schema drift,
        // lock/corruption, missing JSON1) with a real diagnostic — not
        // a missing binary. Carried via StoreUnreadable so the message
        // reaches the operator.
        // S2: sqlite3's stderr can echo the SQL literal (which embeds
        // the session id) on prepare errors — redact it and bound it
        // before it crosses the module boundary inside the error.
        let stderr_raw = String::from_utf8_lossy(&output.stderr);
        let stderr_bounded: String = stderr_raw.chars().take(2048).collect();
        let stderr = redact(stderr_bounded.trim());
        return Err(TranscriptError::StoreUnreadable {
            path: db_path.to_path_buf(),
            source: std::io::Error::other(format!("sqlite3 exited {}: {}", output.status, stderr)),
        });
    }
    // Fresh-review R6/S5: the JSON parse (up to megabytes of stdout —
    // the SQL cap counts CHARS, so multi-byte text can be ~4x the byte
    // budget per row, S6) AND the redact-heavy assembly both run off
    // the reactor thread. A panic in assembly is reported as the store
    // being unreadable (with its path), not as a schema mismatch.
    let stdout = output.stdout;
    let path_for_err = db_path.to_path_buf();
    tokio::task::spawn_blocking(move || assemble_opencode_page(stdout, limit))
        .await
        .map_err(|_| TranscriptError::StoreUnreadable {
            path: path_for_err,
            source: std::io::Error::other("page assembly panicked"),
        })?
}

/// The sync page-assembly half of the opencode reader (R6/S5): parses
/// the sqlite3 stdout and assembles the redacted page.
fn assemble_opencode_page(
    stdout: Vec<u8>,
    limit: usize,
) -> Result<TranscriptPage, TranscriptError> {
    let rows: Vec<Value> = if stdout.iter().all(u8::is_ascii_whitespace) {
        Vec::new()
    } else {
        serde_json::from_slice(&stdout).map_err(|_| TranscriptError::StoreShape)?
    };
    let mut entries = Vec::new();
    let mut skipped = 0usize;
    let mut text_budget = MAX_PAGE_TEXT_BYTES;
    // Cursor advances over every KEYABLE row we passed (accepted or
    // empty). A malformed row cannot advance it — its id/time failed to
    // extract, which is what made it malformed — so a page tail of
    // unkeyable rows is re-read (and re-counted in `skipped`) next page
    // until a keyable row moves the cursor; an ALL-unkeyable full page
    // raises StoreShape rather than stalling (review F6; honesty per
    // fresh-review R7 — `skipped` can inflate across pages on a store
    // with persistent unkeyable rows).
    let mut last_row_key: Option<(i64, String)> = None;
    let mut stopped_early = false;
    let full_rows = rows.len();
    for row in &rows {
        let (Some(id), Some(t)) = (
            row.get("id").and_then(Value::as_str),
            row.get("time_created").and_then(Value::as_i64),
        ) else {
            skipped += 1;
            continue;
        };
        // S1: the SQL substr cap cuts BEFORE redaction, so a secret
        // severed at the seam could arrive too short for rule 4's
        // length threshold and leak a cleartext prefix. A capped string
        // (exactly cap chars — SQL returns cap = budget+1 chars when the
        // message is longer) is trimmed back to its last whitespace
        // BEFORE redact() so the redactor never sees a severed token; a
        // whitespace-free capped string keeps only the marker.
        let raw = row.get("text").and_then(Value::as_str).unwrap_or_default();
        // Cheap pre-filter: chars <= bytes, so a string under budget+1
        // BYTES cannot be at the char cap; only then count chars.
        let sql_capped =
            raw.len() > MAX_PAGE_TEXT_BYTES && raw.chars().count() == MAX_PAGE_TEXT_BYTES + 1;
        let text = if sql_capped {
            match raw.rfind(|c: char| c.is_ascii_whitespace()) {
                Some(cut) => format!("{}{TRUNCATED_MARKER}", redact(&raw[..cut])),
                None => TRUNCATED_MARKER.trim_start().to_string(),
            }
        } else {
            redact(raw).into_owned()
        };
        if text.is_empty() {
            // A message with no text parts (tool/reasoning-only) — a
            // normal record, not torn data: passed over without an entry
            // and without polluting the honesty counter (review F7/F8).
            last_row_key = Some((t, id.to_string()));
            continue;
        }
        if text.len() > text_budget && !entries.is_empty() {
            // Budget hit: stop BEFORE this row; the cursor resumes at
            // the previous row so this one is re-read next page.
            stopped_early = true;
            break;
        }
        // R2: the page's FIRST entry is not exempt from the budget — an
        // oversized message is truncated with an explicit marker (the
        // SQL already caps what it hands us; this holds the documented
        // per-page contract exactly).
        let text = if text.len() > text_budget {
            truncate_to_budget(&text, text_budget)
        } else {
            text
        };
        // R3/S3: the role column is used only when the probe found it;
        // msg_data's JSON is the fallback for either shape.
        let msg_role = row
            .get("msg_data")
            .and_then(Value::as_str)
            .and_then(|d| serde_json::from_str::<Value>(d).ok())
            .and_then(|d| d.get("role").and_then(Value::as_str).map(str::to_string));
        let role = normalize_role(
            row.get("role")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(msg_role)
                .as_deref(),
        );
        text_budget = text_budget.saturating_sub(text.len());
        last_row_key = Some((t, id.to_string()));
        entries.push(Entry {
            role,
            text,
            ts: u64::try_from(t).ok(),
        });
        if entries.len() >= limit {
            stopped_early = true;
            break;
        }
    }
    if full_rows == limit && entries.is_empty() && skipped == full_rows {
        // Every row of a full page failed extraction: the schema does not
        // match this reader. An error, never a silent empty transcript.
        return Err(TranscriptError::StoreShape);
    }
    // More rows may exist iff the query filled its LIMIT or we stopped
    // early; an underfilled, fully-consumed page is exhaustion.
    let more_possible = stopped_early || full_rows == limit;
    let next_cursor = match (&last_row_key, more_possible) {
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

/// Role strings come from externally-written stores — untrusted input,
/// not a closed enum. Normalising onto a closed set both bounds the field
/// (no unredacted/unbudgeted attacker text rides out on `role` — review
/// F4) and gives the UI a stable vocabulary.
fn normalize_role(raw: Option<&str>) -> String {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("user" | "human") => "user",
        Some("assistant" | "ai") => "assistant",
        Some("system") => "system",
        Some("tool" | "tool_result" | "tool_use" | "function") => "tool",
        Some("developer") => "developer",
        Some("summary") => "summary",
        _ => "unknown",
    }
    .to_string()
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

    // Fresh-review R6: parsing + redaction of up to the full window is
    // CPU-bound work — off the reactor thread. The IO above stays async;
    // the fd is released before the blocking hop (S5).
    drop(file);
    let path_for_err = path.to_path_buf();
    tokio::task::spawn_blocking(move || assemble_jsonl_page(buf, lower, end, limit))
        .await
        .map_err(|_| TranscriptError::StoreUnreadable {
            path: path_for_err,
            source: std::io::Error::other("page assembly panicked"),
        })?
}

/// The sync page-assembly half of the JSONL reader (see the R6 note at
/// the call site).
fn assemble_jsonl_page(
    buf: Vec<u8>,
    lower: u64,
    end: u64,
    limit: usize,
) -> Result<TranscriptPage, TranscriptError> {
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
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            // Genuinely torn line: this is what `skipped` means.
            skipped += 1;
            oldest_consumed_start = Some(*line_start);
            continue;
        };
        let Some(entry) = jsonl_entry(&value) else {
            // Well-formed non-transcript record (summary, tool-only turn):
            // normal content, passed over silently (review F8).
            oldest_consumed_start = Some(*line_start);
            continue;
        };
        let mut entry = entry;
        if entry.text.len() > text_budget && entries.is_empty() {
            // R2: the first entry is truncated to the budget with a
            // marker, never admitted whole.
            entry.text = truncate_to_budget(&entry.text, text_budget);
        }
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
    // The walk MUST make progress: a degenerate range (a single line
    // longer than the scan cap — review F1/F9) would otherwise hand back
    // the cursor it was given and loop the caller forever. Stride the
    // window down instead and say so via `skipped`: the oversized line is
    // unreadable by this reader (documented cap), counted, never a stall.
    let resume = match resume {
        Some(r) if r >= end => {
            skipped += 1;
            Some(lower)
        }
        other => other,
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
/// rollout (`{role?, text?/content?}`) shapes.
///
/// `None` = a well-formed record that carries no transcript text
/// (summaries, tool_use/tool_result-only turns, file snapshots). Those
/// are NORMAL in healthy stores and are passed over without touching the
/// `skipped` counter — `skipped` counts only unparseable lines, so a
/// nonzero value really does mean torn data (review F8).
fn jsonl_entry(value: &Value) -> Option<Entry> {
    let msg = value.get("message").unwrap_or(value);
    let raw_role = msg
        .get("role")
        .and_then(Value::as_str)
        .or_else(|| value.get("type").and_then(Value::as_str))?;
    let role = normalize_role(Some(raw_role));
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
    // R9: fall back when "ts" is present-but-not-numeric too (claude
    // records carry a string "timestamp"; codex may carry numeric "ts").
    let ts = value
        .get("ts")
        .and_then(Value::as_u64)
        .or_else(|| value.get("timestamp").and_then(Value::as_u64));
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

    /// The REAL claude transcript record shape: `timestamp` is an
    /// ISO-8601 STRING (so `Entry.ts` is `None` — the deliberate slice-1
    /// deviation), and ordering must therefore be asserted on entry TEXT,
    /// which is what a real caller sees.
    fn claude_line(role: &str, text: &str, seq: u64) -> String {
        serde_json::json!({
            "type": role,
            "message": {"role": role, "content": [{"type": "text", "text": text}]},
            "timestamp": format!("2026-08-18T12:00:{:02}.000Z", seq % 60),
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
        let texts: Vec<String> = all.iter().map(|e| e.text.clone()).collect();
        let expected: Vec<String> = (0..120)
            .rev()
            .map(|i| format!("message number {i}"))
            .collect();
        assert_eq!(texts, expected, "newest-first, no duplicates");
        assert!(
            all.iter().all(|e| e.ts.is_none()),
            "ISO timestamps yield ts: None in slice 1"
        );
        assert!(
            all.iter().all(|e| e.role == "assistant"),
            "roles normalized"
        );
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
        assert!(args.contains(&std::ffi::OsString::from("-json")));
        assert!(
            args.iter()
                .any(|a| a.to_string_lossy().starts_with(".timeout")),
            "busy timeout present"
        );
        // F11: the db path rides as an OsString, never through lossy Display.
        assert_eq!(args[4], Path::new("/tmp/x.db").as_os_str());
    }

    /// SQL quoting: a session id carrying a single quote cannot break out
    /// of the literal.
    #[test]
    fn opencode_sql_escapes_quotes_and_bounds_the_query() {
        let sql = opencode_page_sql("ses'--x", Some((42, "id'y")), 7, false);
        assert!(sql.contains("'ses''--x'"), "session id escaped: {sql}");
        assert!(sql.contains("'id''y'"), "cursor id escaped: {sql}");
        assert!(sql.contains("LIMIT 7"), "bounded in SQL: {sql}");
        assert!(sql.contains("ORDER BY m.time_created DESC, m.id DESC"));
        // R2: the per-message text cap is IN the SQL (budget + 1 so the
        // Rust side detects and marks the truncation).
        assert!(
            sql.contains(&format!(", 1, {})", MAX_PAGE_TEXT_BYTES + 1)),
            "substr text cap missing: {sql}"
        );
        assert!(sql.contains("substr("), "{sql}");
        // R3: no unprobed m.role in the SELECT.
        assert!(!sql.contains("m.role"), "{sql}");
    }

    fn have_sqlite3() -> bool {
        std::process::Command::new("sqlite3")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
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
        // R3: the fixture schema deliberately has NO `role` column —
        // matching the cost reader's fixtures — proving the reader takes
        // role from msg_data's JSON alone (an unprobed `m.role` SELECT
        // would hard-fail every page against such a store).
        let seed = r#"
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, type TEXT, data TEXT);
            INSERT INTO message VALUES ('m1','ses1',100,'{"role":"user"}');
            INSERT INTO part VALUES ('p1','m1','text','{"text":"first question"}');
            INSERT INTO message VALUES ('m2','ses1',200,'{"role":"assistant"}');
            INSERT INTO part VALUES ('p2a','m2','text','{"text":"part A of the answer"}');
            INSERT INTO part VALUES ('p2b','m2','tool','{"summary":"ran a tool"}');
            INSERT INTO part VALUES ('p2c','m2','text','{"text":"token ghp_abcdefghijklmnopqrstuvwxyz0123456789 leaked"}');
            INSERT INTO message VALUES ('m2t','ses1',250,'{"role":"assistant"}');
            INSERT INTO part VALUES ('p2t','m2t','reasoning','{"summary":"thinking only"}');
            INSERT INTO part VALUES ('ptorn','m2','text','{"text": torn-not-json');
            INSERT INTO message VALUES ('m3','ses2',300,'{"role":"user"}');
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
        // Page size 1: newest first, then the cursor walks older. The
        // newest message (m2t) is reasoning-only: it is passed over WITHOUT
        // an entry and WITHOUT a skipped count — the page is empty but its
        // cursor advances (no stall).
        let p0 = read_page(&store, None, 1).await.expect("page 0");
        assert!(p0.entries.is_empty(), "tool-only message yields no entry");
        assert_eq!(p0.skipped, 0, "tool-only message is not torn data");
        let p0_cursor = p0.next_cursor.expect("cursor advances past it");

        let p1 = read_page(&store, Some(&p0_cursor), 1)
            .await
            .expect("page 1");
        assert_eq!(p1.entries.len(), 1);
        assert_eq!(p1.entries[0].role, "assistant");
        // F2: ALL text parts of the multi-part message arrive, in p.id
        // order, as ONE entry (same one-entry-per-message shape as the
        // JSONL readers) — and redacted.
        let text = &p1.entries[0].text;
        // N1: the torn part row (ptorn) must degrade to nothing — never
        // abort the statement and brick every page of the session.
        assert!(
            text.starts_with("part A of the answer"),
            "part order: {text}"
        );
        assert!(
            !text.contains("torn-not-json"),
            "torn part excluded: {text}"
        );
        assert!(text.contains('\n'), "parts joined with newline: {text}");
        assert!(!text.contains("ghp_abcdefgh"), "redacted: {text}");
        assert!(
            !text.contains("ran a tool"),
            "non-text parts excluded: {text}"
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
        assert!(
            read_page(&store, p2.next_cursor.as_ref(), 1)
                .await
                .expect("page 3")
                .entries
                .is_empty(),
            "walk exhausts cleanly"
        );
    }

    /// F1/F9 regression: a line larger than the scan cap must not stall
    /// the walk — it is passed over (counted in `skipped`) and every
    /// OLDER entry stays reachable.
    #[tokio::test]
    async fn jsonl_walk_strides_past_an_oversized_line() {
        let giant = claude_line("assistant", &"z".repeat(600 * 1024), 5);
        let mut lines: Vec<String> = (0..5)
            .map(|i| claude_line("user", &format!("old {i}"), i))
            .collect();
        lines.push(giant);
        for i in 0..3 {
            lines.push(claude_line("user", &format!("new {i}"), 10 + i));
        }
        let f = write_jsonl(&lines);
        let store = StoreRef::Claude {
            jsonl_path: f.path().to_path_buf(),
        };
        let (all, _, skipped) = walk(&store, 50).await;
        let texts: Vec<&str> = all.iter().map(|e| e.text.as_str()).collect();
        assert!(
            texts.contains(&"new 2") && texts.contains(&"old 0"),
            "entries on BOTH sides of the oversized line are reachable: {texts:?}"
        );
        assert_eq!(all.len(), 8, "only the oversized line is lost");
        assert!(skipped >= 1, "the loss is counted, never silent");
    }

    /// F8: well-formed non-transcript records (summaries, tool-only
    /// turns) are passed over WITHOUT polluting the torn-data counter.
    #[tokio::test]
    async fn jsonl_valid_non_message_records_are_not_counted_skipped() {
        let lines = vec![
            serde_json::json!({"type": "summary", "summary": "compacted"}).to_string(),
            claude_line("user", "real content", 1),
            serde_json::json!({"type": "assistant", "message": {"role": "assistant",
                "content": [{"type": "tool_use", "name": "Bash"}]}})
            .to_string(),
        ];
        let f = write_jsonl(&lines);
        let store = StoreRef::Claude {
            jsonl_path: f.path().to_path_buf(),
        };
        let (all, _, skipped) = walk(&store, 50).await;
        assert_eq!(all.len(), 1);
        assert_eq!(skipped, 0, "healthy records never read as torn data");
    }

    /// F4: role is a closed vocabulary — attacker-shaped role strings
    /// cannot ride out of the module unredacted/unbounded.
    #[tokio::test]
    async fn roles_are_normalized_to_a_closed_set() {
        let hostile = serde_json::json!({
            "type": "x", "message": {"role": "sk-ant-api03-".to_string() + &"A".repeat(4096),
            "content": [{"type": "text", "text": "hello"}]}})
        .to_string();
        let f = write_jsonl(&[hostile]);
        let store = StoreRef::Claude {
            jsonl_path: f.path().to_path_buf(),
        };
        let page = read_page(&store, None, 10).await.expect("page");
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].role, "unknown", "unknown roles collapse");
    }

    /// Fresh-review R2: a page's FIRST entry is NOT exempt from the text
    /// budget — an oversized line is truncated to the cap with the
    /// explicit marker, and the walk still advances.
    #[tokio::test]
    async fn first_entry_is_truncated_to_the_budget_not_exempt() {
        let big = "x".repeat(MAX_PAGE_TEXT_BYTES + 50_000);
        let line = serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [{"type": "text", "text": big}]}
        })
        .to_string();
        let small = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "text", "text": "before"}]}
        })
        .to_string();
        let f = write_jsonl(&[small, line]);
        let store = StoreRef::Claude {
            jsonl_path: f.path().to_path_buf(),
        };
        let page = read_page(&store, None, 10).await.expect("page");
        assert_eq!(page.entries.len(), 1, "the oversized entry fills the page");
        let text = &page.entries[0].text;
        assert!(
            text.ends_with(TRUNCATED_MARKER),
            "truncation must be marked"
        );
        assert!(
            text.len() <= MAX_PAGE_TEXT_BYTES + TRUNCATED_MARKER.len(),
            "budget held: {}",
            text.len()
        );
        assert!(
            page.next_cursor.is_some(),
            "the walk continues past the oversized entry"
        );
    }

    /// Fresh-review R2 (opencode): one giant message is capped in SQL
    /// and truncated with the marker — never returned whole.
    #[tokio::test]
    async fn opencode_giant_message_is_capped_not_unbounded() {
        if !have_sqlite3() {
            eprintln!("sqlite3 not on PATH; skipping");
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("opencode.db");
        let big = "y".repeat(MAX_PAGE_TEXT_BYTES + 100_000);
        let seed = format!(
            r#"
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, type TEXT, data TEXT);
            INSERT INTO message VALUES ('m1','ses1',100,'{{"role":"assistant"}}');
            INSERT INTO part VALUES ('p1','m1','text','{{"text":"{big}"}}');
        "#
        );
        let script = dir.path().join("seed.sql");
        std::fs::write(&script, &seed).expect("write seed");
        let status = std::process::Command::new("sqlite3")
            .arg(&db)
            .arg(format!(".read {}", script.display()))
            .status()
            .expect("seed");
        assert!(status.success());

        let store = StoreRef::Opencode {
            db_path: db,
            session_id: "ses1".to_string(),
        };
        let page = read_page(&store, None, 10).await.expect("page");
        assert_eq!(page.entries.len(), 1);
        let text = &page.entries[0].text;
        assert!(
            text.len() <= MAX_PAGE_TEXT_BYTES + TRUNCATED_MARKER.len(),
            "opencode entry must be capped, got {}",
            text.len()
        );
        // S1: a WHITESPACE-FREE capped blob keeps only the marker — the
        // redactor must never see (or emit) a severed token.
        assert_eq!(
            text.as_str(),
            TRUNCATED_MARKER.trim_start(),
            "whitespace-free capped text reduces to the marker alone"
        );
    }

    /// Fresh-review S1: the SQL cap cuts BEFORE redaction — a rule-4
    /// secret severed at the seam must NOT leak a cleartext prefix; the
    /// capped text is trimmed back to whitespace before the redactor
    /// runs, so the severed token is dropped entirely.
    #[tokio::test]
    async fn sql_capped_secret_at_the_seam_does_not_leak_a_prefix() {
        if !have_sqlite3() {
            eprintln!("sqlite3 not on PATH; skipping");
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("opencode.db");
        let secret = "Ab1".repeat(14); // rule-4 shape, 42 alnum chars (built, not a literal, for the secret scanner)
        // ASCII filler of space-separated words, sized so the secret
        // STRADDLES the substr cap (cap = MAX+1 chars == bytes here).
        let filler_len = MAX_PAGE_TEXT_BYTES + 1 - 22; // cut lands 22 chars into the secret
        let word = "wordy ";
        let mut body = word.repeat(filler_len / word.len() + 1);
        body.truncate(filler_len);
        body.push_str(&secret);
        body.push_str(" trailing tail beyond the cap");
        let seed = format!(
            r#"
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, type TEXT, data TEXT);
            INSERT INTO message VALUES ('m1','ses1',100,'{{"role":"assistant"}}');
            INSERT INTO part VALUES ('p1','m1','text','{{"text":"{body}"}}');
        "#
        );
        let script = dir.path().join("seed.sql");
        std::fs::write(&script, &seed).expect("write seed");
        let status = std::process::Command::new("sqlite3")
            .arg(&db)
            .arg(format!(".read {}", script.display()))
            .status()
            .expect("seed");
        assert!(status.success());

        let store = StoreRef::Opencode {
            db_path: db,
            session_id: "ses1".to_string(),
        };
        let page = read_page(&store, None, 10).await.expect("page");
        assert_eq!(page.entries.len(), 1);
        let text = &page.entries[0].text;
        for n in (8..=22).rev() {
            assert!(
                !text.contains(&secret[..n]),
                "a {n}-char cleartext prefix of the severed secret leaked"
            );
        }
        assert!(text.ends_with(TRUNCATED_MARKER), "truncation marked");
        assert!(text.starts_with("wordy "), "trimmed content survives");
    }

    /// Fresh-review R5: against an INDEXED store (the shape a sane live
    /// store has) the page query's plan is a SEARCH, not a full scan of
    /// `message` — the sargability the AC3 claim depends on. (Index
    /// PRESENCE in the live store cannot be pinned from here; the module
    /// doc says so.)
    #[tokio::test]
    async fn opencode_page_query_plan_uses_indexes_when_present() {
        if !have_sqlite3() {
            eprintln!("sqlite3 not on PATH; skipping");
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("opencode.db");
        let seed = r#"
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, type TEXT, data TEXT);
            CREATE INDEX ix_m ON message(session_id, time_created);
            CREATE INDEX ix_p ON part(message_id);
            INSERT INTO message VALUES ('m1','ses1',100,'{"role":"user"}');
            INSERT INTO part VALUES ('p1','m1','text','{"text":"hello"}');
        "#;
        let status = std::process::Command::new("sqlite3")
            .arg(&db)
            .arg(seed)
            .status()
            .expect("seed");
        assert!(status.success());

        let sql = format!(
            "EXPLAIN QUERY PLAN {}",
            opencode_page_sql("ses1", Some((100, "m1")), 5, false)
        );
        let out = std::process::Command::new("sqlite3")
            .arg(&db)
            .arg(&sql)
            .output()
            .expect("eqp");
        let plan = String::from_utf8_lossy(&out.stdout);
        // S4: EQP names the ALIAS `m`, and "USING INDEX" appears even
        // in the worst plan (part's autoindex used FOR the scan) — only
        // SEARCH-vs-SCAN on the alias discriminates (verified: fails on
        // an unindexed store, passes on an indexed one).
        assert!(
            plan.contains("SEARCH m USING INDEX"),
            "message must be SEARCHed, not scanned: {plan}"
        );
        assert!(
            plan.contains("SEARCH p USING INDEX"),
            "part must be SEARCHed per message: {plan}"
        );
    }

    /// #63: the opaque wire cursor round-trips both variants, and every
    /// malformed shape is a typed BadCursor — never a panic (including
    /// multi-byte UTF-8 in the hex segment, which byte-slices).
    #[test]
    fn cursor_wire_roundtrip_and_bad_inputs() {
        let oc = Cursor::Opencode {
            time_created: 1723972000123,
            id: "msg_01'weird.id".to_string(),
        };
        assert_eq!(Cursor::decode(&oc.encode()).expect("oc roundtrip"), oc);
        let b = Cursor::Bytes { offset: 987654 };
        assert_eq!(Cursor::decode(&b.encode()).expect("bytes roundtrip"), b);

        for bad in [
            "",
            "x.1.aa",
            "oc.",
            "oc.12",
            "oc.12.abc",
            "oc.nan.abcd",
            "oc.12.zz",
            "oc.12.é1",
            "b.",
            "b.-1",
            "b.nan",
            "oc.12.61ff",
        ] {
            match Cursor::decode(bad) {
                Err(TranscriptError::BadCursor) => {}
                other => panic!("{bad:?} must be BadCursor, got {other:?}"),
            }
        }
    }
}
