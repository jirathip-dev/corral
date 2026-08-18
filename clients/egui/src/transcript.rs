//! #64 (D35 slice 3): client-side transcript pane state — pure data +
//! parsing for the lazy-paged `GET /transcript` viewer.
//!
//! The daemon serves NEWEST-FIRST pages; the pane appends each fetched
//! page, so the walk extends toward older content. Bounds (review F2 —
//! a SLIDING WINDOW, not a dead end): the pane holds at most
//! [`MAX_ENTRIES`] entries / ~[`MAX_TEXT_BYTES`] of text; past that the
//! NEWEST-loaded entries fall out of the window (`base_offset` counts
//! them, the UI says so) while the walk continues unbounded toward the
//! start of the transcript — a reader paging backwards is reading the
//! old end, so the pages they scrolled past are the right ones to drop.
//! "Reload" returns to the top at any time. The whole transcript is
//! therefore REACHABLE (#27 AC1) while memory stays bounded.
//!
//! Concurrency (review F1): every request is stamped with the pane's
//! `generation`; a response whose generation does not match the CURRENT
//! pane is dropped on the floor — a late page can never fold into a
//! pane that was reset, evicted, or recreated since the request left.
//! Correlation lives in [`crate::state::Fleet::fold_transcript`] so it
//! is unit-testable without the app.
//!
//! Everything in this module is pure (JSON in, state out); fetching
//! lives in `protocol.rs`, wiring in `app.rs`, rendering in
//! `ui/board.rs`.

use serde::Deserialize;

/// Sliding-window caps: entries and (approximate) held text bytes.
pub const MAX_ENTRIES: usize = 1000;
pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
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

    pub fn is_not_granted(&self) -> bool {
        self.kind == "not_granted"
    }
}

/// Per-agent pane state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptPane {
    /// The held window, newest-first. `entries[0]` is absolute index
    /// `base_offset` of the walk (0 = the transcript's newest message).
    pub entries: Vec<TranscriptEntry>,
    /// How many NEWEST-loaded entries the sliding window has dropped —
    /// nonzero means "reload to return to the top" (review F2).
    pub base_offset: usize,
    /// Approximate held text bytes (maintained incrementally).
    pub held_bytes: usize,
    pub next_cursor: Option<String>,
    pub pages: usize,
    pub session: String,
    pub store: String,
    /// The WEAKEST bind provenance seen across the walk (review F8):
    /// once any page answered from the worktree heuristic, the pane
    /// stays labeled best-effort.
    pub bind: String,
    /// UNION of every page's unavailable-store report (review F8): a
    /// store that failed during page 1's bind stays reported even if a
    /// later page found it healthy — the held content may still derive
    /// from the degraded bind.
    pub stores_unavailable: Vec<String>,
    /// Sum of the daemon's per-page torn-data counters.
    pub skipped: usize,
    /// A fetch is in flight (render a spinner, disable "load older").
    pub loading: bool,
    pub error: Option<TranscriptFailure>,
    /// One automatic reload has already been spent on a stale cursor —
    /// a second stale cursor surfaces as an error instead of looping.
    pub auto_reloaded: bool,
    /// Response-correlation stamp (review F1): minted from the fleet
    /// clock at creation and every reset; requests carry it, responses
    /// must match it.
    pub generation: u64,
    /// Fleet-clock value at last access — real LRU eviction (review
    /// F14), not arbitrary map order.
    pub touched: u64,
}

impl TranscriptPane {
    /// True when "load older" should be offered (errors offer "retry"
    /// instead — review F7 — never both).
    pub fn can_load_older(&self) -> bool {
        !self.loading && self.error.is_none() && self.next_cursor.is_some()
    }

    /// A transient failure keeps `next_cursor`, so the SAME cursor can
    /// be retried without throwing the walk away (review F7).
    pub fn can_retry(&self) -> bool {
        !self.loading && self.error.as_ref().is_some_and(|e| !e.is_stale_cursor())
    }

    /// Fold one fetched page in and slide the window (review F2).
    pub fn apply_page(&mut self, page: TranscriptPage) {
        self.loading = false;
        self.error = None;
        self.session = page.session;
        self.store = page.store;
        // Weakest-bind-wins (review F8): "worktree" is sticky.
        if self.bind.is_empty() || page.bind == "worktree" {
            self.bind = page.bind;
        }
        for store in page.stores_unavailable {
            if !self.stores_unavailable.contains(&store) {
                self.stores_unavailable.push(store);
            }
        }
        self.skipped += page.skipped;
        self.next_cursor = page.next_cursor;
        for entry in &page.entries {
            self.held_bytes += entry.text.len();
        }
        self.entries.extend(page.entries);
        self.pages += 1;
        // Slide: drop the NEWEST-loaded entries past the caps. At least
        // one entry is always kept, so a single over-cap entry (the
        // daemon exempts a page's first entry from its text budget)
        // still renders rather than emptying the pane. Known cliff
        // (review R4, accepted): one such giant arriving can slide out
        // EVERYTHING held before it — honestly counted in base_offset,
        // and display-side layout is bounded regardless.
        let mut drop = 0;
        let mut bytes = self.held_bytes;
        while self.entries.len() - drop > 1
            && (self.entries.len() - drop > MAX_ENTRIES || bytes > MAX_TEXT_BYTES)
        {
            bytes = bytes.saturating_sub(self.entries[drop].text.len());
            drop += 1;
        }
        if drop > 0 {
            self.entries.drain(0..drop);
            self.held_bytes = bytes;
            self.base_offset += drop;
        }
    }

    pub fn apply_failure(&mut self, failure: TranscriptFailure) {
        self.loading = false;
        self.error = Some(failure);
    }

    /// Back to empty under a NEW generation (a fresh page-1 fetch
    /// follows; in-flight responses from the old generation are dropped
    /// by the correlation check). Keeps `auto_reloaded` — that flag
    /// guards the reload loop; only [`TranscriptPane::user_reset`]
    /// re-arms it.
    pub fn reset(&mut self, generation: u64) {
        let auto_reloaded = self.auto_reloaded;
        let touched = self.touched;
        *self = Self {
            auto_reloaded,
            loading: true,
            generation,
            touched,
            ..Self::default()
        };
    }

    pub fn user_reset(&mut self, generation: u64) {
        let touched = self.touched;
        *self = Self {
            loading: true,
            generation,
            touched,
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
    /// `None` = (re)load the newest page; `Some` = load older (or retry
    /// the same cursor after a transient failure).
    pub cursor: Option<String>,
}

/// The async fetch outcome delivered back to the UI thread. Carries the
/// generation the request was stamped with (review F1).
#[derive(Debug)]
pub struct TranscriptMsg {
    pub agent_id: String,
    pub generation: u64,
    pub outcome: Result<TranscriptPage, TranscriptFailure>,
}

/// What [`crate::state::Fleet::fold_transcript`] decided — the app acts
/// on it (toast + re-fetch for `NeedsReload`, ledger for `NotGranted`),
/// tests assert on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FoldOutcome {
    /// Failure folded into the live pane.
    Applied,
    /// Served page folded — the read_tail grant is proven live.
    AppliedOk,
    /// Stale generation or no such pane (deleted agent): ignored.
    Dropped,
    /// Stale cursor, first strike: the pane was reset — refetch newest.
    NeedsReload,
    /// The daemon refused the grant — the ledger should hear about it.
    NotGranted,
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn page(n_entries: usize, cursor: Option<&str>) -> TranscriptPage {
        serde_json::from_value(serde_json::json!({
            "agent": "herdr:a1",
            "store": "claude",
            "session": "claude:s1.jsonl",
            "bind": "session_id",
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
    /// skipped accumulates.
    #[test]
    fn pages_append_and_cursor_advances() {
        let mut pane = TranscriptPane {
            loading: true,
            ..TranscriptPane::default()
        };
        pane.apply_page(page(2, Some("b.100.aa")));
        assert!(!pane.loading);
        assert_eq!(pane.entries.len(), 2);
        assert_eq!(pane.next_cursor.as_deref(), Some("b.100.aa"));
        assert!(pane.can_load_older());

        pane.apply_page(page(2, None));
        assert_eq!(pane.entries.len(), 4, "older page appended");
        assert_eq!(pane.next_cursor, None, "exhausted");
        assert_eq!(pane.skipped, 2, "torn-data counters accumulate");
        assert!(!pane.can_load_older(), "no cursor -> nothing to load");
    }

    /// F2: the window SLIDES — newest-loaded entries drop (counted in
    /// base_offset), the walk continues past the cap, nothing dead-ends.
    #[test]
    fn window_slides_past_the_entry_cap_and_the_walk_continues() {
        let mut pane = TranscriptPane::default();
        let per_page = 100;
        for _ in 0..(MAX_ENTRIES / per_page) {
            pane.apply_page(page(per_page, Some("b.1.aa")));
        }
        assert_eq!(pane.entries.len(), MAX_ENTRIES);
        assert_eq!(pane.base_offset, 0);

        pane.apply_page(page(per_page, Some("b.2.aa")));
        assert_eq!(pane.entries.len(), MAX_ENTRIES, "window holds the cap");
        assert_eq!(pane.base_offset, per_page, "NEWEST-loaded dropped, counted");
        assert!(pane.can_load_older(), "the walk is NOT a dead end (F2)");
    }

    /// F2: the byte cap slides too, and a single over-cap entry is kept
    /// rather than emptying the pane.
    #[test]
    fn window_slides_on_bytes_and_keeps_a_lone_giant_entry() {
        let mut pane = TranscriptPane::default();
        let giant = TranscriptPage {
            entries: vec![TranscriptEntry {
                role: "assistant".into(),
                text: "x".repeat(MAX_TEXT_BYTES + 1),
                ts: None,
            }],
            ..page(0, Some("b.9.aa"))
        };
        pane.apply_page(giant);
        assert_eq!(pane.entries.len(), 1, "lone giant survives");

        pane.apply_page(page(3, Some("b.10.aa")));
        assert!(
            pane.held_bytes <= MAX_TEXT_BYTES,
            "the giant slid out once smaller content arrived"
        );
        assert!(pane.base_offset >= 1);
    }

    /// F8: bind provenance is weakest-wins and unavailable stores union
    /// across the walk — honesty signals never silently disappear.
    #[test]
    fn bind_is_weakest_wins_and_unavailable_unions() {
        let mut pane = TranscriptPane::default();
        let mut degraded = page(1, Some("b.1.aa"));
        degraded.bind = "worktree".into();
        degraded.stores_unavailable = vec!["opencode".into()];
        pane.apply_page(degraded);
        assert_eq!(pane.bind, "worktree");
        assert_eq!(pane.stores_unavailable, vec!["opencode".to_string()]);

        let healthy = page(1, Some("b.2.aa")); // bind: session_id, none unavailable
        pane.apply_page(healthy);
        assert_eq!(pane.bind, "worktree", "weakest bind is sticky");
        assert_eq!(
            pane.stores_unavailable,
            vec!["opencode".to_string()],
            "the warning survives a later healthy page"
        );
    }

    /// F7: a transient failure keeps the cursor and offers retry; a
    /// stale-cursor failure routes to reload instead.
    #[test]
    fn transient_failure_is_retryable_with_the_same_cursor() {
        let mut pane = TranscriptPane::default();
        pane.apply_page(page(2, Some("b.9.aa")));
        pane.loading = true;
        pane.apply_failure(TranscriptFailure::from_response(
            503,
            &serde_json::json!({ "kind": "query_timeout", "message": "slow store" }),
        ));
        assert!(!pane.loading);
        assert_eq!(pane.entries.len(), 2, "entries survive a failed page");
        assert!(!pane.can_load_older(), "no load-older while errored");
        assert!(pane.can_retry(), "retry offered");
        assert_eq!(pane.next_cursor.as_deref(), Some("b.9.aa"), "cursor kept");

        pane.apply_failure(TranscriptFailure::from_response(
            400,
            &serde_json::json!({ "kind": "bad_cursor", "message": "stale" }),
        ));
        assert!(!pane.can_retry(), "stale cursor is reload, not retry");
    }

    /// F1: reset mints a NEW generation so in-flight responses from the
    /// old one can be recognized and dropped; the auto-reload guard
    /// survives reset and only user_reset re-arms it.
    #[test]
    fn reset_advances_generation_and_keeps_the_loop_guard() {
        let mut pane = TranscriptPane::default();
        pane.apply_page(page(3, Some("b.5.aa")));
        pane.auto_reloaded = true;
        pane.reset(7);
        assert!(pane.entries.is_empty(), "reload starts clean");
        assert!(pane.loading);
        assert_eq!(pane.generation, 7);
        assert!(pane.auto_reloaded, "reset keeps the loop guard");
        pane.user_reset(9);
        assert_eq!(pane.generation, 9);
        assert!(!pane.auto_reloaded, "an explicit reload re-arms it");
    }

    /// Round-3 gap: an ABSOLUTE selection must resolve to the SAME
    /// message before and after a window slide (the relative-index bug
    /// slipped through once — review R1).
    #[test]
    fn absolute_selection_survives_a_window_slide() {
        let mut pane = TranscriptPane::default();
        // Fill to the cap with distinctly-labeled entries.
        let full: Vec<TranscriptPage> = (0..(MAX_ENTRIES / 100))
            .map(|p| {
                let mut page = page(0, Some("b.1.aa"));
                page.entries = (0..100)
                    .map(|i| TranscriptEntry {
                        role: "user".into(),
                        text: format!("abs-{}", p * 100 + i),
                        ts: None,
                    })
                    .collect();
                page
            })
            .collect();
        for page in full {
            pane.apply_page(page);
        }
        // Select absolute index 250 (the resolution the UI performs).
        let absolute = 250usize;
        let before = pane.entries[absolute - pane.base_offset].text.clone();
        assert_eq!(before, "abs-250");

        // A further page slides the window.
        let mut older = page(0, Some("b.2.aa"));
        older.entries = (0..100)
            .map(|i| TranscriptEntry {
                role: "user".into(),
                text: format!("abs-{}", MAX_ENTRIES + i),
                ts: None,
            })
            .collect();
        pane.apply_page(older);
        assert!(pane.base_offset > 0, "the window slid");

        let after = absolute
            .checked_sub(pane.base_offset)
            .and_then(|i| pane.entries.get(i))
            .map(|e| e.text.clone());
        assert_eq!(
            after.as_deref(),
            Some(before.as_str()),
            "an absolute selection resolves to the same message across a slide"
        );
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
        assert!(no_grant.is_not_granted());
        assert!(!no_grant.is_stale_cursor());
    }
}
