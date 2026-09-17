//! Immutable wire publication. Construction belongs to the coalescer, never
//! an HTTP handler. The history contains encoded frames, not agent maps.
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;

use super::super::model::{Agent, Snapshot};

#[cfg(test)]
thread_local! {
    // Encoder-thread-local so parallel tests cannot contaminate the observation.
    static AGENT_ENCODINGS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[derive(Debug)]
pub(super) struct EncodedAgents {
    pub(super) state: BTreeMap<String, Agent>,
    bytes: Bytes,
}

impl EncodedAgents {
    fn new(state: BTreeMap<String, Agent>) -> Self {
        #[cfg(test)]
        AGENT_ENCODINGS.with(|count| count.set(count.get() + 1));
        let bytes = serde_json::to_vec(&serde_json::to_value(&state).expect("agents serialize"))
            .expect("agent JSON serializes")
            .into();
        tracing::debug!(agents = state.len(), "snapshot agents encoded");
        Self { state, bytes }
    }
}

#[derive(Debug)]
pub struct PublishedSnapshot {
    pub rev: u64,
    pub body: Arc<Bytes>,
    pub event: Arc<Bytes>,
    /// Monotonic age starts when state was captured, including encoding time.
    pub captured_at: Instant,
    pub(crate) git_plane_backlog: bool,
    pub(crate) history: VecDeque<(u64, Arc<Bytes>)>,
    pub(super) agents: Arc<EncodedAgents>,
    /// Agent-free metadata at the same capture boundary as body/event.
    metadata: Snapshot,
    #[cfg(test)]
    pub(super) agent_encodings: u64,
}

impl PublishedSnapshot {
    pub(super) fn new(
        epoch: &str,
        mut snapshot: Snapshot,
        captured_at: Instant,
        history: VecDeque<(u64, Arc<Bytes>)>,
        cached_agents: Option<Arc<EncodedAgents>>,
    ) -> Self {
        #[cfg(test)]
        let encodings_before = AGENT_ENCODINGS.get();
        let state = std::mem::take(&mut snapshot.agents);
        let agents = cached_agents.unwrap_or_else(|| Arc::new(EncodedAgents::new(state)));
        let body = wire_bytes_with_agents(epoch, &snapshot, Some(&agents.bytes));
        tracing::debug!("snapshot published");
        Self {
            rev: snapshot.rev,
            event: Arc::new(event_bytes("snapshot", snapshot.rev, &body)),
            body: Arc::new(body),
            captured_at,
            git_plane_backlog: snapshot.git_plane_backlog,
            history,
            agents,
            metadata: snapshot,
            #[cfg(test)]
            agent_encodings: AGENT_ENCODINGS.get() - encodings_before,
        }
    }

    /// generated_at is assembly time, not an independent state change. Ages
    /// remain exact milliseconds: no bucket can postpone a stale transition.
    pub(super) fn same_health(&self, snapshot: &Snapshot) -> bool {
        let previous = &self.metadata;
        // Equal relative ages after a new observation still need a new time
        // origin. Otherwise consumers age the old generated_at indefinitely.
        (previous.generated_at == snapshot.generated_at
            || (previous.git_plane_last_event_age_ms.is_none()
                && previous
                    .git_worktree_facts
                    .values()
                    .all(|fact| fact.fact_age_ms.is_none())))
            && self.git_plane_backlog == snapshot.git_plane_backlog
            && previous.git_plane_alive == snapshot.git_plane_alive
            && previous.git_plane_last_event_age_ms == snapshot.git_plane_last_event_age_ms
            && previous.git_plane_skipped == snapshot.git_plane_skipped
            && previous.git_worktree_facts.len() == snapshot.git_worktree_facts.len()
            && previous
                .git_worktree_facts
                .iter()
                .zip(&snapshot.git_worktree_facts)
                .all(|((path, old), (next_path, next))| {
                    path == next_path
                        && old.fact_age_ms == next.fact_age_ms
                        && old.stale == next.stale
                })
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
    wire_bytes_with_agents(epoch, data, None)
}

fn wire_bytes_with_agents(
    epoch: &str,
    data: &impl serde::Serialize,
    agents: Option<&[u8]>,
) -> Bytes {
    // Keep the old wire_value -> Json ordering (serde_json::Value sorts keys).
    // Snapshot callers supply agent-free metadata plus a separately encoded
    // JSON value. Compose a NEW object, never parse/splice a published body.
    let mut value = serde_json::to_value(data).expect("delta/snapshot serializes");
    let object = value.as_object_mut().expect("wire object");
    object.insert("epoch".into(), serde_json::json!(epoch));
    let mut body = vec![b'{'];
    for (index, (key, value)) in object.iter().enumerate() {
        if index > 0 {
            body.push(b',');
        }
        serde_json::to_writer(&mut body, key).expect("wire key serializes");
        body.push(b':');
        if key == "agents"
            && let Some(agents) = agents
        {
            body.extend_from_slice(agents);
        } else {
            serde_json::to_writer(&mut body, value).expect("wire value serializes");
        }
    }
    body.push(b'}');
    body.into()
}

pub(crate) fn event_bytes(kind: &str, rev: u64, body: &[u8]) -> Bytes {
    // Exact axum Event::event().id().json_data() framing. Compact JSON escapes
    // CR/LF inside strings, so data is always a single SSE line.
    let mut frame = format!("event: {kind}\nid: {rev}\ndata: ").into_bytes();
    frame.extend_from_slice(body);
    frame.extend_from_slice(b"\n\n");
    frame.into()
}
