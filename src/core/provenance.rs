//! #315: canonical transcript provenance — the Corral-owned record of
//! successfully dispatched signed Prompts.
//!
//! When the drive plane dispatches a signed `prompt` for a target, the
//! handler records one [`PromptEvent`] here. The event is the AUTHORITATIVE
//! fact "Corral sent this exact text to this target at this time"; the
//! matching terminal echo of that text is deduplicated against it when the
//! canonical block stream is built ([`crate::core::blocks::canonical_blocks`]).
//!
//! Design bounds:
//! - Bounded: a fixed-capacity ring (oldest evicted first). The ledger only
//!   needs recent prompts to dedupe the recent-output window clients read;
//!   it is not an unbounded session history (the audit log already is).
//! - Identity-only: events store a SHA-256 of the prompt text plus its
//!   byte length — never the raw text — so the ledger cannot leak prompt
//!   content, and the read path redacts before hashing anyway.
//! - One-to-one (#315 R2): binding echoes to events consumes from a
//!   per-read WINDOW, not from the ledger itself. A recorded event may back
//!   at most one echo per read (exactly-once rendering), repeated reads
//!   stay stable, and identical repeated prompts bind oldest-first so each
//!   echo carries its own signed request id.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

/// Maximum recorded prompt events. Generous vs. the 200-line recent-output
/// window (many prompts per read), small enough to stay trivially bounded.
const LEDGER_CAP: usize = 256;

/// One successfully dispatched signed Prompt, as recorded by the drive
/// handler. `text_sha256`/`text_len` identify the echoed text without
/// carrying it; `request_id` is the signed envelope id clients may audit.
///
/// The identity covers the REDACTED dispatch text (#315 R2): the read path
/// redacts before hashing, so recording the same redacted identity is what
/// lets a prompt containing a secret keep its provenance. The raw secret
/// is never stored here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptEvent {
    /// The signed envelope's `request_id`.
    pub request_id: String,
    /// Canonical agent target the prompt was dispatched to.
    pub target: String,
    /// SHA-256 hex of the REDACTED dispatched text.
    pub text_sha256: String,
    /// Byte length of the REDACTED dispatched text.
    pub text_len: usize,
    /// Epoch millis when the dispatch succeeded.
    pub ts: u64,
}

impl PromptEvent {
    /// Record the identity of dispatched `text` (SHA-256 + byte length) in
    /// its REDACTED form — the exact bytes a client can ever see echoed.
    pub fn new(request_id: &str, target: &str, text: &str, ts: u64) -> Self {
        let redacted = crate::core::redact::redact(text);
        Self {
            request_id: request_id.to_string(),
            target: target.to_string(),
            text_sha256: text_sha256_hex(&redacted),
            text_len: redacted.len(),
            ts,
        }
    }

    /// Whether `text` is this event's dispatched text. The comparison is
    /// hash+length based: the terminal echo (post-redaction) is matched
    /// against the recorded identity, never by provider/harness labels.
    pub fn matches_text(&self, text: &str) -> bool {
        self.text_len == text.len() && self.text_sha256 == text_sha256_hex(text)
    }
}

pub fn text_sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Return the one redacted identity used for structured exchange events.
/// Cleaning removes terminal escapes/overdraw, redaction runs before hashing,
/// and outer whitespace is presentation rather than exchange content.
pub fn canonical_exchange_text(text: &str) -> String {
    let cleaned = crate::core::blocks::clean(text);
    crate::core::redact::redact(cleaned.trim()).into_owned()
}

/// Bounded, thread-safe ledger of dispatched prompts, keyed by target.
#[derive(Debug, Default)]
pub struct PromptProvenance {
    inner: Mutex<PromptLedger>,
}

#[derive(Debug, Default)]
struct PromptLedger {
    /// Per-target ring (oldest first) so a target's echoes only ever match
    /// that target's own prompts.
    per_target: HashMap<String, VecDeque<PromptEvent>>,
    /// Total events retained across all targets (the eviction bound).
    total: usize,
}

impl PromptProvenance {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a successfully dispatched prompt (drive handler, once per
    /// dispatch — the replay table already guarantees exactly-once).
    pub fn record(&self, event: PromptEvent) {
        let mut ledger = self.inner.lock().expect("provenance lock poisoned");
        let ring = ledger.per_target.entry(event.target.clone()).or_default();
        ring.push_back(event);
        ledger.total += 1;
        while ledger.total > LEDGER_CAP {
            // Evict the globally oldest event (front of some target's ring).
            let oldest_key = ledger
                .per_target
                .iter()
                .min_by_key(|(_, ring)| ring.front().map(|e| e.ts).unwrap_or(u64::MAX))
                .map(|(target, _)| target.clone());
            match oldest_key {
                Some(target) => {
                    if let Some(ring) = ledger.per_target.get_mut(&target) {
                        ring.pop_front();
                        if ring.is_empty() {
                            ledger.per_target.remove(&target);
                        }
                        ledger.total -= 1;
                    } else {
                        break;
                    }
                }
                None => break,
            }
        }
    }

    /// Bind `echoes` (per cleaned line: the eligible echo candidate for that
    /// line, or `None`) to recorded events, one-to-one (#315 R2):
    ///
    /// - A snapshot of the target's ring is taken first, so the read works
    ///   over an immutable ledger view and repeated reads see the same
    ///   events (the ledger itself is never consumed).
    /// - Events are offered OLDEST FIRST and each binds to at most one
    ///   echo in this read, so repeated identical prompts keep their own
    ///   request ids instead of all stamping the newest event.
    /// - Within one read, at most `window` echoes may bind overall (the
    ///   trailing window size), bounding any single read's fan-out without
    ///   globally consuming the ledger.
    ///
    /// Returns one [`PromptEvent`] per input slot: the event bound to that
    /// echo, or `None` when the echo stays unattributed (duplicate beyond
    /// the one-to-one map, window excess, or no recorded match).
    pub fn bind_echoes(
        &self,
        target: &str,
        echoes: &[Option<String>],
        window: usize,
    ) -> Vec<Option<PromptEvent>> {
        let mut out: Vec<Option<PromptEvent>> = Vec::with_capacity(echoes.len());
        let Some(snapshot) = self.snapshot(target) else {
            out.resize(echoes.len(), None);
            return out;
        };
        // The window bounds the eligible echo slice to the TRAILING `window`
        // echo slots; earlier eligible echoes in the same read do not bind.
        let eligible_floor = echoes.len().saturating_sub(window);
        // Per-read consumption: an event binds to at most one echo of this
        // read, so identical echoes map to distinct events in ledger order.
        let mut used = vec![false; snapshot.len()];
        for (i, echo) in echoes.iter().enumerate() {
            let Some(text) = echo else {
                out.push(None);
                continue;
            };
            if i < eligible_floor {
                out.push(None);
                continue;
            }
            // The oldest not-yet-bound event whose identity matches this
            // echo — one-to-one, oldest first.
            let mut bound = None;
            for (idx, event) in snapshot.iter().enumerate() {
                if !used[idx] && event.matches_text(text) {
                    used[idx] = true;
                    bound = Some(event.clone());
                    break;
                }
            }
            out.push(bound);
        }
        out
    }

    /// Immutable snapshot of one target's ring, oldest first.
    fn snapshot(&self, target: &str) -> Option<Vec<PromptEvent>> {
        let ledger = self.inner.lock().expect("provenance lock poisoned");
        ledger
            .per_target
            .get(target)
            .map(|ring| ring.iter().cloned().collect())
    }

    /// The recorded prompt for `target` whose dispatched text is `text`
    /// (newest match, pure read). Kept for diagnostics/tests; the canonical
    /// block builder binds echoes via [`Self::bind_echoes`].
    pub fn find_by_text(&self, target: &str, text: &str) -> Option<PromptEvent> {
        let ledger = self.inner.lock().expect("provenance lock poisoned");
        ledger
            .per_target
            .get(target)
            .and_then(|ring| ring.iter().rev().find(|event| event.matches_text(text)))
            .cloned()
    }

    /// Whether any prompt is recorded for `target` (diagnostics/tests).
    pub fn has_events_for(&self, target: &str) -> bool {
        let ledger = self.inner.lock().expect("provenance lock poisoned");
        ledger
            .per_target
            .get(target)
            .is_some_and(|ring| !ring.is_empty())
    }
}

/// #330: the authoritative structured role for a recorded exchange event.
/// Roles come from the STRUCTURED source (a Corral Prompt dispatch for
/// `user`; herdr's `pane.output_matched` classification for the agent's
/// blocked question), never from terminal prose, provider, or model names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExchangeRole {
    /// The agent asked the operator a question (answer_question / menu).
    Assistant,
    /// The agent requested a tool approval (approve_tool).
    Tool,
}

/// #330: one structured agent-side exchange event as observed by Corral —
/// the agent's blocked question (herdr `pane.output_matched` →
/// `waiting_on`). Same identity-only design as [`PromptEvent`]: the ledger
/// stores SHA-256 + length of the CLEANED, REDACTED text — the exact bytes
/// a client can ever see echoed in a read window — never the raw question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangeEvent {
    /// Structured event identity (the claim's `approval_id` when the agent
    /// is blocked on a question).
    pub id: String,
    /// Canonical agent target the event belongs to.
    pub target: String,
    /// Authoritative structured role of the event.
    pub role: ExchangeRole,
    /// SHA-256 hex of the canonical cleaned, redacted, trimmed event text.
    pub text_sha256: String,
    /// Byte length of the canonical cleaned, redacted, trimmed event text.
    pub text_len: usize,
    /// Epoch millis when the event was observed.
    pub ts: u64,
}

impl ExchangeEvent {
    /// Record the identity of `text` in its CLEANED, REDACTED form — the
    /// exact canonical identity the read path compares window lines against
    /// (`canonical_exchange_text` cleans, redacts, and trims before hashing).
    pub fn new(id: &str, target: &str, role: ExchangeRole, text: &str, ts: u64) -> Self {
        let canonical = canonical_exchange_text(text);
        Self {
            id: id.to_string(),
            target: target.to_string(),
            role,
            text_sha256: text_sha256_hex(&canonical),
            text_len: canonical.len(),
            ts,
        }
    }

    /// Whether `text` is this event's text (hash + length identity on the
    /// canonical cleaned, redacted, trimmed form — never a prose or name
    /// comparison).
    pub fn matches_text(&self, text: &str) -> bool {
        let canonical = canonical_exchange_text(text);
        self.text_len == canonical.len() && self.text_sha256 == text_sha256_hex(&canonical)
    }
}

/// #330: bounded, thread-safe ledger of structured agent-side exchange
/// events, keyed by target — the assistant/tool half of the canonical
/// role/provenance seam ([`PromptProvenance`] is the user half). Same
/// bounds and one-to-one per-read binding semantics as the prompt ledger.
#[derive(Debug, Default)]
pub struct ExchangeLedger {
    inner: Mutex<ExchangeLedgerInner>,
}

#[derive(Debug, Default)]
struct ExchangeLedgerInner {
    /// Per-target ring (oldest first) so a target's events only ever match
    /// that target's own windows.
    per_target: HashMap<String, VecDeque<ExchangeEvent>>,
    /// Total events retained across all targets (the eviction bound).
    total: usize,
}

impl ExchangeLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one observed structured event (adapter boundary, exactly once
    /// per `pane.output_matched` push). Bounded identically to the prompt
    /// ledger: the globally oldest event is evicted first.
    pub fn record(&self, event: ExchangeEvent) {
        let mut ledger = self.inner.lock().expect("exchange ledger lock poisoned");
        let ring = ledger.per_target.entry(event.target.clone()).or_default();
        ring.push_back(event);
        ledger.total += 1;
        while ledger.total > LEDGER_CAP {
            let oldest_key = ledger
                .per_target
                .iter()
                .min_by_key(|(_, ring)| ring.front().map(|e| e.ts).unwrap_or(u64::MAX))
                .map(|(target, _)| target.clone());
            match oldest_key {
                Some(target) => {
                    if let Some(ring) = ledger.per_target.get_mut(&target) {
                        ring.pop_front();
                        if ring.is_empty() {
                            ledger.per_target.remove(&target);
                        }
                        ledger.total -= 1;
                    } else {
                        break;
                    }
                }
                None => break,
            }
        }
    }

    /// Bind structured events to a read window's candidate lines, one-to-one
    /// (#315 R2 semantics, mirroring [`PromptProvenance::bind_echoes`]):
    /// - an immutable per-target snapshot is taken first (repeated reads
    ///   stay stable; the ledger is never consumed);
    /// - events are offered OLDEST FIRST and each binds to at most one
    ///   candidate in this read;
    /// - at most `window` trailing candidates may bind overall.
    ///
    /// Returns one event per input slot: the event bound to that candidate,
    /// or `None` when the line stays unattributed. Every non-blank line is
    /// a candidate — the agent's structured question is plain prose in the
    /// terminal, so there is no decoration to require; identity is exact.
    pub fn bind_events(
        &self,
        target: &str,
        candidates: &[Option<String>],
        window: usize,
    ) -> Vec<Option<ExchangeEvent>> {
        let mut out: Vec<Option<ExchangeEvent>> = Vec::with_capacity(candidates.len());
        let Some(snapshot) = self.snapshot(target) else {
            out.resize(candidates.len(), None);
            return out;
        };
        let eligible_floor = candidates.len().saturating_sub(window);
        let mut used = vec![false; snapshot.len()];
        for (i, candidate) in candidates.iter().enumerate() {
            let Some(text) = candidate else {
                out.push(None);
                continue;
            };
            if i < eligible_floor {
                out.push(None);
                continue;
            }
            let mut bound = None;
            for (idx, event) in snapshot.iter().enumerate() {
                if !used[idx] && event.matches_text(text) {
                    used[idx] = true;
                    bound = Some(event.clone());
                    break;
                }
            }
            out.push(bound);
        }
        out
    }

    /// Immutable snapshot of one target's ring, oldest first.
    fn snapshot(&self, target: &str) -> Option<Vec<ExchangeEvent>> {
        let ledger = self.inner.lock().expect("exchange ledger lock poisoned");
        ledger
            .per_target
            .get(target)
            .map(|ring| ring.iter().cloned().collect())
    }

    /// Whether any event is recorded for `target` (diagnostics/tests).
    pub fn has_events_for(&self, target: &str) -> bool {
        let ledger = self.inner.lock().expect("exchange ledger lock poisoned");
        ledger
            .per_target
            .get(target)
            .is_some_and(|ring| !ring.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_matches_by_text_identity() {
        let ledger = PromptProvenance::new();
        ledger.record(PromptEvent::new("req-1", "herdr:a", "ship it", 1));
        assert!(ledger.has_events_for("herdr:a"));
        assert!(!ledger.has_events_for("herdr:b"));
        let found = ledger
            .find_by_text("herdr:a", "ship it")
            .expect("exact echo matches");
        assert_eq!(found.request_id, "req-1");
        assert!(ledger.find_by_text("herdr:a", "ship it!").is_none());
        assert!(ledger.find_by_text("herdr:a", "ship  it").is_none());
        assert!(ledger.find_by_text("herdr:b", "ship it").is_none());
    }

    #[test]
    fn ledger_is_bounded_and_evicts_oldest() {
        let ledger = PromptProvenance::new();
        for i in 0..(LEDGER_CAP + 32) {
            ledger.record(PromptEvent::new(
                &format!("req-{i}"),
                "herdr:a",
                &format!("t{i}"),
                i as u64,
            ));
        }
        // The cap holds...
        let mut count = 0;
        for i in 0..(LEDGER_CAP + 32) {
            if ledger.find_by_text("herdr:a", &format!("t{i}")).is_some() {
                count += 1;
            }
        }
        assert_eq!(count, LEDGER_CAP, "ledger never grows past its cap");
        // ...and eviction dropped the OLDEST events.
        assert!(ledger.find_by_text("herdr:a", "t0").is_none());
        assert!(
            ledger
                .find_by_text("herdr:a", &format!("t{}", LEDGER_CAP + 31))
                .is_some()
        );
    }

    #[test]
    fn events_carry_no_raw_text() {
        let event = PromptEvent::new("req-1", "herdr:a", "some secret prompt content", 1);
        let debug = format!("{event:?}");
        assert!(
            !debug.contains("some secret prompt content"),
            "the ledger stores identity (hash+len), never the raw text"
        );
    }

    // ---- #315 R2: window-scoped one-to-one binding ----

    #[test]
    fn records_redacted_identity_for_secret_prompts() {
        let raw = "deploy with token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef123456";
        let ledger = PromptProvenance::new();
        ledger.record(PromptEvent::new("req-s", "herdr:a", raw, 1));
        // The recorded identity is the REDACTED form — the exact bytes the
        // read path (which redacts before hashing) will compare against.
        let redacted = format!("deploy with token {}", crate::core::redact::REDACTED);
        let echo = ledger.bind_echoes("herdr:a", &[Some(redacted.clone())], 8);
        assert_eq!(
            echo.iter()
                .flatten()
                .map(|e| e.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["req-s"],
            "the redacted echo matches the recorded redacted identity"
        );
        // ...and the raw form is gone from the ledger entirely.
        assert!(ledger.find_by_text("herdr:a", raw).is_none());
        let debug = format!("{ledger:?}");
        assert!(
            !debug.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef123456"),
            "no raw secret in the ledger"
        );
    }

    #[test]
    fn bind_echoes_is_one_to_one_per_read() {
        let ledger = PromptProvenance::new();
        ledger.record(PromptEvent::new("req-9", "herdr:a", "ship it", 1));
        let echoes = vec![
            Some("ship it".to_string()),
            None,
            Some("ship it".to_string()),
        ];
        let bound = ledger.bind_echoes("herdr:a", &echoes, 8);
        assert_eq!(
            bound[0].as_ref().map(|e| e.request_id.as_str()),
            Some("req-9")
        );
        assert!(
            bound[2].is_none(),
            "the second identical echo stays unbound: exactly-once per read"
        );
        // A REPEATED read re-binds stably: the ledger was never consumed.
        let again = ledger.bind_echoes("herdr:a", &echoes, 8);
        assert_eq!(
            again[0].as_ref().map(|e| e.request_id.as_str()),
            Some("req-9")
        );
        assert!(again[2].is_none());
    }

    #[test]
    fn bind_echoes_offers_events_oldest_first() {
        let ledger = PromptProvenance::new();
        ledger.record(PromptEvent::new("req-A", "herdr:a", "continue", 1));
        ledger.record(PromptEvent::new("req-B", "herdr:a", "continue", 2));
        let bound = ledger.bind_echoes(
            "herdr:a",
            &[Some("continue".into()), Some("continue".into())],
            8,
        );
        assert_eq!(
            bound
                .iter()
                .flatten()
                .map(|e| e.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["req-A", "req-B"],
            "identical echoes bind one-to-one in ledger order"
        );
    }

    #[test]
    fn bind_echoes_window_caps_the_eligible_trailing_slice() {
        let ledger = PromptProvenance::new();
        ledger.record(PromptEvent::new("req-1", "herdr:a", "go", 1));
        // Three eligible echoes, window 2: only the trailing two may bind.
        let bound = ledger.bind_echoes(
            "herdr:a",
            &[Some("go".into()), Some("go".into()), Some("go".into())],
            2,
        );
        let ids: Vec<Option<&str>> = bound
            .iter()
            .map(|b| b.as_ref().map(|e| e.request_id.as_str()))
            .collect();
        assert_eq!(ids, vec![None, Some("req-1"), None]);
    }

    #[test]
    fn bind_echoes_without_events_leaves_everything_unbound() {
        let ledger = PromptProvenance::new();
        let bound = ledger.bind_echoes("herdr:a", &[Some("x".into())], 8);
        assert!(bound[0].is_none());
    }
}
