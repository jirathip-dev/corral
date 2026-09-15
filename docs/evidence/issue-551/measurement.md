# Issue 551 measurement scope and reproduction

Production baseline: `b6b69f377883d6b2507f8418b6b4dc34f7452145`.
The three production lifecycle/store/coordinator files and FleetViews.swift are
also byte-equivalent to the reported `8ff97a9b` at this baseline (`git diff
8ff97a9b b6b69f377883d6b2507f8418b6b4dc34f7452145 --` those four paths is empty).
No production fix should be inferred from the added regression tests.

## Simulator mechanism capture

Run from the worktree root:

```
python3 docs/evidence/issue-547/bounded-run.py g551-measure-driver 2800 \
  flock /tmp/n.lock python3 docs/evidence/issue-551/measure.py
python3 docs/evidence/issue-551/timings.py /tmp/g551-measure.log
```

The driver builds the real app/test host, adds `G551_MEASURE=1` to a disposable
xctestrun file in `/tmp/g551-dd/Build/Products`, and runs one opt-in measurement
method five times with `-test-repetition-relaunch-enabled YES`. Each repetition
has a fresh XCTest host process and fresh AppModel. It performs one unseeded
model start, then the actual production `.background` / `.active` seam with
30 and 300 seconds of real elapsed waiting, without changing app sources.
Every wait is checked against monotonic elapsed time. The overall driver has a
2800-second bound and the capture command a 2100-second bound. Five repetitions
produce exactly five samples per scenario, validated by the reducer.

Each model uses the same local URLProtocol URL and immediate response bodies;
there is no invented transport latency. Snapshot revisions 1, 2, 3 ensure each
scenario has a real first data frame and an observable row revision. Frame bytes
pass through the production SSE decoder, trust inbox, store and SwiftUI row hook.
No fake `row_visible` mark is injected by a test. The emitted `G551_SAMPLE` records
read the same ReconnectTiming stamps that the release-active os.Logger emits.
The reducer calculates per-sample intervals first, preserves negative path-ready
differences, then calculates medians; it never adds overlapping branches.

Important limitations:

- `cold_model` is a new AppModel in a fresh XCTest host, not the ordinary app's
  complete force-quit/reopen initialization. `start_uptime` is immediately before
  the model's production start/foreground call, not process-launch time.
- Warm waits exercise the production scene seam but the XCTest process is not
  suspended by iOS. This cannot test OS background networking or socket reuse.
- URLProtocol never reaches a Mac/Bazzite daemon or real sockets. In particular,
  it cannot implicate shared URLSession socket contention in the owner's report.
- The production retry attempts start at 1 inside each newly owned task; no
  inherited retry counter exists in AppModel or HostStreamCoordinator at base.
- A pending pinned host skips signed grants refresh. A separate regression holds
  an actually issued signed grants reply while the first SSE frame applies; it
  tests application ordering, not a real network connection pool.
- Mounted retained rows do not necessarily run onAppear or change revision on
  foreground. A missing `retained_row_visible` mark is not proof of blanking.
  A separate native-window capture verifies retained content, outside the timing
  sample path, to avoid adding screenshot latency to the stage measurements.

The ordinary suite skips the long measurement test unless explicitly opted in.
No fixture result can close the physical iPhone/loaded-host gate or establish
that the user's observed warm-return slowdown is fixed.

## Runtime invariants and injected defects

The existing `ForegroundReconnectTests` class now checks a real failed preflight
followed by background/foreground: both host owners must issue their next first
request in less than one second, rather than inherit the production three-second
retry wait. It also checks retained raw states on both hosts before verification,
independent verified replacement, native rendered retention, and a held signed
grants reply that cannot block first-frame application. The existing mismatch,
verification, lifecycle and epoch tests remain unchanged.

`probes.py` creates a new detached scratch worktree from the committed source,
runs a four-test GREEN control, injects one candidate defect at a time, demands
XCTest assertion RED (exit 65), restores bytes with SHA-256 and git-diff proof,
and finishes with the same pristine four-test GREEN battery. M1 preserves the
old active-host retry task and its wait over background. M2 deletes in-memory
rows at reconnect timing initialization. These are INJECTED candidate defects,
not evidence that either was present in the reported build. No production files
in the implementation worktree are modified by the probes.

The exact physical-device protocol and empty owner result tables are in
`device-protocol.md`. See `.report.md` for actual results, exits and non-claims.
