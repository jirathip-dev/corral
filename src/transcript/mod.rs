//! #62 (D35 slice 1): transcript read-path core — per-store paged readers.
//!
//! Given an explicit store reference, return one page of transcript
//! entries, NEWEST-FIRST, with every entry redacted (D-083 rules,
//! [`crate::core::redact`]) BEFORE it leaves this module — no unredacted
//! text crosses the boundary. No HTTP surface, no UI, no agent→session
//! discovery here (those are #63/#64).
//!
//! Store disciplines:
//! - **opencode** (`opencode.db`, 13GB+ in steady state): a
//!   sqlite3-CLI pattern — `-readonly`, busy
//!   timeout, JSON output, every query bounded in SQL by session id, a
//!   `(time_created, id)` cursor, LIMIT, and a per-message `substr` text
//!   cap (fresh-review R2). Never a write, and no sqlite crate (the
//!   system `sqlite3` binary is the documented trade; its absence is a
//!   typed error, not a panic). Honesty (fresh-review R3/R5): the schema
//!   facts used here (`message.{id,session_id,time_created,data}`,
//!   `part.{id,message_id,type,data}`) go beyond the role-column probe;
//!   `message.role` is PROBED once per store
//!   (memoized, L4) and selected only when present, with the `data` JSON
//!   as the role fallback for either schema shape (S3). Bounded row
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

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

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

/// Deterministic 64-bit FNV-1a over a store's identity (kind + session
/// id / path), stamped into every wire cursor so a cursor can only ever
/// resume against the exact store it was issued for (review F5: a rebind
/// between pages — a new session becoming newest, or a co-resident
/// agent's file — must be a typed `BadCursor`, never a silent
/// continuation at an arbitrary byte offset of a different file).
///
/// Honest residual (fresh review F6): "different file" means a different
/// PATH. A file REWRITTEN in place at the same path keeps its
/// fingerprint, so a byte cursor into it is validated only by
/// `offset <= len` and can land mid-content. Append-only session stores
/// make that unlikely; closing it would need a size/mtime component,
/// which reintroduces exactly the filesystem dependence R7 rejects — a
/// recorded trade-off, not an oversight.
pub fn store_fingerprint(store: &StoreRef) -> u64 {
    fn fnv64(seed: u64, bytes: &[u8]) -> u64 {
        let mut hash = seed;
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    // RAW path spelling, deliberately un-canonicalized (review R7): the
    // binder derives paths deterministically, so raw is stable across
    // pages — while `fs::canonicalize` is a blocking syscall on the
    // async path AND makes the fingerprint depend on live filesystem
    // state (a canonicalize failure between pages would invalidate a
    // logically-valid cursor).
    match store {
        // db_path is chained too (review C2): the same session id in a
        // DIFFERENT database (a $CORRAL_OPENCODE_DB switch between
        // pages) must not accept the old cursor.
        StoreRef::Opencode {
            db_path,
            session_id,
        } => fnv64(
            fnv64(
                fnv64(FNV_OFFSET, b"oc:"),
                db_path.to_string_lossy().as_bytes(),
            ),
            session_id.as_bytes(),
        ),
        StoreRef::Claude { jsonl_path } => fnv64(
            fnv64(FNV_OFFSET, b"cl:"),
            jsonl_path.to_string_lossy().as_bytes(),
        ),
        StoreRef::Codex { rollout_path } => fnv64(
            fnv64(FNV_OFFSET, b"cx:"),
            rollout_path.to_string_lossy().as_bytes(),
        ),
    }
}

impl Cursor {
    /// Opaque wire form for HTTP clients (#63):
    /// `oc.<time>.<hex-id>.<fingerprint>` / `b.<offset>.<fingerprint>`.
    /// The opencode id is hex-encoded so arbitrary id bytes survive the
    /// dot-delimited framing; the trailing segment fingerprints the store
    /// the cursor was issued against (review F5). Clients must treat the
    /// whole string as opaque.
    pub fn encode_for(&self, store: &StoreRef) -> String {
        let fp = store_fingerprint(store);
        match self {
            Cursor::Opencode { time_created, id } => {
                let hex: String = id.bytes().map(|b| format!("{b:02x}")).collect();
                format!("oc.{time_created}.{hex}.{fp:016x}")
            }
            Cursor::Bytes { offset } => format!("b.{offset}.{fp:016x}"),
        }
    }

    /// Structural half of [`Cursor::decode_for`]: framing, prefixes,
    /// hex — everything that needs NO store. Returns the cursor and its
    /// CLAIMED fingerprint, unverified. Fresh review F7: the handler
    /// runs [`Cursor::validate_wire`] before the (expensive) bind so a
    /// malformed cursor is refused without any store IO; only the
    /// fingerprint comparison has to wait for the bound store.
    fn parse_wire(wire: &str) -> Result<(Self, u64), TranscriptError> {
        let (base, fp_hex) = wire.rsplit_once('.').ok_or(TranscriptError::BadCursor)?;
        if fp_hex.len() != 16 {
            return Err(TranscriptError::BadCursor);
        }
        let fp = u64::from_str_radix(fp_hex, 16).map_err(|_| TranscriptError::BadCursor)?;
        Self::parse_base(base).map(|cursor| (cursor, fp))
    }

    /// Structural validation only — no store, no fingerprint check.
    pub fn validate_wire(wire: &str) -> Result<(), TranscriptError> {
        Self::parse_wire(wire).map(|_| ())
    }

    /// Inverse of [`Cursor::encode_for`], validated against the store the
    /// caller just bound: a fingerprint mismatch (different session file,
    /// different opencode session) and every malformed shape are typed
    /// [`TranscriptError::BadCursor`] — never a panic, never a guess.
    pub fn decode_for(wire: &str, store: &StoreRef) -> Result<Self, TranscriptError> {
        let (cursor, fp) = Self::parse_wire(wire)?;
        if fp != store_fingerprint(store) {
            return Err(TranscriptError::BadCursor);
        }
        Ok(cursor)
    }

    fn parse_base(base: &str) -> Result<Self, TranscriptError> {
        if let Some(rest) = base.strip_prefix("b.") {
            let offset = rest.parse().map_err(|_| TranscriptError::BadCursor)?;
            return Ok(Cursor::Bytes { offset });
        }
        let rest = base.strip_prefix("oc.").ok_or(TranscriptError::BadCursor)?;
        let (time, hex) = rest.split_once('.').ok_or(TranscriptError::BadCursor)?;
        let time_created = time.parse().map_err(|_| TranscriptError::BadCursor)?;
        // Empty hex is malformed, not "id is empty" (review F11 — it
        // would silently read as an exhausted transcript). ASCII-hex is
        // checked up front: slicing below is byte-indexed and a
        // multi-byte char would otherwise panic on a char boundary.
        if hex.is_empty() || hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
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

/// L7: appended to an entry whose text was cut by an embedded NUL byte —
/// SQLite's `length()`/`substr()`/`group_concat` stop at the first NUL,
/// so the store content beyond it never reached Rust. The page SQL
/// carries a pre-substr `has_nul` flag so the cut is detected and marked,
/// never silently shortened.
pub const NUL_TRUNCATED_MARKER: &str = "\n… [truncated: text contained a NUL byte]";

/// Truncate `text` to `budget` bytes on a char boundary and append the
/// marker. Only called when `text.len() > budget`.
fn truncate_to_budget(text: &str, budget: usize) -> String {
    let mut end = budget;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATED_MARKER}", &text[..end])
}

/// S1 seam cut for a SQL-capped string: the largest prefix that never
/// hands the redactor a severed rule-4 alnum run or a severed
/// secret-named value fragment. Backs off from the byte budget to the
/// nearest NON-ALNUM boundary, so every alnum run that survives in the
/// prefix is COMPLETE within it — "can never leak a cleartext prefix"
/// holds for RULE-4 alnum runs, judged at their full (unsevered) length.
/// Rule 5's non-env-shaped branch is a different shape: its value is ANY
/// whitespace-free run, so a cut can land inside a secret-named
/// `name=value` token at any non-alnum char (incl. `/`, `+`, `=`,
/// non-ASCII) and leave a short value fragment below the ≥8-char gate
/// (`ENV_VALUE_MIN_LEN`) in cleartext. The guard below models rule 5's
/// value shape exactly: when the cut severs such a value, the prefix
/// backs off past the whole assignment (origin/main strictness), so no
/// fragment survives. A budget window with no non-alnum char at all is
/// one unbroken alnum run of ~256KiB: kept whole, because such a run is
/// far past rule 4's 24-char threshold — the redactor either redacts it
/// wholesale or it is not secret-shaped. (L1: the old whitespace-only
/// fallback reduced a whitespace-free blob — minified JSON, base64 — to
/// the marker alone, an empty entry.)
fn capped_seam_cut(raw: &str) -> usize {
    let mut cut = MAX_PAGE_TEXT_BYTES;
    while cut > 0 && !raw.is_char_boundary(cut) {
        cut -= 1;
    }
    let budget_boundary = cut;
    while cut > 0
        && raw[..cut]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        cut -= raw[..cut].chars().next_back().unwrap().len_utf8();
    }
    // Deliberate availability-over-strictness trade (round-2 N2): a
    // budget window with no non-alnum char is one unbroken SINGLE-CLASS
    // run — rule 4 declines single-class runs at any length, so keeping
    // the whole run (a "severed" one by construction) cannot leak a
    // rule-4 secret, and dropping it would hand the user an empty entry.
    if cut == 0 {
        return budget_boundary;
    }
    // F1/F1.1 (PR #99): the non-alnum backoff cannot see `name=value`
    // shapes (any non-alnum char can be a value-fragment boundary), so
    // extend the backoff past a severed secret-named assignment when the
    // cut lands inside its value.
    severed_secret_assignment_start(raw, cut).unwrap_or(cut)
}

/// F1/F1.1 (PR #99) rule-5 seam guard: when the non-alnum seam cut
/// lands INSIDE a secret-named `name=value` token (the value continues
/// past the cut), the prefix would carry a short value fragment that
/// rule 5's ≥8-char gate (`ENV_VALUE_MIN_LEN`) leaves in cleartext —
/// even though the full (severed) value would have been redacted. Backs
/// off to the last whitespace before the whole non-whitespace run (or
/// 0), so the assignment is dropped from the page prefix. Models rule
/// 5's value shape exactly — a value is ANY whitespace-free run — and
/// walks the run the way `env_value_at` does, so `=` inside values and
/// URL-query runs resolve to the right assignment. Only fires when the
/// cut is mid-value; a complete value in the prefix is judged at full
/// length by the redactor.
fn severed_secret_assignment_start(raw: &str, cut: usize) -> Option<usize> {
    let bytes = raw.as_bytes();
    // No continuation past the cut: a complete value is judged at full
    // length by the redactor.
    if cut >= bytes.len() || bytes[cut].is_ascii_whitespace() {
        return None;
    }
    // The non-whitespace run containing the cut — rule 5's value shape.
    let mut run_start = cut;
    while run_start > 0 && !bytes[run_start - 1].is_ascii_whitespace() {
        run_start -= 1;
    }
    let mut run_end = cut;
    while run_end < bytes.len() && !bytes[run_end].is_ascii_whitespace() {
        run_end += 1;
    }
    // Walk the run the way `env_value_at` does: every STANDALONE ident
    // run (preceded by a non-ident char, per is_ident_char) immediately
    // followed by `=` closes a candidate name. A secret-ish name whose
    // value span `(eq+1, run_end)` contains the cut means the cut severs
    // that secret value — back off to the last whitespace before the
    // run (or 0).
    let mut i = run_start;
    while i < run_end {
        if seam_name_start(bytes[i]) && (i == run_start || !seam_name_char(bytes[i - 1])) {
            let mut j = i + 1;
            while j < run_end && seam_name_char(bytes[j]) {
                j += 1;
            }
            if j < run_end && bytes[j] == b'=' && seam_name_is_secret(&raw[i..j]) {
                let eq = j;
                if cut > eq + 1 {
                    let mut back = run_start;
                    while back > 0 && !bytes[back - 1].is_ascii_whitespace() {
                        back -= 1;
                    }
                    return Some(back);
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }
    None
}

/// F1.1: the `.env` name START charset (`src/core/redact.rs`'s
/// `is_ident_start`).
fn seam_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

/// F1: the `.env` name charset (`src/core/redact.rs`'s `is_ident_char`).
fn seam_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// F1: rule-5 name predicate mirrored from `src/core/redact.rs`'s
/// private `name_is_secret`/`SECRET_NAME_SEGMENTS` (not `pub` there) —
/// keep in sync: any underscore-separated segment (trailing digits
/// trimmed) is a secret-ish segment.
const SEAM_SECRET_NAME_SEGMENTS: [&str; 8] = [
    "api",
    "auth",
    "credential",
    "key",
    "password",
    "passwd",
    "secret",
    "token",
];

fn seam_name_is_secret(name: &str) -> bool {
    name.to_ascii_lowercase().split('_').any(|segment| {
        let segment = segment.trim_end_matches(|c: char| c.is_ascii_digit());
        SEAM_SECRET_NAME_SEGMENTS.contains(&segment)
    })
}

/// Wall-clock cap on one sqlite3 invocation (the busy timeout only covers
/// lock waits, not a scan).
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
///
/// `memo` is the caller-owned role-column probe cache (round-2 N1:
/// per-`AppState` in production, injected in tests — never a
/// process-global; only the opencode arm uses it).
pub async fn read_page(
    memo: &RoleProbeMemo,
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
            read_opencode_page(memo, db_path, session_id, cur, limit).await
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
    // was SQLITE_MAX_LENGTH, ~1GB). S3: `m.role` is selected only when
    // the (memoized, L4) probe found the column — the msg_data JSON is
    // the role fallback for either schema shape, so an absent column
    // cannot hard-fail a page.
    // S6: substr counts CHARS, so the intermediate can overshoot the
    // byte budget by up to 4x on multi-byte text (and group_concat still
    // materialises the full concatenation inside the sqlite3 child,
    // bounded only by SQLITE_MAX_LENGTH); the final entry is byte-capped
    // in Rust. The +1 makes the cap detectable for S1's seam handling.
    // L7: the `has_nul` flag runs instr() on each part BEFORE substr —
    // substr stops at the first NUL and would hide it — so an embedded
    // NUL is detected and marked in Rust, never silently cut.
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
                 WHERE t IS NOT NULL), 1, {cap}) AS text, \
                (SELECT max(CASE WHEN json_valid(p.data) \
                        THEN instr(json_extract(p.data, '$.text'), char(0)) > 0 \
                        ELSE 0 END) \
                 FROM part p \
                 WHERE p.message_id = m.id AND p.type = 'text') AS has_nul \
         FROM message m \
         WHERE m.session_id = '{sid}' {cursor_clause} \
         ORDER BY m.time_created DESC, m.id DESC LIMIT {limit}"
    )
}

/// L4: the role-column probe (a sqlite3 child process) used to run once
/// per page — ~43% of a paged walk's spawn cost and up to 2x the
/// OPENCODE_QUERY_TIMEOUT ceiling. Memoized per canonical db path: a
/// store's schema cannot change mid-walk, so one probe per store is
/// enough. Bounded to a handful of entries (a live host holds a handful
/// of stores); the oldest entry is evicted past the cap.
///
/// Per-INSTANCE, never a process-global (round-2 N1): the memo is owned
/// by the `AppState` a serve runs under (matching `transcript_limiter`),
/// or constructed by a caller/test with an injected probe — a
/// multi-root daemon must not share probe state, and a cold memo must
/// not serialize every store's first page through one lock.
const ROLE_PROBE_MEMO_CAP: usize = 8;

/// The memoized probe's executable shape: a boxed async closure.
type RoleProbe = Arc<dyn Fn(PathBuf) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

/// The role-column probe memo. [`RoleProbeMemo::new`] accepts an
/// injected probe (tests count it; production uses [`probe_role_column`]
/// via [`Default`]); [`RoleProbeMemo::get`] never holds its lock across
/// the probe itself (N1).
#[derive(Clone)]
pub struct RoleProbeMemo {
    probe: RoleProbe,
    inner: Arc<tokio::sync::Mutex<MemoState>>,
}

#[derive(Default)]
struct MemoState {
    by_path: HashMap<PathBuf, bool>,
    order: VecDeque<PathBuf>,
}

impl RoleProbeMemo {
    pub fn new<P, F>(probe: P) -> Self
    where
        P: Fn(PathBuf) -> F + Send + Sync + 'static,
        F: Future<Output = bool> + Send + 'static,
    {
        Self {
            probe: Arc::new(move |p| Box::pin(probe(p))),
            inner: Arc::new(tokio::sync::Mutex::new(MemoState::default())),
        }
    }

    /// The cached probe result for `db_path`'s schema, probing once on a
    /// miss. The lock NEVER spans the probe (N1): the probe is a sqlite3
    /// child process (up to the OPENCODE_QUERY_TIMEOUT ceiling), so
    /// holding the lock across it would serialize every store's cold
    /// path through this memo — the regression the reviewer flagged.
    /// Concurrent cold probes for the same store may race, but the
    /// cached value is a schema bool — idempotent, and strictly no worse
    /// than origin/main, which probed every page concurrently.
    async fn get(&self, db_path: &Path) -> bool {
        let key = canonical_key(db_path).await;
        if let Some(v) = self.inner.lock().await.by_path.get(&key) {
            return *v;
        }
        let v = (self.probe)(key.clone()).await;
        let mut state = self.inner.lock().await;
        if state.by_path.len() >= ROLE_PROBE_MEMO_CAP
            && let Some(oldest) = state.order.pop_front()
        {
            state.by_path.remove(&oldest);
        }
        state.order.push_back(key.clone());
        state.by_path.insert(key, v);
        v
    }
}

impl Default for RoleProbeMemo {
    fn default() -> Self {
        Self::new(probe_role_column)
    }
}

/// The probe key: the CANONICAL path when it resolves (two spellings of
/// one store share a memo entry), the raw spelling otherwise (the file
/// was existence-checked before the probe, so resolution failure is
/// exotic and still leaves a correct, uncached key).
async fn canonical_key(db_path: &Path) -> PathBuf {
    tokio::fs::canonicalize(db_path)
        .await
        .unwrap_or_else(|_| db_path.to_path_buf())
}

/// L5/S3: probe whether `message.role` exists so the page SQL selects it
/// only when present. This is the TABLE-VALUED FUNCTION form
/// (`pragma_table_info('message')` inside a SELECT) — the result must
/// flow through the same `-json` child as the page query — and it needs
/// SQLite >= 3.16, unlike the PRAGMA STATEMENT form (`PRAGMA
/// table_info(message);`) used by the store probe; that floor is
/// documented in docs/corral/DECISIONS.md. Any failure (missing binary,
/// timeout, non-zero exit, unparseable JSON) falls back to `false`: the
/// reader then uses the data-JSON role — the shape that works against
/// every store.
async fn probe_role_column(db_path: PathBuf) -> bool {
    let probe = "SELECT count(*) AS n FROM pragma_table_info('message') WHERE name = 'role'";
    let out = tokio::process::Command::new("sqlite3")
        .args(opencode_sqlite_args(&db_path, probe))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(OPENCODE_QUERY_TIMEOUT, out).await {
        Ok(Ok(o)) if o.status.success() => {
            serde_json::from_slice::<Value>(&o.stdout)
                .ok()
                .and_then(|v| v.as_array().and_then(|rows| rows.first()).cloned())
                .and_then(|row| row.get("n").and_then(Value::as_u64))
                == Some(1)
        }
        _ => false,
    }
}

async fn read_opencode_page(
    memo: &RoleProbeMemo,
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
    // S3/L4: probe whether `message.role` exists — memoized per store so
    // the probe (a sqlite3 child process, ~43% of a paged walk's spawn
    // cost) runs once instead of once per page. The memo is the
    // caller-owned instance threaded through `read_page` (N1: never a
    // process-global). When the column exists the page SQL selects it
    // and role resolution prefers it over the data-JSON fallback; when
    // it doesn't, the column never appears in the SQL. Both schema
    // shapes are real (the fixture schema lacks the column); neither is
    // assumed any more.
    let has_role_column = memo.get(db_path).await;
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
    // Wall-clock cap: the busy timeout
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
        // message is longer) is trimmed back to a non-alnum boundary by
        // capped_seam_cut BEFORE redact() so the redactor never sees a
        // severed token — and a whitespace-free blob keeps its content,
        // not just the marker (L1). L7: a NUL-cut message is marked
        // explicitly (via the SQL's has_nul flag), never silently
        // shortened.
        let raw = row.get("text").and_then(Value::as_str).unwrap_or_default();
        // Cheap pre-filter: chars <= bytes, so a string under budget+1
        // BYTES cannot be at the char cap; only then count chars.
        let sql_capped =
            raw.len() > MAX_PAGE_TEXT_BYTES && raw.chars().count() == MAX_PAGE_TEXT_BYTES + 1;
        let has_nul = row.get("has_nul").and_then(Value::as_i64).unwrap_or(0) > 0;
        let text = if sql_capped {
            format!("{}{TRUNCATED_MARKER}", redact(&raw[..capped_seam_cut(raw)]))
        } else if has_nul {
            format!("{}{NUL_TRUNCATED_MARKER}", redact(raw))
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

    /// Test-side read: the production seam (`read_page` with an explicit
    /// memo) using a fresh default memo per call — tests never share
    /// probe state, and only the memo tests inject a counted probe.
    async fn read(store: &StoreRef, cursor: Option<&Cursor>, limit: usize) -> TranscriptPage {
        read_page(&RoleProbeMemo::default(), store, cursor, limit)
            .await
            .expect("page")
    }

    async fn walk(store: &StoreRef, limit: usize) -> (Vec<Entry>, usize, usize) {
        let mut all = Vec::new();
        let mut cursor: Option<Cursor> = None;
        let mut pages = 0;
        let mut skipped = 0;
        loop {
            let page = read(store, cursor.as_ref(), limit).await;
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
        let first = read(&store, None, 50).await;
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
            let page = read(&store, None, 10).await;
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
        let page = read(&store, None, 50).await;
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
        // S3: no m.role when the probe said the column is absent; with
        // the probe's OK, the column IS selected.
        assert!(!sql.contains("m.role"), "{sql}");
        let sql_with_role = opencode_page_sql("ses", None, 5, true);
        assert!(
            sql_with_role.contains("m.role AS role"),
            "probe found the column: {sql_with_role}"
        );
        // L7: the NUL-detection flag rides on every page query.
        assert!(sql.contains("has_nul"), "NUL flag missing: {sql}");
    }

    fn have_sqlite3() -> bool {
        std::process::Command::new("sqlite3")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// Fixture-driven opencode paging + redaction, skipped cleanly when the
    /// sqlite3 binary is absent.
    #[tokio::test]
    async fn opencode_fixture_pages_newest_first_and_redacts() {
        if !have_sqlite3() {
            eprintln!("skipping: sqlite3 binary not available");
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("opencode.db");
        // R3: the fixture schema deliberately has NO `role` column —
        // matching the no-role fixture shape — proving the reader takes
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
        let p0 = read(&store, None, 1).await;
        assert!(p0.entries.is_empty(), "tool-only message yields no entry");
        assert_eq!(p0.skipped, 0, "tool-only message is not torn data");
        let p0_cursor = p0.next_cursor.expect("cursor advances past it");

        let p1 = read(&store, Some(&p0_cursor), 1).await;
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

        let p2 = read(&store, p1.next_cursor.as_ref(), 1).await;
        assert_eq!(p2.entries.len(), 1);
        assert_eq!(p2.entries[0].text, "first question");
        assert_eq!(
            p2.entries[0].role, "user",
            "other sessions never leak into the page"
        );
        assert!(
            read(&store, p2.next_cursor.as_ref(), 1)
                .await
                .entries
                .is_empty(),
            "walk exhausts cleanly"
        );
    }

    /// S3: the role-column probe detects BOTH schema shapes and the
    /// precedence is pinned — role comes from the `role` COLUMN when the
    /// column exists (a conflicting data-JSON value proves the column
    /// wins), and from the data JSON when it does not.
    #[tokio::test]
    async fn opencode_role_probe_detects_both_schema_shapes_and_precedence() {
        if !have_sqlite3() {
            eprintln!("sqlite3 not on PATH; skipping");
            return;
        }
        // Shape (a): message WITHOUT a role column, so role comes from the
        // data JSON alone.
        let dir = tempfile::tempdir().expect("temp dir");
        let db_a = dir.path().join("no_role_column.db");
        let seed_a = r#"
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, type TEXT, data TEXT);
            INSERT INTO message VALUES ('m1','ses1',100,'{"role":"assistant"}');
            INSERT INTO part VALUES ('p1','m1','text','{"text":"json role only"}');
        "#;
        let status = std::process::Command::new("sqlite3")
            .arg(&db_a)
            .arg(seed_a)
            .status()
            .expect("seed a");
        assert!(status.success(), "fixture a seeded");
        let store_a = StoreRef::Opencode {
            db_path: db_a,
            session_id: "ses1".to_string(),
        };
        let page_a = read(&store_a, None, 10).await;
        assert_eq!(page_a.entries.len(), 1);
        assert_eq!(
            page_a.entries[0].role, "assistant",
            "no role column: role comes from the data JSON"
        );

        // Shape (b): message WITH a role column whose value CONFLICTS
        // with the data JSON — the column must win.
        let db_b = dir.path().join("with_role_column.db");
        let seed_b = r#"
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, role TEXT, data TEXT);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, type TEXT, data TEXT);
            INSERT INTO message VALUES ('m1','ses1',100,'user','{"role":"assistant"}');
            INSERT INTO part VALUES ('p1','m1','text','{"text":"column wins"}');
        "#;
        let status = std::process::Command::new("sqlite3")
            .arg(&db_b)
            .arg(seed_b)
            .status()
            .expect("seed b");
        assert!(status.success(), "fixture b seeded");
        let store_b = StoreRef::Opencode {
            db_path: db_b,
            session_id: "ses1".to_string(),
        };
        let page_b = read(&store_b, None, 10).await;
        assert_eq!(page_b.entries.len(), 1);
        assert_eq!(
            page_b.entries[0].role, "user",
            "role column present: the column wins over the data JSON"
        );
    }

    /// L7: an embedded NUL byte used to silently truncate the message
    /// (SQLite's substr/group_concat stop at the first NUL) — now the
    /// page SQL's has_nul flag detects it and the cut is marked
    /// explicitly, with the pre-NUL content preserved.
    #[tokio::test]
    async fn opencode_nul_byte_is_marked_not_silently_truncated() {
        if !have_sqlite3() {
            eprintln!("sqlite3 not on PATH; skipping");
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("opencode.db");
        // The \u0000 JSON escape is a genuine NUL byte inside the part
        // text once json_extract decodes it.
        let seed = r#"
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, type TEXT, data TEXT);
            INSERT INTO message VALUES ('m1','ses1',100,'{"role":"assistant"}');
            INSERT INTO part VALUES ('p1','m1','text','{"text":"before the NUL\u0000after it"}');
        "#;
        let status = std::process::Command::new("sqlite3")
            .arg(&db)
            .arg(seed)
            .status()
            .expect("seed");
        assert!(status.success(), "fixture seeded");

        let store = StoreRef::Opencode {
            db_path: db,
            session_id: "ses1".to_string(),
        };
        let page = read(&store, None, 10).await;
        assert_eq!(page.entries.len(), 1);
        let text = &page.entries[0].text;
        assert!(
            text.starts_with("before the NUL"),
            "pre-NUL content preserved: {text}"
        );
        assert!(
            !text.contains("after it"),
            "post-NUL content never reaches Rust (SQLite cut it): {text}"
        );
        assert!(
            text.ends_with(NUL_TRUNCATED_MARKER),
            "NUL cut is marked explicitly, never silent: {text}"
        );
        assert_eq!(page.skipped, 0, "a NUL message is not torn data");
    }

    /// L4: the role-column probe is memoized per store — a paged walk
    /// probes once, not once per page, and the memo stays bounded (the
    /// oldest entry is evicted past the cap, so an evicted store
    /// re-probes). The probe is counted directly via the injected probe
    /// seam (`RoleProbeMemo::new`); [`read_opencode_page_uses_the_memoized_role_probe`]
    /// pins the same seam through the production call path.
    #[tokio::test]
    async fn role_probe_memo_probes_once_per_store_and_stays_bounded() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_probe = calls.clone();
        let memo = RoleProbeMemo::new(move |_p: PathBuf| {
            let c = calls_for_probe.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                true
            }
        });
        let a = Path::new("/stores/a/opencode.db");
        assert!(memo.get(a).await, "first page probes");
        assert!(memo.get(a).await, "second page hits the cache");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "one store probes exactly once across pages"
        );
        let b = Path::new("/stores/b/opencode.db");
        assert!(memo.get(b).await, "a second store probes fresh");
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Bounded growth: filling past the cap evicts the oldest entry,
        // which re-probes on its next access; a still-cached one does
        // not.
        for i in 0..=ROLE_PROBE_MEMO_CAP {
            let p = PathBuf::from(format!("/stores/{i}/opencode.db"));
            assert!(memo.get(&p).await);
        }
        assert_eq!(calls.load(Ordering::SeqCst), ROLE_PROBE_MEMO_CAP + 3);
        let first = PathBuf::from("/stores/0/opencode.db");
        assert!(memo.get(&first).await);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            ROLE_PROBE_MEMO_CAP + 4,
            "evicted entry re-probes"
        );
        assert!(memo.get(&first).await);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            ROLE_PROBE_MEMO_CAP + 4,
            "re-probed entry is cached again"
        );
    }

    /// N3.1: `read_opencode_page` USES the memo through the production
    /// seam — a counted probe wired into `read_page` must run once across
    /// a two-page walk of one store. (Reverting the call site to an
    /// inline per-page probe would make this fail.)
    #[tokio::test]
    async fn read_opencode_page_uses_the_memoized_role_probe() {
        if !have_sqlite3() {
            eprintln!("sqlite3 not on PATH; skipping");
            return;
        }
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("opencode.db");
        let seed = r#"
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, type TEXT, data TEXT);
            INSERT INTO message VALUES ('m1','ses1',100,'{"role":"user"}');
            INSERT INTO part VALUES ('p1','m1','text','{"text":"older"}');
            INSERT INTO message VALUES ('m2','ses1',200,'{"role":"assistant"}');
            INSERT INTO part VALUES ('p2','m2','text','{"text":"newer"}');
        "#;
        let status = std::process::Command::new("sqlite3")
            .arg(&db)
            .arg(seed)
            .status()
            .expect("seed");
        assert!(status.success(), "fixture seeded");

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_probe = calls.clone();
        let memo = RoleProbeMemo::new(move |_p: PathBuf| {
            let c = calls_for_probe.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                // The fixture's `message` has no role column — the
                // probe's VALUE is irrelevant to the count assertion,
                // only that it is consulted through the memo.
                false
            }
        });
        let store = StoreRef::Opencode {
            db_path: db,
            session_id: "ses1".to_string(),
        };
        let p1 = read_page(&memo, &store, None, 1).await.expect("page 1");
        assert_eq!(p1.entries.len(), 1);
        let p2 = read_page(&memo, &store, p1.next_cursor.as_ref(), 1)
            .await
            .expect("page 2");
        assert_eq!(p2.entries.len(), 1);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "two-page walk probes exactly once through the production seam"
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
        let page = read(&store, None, 10).await;
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
        let page = read(&store, None, 10).await;
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
        let page = read(&store, None, 10).await;
        assert_eq!(page.entries.len(), 1);
        let text = &page.entries[0].text;
        assert!(
            text.len() <= MAX_PAGE_TEXT_BYTES + TRUNCATED_MARKER.len(),
            "opencode entry must be capped, got {}",
            text.len()
        );
        // L1: a WHITESPACE-FREE capped blob keeps its content up to the
        // byte budget — an unbroken ~256KiB run is far past rule 4's
        // 24-char threshold, so the redactor judges it at full length and
        // never sees a severed token — with the marker on top.
        assert_eq!(
            text.as_str(),
            format!("{}{TRUNCATED_MARKER}", "y".repeat(MAX_PAGE_TEXT_BYTES)),
            "whitespace-free capped text keeps its content, not just the marker"
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
        let page = read(&store, None, 10).await;
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

    /// N3.2: the same seam guarantee in a WHITESPACE-FREE context — the
    /// actual novelty of `capped_seam_cut`. An unbroken alnum filler
    /// (no boundary of any kind) with a rule-4-shaped secret straddling
    /// the cap must not leak a cleartext prefix: the whole run is
    /// redacted, marker appended, content survives.
    #[tokio::test]
    async fn sql_capped_secret_in_an_unbroken_alnum_run_does_not_leak_a_prefix() {
        if !have_sqlite3() {
            eprintln!("sqlite3 not on PATH; skipping");
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("opencode.db");
        let secret = "Ab1".repeat(14); // rule-4 shape, 42 alnum chars
        // Unbroken lowercase filler sized so the substr cap cuts 22
        // chars INTO the secret — the whole window is one alnum run.
        let filler_len = MAX_PAGE_TEXT_BYTES + 1 - 22;
        let mut body = "a".repeat(filler_len);
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
        let page = read(&store, None, 10).await;
        assert_eq!(page.entries.len(), 1);
        let text = &page.entries[0].text;
        for n in (8..=22).rev() {
            assert!(
                !text.contains(&secret[..n]),
                "a {n}-char cleartext prefix of the severed secret leaked"
            );
        }
        assert!(text.contains("[REDACTED]"), "the run was redacted: {text}");
        assert!(text.ends_with(TRUNCATED_MARKER), "truncation marked");
        assert!(
            text.starts_with("[REDACTED]"),
            "content survives as redacted text"
        );
    }

    /// F1/F1.1 (PR #99): the seam can land on ANY non-alnum boundary
    /// INSIDE a secret-named `name=value` token — rule 5's non-env value
    /// is any whitespace-free run, so a `_`, `/`, or `+` boundary all
    /// stop the S1 backoff and leave a short value fragment below the
    /// ≥8-char gate (`ENV_VALUE_MIN_LEN`). The guard backs off past the
    /// whole assignment, so no fragment survives in the page text
    /// (dropped whole, not shown redacted).
    #[tokio::test]
    async fn sql_capped_secret_named_value_fragment_at_the_seam_does_not_leak() {
        if !have_sqlite3() {
            eprintln!("sqlite3 not on PATH; skipping");
            return;
        }
        // `_`-boundary fragment (F1) plus `/`/`+`-boundary fragments
        // (F1.1 residual class): the cap lands inside each value.
        for (value, fragment) in [
            ("ab_c_def", "ab_c_"),
            ("ab/c_def", "ab/"),
            ("ab+c_def", "ab+"),
        ] {
            assert_no_seam_value_fragment_leaks(value, fragment).await;
        }
    }

    /// One shape of the F1/F1.1 seam leak: `fragment` is the value
    /// prefix left by a cap cut at a non-alnum boundary inside `value`;
    /// the page must contain no cleartext fragment of `value` at all.
    async fn assert_no_seam_value_fragment_leaks(value: &str, fragment: &str) {
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("opencode.db");
        // Whitespace filler sized so the SQL cap (budget+1 chars) ends
        // right after `fragment` (filler + "auth_key=" + fragment ==
        // budget): the non-alnum backoff stops at the fragment's last
        // char.
        let filler_len = MAX_PAGE_TEXT_BYTES - ("auth_key=".len() + fragment.len());
        assert_eq!(filler_len % 2, 0, "filler ends on a `w ` boundary");
        let body = format!(
            "{}auth_key={value} trailing tail beyond the cap",
            "w ".repeat(filler_len / 2)
        );
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
        let page = read(&store, None, 10).await;
        assert_eq!(page.entries.len(), 1);
        let text = &page.entries[0].text;
        // 2..=7: a 1-char prefix (`a`) also occurs inside the truncation
        // marker's own words, so it cannot discriminate; every longer
        // fragment of the value contains `_`, `/`, or `+` — absent from
        // both the `w ` filler and the marker ("ab" is the only
        // exception and it does not occur in either).
        for n in 2..=7 {
            assert!(
                !text.contains(&value[..n]),
                "a {n}-char cleartext fragment of the severed secret value leaked: {text}"
            );
        }
        assert!(
            !text.contains("auth_key="),
            "the severed assignment is dropped whole, not shown redacted"
        );
        assert!(text.ends_with(TRUNCATED_MARKER), "truncation marked");
    }

    /// F1/F1.1 doc-scope pin (cheap, no sqlite): `capped_seam_cut`
    /// extends the backoff past a severed secret-named `name=value`
    /// fragment — any whitespace-free run, `_` or `/` boundaries — while
    /// the rule-4 backoff and the unbroken-run budget trade are
    /// byte-for-byte unchanged.
    #[test]
    fn seam_cut_drops_severed_secret_named_value_fragment() {
        // The rule-5 gate the guard defends: the FULL value is redacted,
        // the severed fragment below the gate is not.
        assert_eq!(
            redact("auth_key=ab_c_def").as_ref(),
            "auth_key=[REDACTED]",
            "full value redacted by rule 5"
        );
        assert_eq!(
            redact("auth_key=ab_c_").as_ref(),
            "auth_key=ab_c_",
            "5-char fragment is below the ≥8-char gate"
        );
        // Severed secret-named assignment: the `_` boundary stops the
        // non-alnum backoff, so only the seam guard can drop the
        // fragment.
        let filler_len = MAX_PAGE_TEXT_BYTES - ("auth_key=".len() + "ab_c_".len());
        let mut raw = "w ".repeat(filler_len / 2);
        raw.push_str("auth_key=ab_c_def trailing");
        let cut = capped_seam_cut(&raw);
        let prefix = &raw[..cut];
        assert!(
            !prefix.contains("auth_key"),
            "the severed assignment is dropped from the prefix"
        );
        assert!(
            prefix.ends_with(' '),
            "backed off to the whitespace before the assignment"
        );
        assert!(
            !redact(prefix).contains("ab_c"),
            "no value fragment survives in the redacted prefix"
        );
        // F1.1: rule 5's value is ANY whitespace-free run — a `/`-in-
        // value cut after `/` must drop the assignment the same way.
        assert_eq!(
            redact("auth_key=ab/c_def").as_ref(),
            "auth_key=[REDACTED]",
            "full `/`-containing value redacted by rule 5"
        );
        let slash_filler_len = MAX_PAGE_TEXT_BYTES - ("auth_key=".len() + "ab/".len());
        let mut raw_slash = "w ".repeat(slash_filler_len / 2);
        raw_slash.push_str("auth_key=ab/c_def trailing");
        let cut_slash = capped_seam_cut(&raw_slash);
        let prefix_slash = &raw_slash[..cut_slash];
        assert!(
            !prefix_slash.contains("auth_key"),
            "a `/`-severed secret value is dropped from the prefix"
        );
        assert!(
            !redact(prefix_slash).contains("ab/"),
            "no `/`-value fragment survives in the redacted prefix"
        );
        // The run walk resolves `=`-in-values to the OUTER assignment
        // (the inner `ab` before `=` is not secret and does not confuse
        // it), and never matches non-secret names.
        let eq_run = "w auth_key=ab=cd/ef";
        let cut_in_eq_value = eq_run.find("cd/ef").unwrap() + 1;
        assert_eq!(
            severed_secret_assignment_start(eq_run, cut_in_eq_value),
            Some(2),
            "a cut inside a `=`-containing secret value backs off past the run"
        );
        let plain_run = "w a=1&b=2/3";
        let cut_in_plain_value = plain_run.find("2/3").unwrap() + 1;
        assert_eq!(
            severed_secret_assignment_start(plain_run, cut_in_plain_value),
            None,
            "non-secret names never trigger the guard"
        );
        // Rule-4 seam unchanged: a severed mixed-case run still backs
        // off to the preceding whitespace boundary.
        let mut raw4 = "w ".repeat(filler_len / 2);
        raw4.push_str(&"Ab1cdE2".repeat(6)); // 48-char rule-4 run straddling the cap
        raw4.push_str(" tail");
        let cut4 = capped_seam_cut(&raw4);
        let prefix4 = &raw4[..cut4];
        assert!(prefix4.ends_with("w "), "rule-4 seam stops at whitespace");
        assert!(
            !redact(prefix4).contains("Ab1"),
            "rule-4 secret dropped at the seam"
        );
        // Unbroken-run availability trade unchanged: an all-alnum window
        // keeps its content up to the byte budget.
        let blob = "x".repeat(MAX_PAGE_TEXT_BYTES + 10);
        assert_eq!(
            capped_seam_cut(&blob),
            MAX_PAGE_TEXT_BYTES,
            "unbroken alnum run keeps the budget boundary"
        );
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

    /// #63: the opaque wire cursor round-trips both variants against the
    /// store it was issued for; a fingerprint mismatch (rebound store —
    /// review F5) and every malformed shape (incl. empty hex — review
    /// F11 — and multi-byte UTF-8 in the byte-sliced hex segment) are
    /// typed BadCursor, never a panic and never a silent continuation.
    #[test]
    fn cursor_wire_roundtrip_fingerprint_and_bad_inputs() {
        let oc_store = StoreRef::Opencode {
            db_path: PathBuf::from("/tmp/db"),
            session_id: "ses_a".to_string(),
        };
        let jsonl_store = StoreRef::Claude {
            jsonl_path: PathBuf::from("/p/s1.jsonl"),
        };
        let oc = Cursor::Opencode {
            time_created: 1723972000123,
            id: "msg_01'weird.id".to_string(),
        };
        assert_eq!(
            Cursor::decode_for(&oc.encode_for(&oc_store), &oc_store).expect("oc roundtrip"),
            oc
        );
        let b = Cursor::Bytes { offset: 987654 };
        assert_eq!(
            Cursor::decode_for(&b.encode_for(&jsonl_store), &jsonl_store).expect("b roundtrip"),
            b
        );

        // F5: the same wire cursor against a DIFFERENT store (another
        // session's file; another opencode session) is BadCursor.
        let other_jsonl = StoreRef::Claude {
            jsonl_path: PathBuf::from("/p/s2.jsonl"),
        };
        assert!(matches!(
            Cursor::decode_for(&b.encode_for(&jsonl_store), &other_jsonl),
            Err(TranscriptError::BadCursor)
        ));
        let other_session = StoreRef::Opencode {
            db_path: PathBuf::from("/tmp/db"),
            session_id: "ses_b".to_string(),
        };
        assert!(matches!(
            Cursor::decode_for(&oc.encode_for(&oc_store), &other_session),
            Err(TranscriptError::BadCursor)
        ));

        let fp = format!("{:016x}", store_fingerprint(&oc_store));
        for bad in [
            "".to_string(),
            format!("x.1.aa.{fp}"),
            format!("oc..{fp}"),
            format!("oc.12.{fp}"),
            format!("oc.12..{fp}"),
            format!("oc.12.abc.{fp}"),
            format!("oc.nan.abcd.{fp}"),
            format!("oc.12.zz.{fp}"),
            format!("oc.12.\u{e9}1.{fp}"),
            format!("oc.12.61ff.{fp}"),
            format!("b..{fp}"),
            format!("b.-1.{fp}"),
            format!("b.nan.{fp}"),
            "b.1.zzzz".to_string(),
            "b.1.abcd".to_string(),
            "b.1".to_string(),
        ] {
            match Cursor::decode_for(&bad, &oc_store) {
                Err(TranscriptError::BadCursor) => {}
                other => panic!("{bad:?} must be BadCursor, got {other:?}"),
            }
        }
    }

    /// C4: the fingerprint hashes the RAW spelling (R7) — a symlinked
    /// alias of the same real file must hash DISTINCTLY (a
    /// canonicalizing implementation would collapse them and this would
    /// fail), and the value must not depend on live filesystem state.
    #[test]
    fn store_fingerprint_uses_raw_spelling_not_canonical() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real_dir = dir.path().join("real");
        std::fs::create_dir(&real_dir).expect("mkdir");
        let real = real_dir.join("s.jsonl");
        std::fs::write(&real, "{}\n").expect("write");
        let alias_dir = dir.path().join("alias");
        std::os::unix::fs::symlink(&real_dir, &alias_dir).expect("symlink");
        let alias = alias_dir.join("s.jsonl");
        assert!(alias.is_file(), "alias resolves to the same file");

        let fp_real = store_fingerprint(&StoreRef::Claude {
            jsonl_path: real.clone(),
        });
        let fp_alias = store_fingerprint(&StoreRef::Claude {
            jsonl_path: alias.clone(),
        });
        assert_ne!(fp_real, fp_alias, "raw spellings hash distinctly");

        // No filesystem dependence: identical after the file vanishes.
        std::fs::remove_file(&real).expect("rm");
        assert_eq!(
            fp_alias,
            store_fingerprint(&StoreRef::Claude { jsonl_path: alias })
        );

        // C2: same opencode session id, different database → different
        // fingerprint (a db switch between pages invalidates cursors).
        let a = StoreRef::Opencode {
            db_path: PathBuf::from("/a/opencode.db"),
            session_id: "ses_x".to_string(),
        };
        let b = StoreRef::Opencode {
            db_path: PathBuf::from("/b/opencode.db"),
            session_id: "ses_x".to_string(),
        };
        assert_ne!(store_fingerprint(&a), store_fingerprint(&b));
    }
}
