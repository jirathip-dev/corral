# #547 — ROUND 2 implementation and verification

Worktree: `/Users/jirathip/.herdr/worktrees/corral/impl547-foreground-reconnect`
Branch: `g547-foreground-reconnect`
Tested implementation head: `d31cf77ec4012cf77e03f33aaafdb528a4d3bbb5`
Base: `ed24e6f57075acf1e5b082c7cd29a931dfbfbd62`.

Implementation is complete. Physical-iPhone/real-network before/after performance
remains an explicitly missing human gate, as permitted by the amended brief.
No physical-device improvement or production rollout is claimed.
The commit containing this report adds evidence only; its iOS sources must be
byte-identical to the tested head above. Final commit/remote readbacks are supplied
in the final response, rather than pretending this file can contain its own commit hash.

## Numbered ROUND 2 contract / AC matrix

1. **Real row hook.** `FleetViews.swift:828-850,1550-1552,1705-1707` adds only
   `ReconnectRowVisibility`: onAppear/onDisappear and an onChange observed while
   appeared. Before: no row-stage observation. After: both ordinary and composite
   rows report their own host/revision to `AppModel.noteRowVisible`, then
   `FleetStore.swift:342-348` checks membership in the first applied revision.
   No layout, interaction, authorization, introspection, swizzle, or delayed-task
   substitute. `testRowVisibilityHookIsWiredToBothProductionRowBodies` and the
   native-window test pass in G1/G2 (raw 0, `/tmp/g547-focused.log`, `/tmp/g547-full.log`).
   Release build/binary gates pass (raw 0). This is SwiftUI appearance/update,
   **not GPU scan-out**. A coalesced-away/offscreen/collapsed first revision may
   have no row mark; it is never reported as visible from model publication alone.

2. **Assertion ledger.** `27` exact old→new edit records in
   `docs/evidence/issue-547/assertion-ledger.json`: 21 assertion replacements and
   6 fixture/comment adjustments. Each has file, observed pre-edit coordinate,
   exact old/new text, reason, resolved old Git locations and final source lines.
   `ledger-locations.py` verified every replacement in the tested source and
   every old fragment against an actual prior commit (exit 0).
   Zero `/events` before verification becomes one speculative owner, **no applied
   data/cursor and no Live/read authorization**. Terminal mismatches retain their
   request count across later starts/hints. Exact key retry counts/cadence,
   `.mismatch`, Remove Host-only recovery, read_tail signatures/host routing, and
   duplicate-owner protections remain asserted. Generic application remains
   fail-closed too: two existing read-tail/grants fixture seed helpers now WAIT
   for verification before seeding trusted test data; no authorization assertion
   was removed or relaxed. G1/G2 raw 0, named logs above.

3. **Parallel + bounded fail-closed ingress.** `AppModel.swift:1252-1330,2679-2720`
   and `HostStreamCoordinator.swift:377-466` overlap key verification with stream
   setup; previously setup waited for the key. `FleetStore.swift:137-235,351-400,
   533-538,797-879,1001-1008` owns an ordered, lock-guarded inbox before the actor
   hop. Default cap: **32 frames / 1,048,576 payload UTF-8 bytes / 10 seconds**;
   control deliveries are bounded too. There is no per-frame main-actor task
   queue while unverified. Whole-window overflow/expiry closes transport and
   preserves the last APPLIED cursor; a successful key check opens recovery from
   that cursor. A mismatch discards data and closes transport, terminally.
   Verification gates data application and transport Live callbacks, including
   cold starts and racing generic refresh application. Owner cleanup also checks
   inbox identity so a cancelled stream cannot erase a terminal mismatch error.
   G1/G2 raw 0; M1/M2 assertion RED 65, restored GREEN 0 (G8 below).
   This is a bound on the new ingress window, not a claim about all URLSession,
   parser, retained-board, or process memory.

4. **Release-active stage timing.** `FleetStore.swift:105-134` emits one fixed-name
   `os.Logger` mark per stage/ephemeral attempt, monotonic system uptime, category
   `foreground-reconnect`, subsystem `com.corral.fleetnotifier`. No host URL,
   token, key, durable agent/profile ID, output payload, or row text is logged by
   these new records. Stages and measured simulator medians are below.
   Source/self-test/Release/binary gates all raw 0. Binary byte inspection also
   found the category and all seven stage strings (raw 0,
   `/tmp/g547-release-timing-markers.log`). Physical timings are **missing**, not 0.

5. **Fresh delta resume / epoch recovery.**
   `ForegroundReconnectTests.testSceneOpensBeforeKeyAndAppliesBufferedDeltasInOrder`
   drives real URLSession fixture bytes via `.background`→`.active`; it observes
   the request before releasing the key, asserts `Last-Event-ID: 5` AND
   `Corral-Epoch`, then sees ONLY delta payloads apply in order at revisions 6,7.
   There is no full-snapshot substitution in that same-epoch fixture.
   `testEpochChangeReplacesTheDeltaBaseWithAFullStreamSnapshot` rejects a
   cross-epoch delta and accepts the new epoch's lower-revision full SSE snapshot.
   Existing #450 EpochRecoveryTests remain green. G1/G2 raw 0; M3/M4 parallel-path
   removals produce assertion RED 65, restored GREEN 0. Daemon logic is unchanged.

6. **Retained board, honest status, independent hosts.** Thirteen new tests in
   `FleetNotifierTests.swift:14670-15058` cover buffered ordering, mismatch,
   count/byte cap, expiry, background races, both directions of slow-host
   independence, epoch changes, never-verified cold start, ended transports,
   row wiring and native visibility. G1 raw 0: 44 tests / 0 failures. G2 raw 0:
   627 tests / 0 failures (the brief's 181-base annotation was not used as an
   executed count). Native UIKit-window attachments from the real SwiftUI
   `FleetView` on iPhone 16 Simulator show:
   - `547-retained-unverified.png`: visible **revision 5**, **Connecting**.
   - `547-applied-verified.png`: visible **revision 6**, **Live**.
   Both were inspected visually. They are synthetic URLProtocol fixture UI,
   not a signed-in real-daemon or physical-phone capture. Dimensions: 1179×2556.
   The old-looking duration in the row is intentional fixture timestamp data.

7. **G1–G8.** Exact commands, raw outputs, probe signatures and logs below.
   All canonical non-advisory gates pass; advisory anti-slop is raw 1 at BOTH
   base and head with **ADDED=[]**. All four behavioral mutations produce real
   XCTest assertion failures, not compiler failures. Restored control is 13/13.

8. **Digest pins.** `ios/check-release-demo.py` changes only the two pin values
   and dated #547 comments. Computed from final source bytes:
   - Release: `3fa501712397ab6b5ca34b23448ae2a9186fc3d6bcda7b7276f0ca702de8445b`.
   - Tests: `f8e9b422d92286d58065a6900c7907b635900ba5b7c6b90b923bef9f43eec034`.
   G3 and G5 were rerun after the last source edit/re-pin: raw 0, 0 and empty
   generation diff. The self-test ran standalone with a 600-second outer bound,
   not in the round-1 batch that timed out (historical tool exit 124).

9. **Report/evidence.** This is the new round-2 report, also retained at
   `docs/evidence/issue-547/report-round2.md`. The accepted round-1 report is
   archived byte-for-byte at `report-round1.md` (its historical /tmp gate paths
   may now hold the round-2 runs required by the same brief). Source fences are
   respected: seven iOS files only; no new Swift file or dependency. Other
   changes are this report and allowed evidence files. No daemon, notifications,
   APNs, background execution, project configuration or manifest-list change.

10. **Delivery.** The authorized branch is `g547-foreground-reconnect`.
    Final publication/readback commands, executed after committing this report:
    `git push -u origin g547-foreground-reconnect`, `git rev-parse HEAD`,
    `git rev-parse origin/g547-foreground-reconnect`,
    `git ls-remote --heads origin g547-foreground-reconnect`.
    Final response records the real results and final SHA. No PR, merge,
    issue-close, main/release/TestFlight or hosted-CI success claim.

## Stage inventory and observed simulator numbers

| Stage | Release call site |
| --- | --- |
| path-ready observation | AppModel.swift:2991-2993 (actual path callback) |
| host-key request | AppModel.swift:1290 / HostStreamCoordinator.swift:419 |
| host-key response | AppModel.swift:1293 / HostStreamCoordinator.swift:423 |
| SSE HTTP 200 | FleetStore.swift:849 |
| first frame received | FleetStore.swift:842 |
| first accepted frame applied | FleetStore.swift:584 |
| corresponding row visible | FleetViews.swift row modifier → FleetStore.swift:348 |
| retained old row visible (separate) | FleetStore.swift:344 |

`timings.py /tmp/g547-focused.log` exited 0 and verified all 10 distinct samples.
`simulator-timings.json` retains every raw monotonic mark and per-sample duration.
These are **local simulator fixture/gate/render timings**, NOT device/network
latencies and NOT measured speedups. The first sample includes screenshot capture
before key release; all 10 samples are included, with no exclusions. The path
observer arrives slightly AFTER key dispatch: the negative signed difference is
preserved, not clamped or relabelled. The key and SSE branches overlap.

| Metric | n | Median ms |
| --- | --- | --- |
| key_dispatch_minus_path_observed | 10 | -0.100729 |
| key_round_trip | 10 | 40.118979 |
| path_observed_to_sse_200 | 10 | 0.385521 |
| sse_200_to_frame_received | 10 | 0.124333 |
| frame_received_to_applied | 10 | 39.584229 |
| key_response_to_applied | 10 | 0.140417 |
| applied_to_row_visible | 10 | 2.198104 |
| path_observed_to_retained_row_visible | 10 | 31.045063 |

The physical capture protocol remains `device-protocol.md`: 3 excluded warmups
plus 20 measured cycles for each serial/parallel × Mac/loaded-Bazzite block,
with an identical-instrumentation serial control and owner-supplied reproducible
Bazzite workload. Those control builds/workload/device blocks have NOT run.
`xcrun devicectl list devices` exited 0 and returned `No devices found.`
Raw discovery: `/tmp/g547-devices.log`.

## Canonical commands and real outputs

Every native build/test used `/tmp/n.lock` and the brief's simulator/environment.
`bounded-run.py` records each exact argv, cwd, Git source head, raw exit and log;
outer deadlines include lock waits (1200s for native gates). A process.wait window
expiring is not a test timeout; final raw gate exits below are authoritative.
`gate-results.json` records log byte counts and SHA-256 hashes as well as summaries.

### focused: raw exit 0

```sh
flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g547-dd -only-testing:FleetNotifierTests/ScenePhaseLifecycleTests -only-testing:FleetNotifierTests/EpochRecoveryTests -only-testing:FleetNotifierTests/PreflightRetryCoordinatorTests -only-testing:FleetNotifierTests/PreflightRetryActiveHostTests -only-testing:FleetNotifierTests/SSEStreamRegressionTests -only-testing:FleetNotifierTests/ForegroundReconnectTests
```

Log: `/tmp/g547-focused.log`. Executed 44 tests, 0 failures.

### full: raw exit 0

```sh
flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g547-dd -only-testing:FleetNotifierTests
```

Log: `/tmp/g547-full.log`. Executed 627 tests, 0 failures.

### check-release: raw exit 0

```sh
python3 ios/check-release-demo.py
```

Log: `/tmp/g547-check-release.log`. source PASS: 7 native Swift renderer files; 8 unchanged approved icon inputs
release-demo check: PASS (Debug source preserved; Release boundary verified)
RAW_EXIT=0

### self-test: raw exit 0

```sh
python3 ios/check-release-demo.py --self-test
```

Log: `/tmp/g547-self-test.log`. source PASS: 7 native Swift renderer files; 8 unchanged approved icon inputs
release-demo check: PASS (Debug source preserved; Release boundary verified)
RAW_EXIT=0

### debug: raw exit 0

```sh
flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild build -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -configuration Debug -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/g547-debug-dd
```

Log: `/tmp/g547-debug.log`. ** BUILD SUCCEEDED **
RAW_EXIT=0

### release: raw exit 0

```sh
flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild build -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -configuration Release -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/g547-release-dd CODE_SIGNING_ALLOWED=NO
```

Log: `/tmp/g547-release.log`. ** BUILD SUCCEEDED **
RAW_EXIT=0

### binary: raw exit 0

```sh
python3 ios/check-release-demo.py --binary /tmp/g547-release-dd/Build/Products/Release-iphonesimulator/FleetNotifier.app/FleetNotifier
```

Log: `/tmp/g547-binary.log`. source PASS: 7 native Swift renderer files; 8 unchanged approved icon inputs
bundle PASS: Mach-O /tmp/g547-release-dd/Build/Products/Release-iphonesimulator/FleetNotifier.app/FleetNotifier; 6 product files; catalog names ['AppIcon', 'BayPreview', 'Black', 'BlackPreview', 'Grey', 'GreyPreview', 'Palomino', 'PalominoPreview']
release-demo check: PASS (Debug source preserved; Release boundary verified)
release-demo check: inspected /tmp/g547-release-dd/Build/Products/Release-iphonesimulator/FleetNotifier.app/FleetNotifier
RAW_EXIT=0

### xcodegen: raw exit 0

```sh
bash -c 'cd ios && xcodegen generate --spec project.yml'
```

Log: `/tmp/g547-xcodegen.log`. RAW_EXIT=0

### G5 generated-project comparison: raw exit 0, empty diff

```sh
git diff --exit-code -- ios/ > /tmp/g547-xcodegen-diff.log 2>&1
```

### G6 advisory: base raw 1, head raw 1; identity delta raw 0

```sh
# In /tmp/g547-base at ed24e6f57075acf1e5b082c7cd29a931dfbfbd62:
swift run --package-path /Users/jirathip/.herdr/worktrees/corral/impl547-foreground-reconnect/ios/tools/anti-slop-swift anti-slop ios/FleetNotifier ios/FleetNotifierTests
# In the implementation worktree:
swift run --package-path ios/tools/anti-slop-swift anti-slop ios/FleetNotifier ios/FleetNotifierTests
python3 docs/evidence/issue-547/anti-slop-delta.py /tmp/g547-base .
```

Logs: `/tmp/g547-aslop-base.log`, `/tmp/g547-aslop-head.log`,
`/tmp/g547-aslop-delta.json`. Identity is file/rule/severity/column/source text/message,
with duplicate multiplicities preserved, not a totals-only comparison.
24→24: no-force-try 1→1; no-force-unwrap 18→18;
no-shape-in-symbol-names 4→4; no-swallowed-errors 1→1.
ADDED=[], REMOVED=[]. No suppressions or baseline edits.

### G7 hygiene

```sh
git diff --check > /tmp/g547-diffcheck.log 2>&1
```

Raw exit 0. Final staged/committed whitespace and iOS-source-identity checks are
also run at publication, with the actual exits included in the final readback.

### G8 mutation runner: raw exit 0

```sh
bash /tmp/g547-probes.sh > /tmp/g547-probes.log 2>&1
```

Reproducer: copy `bounded-run.py` to `/tmp/g547-run.py`; `/tmp/g547-probes.sh`
invokes `python3 /Users/jirathip/.herdr/worktrees/corral/impl547-foreground-reconnect/docs/evidence/issue-547/probes.py`.
It refuses to reuse an existing `/tmp/g547-scratch`, creates a detached worktree
at the tested implementation head, verifies control GREEN, and restores each
mutated file byte-for-byte in a finally block. No implementation-worktree source
was mutated. Before/after `shasum -a 256` output and empty restore diffs are in
`/tmp/g547-probes.log`; exact splices and hashes are in `probe-results.json`.

| Probe | Raw exit | Assertion-failing tests | Log |
| --- | --- | --- | --- |
| control | 0 |  | `/tmp/g547-probe-control.log` |
| M1-verify | 65 | testNeverVerifiedColdStartBuffersItsFirstSnapshot, testSceneOpensBeforeKeyAndAppliesBufferedDeltasInOrder | `/tmp/g547-probe-M1-verify.log` |
| M2-cap | 65 | testByteCapRejectsOneOversizedFrameBeforeApplication, testCapDiscardsWholeWindowAndResumesOnlyAfterVerification | `/tmp/g547-probe-M2-cap.log` |
| M3-active-serial | 65 | testSceneOpensBeforeKeyAndAppliesBufferedDeltasInOrder | `/tmp/g547-probe-M3-active-serial.log` |
| M4-coordinator-serial | 65 | testSlowCoordinatorDoesNotDelayHealthyActiveHost | `/tmp/g547-probe-M4-coordinator-serial.log` |
| restored-green | 0 |  | `/tmp/g547-probe-restored-green.log` |

M1 changes `requireHostVerification` to leave its application gate open, so the
never-verified cold snapshot is applied and claims connected, and a resumed
cursor advances from 5 to 7 BEFORE key release. These are the observed failure
signatures, not just timeout assertions. M2 disables count/byte/control-window
caps. M3 restores the active host's serial return; M4 removes only the
coordinator's speculative open. Both prevent the required pre-key stream/frame
observation. Restore SHA-256 matches before SHA-256 for EVERY probe. Final GREEN
runs the entire 13-test new class, not just the failure subsets.

### control

```sh
flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g547-probe-dd -only-testing:FleetNotifierTests/ForegroundReconnectTests
```

### M1-verify

```sh
flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g547-probe-dd -only-testing:FleetNotifierTests/ForegroundReconnectTests/testSceneOpensBeforeKeyAndAppliesBufferedDeltasInOrder -only-testing:FleetNotifierTests/ForegroundReconnectTests/testNeverVerifiedColdStartBuffersItsFirstSnapshot
```

### M2-cap

```sh
flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g547-probe-dd -only-testing:FleetNotifierTests/ForegroundReconnectTests/testCapDiscardsWholeWindowAndResumesOnlyAfterVerification -only-testing:FleetNotifierTests/ForegroundReconnectTests/testByteCapRejectsOneOversizedFrameBeforeApplication
```

### M3-active-serial

```sh
flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g547-probe-dd -only-testing:FleetNotifierTests/ForegroundReconnectTests/testSceneOpensBeforeKeyAndAppliesBufferedDeltasInOrder
```

### M4-coordinator-serial

```sh
flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g547-probe-dd -only-testing:FleetNotifierTests/ForegroundReconnectTests/testSlowCoordinatorDoesNotDelayHealthyActiveHost
```

### restored-green

```sh
flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g547-probe-dd -only-testing:FleetNotifierTests/ForegroundReconnectTests
```

## Diagnostics, boundaries and non-claims

- Initial development compilation/focused logs: `/tmp/g547-r2-new-tests-1.log`,
  `/tmp/g547-r2-new-tests-2.log`, `/tmp/g547-r2-focused-1.log`. A nested fixture
  required MainActor isolation; a cancelled stream cleanup required an inbox
  identity guard to preserve mismatch errors. These development failures are
  not passed off as regression/mutation proof.
- First full run at `50ba6e02e004bd1f0c962bf2f0044c4b74e809a1` was raw 65:
  `/tmp/g547-full-head50ba.log` and `.json`. It exposed the remaining coordinator
  path-hint zero-events assertions and two fixture helpers applying data while
  verification was pending. The ledger records the changes; all security/read
  authorization assertions survived. Final G1/G2 at `d31cf77ec4012cf77e03f33aaafdb528a4d3bbb5` are raw 0.
- One edit hit transient ENOSPC; the failed edit was inspected and reapplied.
  No other lane's resources, host software or system configuration were changed.
- Exact structural commands:
  `ast-grep run -l swift -p '$TRACE.mark($STAGE)' ios/FleetNotifier/App/AppModel.swift ios/FleetNotifier/App/FleetStore.swift ios/FleetNotifier/Profiles/HostStreamCoordinator.swift`
  exited 0 with 11 call sites (`/tmp/g547-timing-ast.log`).
  `ast-grep outline ios/FleetNotifierTests/FleetNotifierTests.swift --match ForegroundReconnectTests --view expanded`
  exited 0 and enumerated 13 test methods (`/tmp/g547-r2-tests-outline.log`).
  `ast-grep outline ios/FleetNotifier/App/AppModel.swift --match 'beginKeyContinuityCheck|failKeyContinuity|startLive|stopLive|handleScenePhaseChange|noteBoardRowVisible|boardRevision' --view expanded`
  exited 0 but returned `nothing found`; the focused call-pattern query supplied
  the inventory instead.
- Scope/ownership: no real daemon/API fixture writes, no live credentials used;
  all new network tests use a fail-closed local URLProtocol. The original
  untracked context was copied byte-identically to
  `/tmp/g547-original-hermes-context.md` before removal from the source tree
  (exit 0; `/tmp/g547-context-backup.log`), SHA-256
  `97fda4999301ee9dfdd1904723cec6e50517e5f1817750659c6bca6ccb4f92a0`.
  Scratch and base worktrees were removed after clean-diff/byte-restore proof
  (both exits 0). To replay the advisory baseline, first recreate it with
  `git worktree add --detach /tmp/g547-base ed24e6f57075acf1e5b082c7cd29a931dfbfbd62`.
  Raw gate logs and DerivedData remain at their named paths.
- Simulator cleanup: the locked `xcrun simctl shutdown 59DDC0C5-891E-4EC0-91AF-4F50DF68D793`
  returned 149, explicitly `Unable to shutdown device in current state: Shutdown`
  (`/tmp/g547-sim-shutdown.log`). Readback confirmed the exact UDID is Shutdown;
  this was an already-shut-down cleanup result, not a failing test gate.
- Missing physical gate: no iPhone before/after samples, no real Mac/Bazzite
  server load measurements, no instrumented serial-control timing comparison,
  no TestFlight/production confirmation, and no hosted CI result. Simulator UI
  and Release binary checks are not substitutes for those claims.
