//! Persistent rotating JSONL ring of status-transition events.
//!
//! Layout on disk (D23): `seg-<seq>-<start_ts>.jsonl` append-only files under
//! the history dir. One line per event; the newest file is the active
//! segment. Rotation (size / event-count / age) seals the active segment and
//! starts the next; pruning deletes the oldest sealed segments until the
//! whole ring fits [`RotationPolicy`]. A torn trailing line (crash mid-write)
//! is skipped on load. In-memory the ring mirrors the disk bound so queries
//! and restarts agree.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::core::model::AgentState;
use crate::core::util::now_millis;

/// One status transition. Ring insertion order is authoritative; `ts` is
/// wall-clock (display / `since`-window only, per the codebase convention
/// that ordering never depends on wall-clock equality).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEvent {
    /// Epoch millis when the transition was observed.
    pub ts: u64,
    /// Source pane (identity, never redacted).
    pub pane_id: Option<String>,
    /// Canonical agent id.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// State before the transition; `None` = first time the agent was seen
    /// (new pane, or daemon start re-observing a live agent).
    #[serde(default)]
    pub old_status: Option<AgentState>,
    pub new_status: AgentState,
    /// Adapter/source name, e.g. "herdr".
    pub source: String,
    /// Worktree repo at transition time, when the adapter reported one.
    #[serde(default)]
    pub repo: Option<String>,
}

/// Bounds for the ring — enforced on disk (segments) and in memory (ring).
///
/// Defaults: 4 segments x 256 events x 256 KiB ≈ 1024 retained events,
/// ≤ ~1 MiB on disk, with a 2 MiB absolute budget as a hard floor.
#[derive(Debug, Clone, Copy)]
pub struct RotationPolicy {
    /// Hard cap on retained events (memory ring and disk after pruning).
    pub max_events: usize,
    /// Rotate the active segment after this many events.
    pub max_events_per_segment: usize,
    /// Rotate the active segment after this many bytes.
    pub max_bytes_per_segment: u64,
    /// Keep at most this many segments (oldest pruned first).
    pub max_segments: usize,
    /// Absolute disk budget for all segments combined.
    pub max_total_bytes: u64,
    /// Rotate the active segment after this age even if it is small, so a
    /// quiet daemon's segments stay digest-friendly.
    pub max_segment_age: Duration,
}

impl Default for RotationPolicy {
    fn default() -> Self {
        Self {
            max_events: 1024,
            max_events_per_segment: 256,
            max_bytes_per_segment: 256 * 1024,
            max_segments: 4,
            max_total_bytes: 2 * 1024 * 1024,
            max_segment_age: Duration::from_secs(24 * 3600),
        }
    }
}

/// Pure rotation predicate: the active segment (opened at `start_ts`) must
/// rotate once any cap — event count, bytes, age — is hit.
pub fn should_rotate(
    policy: &RotationPolicy,
    events: usize,
    bytes: u64,
    start_ts: u64,
    now: u64,
) -> bool {
    events >= policy.max_events_per_segment
        || bytes >= policy.max_bytes_per_segment
        || now.saturating_sub(start_ts) >= policy.max_segment_age.as_millis() as u64
}

struct ActiveSegment {
    start_ts: u64,
    events: usize,
    bytes: u64,
    file: File,
}

#[derive(Default)]
struct RingInner {
    /// Insertion-ordered retained events (oldest first), mirroring the disk
    /// when persistent (last `max_events` disk events).
    events: VecDeque<HistoryEvent>,
    /// `None` = in-memory-only mode (no persistence).
    dir: Option<PathBuf>,
    policy: RotationPolicy,
    /// Number of events currently on disk (append +1, prune -dropped).
    disk_events: usize,
    next_segment_seq: u64,
    active: Option<ActiveSegment>,
    loaded: bool,
}

/// Cloneable handle into the ring.
#[derive(Clone)]
pub struct HistoryRing {
    inner: Arc<Mutex<RingInner>>,
}

impl HistoryRing {
    /// In-memory-only ring (tests, read-path defaults). Bounded by the
    /// policy, but never touches disk.
    pub fn in_memory(policy: RotationPolicy) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RingInner {
                policy,
                ..Default::default()
            })),
        }
    }

    /// Persistent ring over `dir`: existing segments are loaded (lazily, on
    /// the first push/query) and new events append to the newest segment.
    /// History is auxiliary — if the dir cannot be created the ring degrades
    /// to in-memory with a warning instead of taking the daemon down.
    pub fn open(dir: PathBuf, policy: RotationPolicy) -> Self {
        let usable = match fs::create_dir_all(&dir) {
            Ok(()) => true,
            Err(e) => {
                warn!(path = %dir.display(), error = %e, "history dir unusable; history degraded to in-memory");
                false
            }
        };
        Self {
            inner: Arc::new(Mutex::new(RingInner {
                dir: usable.then_some(dir),
                policy,
                ..Default::default()
            })),
        }
    }

    /// Append one event. Preserves insertion order. In-memory rings drop the
    /// oldest past `max_events`; persistent rings mirror the disk exactly
    /// (the last `max_events` disk events), so the ring and the segments
    /// always agree and restart views match. Persistence is best-effort: a
    /// failed append keeps the event in memory and logs (never blocks on
    /// anything but one page-cache write syscall).
    pub fn push(&self, event: HistoryEvent) {
        let mut inner = self.inner.lock().expect("history mutex poisoned");
        self.load(&mut inner);
        inner.events.push_back(event.clone());
        if inner.dir.is_some() {
            let (dropped, wrote) = self.append_segment(&mut inner, &event);
            inner.disk_events = inner.disk_events.saturating_sub(dropped);
            if wrote {
                inner.disk_events += 1;
            }
            while inner.events.len() > inner.disk_events {
                inner.events.pop_front();
            }
        } else {
            while inner.events.len() > inner.policy.max_events {
                inner.events.pop_front();
            }
        }
    }

    /// Retained events in insertion order (oldest first), optionally filtered
    /// to `ts >= since` and capped at `limit`.
    pub fn query(&self, since: Option<u64>, limit: Option<usize>) -> Vec<HistoryEvent> {
        let mut inner = self.inner.lock().expect("history mutex poisoned");
        self.load(&mut inner);
        inner
            .events
            .iter()
            .filter(|e| since.is_none_or(|s| e.ts >= s))
            .take(limit.unwrap_or(usize::MAX))
            .cloned()
            .collect()
    }

    /// All retained events in insertion order.
    pub fn events(&self) -> Vec<HistoryEvent> {
        self.query(None, None)
    }

    pub fn len(&self) -> usize {
        self.events().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn load(&self, inner: &mut RingInner) {
        if inner.loaded {
            return;
        }
        inner.loaded = true;
        let Some(dir) = inner.dir.clone() else {
            return;
        };
        let mut segments = match list_segments(&dir) {
            Ok(s) => s,
            Err(e) => {
                warn!(path = %dir.display(), error = %e, "history segment listing failed");
                return;
            }
        };
        segments.sort_by_key(|s| s.seq);
        inner.next_segment_seq = segments.last().map_or(0, |s| s.seq + 1);
        let mut loaded: Vec<HistoryEvent> = Vec::new();
        for seg in &segments {
            match read_segment(&seg.path) {
                Ok(events) => loaded.extend(events),
                Err(e) => {
                    warn!(path = %seg.path.display(), error = %e, "history segment read failed");
                }
            }
        }
        // Keep only the most recent max_events (segments may outlive the
        // in-memory bound if the policy was tightened).
        let skip = loaded.len().saturating_sub(inner.policy.max_events);
        inner.disk_events = loaded.len();
        inner.events = loaded.into_iter().skip(skip).collect();
        // Reopen the newest segment as active; the next push rotates it if it
        // already exceeded the caps (e.g. crash mid-burst).
        if let Some(newest) = segments.last() {
            match OpenOptions::new().append(true).open(&newest.path) {
                Ok(file) => {
                    inner.active = Some(ActiveSegment {
                        start_ts: newest.start_ts,
                        events: segment_event_count(&newest.path),
                        bytes: newest.size,
                        file,
                    });
                }
                Err(e) => {
                    warn!(path = %newest.path.display(), error = %e, "history active segment reopen failed");
                }
            }
        }
    }

    /// Append one event to the active segment, rotating/pruning as needed.
    /// Returns `(events pruned from disk, whether this event was written)` —
    /// the caller mirrors both into the in-memory ring.
    fn append_segment(&self, inner: &mut RingInner, event: &HistoryEvent) -> (usize, bool) {
        let now = now_millis();
        let rotate = inner
            .active
            .as_ref()
            .is_none_or(|a| should_rotate(&inner.policy, a.events, a.bytes, a.start_ts, now));
        if rotate {
            // Seal the active segment (it is just not appended to anymore).
            inner.active = None;
        }
        let mut dropped = 0usize;
        if inner.active.is_none() {
            let Some(dir) = &inner.dir else {
                return (0, false);
            };
            let seq = inner.next_segment_seq;
            inner.next_segment_seq += 1;
            let path = dir.join(format!("seg-{seq:06}-{now}.jsonl"));
            let file = match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => f,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "history segment create failed; event kept in memory only");
                    return (0, false);
                }
            };
            inner.active = Some(ActiveSegment {
                start_ts: now,
                events: 0,
                bytes: 0,
                file,
            });
            // Prune only after the new segment exists, so the on-disk count
            // including the active segment fits the policy.
            dropped = self.prune(inner);
        }
        let Some(active) = &mut inner.active else {
            return (dropped, false);
        };
        let Ok(line) = serde_json::to_string(event) else {
            warn!("history event serialization failed; kept in memory only");
            return (dropped, false);
        };
        let mut buf = line.into_bytes();
        buf.push(b'\n');
        if active.file.write_all(&buf).is_err() {
            warn!("history segment write failed; event kept in memory only");
            return (dropped, false);
        }
        active.events += 1;
        active.bytes += buf.len() as u64;
        (dropped, true)
    }

    /// Delete the oldest segments until the segment count and total bytes
    /// fit the policy. The active (newest) segment is never a candidate: it
    /// was just created or rotated to, so it sorts last. Returns how many
    /// events were removed from disk (the caller mirrors them out of the
    /// in-memory ring).
    fn prune(&self, inner: &mut RingInner) -> usize {
        let Some(dir) = inner.dir.clone() else {
            return 0;
        };
        let Ok(segments) = list_segments(&dir) else {
            return 0;
        };
        let mut candidates: Vec<SegmentInfo> = segments;
        candidates.sort_by_key(|s| s.seq);
        let mut dropped = 0usize;
        while candidates.len() > inner.policy.max_segments
            || candidates.iter().map(|s| s.size).sum::<u64>() > inner.policy.max_total_bytes
        {
            let oldest = candidates.remove(0);
            dropped += segment_event_count(&oldest.path);
            if let Err(e) = fs::remove_file(&oldest.path) {
                warn!(path = %oldest.path.display(), error = %e, "history segment prune failed");
                break;
            }
        }
        dropped
    }
}

#[derive(Debug, Clone)]
struct SegmentInfo {
    seq: u64,
    start_ts: u64,
    size: u64,
    path: PathBuf,
}

fn list_segments(dir: &Path) -> io::Result<Vec<SegmentInfo>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name
            .strip_prefix("seg-")
            .and_then(|r| r.strip_suffix(".jsonl"))
        else {
            continue;
        };
        let mut parts = rest.splitn(2, '-');
        let (Some(seq), Some(start_ts)) = (
            parts.next().and_then(|p| p.parse::<u64>().ok()),
            parts.next().and_then(|p| p.parse::<u64>().ok()),
        ) else {
            continue;
        };
        out.push(SegmentInfo {
            seq,
            start_ts,
            size: entry.metadata().map(|m| m.len()).unwrap_or(0),
            path: entry.path(),
        });
    }
    Ok(out)
}

fn read_segment(path: &Path) -> io::Result<Vec<HistoryEvent>> {
    let text = fs::read_to_string(path)?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<HistoryEvent>(line) {
            Ok(event) => out.push(event),
            Err(e) => {
                warn!(path = %path.display(), line = i + 1, error = %e, "history line skipped (torn append?)");
            }
        }
    }
    Ok(out)
}

fn segment_event_count(path: &Path) -> usize {
    fs::read_to_string(path)
        .map(|text| text.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}
