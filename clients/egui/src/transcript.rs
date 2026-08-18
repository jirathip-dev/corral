//! #64 (D35 slice 3): client-side transcript pane state — pure data +
//! parsing for the lazy-paged `GET /transcript` viewer.
//!
//! The daemon serves NEWEST-FIRST pages; the pane appends each fetched
//! page, so `entries[0]` is the newest message and "load older" extends
//! the tail. Bounded by construction: at most [`MAX_PAGES`] pages are
//! ever held (transcripts can be 100MB+ — the pane never holds the whole
//! thing); at the cap the pane stops offering older pages and says so,
//! rather than silently dropping what the user already scrolled
//! (drop-oldest would evict the NEWEST messages here, which is the wrong
//! end to lose).
//!
//! Everything in this module is pure (JSON in, state out) so the paging
//! rules are unit-tested without a daemon; fetching lives in
//! `protocol.rs`, wiring in `app.rs`, rendering in `ui/board.rs`.

use serde::Deserialize;

/// Page cap: 20 pages × ≤50 entries = ≤1000 entries held per agent.
pub const MAX_PAGES: usize = 20;
/// Entries requested per page (the daemon clamps to its own cap of 50).
pub const PAGE_LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TranscriptEntry {
    pub role: String,
    pub text: String,
    /// Epoch millis when the store carried one; `None` renders blank.
    pub ts: Option<u64>,
}

/// One daemon page, exactly the wire shape of a 200 response.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TranscriptPage {
    pub agent: String,
    pub store: String,
    /// The bound session's label (e.g. `claude:2d5e….jsonl`).
    pub session: String,
    /// `"session_id"` (exact) or `"worktree"` (best-effort heuristic).
    pub bind: String,
    #[serde(default)]
    pub stores_unavailable: Vec<String>,
    pub entries: Vec<TranscriptEntry>,
    /// Opaque; `None` = the store is exhausted.
    pub next_cursor: Option<String>,
    #[serde(default)]
    pub skipped: usize,
}

/// A typed `{kind, message}` failure from the endpoint (any non-200).
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptFailure {
    pub kind: String,
    pub message: String,
    /// Populated for `ambiguous_session` (409): the candidate list the
    /// daemon returns instead of guessing.
    pub candidates: Vec<String>,
}

impl TranscriptFailure {
    /// Parse a non-200 body. Falls back to a transport-shaped failure
    /// when the body is not the endpoint's JSON contract.
    pub fn from_response(status: u16, body: &serde_json::Value) -> Self {
        let kind = body
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("transport")
            .to_string();
        let message = body
            .get("message")
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}"));
        let candidates = body
            .get("candidates")
            .and_then(|c| c.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|c| c.get("label").and_then(|l| l.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            kind,
            message,
            candidates,
        }
    }

    /// A stale fingerprinted cursor after the daemon rebound to a newer
    /// session — the documented client behavior is drop-and-reload from
    /// the newest page, not retry.
    pub fn is_stale_cursor(&self) -> bool {
        self.kind == "bad_cursor"
    }
}

/// Per-agent pane state: the fetched pages (flattened, newest first),
/// the resume cursor, and the last outcome.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptPane {
    pub entries: Vec<TranscriptEntry>,
    pub next_cursor: Option<String>,
    pub pages: usize,
    pub session: String,
    pub store: String,
    pub bind: String,
    pub stores_unavailable: Vec<String>,
    /// Sum of the daemon's per-page torn-data counters.
    pub skipped: usize,
    /// A fetch is in flight (render a spinner, disable "load older").
    pub loading: bool,
    pub error: Option<TranscriptFailure>,
    /// One automatic reload has already been spent on a stale cursor —
    /// a second stale cursor surfaces as an error instead of looping.
    pub auto_reloaded: bool,
}

impl TranscriptPane {
    /// True when "load older" should be offered.
    pub fn can_load_older(&self) -> bool {
        !self.loading && self.error.is_none() && self.next_cursor.is_some() && !self.at_cap()
    }

    /// The page cap is a STOP, not an eviction (module doc).
    pub fn at_cap(&self) -> bool {
        self.pages >= MAX_PAGES
    }

    /// Fold one fetched page in. A page-1 fetch (no pages held yet, or
    /// after `reset`) REPLACES state; a cursor fetch appends older
    /// entries. The daemon's `session`/`bind` metadata always reflects
    /// the latest response — a mid-walk metadata change cannot happen
    /// without a `bad_cursor` first (the cursor is store-fingerprinted),
    /// so overwrite is safe.
    pub fn apply_page(&mut self, page: TranscriptPage) {
        self.loading = false;
        self.error = None;
        self.session = page.session;
        self.store = page.store;
        self.bind = page.bind;
        self.stores_unavailable = page.stores_unavailable;
        self.skipped += page.skipped;
        self.next_cursor = page.next_cursor;
        self.entries.extend(page.entries);
        self.pages += 1;
    }

    pub fn apply_failure(&mut self, failure: TranscriptFailure) {
        self.loading = false;
        self.error = Some(failure);
    }

    /// Back to empty (a fresh page-1 fetch follows). Keeps
    /// `auto_reloaded` — that flag guards the reload loop, so only an
    /// explicit user action clears it (see [`TranscriptPane::user_reset`]).
    pub fn reset(&mut self) {
        let auto_reloaded = self.auto_reloaded;
        *self = Self {
            auto_reloaded,
            loading: true,
            ..Self::default()
        };
    }

    pub fn user_reset(&mut self) {
        *self = Self {
            loading: true,
            ..Self::default()
        };
    }
}

/// What the board asks the app to fetch (deferred-action pattern — the
/// board renders against `&Fleet`, so intents are collected and executed
/// after `show` returns, same as drive intents).
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptRequest {
    pub agent_id: String,
    /// `None` = (re)load the newest page; `Some` = load older.
    pub cursor: Option<String>,
}

/// The async fetch outcome delivered back to the UI thread.
#[derive(Debug)]
pub struct TranscriptMsg {
    pub agent_id: String,
    pub outcome: Result<TranscriptPage, TranscriptFailure>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_json(n_entries: usize, cursor: Option<&str>) -> TranscriptPage {
        serde_json::from_value(serde_json::json!({
            "agent": "herdr:a1",
            "store": "claude",
            "session": "claude:s1.jsonl",
            "bind": "worktree",
            "stores_unavailable": [],
            "entries": (0..n_entries).map(|i| serde_json::json!({
                "role": if i % 2 == 0 { "assistant" } else { "user" },
                "text": format!("entry {i}"),
                "ts": 1_700_000_000_000u64 + i as u64,
            })).collect::<Vec<_>>(),
            "next_cursor": cursor,
            "skipped": 1,
        }))
        .expect("page parses")
    }

    /// Paging appends older entries with no gaps and tracks the cursor;
    /// metadata reflects the latest page; skipped accumulates.
    #[test]
    fn pages_append_and_cursor_advances() {
        let mut pane = TranscriptPane {
            loading: true,
            ..TranscriptPane::default()
        };
        pane.apply_page(page_json(2, Some("b.100.aa")));
        assert!(!pane.loading);
        assert_eq!(pane.entries.len(), 2);
        assert_eq!(pane.next_cursor.as_deref(), Some("b.100.aa"));
        assert!(pane.can_load_older());

        pane.apply_page(page_json(2, None));
        assert_eq!(pane.entries.len(), 4, "older page appended");
        assert_eq!(pane.next_cursor, None, "exhausted");
        assert_eq!(pane.skipped, 2, "torn-data counters accumulate");
        assert!(!pane.can_load_older(), "no cursor -> nothing to load");
    }

    /// The page cap STOPS paging (visible, not an eviction): the newest
    /// entries the user already has must never be silently dropped.
    #[test]
    fn page_cap_stops_offering_older_pages() {
        let mut pane = TranscriptPane::default();
        for _ in 0..MAX_PAGES {
            pane.apply_page(page_json(1, Some("b.1.aa")));
        }
        assert!(pane.at_cap());
        assert!(pane.next_cursor.is_some(), "the store has more");
        assert!(!pane.can_load_older(), "cap is a stop");
        assert_eq!(pane.entries.len(), MAX_PAGES, "nothing evicted");
    }

    /// A stale fingerprinted cursor (daemon rebound between pages) is
    /// recognized for the drop-and-reload path, once.
    #[test]
    fn stale_cursor_resets_once_then_surfaces() {
        let failure = TranscriptFailure::from_response(
            400,
            &serde_json::json!({ "kind": "bad_cursor", "message": "cursor does not match" }),
        );
        assert!(failure.is_stale_cursor());

        let mut pane = TranscriptPane::default();
        pane.apply_page(page_json(3, Some("b.5.aa")));
        assert!(!pane.auto_reloaded);
        pane.auto_reloaded = true;
        pane.reset();
        assert!(pane.entries.is_empty(), "reload starts clean");
        assert!(pane.loading);
        assert!(pane.auto_reloaded, "reset keeps the loop guard");
        pane.user_reset();
        assert!(!pane.auto_reloaded, "an explicit reload re-arms it");
    }

    /// Failure parsing: the typed contract, the ambiguous candidate
    /// list, and the non-JSON transport fallback.
    #[test]
    fn failures_parse_typed_ambiguous_and_transport() {
        let ambiguous = TranscriptFailure::from_response(
            409,
            &serde_json::json!({
                "kind": "ambiguous_session",
                "message": "more than one session",
                "candidates": [
                    { "label": "claude:s1.jsonl", "recency_ms": 5 },
                    { "label": "claude:s2.jsonl", "recency_ms": 5 },
                ],
            }),
        );
        assert_eq!(ambiguous.kind, "ambiguous_session");
        assert_eq!(
            ambiguous.candidates,
            vec!["claude:s1.jsonl".to_string(), "claude:s2.jsonl".to_string()]
        );

        let transport = TranscriptFailure::from_response(502, &serde_json::Value::Null);
        assert_eq!(transport.kind, "transport");
        assert_eq!(transport.message, "HTTP 502");
        assert!(transport.candidates.is_empty());

        let no_grant = TranscriptFailure::from_response(
            403,
            &serde_json::json!({ "kind": "not_granted", "message": "read_tail not granted" }),
        );
        assert!(!no_grant.is_stale_cursor());
    }

    /// A failure never clears already-fetched entries — the user keeps
    /// what they have, with the error alongside.
    #[test]
    fn failure_keeps_fetched_entries() {
        let mut pane = TranscriptPane::default();
        pane.apply_page(page_json(2, Some("b.9.aa")));
        pane.loading = true;
        pane.apply_failure(TranscriptFailure::from_response(
            503,
            &serde_json::json!({ "kind": "query_timeout", "message": "slow store" }),
        ));
        assert!(!pane.loading);
        assert_eq!(pane.entries.len(), 2, "entries survive a failed page");
        assert_eq!(
            pane.error.as_ref().map(|e| e.kind.as_str()),
            Some("query_timeout")
        );
        assert!(!pane.can_load_older(), "no retry spam while errored");
    }
}
