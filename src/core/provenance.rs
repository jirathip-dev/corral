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
//! - No harness/provider/model metadata exists anywhere in this module by
//!   construction: attribution comes from the signed request, not labels.

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptEvent {
    /// The signed envelope's `request_id`.
    pub request_id: String,
    /// Canonical agent target the prompt was dispatched to.
    pub target: String,
    /// SHA-256 hex of the exact dispatched text.
    pub text_sha256: String,
    /// Byte length of the exact dispatched text.
    pub text_len: usize,
    /// Epoch millis when the dispatch succeeded.
    pub ts: u64,
}

impl PromptEvent {
    /// Record the identity of dispatched `text` (SHA-256 + byte length).
    pub fn new(request_id: &str, target: &str, text: &str, ts: u64) -> Self {
        Self {
            request_id: request_id.to_string(),
            target: target.to_string(),
            text_sha256: text_sha256_hex(text),
            text_len: text.len(),
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

    /// The recorded prompt for `target` whose dispatched text is `text`
    /// (the terminal echo match), newest first. Read by the canonical
    /// block builder to dedupe the echo into exactly one `user` block.
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
}
