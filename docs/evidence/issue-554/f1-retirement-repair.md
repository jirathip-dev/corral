# Issue 554 round 2 — F1: task CREATION on a retired transport

The round-1 head (`42ca305`) was reviewed by an independent reviewer
(`rev554-live-session-r1`) and returned FAIL on exactly one defect, F1: a
foreground-reachable production path created a task on an INVALIDATED
`URLSession` and the process died on an uncatchable `NSGenericException`
(`PROBE2_EXIT=65`, in-app, `liveSession_nil=true`, `mode=live`).

This file is the round-2 repair record: the mechanism, the fix, the new pin,
its RED at `42ca305` and its GREEN after the fix.

## The defect (round-1 head `42ca305`)

- `AppModel.stopLive()` retired the live session's identity first
  (`liveSession = nil`, `AppModel.swift:2915-2916` at that head) and
  invalidated it last (`retiring?.invalidateAndCancel()`, `:2971`).
- `HostStreamCoordinator` CACHES the transport it builds host clients from
  (`private var urlSession`, `HostStreamCoordinator.swift:335`, base head),
  adopted by `AppModel.startLive()` → `adoptTransportSession(_:)` (`:386`,
  base head), and used to build clients and open streams
  (`:449`, `:611`, `:702`, base head).
- Nothing reset that stored transport when the live session was invalidated,
  and `liveSession = nil` happens while `mode` is still `.live` — so between
  the retirement and the next `startLive()` the coordinator still held the
  dead session. Settings ▸ Retry on a coordinator host
  (`AppModel.retryHostConnection(_:)` → `coordinator.startSessionIfNeeded`)
  built its client from it and created a task on it:
  `-[NSURLSession dataTaskWithRequest:]` / `URLSession.bytes(for:)` on an
  invalidated session raises `NSGenericException` — not catchable, process
  dead. The round-1 suite could not see it because every guard it added was a
  COMPLETION-TIME identity check; nothing ever CREATED a task on a retired
  session.

Round 1 had DISCLOSED this window as a known gap in `.report.md` ("Coordinator
transport after the boundary") and chosen not to close it. The review's
execution-level probe is what turned it from a disclosed gap into a blocker —
that attribution is recorded in `.report.md`.

## The fix at the round-2 head

| File:line (round-2 head `20533cd`) | Change |
| --- | --- |
| `ios/FleetNotifier/App/AppModel.swift:2766-2769` | New `retireLiveTransport(_:)` — the SINGLE retirement point: it repairs every holder (the coordinator's adopted transport is reset to the base session, which is never invalidated) and only then calls `invalidateAndCancel()` on the retired transport. |
| `ios/FleetNotifier/App/AppModel.swift:3002` | `stopLive()` calls `retireLiveTransport(retiring)` where it used to call `invalidateAndCancel()` directly. The ordering the contract fixes is unchanged: cursor/metadata persistence first, invalidation last. |
| `ios/FleetNotifier/App/AppModel.swift:3233` | `refreshFleet()`'s active-refresh task re-checks `isLiveTransport(transport)` BEFORE the first request: its client was built on whatever transport was live at dispatch time, and the body can be scheduled after a retirement boundary. |
| `ios/FleetNotifier/Profiles/HostStreamCoordinator.swift:719` | `refreshAll(profiles:)`'s per-host task re-checks `self.urlSession === client.session` before the first request, same reason (`"transport retired during refresh"`). |
| `ios/FleetNotifier/Profiles/HostStreamCoordinator.swift:377-392` | `adoptTransportSession(_:)` documented as both halves: the startLive adoption and the retirement repair. |

**RETRACTED (round 3).** This file said here that "an invalidated session cannot
remain any live-path dispatch's transport … a dispatch that captured a client
earlier refuses to create its task once the transport has moved on", i.e. that no
window remained. That was FALSE, and the round-2 reviewer disproved it by
execution at this very head (`1bcdd24`, exit 65, 2/2 — see the ROUND 3 record
below). The repair is necessary and was verified correct, but it is not
sufficient: it fixes WHO HOLDS the transport, not WHEN a task is created on it.
`invalidateAndCancel` still appears exactly once in production source (that one
function) — that part stands.

## ROUND 3 — the window is the AWAIT HOP between a guard and the creation

The round-2 reviewer ran the production route (220 jittered
`.active`/`.background` cycles plus `retryHostConnection(secondaryHost)` on two
pinned hosts) at `1bcdd24` and aborted the process (exit 65, 2/2) with two
stacks, both from live-path code:

    FleetStore.connect → CorraldClient.stream → URLSession.bytes(for:) → -[__NSURLSessionLocal taskForClassInfo:]
    CorraldClient.fetchHostKey(within:) → fetchHostKey() → URLSession.data(for:) → same

Controls from the same review: `42ca305` exits 65 as well (this defect was never
closed, not reopened), and pure scene flapping with NO retry exits 0 (the
trigger needs a window-reachable dispatch — the Settings ▸ Retry route).

**Why no caller-side guard can close it** (the reviewer's point, and the reason
round 2's two guards are "correct but not the full set"):
the creation is not performed by the caller. The throwing frames of the abort
stack are `URLSession.bytes(for:)` → `withTaskCancellationHandler`'s operation →
`-[__NSURLSessionLocal taskForClassInfo:]`, i.e. the task is created one await
hop AFTER the call, on another executor. A guard evaluated in the caller's step
can therefore never be atomic with it, and `Task.cancel()` is cooperative: a
cancelled owner that is already past its last guard still creates its task.

**The four sites the reviewer named** (all of them, not "the two that can
defer" — that round-2 wording understated the set):

| # | Site | Why it was still open |
| --- | --- | --- |
| 1 | `FleetStore.swift:838-839` — `streamTask = Task { await client.stream(…) }` | the client is captured at `connect()` and the reconnect loop re-creates a transport task on every rung; cancellation is cooperative |
| 2 | `CorraldClient.swift:200` — `session.bytes(for:)` | the abort site of stack #1 |
| 3 | `CorraldClient.swift:126` — `session.data(for:)` | the abort site of stack #2, reached after `HostStreamCoordinator.swift:496` / `AppModel.swift:1327` crossed their guards |
| 4 | `HostStreamCoordinator.swift:505` — `openStream` after `await fetchHostKey(within:)` | funnels into site 1 |

**The fix (round 3 — the round-3 head SHA is recorded in `.report.md`)**: the
retirement WAITS. `FleetStore.disconnect()` already returns its stream task (its
return value used to be discarded), and `HostStreamCoordinator.stopAll()` now
returns every owner it cancelled (per-host stream, preflight ladder, refresh);
`AppModel.liveTransportOwners()` collects those plus the ACTIVE host's stream
task, its preflight ladder and the registered life-path tasks
(`refreshFleet`'s two tasks are registered in `lifecycleTasks` for exactly this),
and `retireLiveTransport(_:owners:)` cancels them, waits (bounded, 2 s, so a
wedged owner cannot hang the lifecycle) for their termination, and only then
invalidates. Repair-first (round 2) plus MainActor-serialized owner creation is
what makes the set complete: an owner either exists before the retirement step
(and is collected and awaited) or resolves its transport after the repair.
`CorraldClient.swift` and `FleetStore.swift`'s rung loop needed no edit — the
rungs are covered because the stream task itself is awaited.

## The new pin (the missing CLASS: task creation, not completion delivery)

`FleetNotifierTests/LiveSessionTransportTests/testRetryDuringRetirementNeverDispatchesOnTheRetiredTransport`
(`ios/FleetNotifierTests/PreflightRetryTests.swift:1300-1410`) drives the
reviewer's production route with no reimplementation:

1. an ACTIVE host and ONE coordinator-owned (non-active) host, both pinned and
   scripted through the injected `URLProtocol` transport;
2. `model.startLive()` — the live session is installed and adopted, both hosts
   verify and open their streams on it;
3. a real task is dispatched on the live session BEFORE the boundary, so its
   post-retirement cancellation is observation that the session is invalidated;
4. `model.handleScenePhaseChange(.background)` — the retirement boundary, with
   `liveSession == nil` and `mode == .live` asserted (the window the defect
   lives in);
5. `model.retryHostConnection(secondaryHost)` — the Settings ▸ Retry route that
   reaches `coordinator.startSessionIfNeeded`;
6. the retry's stream attempt and its bounded `/host-key` attempt must BOTH
   dispatch (bounded waits, then counter assertions) and the preflight must be
   answered — i.e. the retry is not silently a no-op;
7. `handleScenePhaseChange(.active)` installs the NEXT live session
   (`!== retiring`, `!== base`) and a following live-path dispatch
   (`refreshFleet()`) still works on it.

### RED at `42ca305` (the base, production source PRISTINE)

Command (lane-scripted, shared heavy lock, lane simulator):

    git worktree add --detach /tmp/g554-r2-base 42ca305…   # pristine base
    cp <head>/ios/FleetNotifierTests/PreflightRetryTests.swift  # ONLY the pin
    bash /tmp/g554-r2-basered.sh
    # -> flock /tmp/n.lock python3 docs/evidence/issue-547/bounded-run.py \
    #      g554-r2-basered 1800 env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 \
    #      xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier \
    #      -destination 'platform=iOS Simulator,id=C8E69C58-66A4-440D-850C-8C1628C720E6' \
    #      -derivedDataPath /tmp/g554-r2-base-dd \
    #      -only-testing:FleetNotifierTests/LiveSessionTransportTests/testRetryDuringRetirementNeverDispatchesOnTheRetiredTransport

Raw result: `BASERED_EXIT=65`, `RAW_EXIT=65`, pin file sha256
`ac7dcb99de01b14b204577ea189a7e268b159cb35f185aae9945e953157602ea`,
`PROD_DIFF_LINES=0` (no production file differs from the base commit by even a
line), `TESTFILE_SHA` recorded by the driver.

Witnesses from `/tmp/g554-r2-basered.log`:

    Test Case '-[FleetNotifierTests.LiveSessionTransportTests testRetryDuringRetirementNeverDispatchesOnTheRetiredTransport]' started.
    …: error: -[…] testRetryDuringRetirementNeverDispatchesOnTheRetiredTransport : Task created in a session that has been invalidated (NSGenericException)
    *** Terminating app due to uncaught exception 'NSGenericException', reason: 'Task created in a session that has been invalidated'
    *** First throw call stack: … -[__NSURLSessionLocal taskForClassInfo:] … URLSession.bytes(for:delegate:)
    INVALIDATED_MESSAGE_HITS=8

i.e. the abort is raised inside the streaming path the retry opens
(`URLSession.bytes(for:)` on the retired session), which is the same class the
reviewer hit through the preflight path — the process dies, `rc=65`, and no
assertion can rescue it.

### GREEN at the fix head

Same pin, fix head, single-test selection
(`bounded-run.py g554-r2-headpin 1200 …`, derived data `/tmp/g554-r2-head-dd`):

    HEADPIN_EXIT=0
    Test Case '-[FleetNotifierTests.LiveSessionTransportTests testRetryDuringRetirementNeverDispatchesOnTheRetiredTransport]' passed (0.548 seconds).
    ** TEST SUCCEEDED **

and inside the G1 focused leg at the committed head:
`passed (0.118 seconds)`, `Executed 47 tests, with 1 test skipped and 0 failures`.

## Probe P6 — the pin bites the fix

`docs/evidence/issue-554/probes.py` P6 (`P6-no-repair`) applies exactly the
round-1 behaviour as a candidate defect — the repair line is removed from
`retireLiveTransport`, leaving only `transport.invalidateAndCancel()` — in a
disposable detached worktree, and runs the extended battery:

- control (pristine, 6 tests): exit 0
- P6: RED **by abort** — the pin is the running test case, the log carries
  `Task created in a session that has been invalidated (NSGenericException)`
  and a non-zero exit; the source is restored byte-identically
  (`shasum -a 256` before == after, `git diff --exit-code` clean).
- P1–P5 keep their round-1 REDs (5/5 as before).

Raw numbers, per-leg exits and the restore hashes:
`docs/evidence/issue-554/probe-results-round2.json` and the probe table in
`.report.md`. Driver summary at head `20533cd` (`PROBES_EXIT=0`):

    PROBES=6 ASSERTION_RED=6 RESTORED_GREEN=6/6 RESTORE=BYTE_IDENTICAL

P6's leg (`RAW_EXIT=65`) witnesses the abort itself: the pin is the test case the
log names, with `Task created in a session that has been invalidated
(NSGenericException)` and the `Terminating app due to uncaught exception` line.
Control and restored-green legs: `Executed 6 tests, with 0 failures`, exit 0.

## What this does NOT prove (disclosed)

- No device, no TestFlight, no daemon, no Tailscale, no CI, no PR. The owner
  device gate stays unexecuted with empty tables
  (`docs/evidence/issue-554/device-protocol.md`).
- The two deferred-dispatch re-checks (`refreshFleet()`'s active refresh and
  `refreshAll(profiles:)`) are NOT separately pinned: the window they close is
  "the body of an already-created task is scheduled after the retirement",
  which the test seam cannot interleave deterministically (the body is
  enqueued on the main actor and the boundary is delivered on the main thread).
  The pinned route (Settings ▸ Retry → `startSessionIfNeeded`) is the route the
  reviewer executed and it covers task creation on the retired transport;
  the re-checks are the same class closed at the two sites that can defer.
- `URLProtocol`-mocked transport never reaches a socket pool, so nothing here
  claims anything about the stale-pool warm return itself.
