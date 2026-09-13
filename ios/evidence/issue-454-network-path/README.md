# #454 network-path retry hint — lane evidence

Implementation lane: `impl-454-path` on `prep454-path-contract` (adopted at
`9d914f78c2d352cabf49fe859cf9d47b66d9d50e`).

This directory holds the COMMITTABLE part of the lane's evidence (the runner
script + this index). The heavy, durable artifacts — raw logs, RED/GREEN
`.xcresult` bundles and exact source hashes — are retained at:

    /Users/jirathip/.config/fleet-operations/run/corral/evidence/issue-454/

## What was implemented

One lifecycle-owned, debounced network-path signal that ACCELERATES an
existing stale/disconnected retry wait. It is a retry hint only:

- `AppModel.swift` — `NetworkPathMonitoring` seam + `SystemNetworkPathMonitor`
  (thin `NWPathMonitor` adapter), started/stopped at the existing
  `startLive()`/`stopLive()` lifecycle seam; a trailing 500 ms debounce
  (injectable sleep, implementation parameter — not a physical guarantee);
  only a RESTORATION (satisfied after unsatisfied) is actionable, so repeated
  satisfied updates coalesce and never multiply attempts; the hint action
  reuses the existing per-host single owners (`FleetStore.reconnectIfNeeded`
  for stream hosts, the #451 preflight ladder re-drive — only while WAITING —
  for pinned-unverified hosts). It never marks heartbeat/connected.
- `HostStreamCoordinator.swift` — `hintPathRestored(profiles:)`: per-host
  guard reuse; `.mismatch` is terminal, `.verifying` re-drives only a
  WAITING ladder, `.verified`/`.unpinned` go through the store seam.
- `FleetNotifierTests.swift` — `NetworkPathHintTests` (11 deterministic
  runtime tests driving the REAL transport path through a scripted
  `URLProtocol` plus an injected path-signal double), observing per-URL
  attempt counts, stream ownership and lifecycle effects.

No changes to `FleetStore.swift` or `CorraldClient.swift` were needed: the
existing #425 `reconnectIfNeeded` seam and the #452 transport already provide
the single-owner replacement the hint drives.

## Runner

    bash ios/evidence/issue-454-network-path/run-454-tests.sh \
      <label> <result-bundle.xcresult> [FleetNotifierTests/Class ...]

Uses the lane's private simulator + private DerivedData and serializes on the
shared `/tmp/corral-heavy-gate.lock`. The raw `xcodebuild` exit status is the
gate status; logs are named per run in the durable evidence directory.

## Re-anchoring + portability (maintenance, #523)

`red-probe-454.py` and `pre-review-fixes-454.py` locate the code they swap by
exact string anchors, so they **fail closed** (exit 1, nothing written) when
`AppModel.startPathMonitor()` or the pre-review blocks legitimately change. The
GREEN anchor is the live `pathMonitor.start { … }` wiring **including its
generation guard**; when that wiring changes, re-derive the anchor from the
current `AppModel.swift` and keep the RED text a compiling stand-in for "no
path integration is wired" — a ~30-second edit, never a loosened fragment
(`red-probe-454.py` asserts exactly one match before splicing). The sibling
script's `F2_START_FIXED` anchor tracks the same wiring and moves with it.

Both scripts honour the same worktree override as the runner — the variable
name and default mirror `run-454-tests.sh`:

    IMPL454_WORKTREE=<checkout> python3 \
      ios/evidence/issue-454-network-path/red-probe-454.py check

The default remains the original lane worktree
(`/Users/jirathip/.herdr/worktrees/corral/prep454-path-contract`).

## RED/GREEN proof

`red-probe-454.py` (durable evidence directory) flips the lane back to the
EXACT pre-#454 behavior — the monitor is never wired, so no path signal can
reach the model — without touching the tests. The focused suite is run on
that tree (RED), the probe is reverted (sha256-verified byte restore), and
the same focused suite is run again (GREEN). See the lane report for the raw
logs and exits.

## Deliberately out of scope

- Final checker/manifest re-pins (`APPROVED_RELEASE_SOURCE_DIGEST` /
  `APPROVED_TEST_SOURCE_DIGEST` in `check-release-demo.py`) belong to the
  named post-#428 handoff — this lane did not edit the checker.
- Physical iPhone / Wi-Fi↔cellular / Tailscale / half-open / restart matrix
  (issue #454 AC6) requires hardware and remains OPEN.
- Measured recovery budgets are proposed, not approved; no p50/p95 claim is
  made from simulator fixtures.
