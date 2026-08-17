//! D23 + D33: persistent status-transition event history + daily digest.
//!
//! herdr-remote has no durable timeline (competitor review), so this ring is
//! our own design: a PERSISTENT, bounded record of status transitions
//! `{ts, pane_id, agent_id?, old_status, new_status, source, repo?}` — the
//! atomic unit of "what did each agent do". It survives daemon restarts,
//! drops the oldest events past a fixed budget, and feeds two consumers:
//! the live `GET /history` feed and the offline `corrald digest` CLI.
//!
//! ## Storage: rotating JSONL segments
//!
//! One line per event, append-only, under `$CORRAL_CONFIG_DIR/history/` as
//! `seg-<seq>-<start_ts>.jsonl`. The active segment rotates when it passes
//! the per-segment event/byte caps or its age cap (a quiet daemon must not
//! hold one stale segment forever); sealed segments are pruned oldest-first
//! so the ring stays within [`RotationPolicy`] (default: 4 segments x 256
//! events x 256 KiB ≈ 1024 events, ≤ ~1 MiB on disk, hard budget 2 MiB).
//! Writes are plain `File::write_all` on an unbuffered append handle — one
//! page-cache copy per event, no fsync, so ingestion is non-blocking at the
//! adapter's event-loop rate and events survive process restarts. A torn
//! trailing line (crash mid-write) is skipped on load.
//!
//! ## Ingest: one choke point
//!
//! [`crate::core::store::Store::apply`]'s `Change::Upsert` arm is where every
//! status change lands (all callers are the herdr adapter; the integrator
//! only merges workspace fields via `update_where`, never `state`). The store
//! compares the incoming record's state against the current one and pushes a
//! [`HistoryEvent`] only on an actual transition (old != new). Zero polling,
//! zero extra git calls. `Change::Remove` emits no event: removal is not a
//! status transition.
//!
//! ## Digest scheduling hook (D33)
//!
//! `corrald digest` is the cron/launchd artifact. It runs OFFLINE: it loads
//! the ring segments directly from the config dir (no daemon, no socket) and
//! prints a per-agent transition summary with blocked durations and work per
//! repo. `--since` is epoch millis (matching every `ts` in the ring).
//!
//! ```sh
//! # every morning 09:00, digest the previous 24h (cron)
//! 0 9 * * * corrald digest --since "$(( $(date +%s) * 1000 - 86400000 ))"
//!
//! # explicit window (epoch millis)
//! corrald digest --since 1784210400000
//!
//! # non-default config dir (must match the daemon's $CORRAL_CONFIG_DIR)
//! CORRAL_CONFIG_DIR=/var/lib/corral corrald digest --since 1784210400000
//! ```
//!
//! Blocked duration is a computed field: consecutive blocked -> non-blocked
//! pairs on the same pane. A span whose start predates the window (the first
//! in-window event for an agent is Blocked) is counted from the window start,
//! and a span still open at the window end is reported as "still blocked".

pub mod digest;
pub mod ring;

pub use digest::Digest;
pub use ring::{should_rotate, HistoryEvent, HistoryRing, RotationPolicy};
