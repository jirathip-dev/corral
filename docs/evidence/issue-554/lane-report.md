# Issue 554 — lane report (`g554-live-session-preflight`)

Heads:

- `e1431422d5147eb89f0d592178a8cccd52b54ce4` — the production SOURCE commit (the
  per-live-session transport).
- `ee89f95a1355611c4df4d1af9349010cec76a0fb`, then
  `33c32887f5f3bf274109c0594c1596d8dd4a304d` — the #554 test revisions.
- `33c3288…` is the round-1 head every round-1 gate and probe below was run at.
  The whole `ios/FleetNotifier` source set is byte-identical to `e143142`
  (`git diff e143142 33c3288 -- ios/FleetNotifier` is empty); the later commits
  touch only `ios/FleetNotifierTests/PreflightRetryTests.swift` and
  `docs/evidence/issue-554/**` + this report.
- `42ca305a445fad5af0bf03384ebdcd178c727e18` — the round-1 head as REVIEWED:
  it returned FAIL on F1 (see the ROUND 2 section; it kept the whole round-1
  content above and added the evidence commit).
- `20533cd7d3c467d827f1d02381a5a43213520481` — **the round-2 head**: F1's fix
  (single retirement point + the two deferrable dispatch re-checks) and the
  missing class of test. Every round-2 gate, probe and measurement below was run
  at this head.

Note: `.report.md` is this repo's per-lane scratch report path (the file says so
itself, and earlier lanes replaced it). This lane replaced the previous content,
which was the #551/#552 integration merge record — it remains recoverable in git
(`git show 7301707:.report.md`).

Base: `integration` @ `97ed9e03cb4d265da02ac28bad1685000b159f80`.

## ROUND 3 — the window is the AWAIT HOP; the retirement now WAITS (`1da21ba`)

The round-2 review at `1bcdd24` FAILed: a task can still be created on an
invalidated `URLSession`. The reviewer executed it (not argued it): 220 jittered
`.active`/`.background` cycles plus `retryHostConnection(secondaryHost)` on two
pinned hosts → `STRESS_HEAD_EXIT=65`, 2/2, with TWO abort stacks from live-path
code —

    FleetStore.connect → CorraldClient.stream → URLSession.bytes(for:) → -[__NSURLSessionLocal taskForClassInfo:]
    CorraldClient.fetchHostKey(within:) → fetchHostKey() → URLSession.data(for:) → same

and controls: `42ca305` (the base) also exits 65 — this defect was never closed,
not reopened — and pure scene flapping with NO retry exits 0 (the trigger needs
a window-reachable dispatch: the Settings ▸ Retry route).

**Why round 2's guards could not close it.** The creation is not performed by
the caller: the throwing frames are `URLSession.bytes(for:)` →
`withTaskCancellationHandler`'s operation → `-[__NSURLSessionLocal
taskForClassInfo:]`, i.e. the task is created one await hop AFTER the call, on
another executor, and `Task.cancel()` is cooperative (a cancelled owner already
past its last guard still creates its task). A caller-side guard therefore can
never be atomic with the creation; the only closable point is "this owner is not
allowed to run at all". The reviewer named four sites, not the two round 2
described: `FleetStore.swift:838-839` (`streamTask = Task { await
client.stream(…) }`, whose reconnect loop re-creates a transport task every
rung), `CorraldClient.swift:200` (`bytes(for:)`), `CorraldClient.swift:126`
(`data(for:)`) and `HostStreamCoordinator.swift:505` (`openStream` after
`await fetchHostKey(within:)`, funnelling into the first).

**The fix.** The retirement WAITS:

- `AppModel.liveTransportOwners()` cancels and RETURNS every live-path owner
  whose body can still create a task on the live transport: the coordinator's
  owners (via `stopAll()`), the ACTIVE host's stream task (`fleet.disconnect()`
  already returned it — the handle used to be discarded), the ACTIVE host's
  preflight ladder, and the registered life-path tasks. `refreshFleet`'s two
  tasks are now registered in `lifecycleTasks` precisely so a boundary cancels
  AND waits for them.
- `HostStreamCoordinator.stopAll()` returns the owners it cancelled (per-host
  stream task, preflight ladder, refresh task).
- `stopLive()` repairs (round 2) and collects in the SAME synchronous step as
  the retirement decision — repair → collect → drain → invalidate, with the
  contract's persistence-first ordering untouched. Owner creation is
  MainActor-serialized, so the set is complete: an owner either already exists
  at that step (and is collected and awaited) or it resolves its transport
  after the repair (the base session, which is never invalidated).
- `AppModel.retireLiveTransport(_:owners:)` then waits (bounded, 2 s) for those
  owners to TERMINATE and only then calls `invalidateAndCancel()`. The wait is a
  latch + 25 ms poll because awaiting `Task.value` is not cancellable, so a
  deadline cannot race a hanging waiter; the bound is what keeps a wedged owner
  from hanging the lifecycle.
- `CorraldClient.swift` needed no edit and `FleetStore.swift`'s rung loop
  needed no re-check: the stream task itself is awaited, which is stronger.

**F2 — the pin, and what it does and does NOT show.** Added
`LiveSessionTransportTests.testBoundaryJitterWithRetryNeverCreatesATaskOnARetiredTransport`
(the reviewer's production route as a pin: jittered `.active`/`.background`
cycles, the Settings ▸ Retry dispatch on a pinned secondary host, the boundary
fired from a mix of interleavings, over two pinned hosts) plus
`testBoundaryJitterWithoutRetryIsClean` (the scoping control).

  * GREEN at the round-3 head: 8/8 `LiveSessionTransportTests`, raw exit 0.
  * **RED at `1bcdd24`: NOT REPRODUCED.** I ran five recipes against `1bcdd24`
    (120/300/400/600 cycles, sub-ms jitter, queue-turn interleavings, boundary
    sync and as a queued job, retries on one and both hosts) — every run exited
    0 with zero `has been invalidated` lines. The reviewer's probe is the RED
    evidence for this class at this head (and is quoted above); I am not
    claiming mine reproduced it. Consequence: this pin is a GREEN-side class
    control, not a base-RED pin, and round 2's `P6-no-repair` probe was removed
    from the battery for the same reason (the deferred invalidation masks the
    holder defect — disclosed in `probes.py` and in
    `docs/evidence/issue-554/boundary-stress-pin.md`).

### What changed (round 3, all in the fences the brief names)

| file | change |
| --- | --- |
| `ios/FleetNotifier/App/AppModel.swift` | `liveTransportOwners()` (cancel + collect every live-path owner); `waitForTermination(of:until:)` + `DrainLatch` (bounded drain, latch + 25 ms poll); `retireLiveTransport(_:owners:)` waits then invalidates; `stopLive()` repairs then collects in one synchronous step; `refreshFleet`'s two tasks registered in `lifecycleTasks` |
| `ios/FleetNotifier/Profiles/HostStreamCoordinator.swift` | `stopAll()` returns the owners it cancelled (`@discardableResult`) |
| `ios/FleetNotifierTests/PreflightRetryTests.swift` | the boundary-stress pin + its no-retry scoping control |
| `ios/check-release-demo.py` | `APPROVED_RELEASE_SOURCE_DIGEST` re-pinned from the tree |
| `docs/evidence/issue-554/{probes.py,gate-exits-round3.txt,boundary-stress-pin.md,f1-retirement-repair.md}` | battery (stress scenario added, P5 re-targeted, P6 removed with its measured reason), round-3 exits, pin record, F1 retraction + round-3 mechanism |

`CorraldClient.swift` and `FleetStore.swift` were NOT edited (the stream task
itself is awaited, which is stronger than a per-rung re-check). **Non-claims,
unchanged from round 2: no physical device, no TestFlight, no Tailscale, no
daemon, no CI run, no PR, no merge, no issue-close — none was performed, so none
is claimed.**

## Gates — ROUND 3 (head `1da21ba`, F1 closed, F2 pin added)

Driver: `/tmp/g554-r3-logs/ladder-driver.out`; raw exits also in
`docs/evidence/issue-554/gate-exits-round3.txt`.

| gate | command | raw exit / result |
| --- | --- | --- |
| G1 focused | `run-gates.sh` focused leg (#554 class + #451 ladders + scene seam/foreground) | `FOCUSED_EXIT=0` — 49 tests, 1 skipped, 0 failures; `LiveSessionTransportTests` 8/8, `ISSUE554_STRESS cycles=600 retrying=true started=2564 stopped=2564`, `cycles=60 retrying=false started=254 stopped=254` |
| G1b contract-item-4 pins | unchanged-behaviour pins | `PINS_EXIT=0` — 21 tests, 0 failures |
| G2 full suite | whole test target | `FULL_EXIT=0` — 647 tests, 1 skipped, 0 failures (round 2: 645; +2 stress tests) |
| G3 boundary + self-test | `check-release-demo.py` (+ `--self-test`) | `CHECK_RELEASE_EXIT=0`, `SELF_TEST_EXIT=0` |
| G4 builds + binary | Debug + Release + binary check | `DEBUG_EXIT=0`, `RELEASE_EXIT=0`, `BINARY_EXIT=0` |
| G5 project drift | xcodegen regen + diff | `XCODEGEN_EXIT=0`, `XCODEGEN_DIFF_EXIT=0` |
| G6 anti-slop (advisory) | head rules | `ASLOP_EXIT=1` — 24 findings, exactly as at base |
| G7 hygiene | `git diff --check` (worktree) | `DIFFCHECK_EXIT=0` |
| G6b anti-slop delta | base identity compare over touched files | `BASE_TOTAL=24 HEAD_TOTAL=24 ADDED={} GONE={} COUNT_CHANGED={}` → `ASLOP_COMPARE_EXIT=0` (delta ZERO) |
| G8 staged timings | #551 harness re-run (`measure`) | `MEASURE_EXIT=0`, `TIMINGS_EXIT=0` — 15 samples, every compared stage at or under the recorded baseline median |

## Probes — ROUND 3 (head `1da21ba`): 5/5 candidates RED + byte-identical restore

`docs/evidence/issue-554/run-probes.sh` (one `/tmp/n.lock` invocation, disposable
worktree at the head, every restore proven by sha256 before/after **and**
`git diff --exit-code`), raw driver output
`/tmp/g554-r3-logs/probes-driver2.out`, machine record
`docs/evidence/issue-554/probe-results-round3.json` (`source_head=1da21ba…`):

    PROBES=5 ASSERTION_RED=5 RESTORED_GREEN=8/8 RESTORE=BYTE_IDENTICAL   PROBES_EXIT=0

| probe | candidate defect | raw leg exit | assertion that bit | restore |
| --- | --- | --- | --- | --- |
| control | — (battery, 8 selectors) | 0 | GREEN (invalid control otherwise) | — |
| P1 shared-revert | pre-#554 behaviour: no per-live-session transport | 65 | `testForegroundCycleInstallsANewSessionAndInvalidatesTheRetiredOne` | `restore_diff_exit=0` |
| P2 no-invalidate | retire without killing the pool | 65 | `…InstallsANewSessionAndInvalidatesTheRetiredOne` + `testRapidPhaseFlapping…NoLeakedTasks` | `restore_diff_exit=0` |
| P3 slow-preflight | production preflight bound raised to 60 s | 65 | `testNeverRespondingHostKeyFailsWithinTheBoundAndEngagesTheLadder` | `restore_diff_exit=0` |
| P4 shrunk-stream-timeout | live session's stream timeout shortened to 5 s | 65 | `testLiveSessionKeepsTheStreamTimeoutAndTheLivenessBudget` | `restore_diff_exit=0` |
| P5 no-guard-on-a-completion-path | retired session's failure applied (no cancellation/identity check) | 65 | `testCompletionFromTheRetiredSessionIsNeverApplied` | `restore_diff_exit=0` |
| restored-green | — (pristine head again) | 0 | 8/8 GREEN | `BYTE_IDENTICAL` |

The battery carries the round-3 stress scenario in EVERY leg (8 selectors per
leg, `RESTORED_GREEN=8/8`), and `P6-no-repair` was removed with its measured
reason (see the disclosures above). The two `…boundary…` tests also ran in the G1
focused leg and the G2 full suite.

## ROUND 2 — F1, found by the independent reviewer, fixed here

### RETRACTED in round 3 (the claims in this section that did not hold)

Round 3's review FAILed at this section's head (`1bcdd24`, exit 65, 2/2 — see
the ROUND 3 section above), so these round-2 claims do NOT hold:

- "…the two dispatch sites whose task creation can be deferred past a boundary
  re-check the transport before dispatching" — the enumeration was too small
  (four sites, listed in ROUND 3) and, more importantly, guards of that shape
  are necessary but NOT sufficient: the creation happens one await hop after any
  caller-side check. The repair and the two guards ARE still in the tree and
  still load-bearing (the repair is what keeps post-boundary dispatches off the
  doomed session), but they did not close the class.
- "every guard it added was a COMPLETION-TIME identity check, so nothing ever
  CREATED a task on a retired session" — true of round 1; round 2's added test
  also never created one ACROSS a boundary (it dispatched only after retirement
  had completed), which is why it was green while the class was open.
- any reading of this section as "the F1 window is closed" — it was closed only
  by making the retirement wait (ROUND 3).

Round 1's head `42ca305` was reviewed by a separate reviewer lane and returned
FAIL on one defect. The reviewer executed it in-app rather than arguing it:
`PROBE2_EXIT=65`, `Terminating app due to uncaught exception 'NSGenericException'`,
with `liveSession_nil=true` and `mode=live`, against a surviving base control at
`97ed9e0` (`BASE_EXIT=0`). Everything else the review checked passed (design,
derived configuration, `waitsForConnectivity = false`, the injectable 5 s bound,
the ladder left intact, the completion-time identity guards, scope, gate set,
digest honesty). Round 1's own report had DISCLOSED this window as a gap and
chosen not to close it — the review is what established it was reachable and
fatal, so that judgement is recorded here as wrong, not as an oversight in
reporting.

- **Defect.** `stopLive()` cleared `liveSession` while `mode` stayed `.live`,
  then invalidated that session at the end of the teardown — but
  `HostStreamCoordinator` still had it installed (`private var urlSession`,
  adopted in `startLive`). A foreground-reachable path in that window (the
  reviewer's repro: Settings ▸ Retry on a coordinator host → retryHostConnection
  → `startSessionIfNeeded`) built its client from the dead session; creating a
  task on an invalidated `URLSession` raises an UNCATCHABLE `NSGenericException`
  and kills the process. The round-1 suite could not see it: every guard it added
  was a COMPLETION-TIME identity check, so nothing ever CREATED a task on a
  retired session.
- **Fix (round-2 head `20533cd`).** `AppModel.retireLiveTransport(_:)`
  (`AppModel.swift:2766`) is now the single retirement point — it repairs every
  holder (the coordinator's adopted transport is reset to the base session,
  which is never invalidated) and only then invalidates the retired transport;
  `stopLive()` calls it where it used to call `invalidateAndCancel()` directly
  (`:3002`), so the contract's ordering (persistence first, invalidation last)
  is untouched. `invalidateAndCancel` appears exactly once in production source
  (`rg -n "invalidateAndCancel" ios/FleetNotifier/` → one hit, inside that
  function). The two dispatch sites whose task creation can be deferred past a
  boundary re-check the transport before dispatching:
  `refreshFleet()`'s active refresh (`AppModel.swift:3233`) and
  `refreshAll(profiles:)`'s per-host task (`HostStreamCoordinator.swift:719`).
  The round-1 design (per-live-session transport, bounded preflight, the #425
  budget) is unchanged.
- **The missing class of test.** `LiveSessionTransportTests.testRetryDuringRetirementNeverDispatchesOnTheRetiredTransport`
  drives the reviewer's route through the production seam (startLive → both hosts
  live → background → Settings Retry on the coordinator host → foreground) and
  asserts task CREATION, not completion delivery. RED at `42ca305` with the
  pristine production source (`BASERED_EXIT=65`, `RAW_EXIT=65`, the abort raised
  in `URLSession.bytes(for:)` on the retired session, the pin named as the
  running test case); GREEN at the fix head (`HEADPIN_EXIT=0`, passed 0.548 s;
  in the G1 leg: passed 0.118 s). Probe P6 removes exactly the repair and turns
  that pin RED by abort again; the round-1 probes are kept in the same battery
  (the probe table below is the record of what was measured).
  Full record: `docs/evidence/issue-554/f1-retirement-repair.md`.

## What changed and why

Owner observation: background → return was sometimes MANY seconds, force-quit →
reopen was instant, and #551's mocked-transport simulator measurement implicated
no client stage — consistent with the cause living below the mock. The
owner-approved hypothesis to proceed on is a stale pooled connection surviving
the background on `URLSession.shared`. The fix makes every foreground start with
an EMPTY pool, exactly like a cold launch, and bounds the one request the warm
path starts with.

### `ios/FleetNotifier/App/AppModel.swift`

| Where | Change |
| --- | --- |
| `:473-507` | `session` is now documented as the BASE/injected session. Added `liveSession` (`private(set)`, `:489`, observable by the scene-seam tests), `liveTransport` (`liveSession ?? session`, `:495`) and the identity guard `isLiveTransport(_:)` (`:501`). |
| `:2737` | New `installLiveSession()`: derives the live session from the base session's configuration (same timeouts/headers/cookie/cache policy → the injected URLProtocol test stack rides along; request timeout stays 60 s), sets `waitsForConnectivity = false`, and is idempotent so a repeated `startLive()` inside one live session never replaces the session its in-flight requests belong to. |
| `:2749-2751` | `startLive()` installs the live session FIRST and adopts it in the coordinator, before any client is built. |
| `:2786` | The ACTIVE host's stream client rides `liveTransport`. |
| `:2915`, `:2971` | `stopLive()` retires `liveSession` (identity nil-ed) at the TOP so no late completion can be applied during teardown, and `invalidateAndCancel()`s the retired session at the END — after the coordinator cursor/metadata persistence, the ACTIVE cursor/metadata persistence and the verification reset. |
| `:1298`, `:1327`, `:1319/1327/1346` | The ACTIVE host's `/host-key` preflight ladder now rides `liveTransport`, uses the bounded attempt (`fetchHostKey(within:)`), and each of its three completion guards also checks `isLiveTransport(transport)`. |
| `:3132` | Path-hint reconnect (`fleet.reconnectIfNeeded`) rides `liveTransport`. |
| `:3191-3192`, `:3197`, `:3208` | Pull refresh: live transport + identity guard on both the success and failure completion (a retired session's cancelled refresh must not publish a `fleet_refresh` banner). |
| `:4052-4053`, `:4058`, `:4060` | Stale-agent reconciliation: live transport + identity guard on both completions. |

Deliberately NOT changed (documented decision, per the brief's "decide and document
which is which"): enrollment (`EnrollmentClient`/enrollment `DriveClient`),
host pairing (`prepareHostPairing`, `fetchHostKey(profileID:)`), device
registration / push-token upload / pending-token clear, grants refresh, and every
signed `DriveClient` read keep the injected BASE session exactly as today. Those
paths are not the warm-return first-request path, and moving them would change
signing/identity-boundary behaviour the contract requires to stay byte-for-byte
identical. `FleetStore` is untouched: its `connect()` already derives
`StreamLiveness` from the connecting client's session configuration, which is
why the live session must NOT shorten its inactivity timeout (see (d)).

### `ios/FleetNotifier/Profiles/HostStreamCoordinator.swift`

| Where | Change |
| --- | --- |
| `:187-194` | `HostPreflightRetryPolicy.attemptTimeout` (default **5 s**) — the bound on ONE `/host-key` attempt, injectable so a test drives it deterministically. |
| `:227-260` | New `CorraldClient.fetchHostKey(within:)`: runs one bounded attempt by cancelling it at the deadline. |
| `:335` | `urlSession` is now mutable (was `let`). |
| `:386` | New `adoptTransportSession(_:)` — AppModel hands the coordinator the live session's transport (the coordinator is built in `AppModel.init`, before any live session exists); a repeated adoption is a no-op. |
| `:453`, `:483`, `:490`, `:493`, `:513` | The per-host preflight ladder bounds each attempt and additionally guards every completion with `urlSession === client.session`, so a completion from a superseded transport can never set a host's posture or open its stream. |

### Why the bound is a ladder deadline and not a per-request timeout

`Network/CorraldClient.swift` is outside this lane's fence, so the request's own
`timeoutInterval` cannot be shortened at the request site. A session-level
`timeoutIntervalForRequest` cannot express it either: this lane MEASURED that a
request-level `timeoutInterval` wins over the session-level one, and
`FleetStore.connect` derives the SSE liveness budget from the connecting
client's session configuration — so a 5 s live-session timeout would silently
shrink `StreamLiveness` from 30 s to 2.5 s and change the #425 watchdog contract.
Measured facts and raw output: `docs/evidence/issue-554/session-probe.log`,
interpretation in `docs/evidence/issue-554/timeout-precedence.md`.

## Gates — ROUND 2 (head `20533cd`, F1 fixed)

Driver: `bash docs/evidence/issue-554/run-gates.sh all` (bounded, serialized on
`/tmp/n.lock`) → driver log `/tmp/g554-r2-logs/chain-driver.out`.

| Gate | Command | Raw exit |
| --- | --- | --- |
| G1 focused (incl. the NEW F1 pin) | `xcodebuild test … -only-testing:{LiveSessionTransportTests,PreflightRetryCoordinatorTests,PreflightRetryActiveHostTests,ScenePhaseLifecycleTests,ForegroundReconnectTests}` | `FOCUSED_EXIT=0` — `Executed 47 tests, with 1 test skipped and 0 failures`, `** TEST SUCCEEDED **` (`LiveSessionTransportTests` is now 6 tests: the 5 round-1 criteria + the F1 creation pin, `passed (0.118 seconds)`) |
| G1b contract-item-4 pins | `xcodebuild test … -only-testing:{HostKeyContinuityModelTests,EpochRecoveryTests,RecentOutputNotGrantedStateTests,GrantsRefreshTests,PerHostGrantsRefreshTests}` | `PINS_EXIT=0` |
| G2 full suite | `xcodebuild test … -only-testing:FleetNotifierTests` | `FULL_EXIT=0` — `Executed 645 tests, with 1 test skipped and 0 failures`, `** TEST SUCCEEDED **` (644 at round 1 + the new pin) |
| G3 boundary | `python3 ios/check-release-demo.py` | `CHECK_RELEASE_EXIT=0` (with the round-2 `APPROVED_RELEASE_SOURCE_DIGEST`, recomputed from the tree) |
| G3 self-test | `python3 ios/check-release-demo.py --self-test` | `SELF_TEST_EXIT=0` |
| G4 Debug build | `xcodebuild build -configuration Debug -destination 'generic/platform=iOS Simulator'` | `DEBUG_EXIT=0` |
| G4 Release build | `xcodebuild build -configuration Release -sdk iphonesimulator … CODE_SIGNING_ALLOWED=NO` | `RELEASE_EXIT=0` |
| G4 binary boundary | `python3 ios/check-release-demo.py --binary …/Release-iphonesimulator/FleetNotifier.app/FleetNotifier` | `BINARY_EXIT=0` |
| G5 xcodegen drift | `cd ios && xcodegen generate --spec project.yml` + `git diff --exit-code -- ios/` | `XCODEGEN_EXIT=0`, `XCODEGEN_DIFF_EXIT=0` (no new file: the F1 pin lives in the existing `PreflightRetryTests.swift`) |
| G6 anti-slop (head) | `swift run --package-path ios/tools/anti-slop-swift anti-slop ios/FleetNotifier ios/FleetNotifierTests` | `ASLOP_EXIT=1` — advisory baseline, exactly as at base |
| G6b anti-slop delta | `bash docs/evidence/issue-554/aslop-compare.sh` over base `97ed9e0` | `ASLOP_COMPARE_EXIT=0` — `BASE_TOTAL=24 HEAD_TOTAL=24`, `ADDED={}`, `GONE={}`, `COUNT_CHANGED={}` (delta 0) |
| G7 hygiene | `git diff --check` — the WORKTREE check the gate suite runs (`git diff --check` over a RANGE, e.g. `42ca305..HEAD`, exits 2 and is not this gate) | `DIFFCHECK_EXIT=0` |
| G8 staged timings | `run-gates.sh measure` (#551 harness, 5 repetitions) | `MEASURE_EXIT=0`, `TIMINGS_EXIT=0` — 15 samples, exactly `{cold_model: 5, warm_30: 5, warm_300: 5}`; reduced output `docs/evidence/issue-554/timings-round2.json` |

Raw driver output is also transcribed to
`docs/evidence/issue-554/gate-exits-round2.txt`; the round-1 transcription
(`gate-exits.txt`) is left byte-unchanged as the round-1 record.

## Gates — ROUND 1 (head `33c3288`, superseded: F1 was found in this head)

Driver: `bash docs/evidence/issue-554/run-gates.sh all`
(`bounded-run.py g554-chain 5400 …`), extracted to
`docs/evidence/issue-554/gate-exits.txt`.

| Gate | Command | Raw exit |
| --- | --- | --- |
| G1 focused | `xcodebuild test … -only-testing:{LiveSessionTransportTests,PreflightRetryCoordinatorTests,PreflightRetryActiveHostTests,ScenePhaseLifecycleTests,ForegroundReconnectTests}` | `FOCUSED_EXIT=0` — `Executed 46 tests, with 1 test skipped and 0 failures`, `** TEST SUCCEEDED **` (LiveSessionTransportTests 5/5, PreflightRetryCoordinatorTests 9/9, PreflightRetryActiveHostTests 6/6, ScenePhaseLifecycleTests 7/7, ForegroundReconnectTests 19 with 1 skipped) |
| G1b item-4 pins | `xcodebuild test … -only-testing:{HostKeyContinuityModelTests,EpochRecoveryTests,RecentOutputNotGrantedStateTests,GrantsRefreshTests,PerHostGrantsRefreshTests}` | `PINS_EXIT=0` — `Executed 21 tests, with 0 failures`, `** TEST SUCCEEDED **` |
| G2 full suite | `xcodebuild test … -only-testing:FleetNotifierTests` | `FULL_EXIT=0` — `Executed 644 tests, with 1 test skipped and 0 failures`, `** TEST SUCCEEDED **` |
| G3 boundary | `python3 ios/check-release-demo.py` | `CHECK_RELEASE_EXIT=0` (`release-demo check: PASS`) |
| G3 self-test | `python3 ios/check-release-demo.py --self-test` | `SELF_TEST_EXIT=0` |
| G4 Debug build | `xcodebuild build -configuration Debug -destination 'generic/platform=iOS Simulator'` | `DEBUG_EXIT=0` |
| G4 Release build | `xcodebuild build -configuration Release -sdk iphonesimulator … CODE_SIGNING_ALLOWED=NO` | `RELEASE_EXIT=0` at this head. An EARLIER chain attempt on this disk-starved host failed the same leg for space only (`lipo: can't write to output file … .dSYM/…/FleetNotifier.lipo (No space left on device)`, host at 116 MiB free, `RELEASE_EXIT=65`); it was re-run standalone with the identical command: `g554-release_EXIT=0`, `** BUILD SUCCEEDED **` |
| G4 binary boundary | `python3 ios/check-release-demo.py --binary …/Release-iphonesimulator/FleetNotifier.app/FleetNotifier` | `BINARY_EXIT=0` (`inspected …`) |
| G5 xcodegen drift | `cd ios && xcodegen generate --spec project.yml` + `git diff --exit-code -- ios/` | `XCODEGEN_EXIT=0`, `XCODEGEN_DIFF_EXIT=0` (re-run after the final test commit: the pre-commit chain leg saw the lane's own uncommitted test diff and returned 1) |
| G6 anti-slop (head) | `swift run --package-path ios/tools/anti-slop-swift anti-slop ios/FleetNotifier ios/FleetNotifierTests` | `ASLOP_EXIT=1` (advisory baseline, as at base; the claim is the delta below) |
| G6b anti-slop delta | `bash docs/evidence/issue-554/aslop-compare.sh` over base `97ed9e0` | `ASLOP_COMPARE_EXIT=0` — `BASE_TOTAL=24 HEAD_TOTAL=24`, `ADDED={}`, `GONE={}`, `COUNT_CHANGED={}`; `aslop-base-rules.txt` and `aslop-head-rules.txt` are byte-identical (`diff` exit 0) |
| G7 hygiene | `git diff --check` — the WORKTREE check the gate suite runs (`git diff --check` over a RANGE, e.g. `42ca305..HEAD`, exits 2 and is not this gate) | `DIFFCHECK_EXIT=0` |
| G8 simulator timings | `run-gates.sh measure` (#551 harness, 5 repetitions) | `MEASURE_EXIT=0`, `TIMINGS_EXIT=0` (run at `e143142`; the measured method `FleetNotifierTests.swift` and every app source are byte-identical at `ee89f95` — see “Simulator evidence”) |

Disk was checked before each heavy leg and each build tree was deleted between
legs (`DF_*` lines in the driver log). The host ran as low as 116 MiB free during
this lane (the G4 ENOSPC above); `run-gates.sh` therefore also accepts `g4`, `g6`
and `aslop` selectors so a disk-starved leg can be re-run alone without
re-running the whole chain.

## Probes — ROUND 2 (head `20533cd`): 6/6, each with raw exit + byte-identical restore

**RETRACTED in round 3:** this green battery did NOT prove the class closed —
the round-3 reviewer's boundary probe aborted at the same head this table
reports (exit 65, 2/2; see the ROUND 3 section). A battery full of greens is
evidence that the tests BITE for the defects it mutates, not evidence that no
defect remains, and round 2 read it as the latter. `P6-no-repair` (below) was
removed from the battery in round 3 with its measured reason (`probes.py`).

Driver: `bash docs/evidence/issue-554/run-probes.sh` at head `20533cd`.
The battery now carries the F1 scenario (the new pin) in every leg and adds
**P6**, whose defect is exactly the round-1 behaviour the reviewer found
(remove the repair from `retireLiveTransport`, keep the invalidation). A leg
whose candidate defect ABORTS the test process (P6) is a non-zero RED with the
pin named as the running test case, not an assertion-failure line.

| Probe | Mutation | RED | Restore |
| --- | --- | --- | --- |
| control | none (pristine battery of 6) | `exit=0`, `Executed 6 tests, with 0 failures` | — |
| P1 | no per-live-session session at all (the pre-#554 production behaviour) | (a) `testForegroundCycleInstallsANewSessionAndInvalidatesTheRetiredOne` | byte-identical |
| P2 | never `invalidateAndCancel()` the retired session | (a) + (e) `testRapidPhaseFlappingLeavesOneLiveSessionAndNoLeakedTasks` | byte-identical |
| P3 | `HostPreflightRetryPolicy.attemptTimeout` 5 s → 60 s | (c) `testNeverRespondingHostKeyFailsWithinTheBoundAndEngagesTheLadder` | byte-identical |
| P4 | shrink the live session's `timeoutIntervalForRequest` to 5 s | (d) `testLiveSessionKeepsTheStreamTimeoutAndTheLivenessBudget` | byte-identical |
| P5 | drop the live-session identity guard on the refresh completion path | (b) `testCompletionFromTheRetiredSessionIsNeverApplied` | byte-identical |
| P6 | retire the transport WITHOUT repairing the installed holder (round-1 behaviour) | (F1) `testRetryDuringRetirementNeverDispatchesOnTheRetiredTransport` — RED **by abort** (`NSGenericException: Task created in a session that has been invalidated`) | byte-identical |
| restored-green | pristine battery again | `exit=0`, `Executed 6 tests, with 0 failures` | `git diff --exit-code` clean |

Machine-readable: `docs/evidence/issue-554/probe-results-round2.json`
(`source_head` = `20533cd`). Raw leg logs: `/tmp/g554-probe-*.log`.
Driver summary line (`PROBES_EXIT=0`):

    PROBES=6 ASSERTION_RED=6 RESTORED_GREEN=6/6 RESTORE=BYTE_IDENTICAL

P6's RED is an ABORT rather than an assertion-failure stop, and the driver's
witness for it is the named test case plus the exception line in the leg log:
`RAW_EXIT=65`, `Test Case '…testRetryDuringRetirementNeverDispatchesOnTheRetiredTransport' started`,
`error: -[…] : Task created in a session that has been invalidated (NSGenericException)`,
`*** Terminating app due to uncaught exception 'NSGenericException'`. Control and
restored-green both report `Executed 6 tests, with 0 failures`.

## Probes — ROUND 1 (head `33c3288`) — superseded record

Driver: `bash docs/evidence/issue-554/run-probes.sh` at head `33c3288` →
`PROBES=5 ASSERTION_RED=5 RESTORED_GREEN=5/5 RESTORE=BYTE_IDENTICAL`, `PROBES_EXIT=0`.
Each mutation is a CANDIDATE defect applied in a disposable detached worktree
(`/tmp/g554-probe-tree`, removed afterwards), never on this branch. Per probe the
driver asserts the leg's exit is non-zero, that the NAMED test is in the failure
list, and that `shasum -a 256` before == after plus `git diff --exit-code` is empty.
Machine-readable: `docs/evidence/issue-554/probe-results.json` (`source_head` = `33c3288`).

| Probe | Mutation | RED | Restore |
| --- | --- | --- | --- |
| control | none (pristine battery) | `exit=0`, `Executed 5 tests, with 0 failures` | — |
| P1 | no per-live-session session at all (the pre-#554 production behaviour: every client rides the base/shared session, nothing is invalidated) | (a) `testForegroundCycleInstallsANewSessionAndInvalidatesTheRetiredOne` | byte-identical |
| P2 | never `invalidateAndCancel()` the retired session | (a) + (e) `testRapidPhaseFlappingLeavesOneLiveSessionAndNoLeakedTasks` | byte-identical |
| P3 | `HostPreflightRetryPolicy.attemptTimeout` 5 s → 60 s | (c) `testNeverRespondingHostKeyFailsWithinTheBoundAndEngagesTheLadder` | byte-identical |
| P4 | shrink the live session's `timeoutIntervalForRequest` to 5 s | (d) `testLiveSessionKeepsTheStreamTimeoutAndTheLivenessBudget` | byte-identical |
| P5 | drop the live-session identity guard on the refresh completion path | (b) `testCompletionFromTheRetiredSessionIsNeverApplied` | byte-identical |
| restored-green | pristine battery again | `exit=0`, `Executed 5 tests, with 0 failures` | `git diff --exit-code` clean |

The probe battery erases the lane's OWN simulator before every leg
(`probes.py:reset_simulator`) because this host ran at 95–100% disk and a leg
that runs out of space silently truncates its own log — which is exactly how the
earlier battery attempts failed. Two earlier attempts at this head are part of
the record: one aborted on `Refusing to reuse scratch` (a leftover probe tree),
one was killed during a disk-full leg. Neither is counted as evidence.

## Seam: how each acceptance criterion was OBSERVED

All five drive the production scene seam (`handleScenePhaseChange` →
`startLive`/`stopLive`) in `FleetNotifierTests/LiveSessionTransportTests`
(`PreflightRetryTests.swift:952`). No assertion reads source text.

- **(a) identity differs + the previous session is invalidated** —
  `testForegroundCycleInstallsANewSessionAndInvalidatesTheRetiredOne` (`:1037`).
  After `startLive()` the test holds the live session, confirms its own
  `/host-key` preflight is hanging (`deliveredCount == 0`) and starts its own
  `URLProtocol` request on that session; then `.background`/`.active`; then it
  asserts the new `model.liveSession` is a DIFFERENT object, that its own task on
  the retired session completed with `NSURLErrorDomain`/`NSURLErrorCancelled
  (-999)` — i.e. the model really called `invalidateAndCancel()` on that exact
  session (host probe 3 in `session-probe.log` establishes what that completion
  means) — and that the foreground session's OWN preflight is answered on the
  derived transport (`deliveredCount == 1`) and verifies.
- **(b) a completion from the old session is dropped** —
  `testCompletionFromTheRetiredSessionIsNeverApplied` (`:1083`). A pull refresh
  is left in flight on session #1 (transport holds it open), the boundary is
  crossed, and after the refresh task settles the test asserts state was NOT
  touched: no `fleet_refresh` banner, `keyContinuityState == .pending` (session #2
  must re-verify; nothing carried over), store not `.connected`, no agents.
  P5 shows this assertion is load-bearing for the identity guard.
- **(c) never-responding `/host-key` fails within the bound and the ladder
  engages** — `testNeverRespondingHostKeyFailsWithinTheBoundAndEngagesTheLadder`
  (`:1132`). Injectable bound 0.3 s; the transport records the attempt's
  tear-down; the test asserts `HostPreflightRetryPolicy.default.attemptTimeout <= 5`,
  that the first attempt failed at `+0.324 s` (bound + 0.5 s slack) and that the
  #451 ladder retried ≥ 3 attempts with the key state still `.pending` and the
  store not `.connected`. The log records the measurement against the constant:
  `ISSUE554_PREFLIGHT bound=0.3 first_failure_after=0.3243969678878784 attempts=3`.
- **(d) the stream timeout is unchanged** —
  `testLiveSessionKeepsTheStreamTimeoutAndTheLivenessBudget` (`:1168`). The live
  session inherits the base session's `timeoutIntervalForRequest` (60 s, equal to
  the injected session's), has `waitsForConnectivity == false`, keeps the injected
  `URLProtocol` stack, and the store's derived `StreamLiveness` is exactly
  `inactivityBudget 30` / `tickInterval 5` (`StreamLiveness(transportTimeout: 60)`
  pinned too). P4 shows the assertion bites if the live session's timeout shrinks.
- **(e) flapping leaves exactly ONE live session and ZERO leaked tasks** —
  `testRapidPhaseFlappingLeavesOneLiveSessionAndNoLeakedTasks` (`:1200`). Four
  background→foreground cycles each capture the live session and start a probe
  request on it, letting each session dispatch real preflight/stream work; all
  five observed sessions are pairwise distinct; `startLive()` ×2 inside one live
  session reuses it; after the final boundary `liveSession == nil`, every retired
  session's probe failed cancelled, and no dispatched transport request is left
  unmatched: `ISSUE554_TASKS started=12 stopped=14`. The leak invariant is
  deliberately `stopped >= started` (plus per-URL for the probe requests) rather
  than strict equality: URLSession also tears down a task that was cancelled
  before its transport started — measured at this head, and the reason an
  earlier strict-equality version of this assertion was flaky (it failed once,
  12 vs 14, and passes now; P2 still goes RED under it).
- **(F1, round 2) task CREATION on a retired transport** —
  `testRetryDuringRetirementNeverDispatchesOnTheRetiredTransport` (`:1300`).
  Both hosts live on the adopted transport, a real task in flight on the live
  session, then the retirement boundary (asserted: `liveSession == nil`,
  `mode == .live`), then the Settings ▸ Retry route on the coordinator host. It
  asserts the retry's stream attempt AND its bounded `/host-key` attempt both
  dispatch and the preflight is answered (so the route is not silently a no-op),
  that no live session is invented, and that the next foreground owns the only
  live transport a later live-path dispatch (`refreshFleet()`) rides. At
  `42ca305` the same pin aborts the process instead of reaching any of this
  (`BASERED_EXIT=65`, `NSGenericException` from `URLSession.bytes(for:)` on the
  retired session). P6 reproduces that RED by removing the repair.

## Contract item 4: byte-for-byte unchanged behaviour, SHOWN

- The named pin suites are green at this head (G1b, 21 tests, raw exit 0): host
  pinning and `.mismatch` terminality (`HostKeyContinuityModelTests`), #450 epoch
  rules (`EpochRecoveryTests`), `read_tail` grants (`RecentOutputNotGrantedStateTests`,
  `GrantsRefreshTests`, `PerHostGrantsRefreshTests`), and #547 verify-before-apply
  plus #551 last-known retention (`ForegroundReconnectTests`, G1, 19 tests with 1
  skipped — the skip is the opt-in measurement method).
- The diff touches no code path on those behaviours: the changes are the live
  transport plumbing plus guards listed above; `FleetStore.swift` is not modified
  at all, and the host-pinning / mismatch / epoch / verify-before-apply / retention
  helpers are untouched.
- The whole suite is green (G2, 644 tests) rather than a selection.

## Simulator evidence

See `docs/evidence/issue-554/measurement.md`; the #551 harness method
(`testWarmReturnStageMeasurements`, `G551_MEASURE=1`, 5 fresh processes with real
30 s/300 s scene-seam waits) was re-run **unchanged**
(`bash docs/evidence/issue-554/run-gates.sh measure`, raw exit `MEASURE_EXIT=0`,
`TIMINGS_EXIT=0`). The leg ran at `e143142` and remains valid at `ee89f95`: the
measured method lives in `ios/FleetNotifierTests/FleetNotifierTests.swift` and the
whole `ios/FleetNotifier` source set is byte-identical between those heads
(`git diff e143142 ee89f95 -- ios/FleetNotifier ios/FleetNotifierTests/FleetNotifierTests.swift`
is empty; only `PreflightRetryTests.swift` + docs were added). Staged timings are
still recorded — 15 samples, exactly
`{cold_model: 5, warm_30: 5, warm_300: 5}` — and all raw monotonic marks are in
`/tmp/g554-measure.log`; reduced output: `docs/evidence/issue-554/timings.json`.

### ROUND 2 re-run of the same harness (head `20533cd`)

Same method, same 5-repetition shape, re-run at the round-2 head
(`MEASURE_EXIT=0`, `TIMINGS_EXIT=0`, 15 samples = `{cold_model: 5, warm_30: 5,
warm_300: 5}`, raw marks `/tmp/g554-measure.log`, reduced
`docs/evidence/issue-554/timings-round2.json`). Medians, round-1 head vs round-2
head (ms; n=5 each, one host, different load windows — no significance is
claimed from 5 samples):

| Metric | cold r1 → r2 | warm_30 r1 → r2 | warm_300 r2 |
| --- | --- | --- | --- |
| `seam_entry_to_apply` | 42.087 → **41.685** | 8.450 → 9.922 | 8.972 |
| `apply_to_row` | 21.873 → **25.714** | — | 3.507 |
| `key_rtt` | 2.603 → 2.693 | 0.886 → 1.023 | 1.097 |
| `path_to_apply` | 2.363 → 2.246 | 4.619 → 5.510 | 4.809 |
| `path_to_sse_200` | — | 0.476 → 0.513 | 0.569 |
| `frame_to_apply` | — | 3.926 → 4.606 | 3.852 |
| `key_response_to_apply` | — | 4.000 → 4.688 | 3.934 |
| `sse_200_to_frame` | — | — | 0.250 |

So: staged timings are recorded at the round-2 head, in the same envelope as the
round-1 head (largest movement `apply_to_row` +3.8 ms on a 5-sample median; the
round-1 lane reported ±sub-ms to ±4 ms movements of the same size between its own
runs and the #551 baseline), and nothing here is a cold-launch regression of the
kind the AC names. The harness's own non-claims are unchanged: a local
`URLProtocol` never reaches a socket pool, so it can neither reproduce nor
falsify the stale-pool warm return.

Cold-launch non-regression (head median vs the #551 lane's median for the same
method, transcribed with provenance in
`docs/evidence/issue-554/baseline-551-medians.json`):

| Cold metric | #551 baseline | #554 head | delta |
| --- | ---: | ---: | ---: |
| `seam_entry_to_apply` | 111.149 ms | **42.087 ms** | −69.062 |
| `apply_to_row` | 48.948 ms | **21.873 ms** | −27.075 |
| `key_rtt` | 5.656 ms | **2.603 ms** | −3.053 |
| `path_to_apply` | 5.015 ms | **2.363 ms** | −2.652 |

Warm staging is unchanged (warm_30 medians, head vs baseline): `key_rtt`
0.886/0.928, `path_to_sse_200` 0.476/0.514, `frame_to_apply` 3.926/3.779,
`key_response_to_apply` 4.000/3.840, `path_to_apply` 4.619/4.542,
`seam_entry_to_apply` 8.450/8.338 ms — i.e. sub-millisecond movements, no
regression, and (as the harness's own non-claims state) no claim at all about the
stale-pool warm return, which a local `URLProtocol` cannot exercise.

## What this lane did NOT exercise (non-claims)

- No physical iPhone, no TestFlight build, no daemon deploy, no Tailscale path,
  no device claim. The owner gate is unexecuted with EMPTY tables:
  `docs/evidence/issue-554/device-protocol.md`.
- No CI run and no PR: this lane pushed the branch only (no PR, no merge, no
  release action, no issue mutation).
- The harness that re-ran the staged timings uses a local `URLProtocol`: it never
  reaches a socket pool, so it can neither reproduce nor falsify the stale-pool
  warm return. The stale-pool hypothesis remains a hypothesis.
- The per-live-session session removes the *pool carry-over*; it cannot make a
  dead network path fast, which is why the preflight bound exists. No claim is
  made about daemon-side latency.
- No claim that the identity guard is the ONLY guard on a completion path: the
  pre-existing cancellation checks (`Task.isCancelled`, `FleetStore` connection
  generations, coordinator session identity) also stop late work; the identity
  guard is the session-scoped guard the contract asked for, and (b)/P5 show it is
  load-bearing on the refresh completion path.
- Contract term “per-request timeout” for `/host-key` is implemented as a bounded
  per-attempt deadline in the ladder, not as a `URLRequest.timeoutInterval`
  change: `Network/CorraldClient.swift` is fenced out of this lane, and the
  host measurements above show a session-level timeout could not express it
  without changing the SSE liveness budget. This is a disclosed deviation from
  the literal wording, not from the acceptance criterion.
- Anti-slop is advisory: `ASLOP_EXIT=1` at both base and head (24 findings each);
  the claim is the empty delta, not a clean run.
- **The two deferrable-dispatch re-checks are not separately pinned.** The
  windows they close — the body of an already-created task being scheduled after
  the retirement — cannot be interleaved deterministically through the test seam
  (the body is enqueued on the main actor, the boundary arrives on the main
  thread). What IS pinned is the route the reviewer executed (Settings ▸ Retry →
  `startSessionIfNeeded`, which is the mechanism the defect was proved with) and
  the invariant that `invalidateAndCancel` has exactly one caller, which repairs
  the holder first. The re-checks are the same class closed at the two sites that
  can defer.
- **No device, no CI, no PR confirmation.** Nothing in this report is a device,
  TestFlight, Tailscale, daemon, CI or PR result; the owner device gate remains
  unexecuted with empty tables.

## Disclosed gaps a reviewer should weigh

- **ROUND 3 disclosures (the round-3 review FAILed at `1bcdd24`).**
  * The closure claim below ("no window remains") was FALSE. The reviewer
    reproduced the abort at `1bcdd24` by execution (exit 65, 2/2, two live-path
    stacks); see the ROUND 3 section for the mechanism and the fix (the
    retirement now cancels AND WAITS for its owners before invalidating).
  * **The round-3 stress pin is NOT a base-RED pin.** Six of my own recipes
    against `1bcdd24` exited 0 (five shapes, up to 600 jittered cycles, raw exits
    tabulated in `docs/evidence/issue-554/boundary-stress-pin.md`) — I did not
    reproduce the reviewer's abort, and I am not claiming I did. The RED for this
    class at that head is the reviewer's executed probe, quoted in the ROUND 3
    section. My pin is a GREEN-side class control: it drives the same production
    route at the fix head without aborting and asserts the class invariants.
  * **The battery changed for a measured reason.** `P6-no-repair` was removed
    (the round-3 deferred invalidation masks the holder defect — a post-boundary
    dispatch now runs on a doomed-but-still-valid session and is cancelled by the
    drained invalidation, so the ancient probe would have been a "usually RED"
    claim), and `P5` was re-targeted (registering the pull refresh as a life-path
    owner makes CANCELLATION the discriminator at the boundary, so the probe now
    removes the cancellation and the identity checks together: verified RED,
    exit 65, exact assertion, before the battery was re-run). Both are stated in
    `probes.py`'s docstring.
  * **The drain is bounded (2 s), and that is the residual case.** A task wedged
    longer than the bound still gets its transport invalidated under it; the
    bound exists so a wedged owner cannot hang the whole foreground lifecycle.
- **Coordinator transport after the boundary — RETRACTED by round 2 (see the
  ROUND 2 section above).** Round 1 recorded here that "no production path
  reaches a coordinator client in that window" and therefore left the
  invalidation without a repair. That claim was FALSE, and the independent
  reviewer disproved it by EXECUTING the path rather than arguing about it:
  Settings ▸ Retry on a coordinator host in exactly that window created a task on
  the invalidated session and killed the process (`PROBE2_EXIT=65`,
  `Terminating app due to uncaught exception 'NSGenericException'`,
  `liveSession_nil=true`, `mode=live`). F1 is fixed at `20533cd`:
  `retireLiveTransport(_:)` repairs every holder in the same step as the
  invalidation, and the two dispatch sites whose task creation can be deferred
  (`refreshFleet()`'s active refresh, `refreshAll(profiles:)`'s per-host task)
  re-check the transport before dispatching. The class is pinned by
  `testRetryDuringRetirementNeverDispatchesOnTheRetiredTransport`, RED at
  `42ca305` and GREEN after the fix. `No window remains in which an invalidated
  session can be the transport a live-path dispatch uses` — **RETRACTED in round
  3**: the repair fixed WHO HOLDS the transport, not WHEN a task is created on
  it; the window (the await hop between a guard and Foundation's task creation)
  was still open, and the retirement now WAITS for its owners before
  invalidating (`1da21ba`).
- **The identity guard is not the only guard.** Late work is already stopped by
  the pre-existing cancellation checks (`Task.isCancelled`, `FleetStore`
  connection generations, coordinator session identity). The session-identity
  guard is additive defence-in-depth; (b)/P5 show it is load-bearing on the
  pull-refresh completion path, and the plan requires it on every live-path
  completion regardless.
- **`installLiveSession` is inside `startLive`'s `hostURL` guard**, so a live
  session exists only while there is an active host URL (the state the production
  app launches into). No live session is created for a host-less state.
- **Host probe scope.** The measured 15 s request-timeout precedence, the
  `invalidateAndCancel` semantics and the `protocolClasses` inheritance come from
  a host `swift` process (macOS Foundation), not from the simulator app. The
  in-app consequences are covered by (a)-(e) instead.
