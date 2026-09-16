//! Immutable wire publication. Construction belongs to the coalescer, never
//! an HTTP handler. The history contains encoded frames, not agent maps.
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;

use super::super::model::Snapshot;

#[derive(Debug)]
pub struct PublishedSnapshot {
    pub rev: u64,
    pub body: Arc<Bytes>,
    pub event: Arc<Bytes>,
    /// Monotonic age starts when state was captured, including encoding time.
    pub captured_at: Instant,
    pub(crate) git_plane_backlog: bool,
    pub(crate) history: VecDeque<(u64, Arc<Bytes>)>,
}

impl PublishedSnapshot {
    pub(crate) fn new(
        epoch: &str,
        snapshot: &Snapshot,
        captured_at: Instant,
        history: VecDeque<(u64, Arc<Bytes>)>,
    ) -> Self {
        let body = wire_bytes(epoch, snapshot);
        Self {
            rev: snapshot.rev,
            event: Arc::new(event_bytes("snapshot", snapshot.rev, &body)),
            body: Arc::new(body),
            captured_at,
            git_plane_backlog: snapshot.git_plane_backlog,
            history,
        }
    }

    /// Same #450 cursor rules as Store::resume_from_epoch, over the published
    /// boundary only. No agent cloning, serialization or writer lock here.
    pub(crate) fn resume(&self, last_rev: Option<u64>, same_epoch: bool) -> Vec<Arc<Bytes>> {
        if same_epoch && let Some(rev) = last_rev {
            if rev == self.rev {
                return Vec::new();
            }
            if rev < self.rev
                && self
                    .history
                    .front()
                    .is_some_and(|(oldest, _)| rev >= *oldest)
            {
                return self
                    .history
                    .iter()
                    .filter(|(r, _)| *r > rev)
                    .map(|(_, frame)| frame.clone())
                    .collect();
            }
        }
        vec![self.event.clone()]
    }

    pub(crate) fn delta(&self, rev: u64) -> Option<Arc<Bytes>> {
        self.history
            .iter()
            .find(|(r, _)| *r == rev)
            .map(|(_, frame)| frame.clone())
    }
}

pub(crate) fn wire_bytes(epoch: &str, data: &impl serde::Serialize) -> Bytes {
    // Keep the old wire_value -> Json ordering (serde_json::Value sorts keys).
    let mut value = serde_json::to_value(data).expect("delta/snapshot serializes");
    value
        .as_object_mut()
        .expect("wire object")
        .insert("epoch".into(), serde_json::json!(epoch));
    serde_json::to_vec(&value)
        .expect("wire JSON serializes")
        .into()
}

pub(crate) fn event_bytes(kind: &str, rev: u64, body: &[u8]) -> Bytes {
    // Exact axum Event::event().id().json_data() framing. Compact JSON escapes
    // CR/LF inside strings, so data is always a single SSE line.
    let mut frame = format!("event: {kind}\nid: {rev}\ndata: ").into_bytes();
    frame.extend_from_slice(body);
    frame.extend_from_slice(b"\n\n");
    frame.into()
}
