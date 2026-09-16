# #492: supervised local git and bounded diagnostics

Implementation source commit: `689139cae7fe4c20d0ddf8c9928ec9d5ae8049eb`.
Baseline: `92ac61d32068c882ff8e971a1470847717e12cd7`.
The accepted, report-only fence stop at `5cb23da2` remains in history;
Amendment 1 authorizes the implementation above it. No protected read-timing,
publication-test, HTTP-test, client, dependency, installer or gh-plane file changed.

## Behavior

* All state-cache locks recover poison with `into_inner()`. No logging runs
  inside the state-cache critical section. The WARN-subscriber panic test
  deliberately makes the writer panic and proves the cache remains unpoisoned.
* A `JoinSet` owns watcher, topology/discovery and paced-status loops plus all
  debounce/retry children. Any child panic is joined, ERROR-logged with its
  payload, and replaces the complete generation after a one-second backoff.
  The old generation is aborted and drained before replacement; pending flags
  and cached observations are reset so it re-probes, not merely restarts a timer.
* A one-second watchdog observes independent loop progress; no progress for
  120 seconds triggers the same recovery. Idle loop heartbeats distinguish
  quiet from stalled. Closing the sink stops the plane; no replacement is
  started against a closed receiver. The tests cover a held-permit silent stall
  and a healthy-idle control. This relies on Tokio scheduling/cooperative task
  cancellation; it is not a process watchdog for a wedged OS or runtime.
* Boot/recovery probes hydrate all known worktrees immediately through the same
  four permits. Periodic status probes are spaced by `max(60s / count, 50ms)`
  with at most four futures. Large fleets stretch rather than burst. Topology's
  10-second cadence and discovery's 15-minute cadence remain independent of
  status pacing. Permit wait is outside the 200ms execution budget; one permit
  covers status and the optional subject read, with no nested reacquisition.
* Each worktree emits at most one over-budget WARN per five minutes. Every
  over-budget probe still increments the shared skipped counter and preserves
  backlog behavior. Diagnostics state total queue-plus-probe latency explicitly.

## Additive schema-5 fields

All are encoded through the existing published snapshot, off the HTTP path.
The idle coalescer also refreshes health/age at its existing up-to-two-second
publication cadence. No fabricated agent revisions/deltas are emitted for
health-only updates. `/events` framing and `Corral-Epoch` are unchanged.

| Field | Meaning |
| --- | --- |
| `git_plane_alive` | A supervised generation is running, not stopped/failed/stalled. |
| `git_plane_last_event_age_ms` | Monotonic age since a successful probe or responsive loop iteration, including an idle heartbeat; null before any progress. This measures responsiveness, not newest Git content. |
| `git_plane_skipped` | Process-lifetime total over-budget probes, including WARN-suppressed skips. |
| `git_worktree_facts[path].fact_age_ms` | Monotonic age of the last complete successful observation for the canonical worktree path; null means unobserved or invalidated by recovery. |
| `git_worktree_facts[path].stale` | True if the plane is not alive, the path has no complete observation, or its observation age is at least 120,000ms. Unknown agent paths are included as absent/stale. |

Policy is KEEP historical values with explicit age/stale metadata, never invent
fresh zero counts. Age is as of `generated_at`, and can be up to two seconds old
when delivered. A client retaining a snapshot must continue aging the metadata
and regard it as stale at the 120-second bound; fetching `/snapshot` or receiving
a new SSE initial snapshot refreshes that metadata. Ordinary agent deltas do
not carry this new top-level map. Existing clients can ignore additive fields;
no iOS rendering change or phone-visible freshness badge is claimed here.

## First-panic evidence and logging policy

The retained Mac log contains an ENOSPC subscriber-write failure at line 50334,
then only the SECOND panic (`PoisonError` at the old `map_event_path` unwrap,
line 50339). It does not preserve the initial panicker's payload. Attribution
to a particular original panic is therefore NOT established. The known
failure boundary is removed: WARN formatting/writing cannot poison the cache,
poison is recoverable, and panicking children cannot silently disappear.

`src/bounded_log.rs` implements storage using only stdlib plus the existing
tracing writer trait (no added dependency). The logger now writes
`$CORRAL_CONFIG_DIR/corrald.log` and `corrald.log.1`, each capped at 1 MiB.
Synchronous writes rotate rather than refuse at the cap; there is no lossy
queue or priority-based drop. The panic hook uses the same writer. If file I/O
fails it emits a notice and the original bytes to stderr. If stderr also fails,
it aborts rather than silently claim successful diagnostic delivery. This is
bounded retained history, not unlimited historical retention. Emergency stderr
is deliberately not capped by this writer, so an I/O-failure incident remains
observable. Old launchd/systemd logging configuration is not changed or used
as a rotator, and its existing files are not removed/truncated by this lane.

The subprocess test fills the file to its bound twice, records an actual ERROR
and then really panics, and verifies both markers survive in the two files.
A separate test covers oversized writes and a poisoned logging mutex. Filling
the actual host filesystem or forcing both OS sinks to fail was not attempted.

## Reproduction and evidence

Use the original worktree and `CARGO_TARGET_DIR=/tmp/g492-target`. There is no
justfile at this head. Every heavy command is serialized with `flock /tmp/n.lock`;
`run-command.py` runs `df`, bounds the child lifetime, saves unfiltered output
under `/tmp/g492-*.log`, and records raw exit/time/command/cwd/target JSON.

* `gates.py verified`: exact workspace test, clippy, fmt, deny and release gates.
* `mutations.py`: three single-defect RED/GREEN probes, SHA-256 before/after and
  `git diff --exit-code HEAD -- <file>` after every byte restore.
* `mac-proof.py`: after full gates, preserve the fixed release binary, perform
  mutations, build the pinned baseline archive in a SEPARATE target directory,
  then measure both binaries. The first Mac cost attempt is REJECTED: sharing
  the release target aliased the fixed library into the old main (the supposed
  baseline exposed new fields with false liveness). Its exit-zero receipt and
  data are diagnostic only, not a valid comparison. `mac-cost-correction.py`
  preserves the original fixed binary, verifies its hash, uses the isolated
  baseline build, and repeats the comparison without replaying passed gates.
  The measurement driver now refuses an unexpected baseline/fixed wire schema;
  Linux likewise uses two independent target directories.
* `bazzite-proof.py`: build both verified source archives on Bazzite using the
  existing pinned 1.97.1 toolchain, run the real-loopback liveness/regression
  and logger tests, then repeat cost measurement. Run only after the Mac leg.
  Bazzite's tmpfs exhausted its user quota during the first target build;
  a second attempt could not even write its receipt. The final driver uses
  separate fixed/base targets and TMPDIR/runtime scratch under an owned
  home-cache directory, with only small required logs/receipts in `/tmp`.
  Only this lane's failed target was reclaimed; no shared artifact or
  worktree was removed. Earlier failed receipts remain as diagnostics.
* `measure.py`: private foreground daemon on an ephemeral loopback port;
  scratch HOME/config, a read-only Projects symlink and explicit live Herdr
  socket/worktree root. No gh executable/credentials, no `/drive` calls.
  Warm up for 15 seconds, then sample cumulative daemon CPU every five seconds
  for 120 seconds and use CPU deltas divided by monotonic wall-time deltas.
  This is NOT `ps %cpu` lifetime-average and excludes git child CPU. Count every
  newly written WARN/over-budget line across rotations. Capture real populated
  HTTP snapshots, save raw responses privately in the owned measurement
  directory (`/tmp` on Mac, home-cache on Bazzite), and check the
  same real watched-worktree inventory before/after/between legs. Only the
  aggregates, not agent prompts/auth/config/raw snapshot contents, are published.

The appended `.report.md` section supplies the measured results, gate table,
precise file references, full `tests/model.rs` diff and delivery qualifications.
The multi-day heap/RSS question, post-deploy dual-host soak and actual phone
surface remain owner gates. No live service deployment/restart, pruning,
GitHub-plane change, PR, merge, main/release promotion or TestFlight action.
