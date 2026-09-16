# #554 round 3 — the boundary-stress pin: what it shows, and what it does NOT

Pin (added in round 3, `ios/FleetNotifierTests/PreflightRetryTests.swift`):

    LiveSessionTransportTests/testBoundaryJitterWithRetryNeverCreatesATaskOnARetiredTransport
    LiveSessionTransportTests/testBoundaryJitterWithoutRetryIsClean   (scoping control)

Both drive the reviewer's production route: jittered `.active`/`.background`
cycles interleaved with the window-reachable Settings ▸ Retry dispatch
(`retryHostConnection(secondary)`) over two pinned hosts, the boundary fired from
a mix of interleavings (immediately, after queue turns, and as its own queued
`MainActor` job), with a real `URLSession` task created on each foreground
session so its retirement is observable. The pin asserts: the process survives,
every foreground installed a session no earlier cycle had used, the retry
dispatches were observed, every retired session's task ends cancelled, stream
task accounting balances (`stopped >= started`), and no live session survives
the final boundary. The control runs the identical flapping with no retry.

## Heads

* fix commit (all gates, probes and the G8 re-run ran here): `1da21ba604ccc068bf3fe1e3ec7f2df81332835d`
* evidence commit (this record; `ios/` byte-identical to the fix head, `git diff --exit-code 1da21ba HEAD -- ios/` = 0):
  `ea97eb6` on `g554-live-session-preflight`, pushed.

## GREEN at the round-3 head

`1da21ba`: raw exit 0,
`LiveSessionTransportTests` 8/8, both stress tests pass with real traffic, with
the per-leg `ISSUE554_STRESS cycles=… retrying=… started=… stopped=…` counters
in the battery log (`/tmp/g554-probe-control.log`, one entry per battery leg) —
the counters are equal on every leg, i.e. no stream task is leaked by the
retirement.

## RED at `1bcdd24`: NOT reproduced — and that is the honest result

The round-2 reviewer reproduced the class at `1bcdd24` **by execution**
(`STRESS_HEAD_EXIT=65`, 2/2, two abort stacks from live-path code, base control
`42ca305` also 65, negative control without the retry 0 — see
`/tmp/rev554r2-stress.out` and `/tmp/rev554r2-stress-head.log`). Six of my own
recipes against the same head did NOT:

| # | pin sha256 (first 8) | shape | raw exit | `has been invalidated` |
| --- | --- | --- | --- | --- |
| 1 | `ad21eb91` | 120 cycles, sub-ms seed jitter (0-600 µs) | 0 | 0 |
| 2 | `26051d88` | 300 cycles, hold-open `/host-key` (ladder churn), sessions not retained → my own identity assertion | 65 (assertion, NOT the abort) | 0 |
| 3 | `f259c27` | 300 cycles — INVALID attempt (test target did not compile) | 65 (build) | 0 |
| 4 | `1b50915` | 300 cycles, hold-open `/host-key`, active host verifies, pull refresh mixed in | 0 | 0 |
| 5 | `a46f45f` | 400 cycles, mixed `Task.yield()`/tiny-sleep jitter | 0 | 0 |
| 6 | `f5e2c7f` | 120 cycles, boundary fired as its own queued `MainActor` job | 0 | 0 |
| 7 | `bb1f342` | 600 cycles, boundary sync AND queued, randomized pre-yields, retry on both hosts | 0 | 0 |

Logs: `/tmp/g554-r3-logs/legs-driver{,2,4,5,6}.out`, `baseonly-driver.out`; each
leg's full xcodebuild log is beside it (`g554-r3-basered.log`).

## Why this class has no deterministic RED at this seam

The failing creation is inside Foundation's own async machinery — the abort
stack is `URLSession.bytes(for:)` → `withTaskCancellationHandler`'s operation →
`-[__NSURLSessionLocal taskForClassInfo:]`, i.e. one await hop after the call —
and `Task.cancel()` is cooperative. To abort, the retirement must land in the
microsecond-scale window in which an owner is past its last guard and inside
that hop. Whether a given cycle lands there is decided by executor scheduling,
not by anything the test controls: firing the boundary synchronously, after
`sleep`s, or as a queued job all change the odds but do not make it
deterministic, and FIFO job ordering (the ladder's bounded child task is enqueued
by the ladder's own job) actively protects the common cases. Six recipes over
4 500+ cycles found no abort; the reviewer's 220-cycle probe did. Same code, same
route — the difference is scheduling luck, and I will not dress a lucky RED up as
a deterministic one.

## Consequence for the probe battery

`P6-no-repair` (round 2 — retire WITHOUT repairing the holder) was REMOVED from
`probes.py`, and this is itself a measured finding: the round-3 retirement
DEFERS `invalidateAndCancel()` until the pre-boundary owners terminate, so the
holder defect no longer reproduces as an abort — a post-boundary dispatch now
runs on a doomed-but-still-valid session and is cancelled by the drained
invalidation instead. A probe whose expected RED is "usually" would be a false
claim. The battery therefore carries P1-P5 (5 candidate defects, each RED in the
battery log with a byte-identical restore) plus both stress tests in EVERY leg
(8 selectors, `RESTORED_GREEN=8/8`).
