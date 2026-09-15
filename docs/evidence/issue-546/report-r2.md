# #546 round 2 — runtime trust and admission witnesses

## Scope and provenance

Reviewed head: `b5d0da7945d072ff79453c2202318989d5b74584` (independent review FAIL).
Fixed/tested source head: `8b8e2fe6d80d32cf6eadbcab25d702e4a8c52ad0`.
Branch: `g546-push-refresh-client`.
Worktree: `/Users/jirathip/.herdr/worktrees/corral/impl546-push-refresh`.
This round supersedes round 1's readiness/trust-coverage claims, not its historical
run receipts. The original report is preserved as a byte prefix of `.report.md`;
round-1 JSON receipts, probe driver and physical-device protocol are unchanged.
The delivery commit adds evidence only; its hash is reported outside this file.

## Numbered response

| Contract | Change / witness | Discrimination |
|---|---|---|
| 1 — blocker | One added admission predicate in `AppModel.swift:3149`, before quota admission, asks the non-active host's coordinator `allowsLiveWork`. `testRuntimeCoordinatorMismatchRefusesHintWithoutQuota` establishes a real coordinator `.mismatch` through scripted preflight while asserting persisted `mayConnect == true`, then sends the real delegate callback. It asserts `.noData`, zero requests, no attempt record, no in-flight owner, unchanged posture, empty agents and nil cursor/cache. | Pre-fix assertion RED (exit 65, one test / six failures); fixed GREEN; R201 removes only the added line. |
| 2 — foreign stamp | `testForeignHostSnapshotIsRefusedBeforeStoreMutation` supplies a matching preflight but a snapshot agent stamped with a different pinned host. Agents remain empty, cursor/cache nil and connection posture unchanged. | R202 removes only the handler's `conformsToPinnedHost` leg. The posture assertion distinguishes the handler fence from FleetStore's downstream rejection, which would change error posture. |
| 3 — already streaming | `testAlreadyStreamingHintHasNoRequestsOrQuota` opens a real held `/events` stream before receipt. The sole request remains that setup stream; no hint requests, quota or owner appear. `testStreamOpenedDuringHintPreventsLateApply` separately opens a stream while the snapshot is held in flight. | R203 removes admission's `!target.isStreaming`. R208/R209 distinguish the post-await stream leg from its generation backstop (see masking below). |
| 4 — mode | `testNonLiveModesRefuseHintWithoutQuota` covers `.demo` and `.needsSetup` while retaining a resolvable valid enrollment, so unknown-host refusal cannot conceal a missing mode gate. | R204 removes only `mode == .live`; quota assertions discriminate even if a later lifecycle check also prevents networking. |
| 5 — quota sanity | `testOversizedAndMalformedQuotaBlobsFailClosed` uses three EXPIRED attempts, malformed JSON, an overflowing numeric timestamp and string `NaN`. Each callback is refused without requests, owner, cursor/cache or mutation of the original defaults bytes. | R205 removes `attempts.count <= 2`; expired entries avoid masking by the rolling-hour ceiling. R207 explicitly measures strict-decoder masking of `isFinite`; no inner-leg discrimination is claimed. |
| 6 — missing model | `testDelegateWithoutRestoredModelCompletesNoDataOnce` clears the delegate's restored-model reference and invokes the real UIKit callback. It requires exactly one `.noData` completion and zero requests/quota/owner. | R206 removes only the nil-model branch's completion. The fixture's three-second observation assertion fails; this is not an xcodebuild watchdog timeout. |

The runtime gate applies to non-active hosts because the coordinator excludes the
active profile from its sessions. Applying its predicate blindly to the active
profile would reject valid active-host hints. Active-host ownership remains with
AppModel; its existing trust/preflight path is unchanged. Coordinator `.verifying`
as well as `.mismatch` refuses non-active hint admission, matching the existing
read-path policy. The positive non-active quota/removal fixtures now establish
`.verified` through a real coordinator preflight, then drain stream ownership.
No coordinator implementation or foreground scene-phase function was changed.

The new helper parks SSE before headers (no setup cache publication), cancels and
boundedly awaits every captured stream task before session invalidation. The
mismatch fixture deliberately offers a now-matching key on the subsequent hint:
a hint must not repair terminal runtime mismatch even if a poll could succeed.

## Assertion ledger and file fence

Existing assertions changed/deleted/weakened: NONE. Seven test methods were added
to the existing test file, bringing BackgroundHintTests from 19 to 26. The two
existing positive non-active tests gained only a verified-coordinator setup call:
`testHostThirtyMinuteCeilingAndDeviceHourlyCeiling` and
`testRemovedHostCannotBeRecreatedByLateSnapshot`. The shared fixture gained
optional foreign-stamp and held-events inputs. No production seam was added for
testing, and there is no new Swift file, dependency or project change.

`/tmp/g546-r2-preservation.log` (exit 0) proves:
- The complete AppModel diff is exactly the single admission leg. Therefore
  `handleScenePhaseChange` and every other AppModel body are byte-preserved.
- All pre-#546 test bytes are unchanged. Reversing just the fixture inputs and
  two setup calls reproduces all 19 original BackgroundHintTests byte-for-byte.
- Original evidence files and the unfilled device protocol match the reviewed head.

Production files changed this round: `ios/FleetNotifier/App/AppModel.swift` only.
Tests: `ios/FleetNotifierTests/FleetNotifierTests.swift`.
Pins: only the two values and dated #546 R2 comments in `ios/check-release-demo.py`.
Evidence: `.report.md` and new `docs/evidence/issue-546/*-r2.*` files.
AppDelegate, PushPayload, LocalNotifier, NotificationPermission, FleetViews,
FleetStore, HostStreamCoordinator, project.yml/pbxproj, manifest and forbidden
Rust/service/CI/release surfaces are unchanged from the reviewed head.
The visible generic alert path is unchanged; this remains the existing
visible/local alert behavior, not a claim that #544's future service exists.

Recomputed pins after final Swift edits:
- Release: `7e984a5aa8f390fd8a1f0dc1ee1a386c359b377cce212f7e81e14055c9ee2413`
- Tests: `9704ac79eeb9d9d83e299ee5be84b1044fe49c22f4955db5ca534f805e3e40cc`

## Pre-fix RED and initial GREEN

Both commands ran from the implementation worktree, with a 600-second bound.
The RED run used unchanged b5d0da7 production sources plus the new fixture; no
compiler failure is being counted as discrimination.

    flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g546-dd -only-testing:FleetNotifierTests/BackgroundHintTests/testRuntimeCoordinatorMismatchRefusesHintWithoutQuota > /tmp/g546-r2-base-red.log 2>&1
    BASE_RED_EXIT=65
    Executed 1 test, with 6 failures (0 unexpected) in 0.176 (0.177) seconds

The assertions include `runtime-mismatched host must not be re-polled`,
`refusal must not burn device quota` (79 bytes were written), populated agents,
cursor 5 and a nonnil board cache. The setup's runtime `.mismatch`, persisted
`mayConnect == true` and `allowsLiveWork == false` assertions all passed.

    flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g546-dd -only-testing:FleetNotifierTests/BackgroundHintTests > /tmp/g546-r2-initial-green.log 2>&1
    INITIAL_GREEN_EXIT=0
    Executed 26 tests, with 0 failures (0 unexpected) in 0.486 (0.492) seconds

A pre-commit `git diff --cached --check` initially exited 2 for an extra blank
line at the end of the new Python gate driver. It was removed before source
commit 8b8e2fe; no Swift edit occurred after the passing initial GREEN.

## G1–G7 at the committed source head

Driver: `python3 docs/evidence/issue-546/gates-r2.py`, raw exit 0.
Transcript: `/tmp/g546-r2-gates-driver.log`. Exact argv/cwd, raw exit, process
bounds, elapsed seconds, summaries and log SHA256s are in `gates-r2.json`.
There was one complete invocation, not a synthesized combination of reruns.
All processes completed within their 600-second deadlines. A 180-second
observation wait returned while the driver was still running; that was not a
gate timeout. The background wrapper printed two `can't change option: zle`
warnings and then `R2_GATES_DRIVER_EXIT=0`.

G1: exit 0 — 67 tests, 0 failures (26 hints plus existing notification suites).
G2: exit 0 — 656 tests, 0 failures (146.085 seconds test execution).
G3: source check and hermetic self-test each exit 0, Release boundary PASS.
G4: Debug build, Release build, Release binary proof each exit 0.
G5: xcodegen and full `git diff --exit-code -- ios/` each exit 0; empty diff.
G6: advisory exit 1, exactly the same 24 existing findings; zero added/removed.
G7: `git diff --check` exit 0.

Anti-slop compares full `(file, rule, message, source-line)` identity Counters,
not totals alone, against the already-recorded original-base results (also
reproduced at the reviewed head). `anti-slop-r2.json` records identities and
log hashes. Counts: no-force-unwrap 18, no-force-try 1, no-swallowed-errors 1,
no-shape-in-symbol-names 4. No baseline, rule or suppression was changed.
No justfile/AGENTS.md/CLAUDE.md existed at the inspected roots; no recipe was
invented. Only log names gain `-r2`; native arguments and DerivedData paths
remain those in the brief.

    flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g546-dd -only-testing:FleetNotifierTests/PushPayloadTests -only-testing:FleetNotifierTests/BackgroundHintTests -only-testing:FleetNotifierTests/HostAwarePushModelTests -only-testing:FleetNotifierTests/NotificationOptInRuntimeTests -only-testing:FleetNotifierTests/NotificationTapDeferredLifecycleTests -only-testing:FleetNotifierTests/NotificationEnableModelTests > /tmp/g546-r2-focused.log 2>&1
    FOCUSED_EXIT=0; timed_out=false

    flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g546-dd -only-testing:FleetNotifierTests > /tmp/g546-r2-full.log 2>&1
    FULL_EXIT=0; timed_out=false

    python3 ios/check-release-demo.py > /tmp/g546-r2-check-release.log 2>&1
    CHECK_RELEASE_EXIT=0; timed_out=false

    python3 ios/check-release-demo.py --self-test > /tmp/g546-r2-self-test.log 2>&1
    SELF_TEST_EXIT=0; timed_out=false

    flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild build -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -configuration Debug -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/g546-debug-dd > /tmp/g546-r2-debug.log 2>&1
    DEBUG_EXIT=0; timed_out=false

    flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild build -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -configuration Release -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/g546-release-dd CODE_SIGNING_ALLOWED=NO > /tmp/g546-r2-release.log 2>&1
    RELEASE_EXIT=0; timed_out=false

    python3 ios/check-release-demo.py --binary /tmp/g546-release-dd/Build/Products/Release-iphonesimulator/FleetNotifier.app/FleetNotifier > /tmp/g546-r2-binary.log 2>&1
    BINARY_EXIT=0; timed_out=false

    xcodegen generate --spec project.yml > /tmp/g546-r2-xcodegen.log 2>&1
    XCODEGEN_EXIT=0; timed_out=false
    # cwd: /Users/jirathip/.herdr/worktrees/corral/impl546-push-refresh/ios

    git diff --exit-code -- ios/ > /tmp/g546-r2-xcodegen-diff.log 2>&1
    XCODEGEN_DIFF_EXIT=0; timed_out=false

    swift run --package-path ios/tools/anti-slop-swift anti-slop ios/FleetNotifier ios/FleetNotifierTests > /tmp/g546-r2-aslop-head.log 2>&1
    ASLOP_EXIT=1; timed_out=false

    git diff --check > /tmp/g546-r2-diffcheck.log 2>&1
    DIFFCHECK_EXIT=0; timed_out=false

## G8 — scratch mutation evidence

Invocation:

    flock /tmp/n.lock python3 docs/evidence/issue-546/probes-r2.py > /tmp/g546-r2-probes.log 2>&1

Raw outer exit 0. 31/31 expected assertion-RED mutations exited 65; 2 explicitly masked single-leg observations exited 0. All 33 restores were byte-identical.
Initial pristine control: Executed 26 tests, with 0 failures (0 unexpected) in 0.476 (0.482) seconds, exit 0.
Final pristine control: Executed 42 tests, with 0 failures (0 unexpected) in 1.193 (1.218) seconds, exit 0.
No native process timed out or restarted. The required runtime-guard splice R201 failed by assertion.

The new driver reuses the original 24 mutation definitions and bounded runner,
then adds nine round-2 observations. It starts a fresh detached worktree at
8b8e2fe, never mutates the implementation worktree, asserts each replacement
anchor occurs exactly once, prints shasum before/after, restores exact bytes with
fresh mtimes after every probe, and ends with pristine GREEN. `probes-r2.json`
contains every exact command/replacement, expected/observed result, log hash,
failure diagnostic and before/after source hash.

| Probe | Test | Raw exit | Observation | Restore | Log |
|---|---|---:|---|---|---|
| M01-opt-out | `testOffByDefaultAndIndependentOfVisiblePreference` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M01-opt-out.log` |
| M02-unknown-host | `testUnknownRemovedUnenrolledAndTrustDeniedDoNotFetch` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M02-unknown-host.log` |
| M03-local-trust | `testUnknownRemovedUnenrolledAndTrustDeniedDoNotFetch` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M03-local-trust.log` |
| M04-enrollment | `testUnenrolledHostHasNoFetchOrPersistence` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M04-enrollment.log` |
| M05-fresh-pin | `testFreshKeyMismatchStopsBeforeSnapshot` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M05-fresh-pin.log` |
| M06-closed-top-level | `testClosedSchemaRejectsMalformedAndAllExtraContent` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M06-closed-top-level.log` |
| M07-closed-aps | `testClosedSchemaRejectsMalformedAndAllExtraContent` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M07-closed-aps.log` |
| M08-numeric-schema | `testClosedSchemaRejectsMalformedAndAllExtraContent` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M08-numeric-schema.log` |
| M09-host-ceiling | `testHostThirtyMinuteCeilingAndDeviceHourlyCeiling` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M09-host-ceiling.log` |
| M10-device-ceiling | `testHostThirtyMinuteCeilingAndDeviceHourlyCeiling` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M10-device-ceiling.log` |
| M11-inflight | `testCoalescesInflightEvenAfterRateWindow` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M11-inflight.log` |
| M12-exactly-once | `testCompletionOwnerIsExactlyOnceAcrossTerminalPaths` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M12-exactly-once.log` |
| M13-local-deadline | `testLocalDeadlineCompletesOnceWithoutHostResponseAndCancels` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M13-local-deadline.log` |
| M14-cancel-opt-out | `testDisableCancelsAndLateResultCannotApply` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M14-cancel-opt-out.log` |
| M15-foreground | `testForegroundNotificationCancelsWithoutSceneHandlerChanges` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M15-foreground.log` |
| M16-late-budget | `testLateSnapshotIsRefusedEvenBeforeDeadlineTaskRuns` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M16-late-budget.log` |
| M17-newer-stream | `testNewerStreamDataAndNewEpochWinEvenOverHigherHintRev` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M17-newer-stream.log` |
| M18-epoch-downgrade | `testOlderAndCrossEpochSnapshotsCannotResetCursor` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M18-epoch-downgrade.log` |
| M19-older-snapshot | `testOlderAndCrossEpochSnapshotsCannotResetCursor` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M19-older-snapshot.log` |
| M20-no-live | `testOneSnapshotPersistsOnlyBoardMetadataAndNeverMarksLive` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M20-no-live.log` |
| M21-one-snapshot | `testOneSnapshotPersistsOnlyBoardMetadataAndNeverMarksLive` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M21-one-snapshot.log` |
| M22-removed-host | `testRemovedHostCannotBeRecreatedByLateSnapshot` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M22-removed-host.log` |
| M23-os-availability | `testOSUnavailableAndNetworkErrorStayStaleWithoutRetry` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M23-os-availability.log` |
| M24-settings-wiring | `testSettingsUsesIndependentRefreshBindingAndTruthfulGuidance` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-M24-settings-wiring.log` |
| R201-runtime-posture | `testRuntimeCoordinatorMismatchRefusesHintWithoutQuota` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-R201-runtime-posture.log` |
| R202-foreign-stamp | `testForeignHostSnapshotIsRefusedBeforeStoreMutation` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-R202-foreign-stamp.log` |
| R203-stream-admission | `testAlreadyStreamingHintHasNoRequestsOrQuota` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-R203-stream-admission.log` |
| R204-live-mode | `testNonLiveModesRefuseHintWithoutQuota` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-R204-live-mode.log` |
| R205-quota-count | `testOversizedAndMalformedQuotaBlobsFailClosed` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-R205-quota-count.log` |
| R206-model-absent-completion | `testDelegateWithoutRestoredModelCompletesNoDataOnce` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-R206-model-absent-completion.log` |
| R207-finite-masked-by-decoder | `testOversizedAndMalformedQuotaBlobsFailClosed` | 0 | GREEN — masked, not discrimination | SHA256 identical | `/tmp/g546-r2-probe-R207-finite-masked-by-decoder.log` |
| R208-stream-masked-by-generation | `testStreamOpenedDuringHintPreventsLateApply` | 0 | GREEN — masked, not discrimination | SHA256 identical | `/tmp/g546-r2-probe-R208-stream-masked-by-generation.log` |
| R209-stream-and-generation | `testStreamOpenedDuringHintPreventsLateApply` | 65 | assertion RED | SHA256 identical | `/tmp/g546-r2-probe-R209-stream-and-generation.log` |

Two single-leg mutations are explicitly NOT assertion-discriminating:
- R207: default JSONDecoder cannot produce a nonfinite Double from the tampered
  numeric/string representations. The fixture also asserts these decoding
  failures directly. Removing `isFinite` therefore stays GREEN, because strict
  decoding already fails closed. R205 separately proves the array-count guard.
- R208: opening a real stream also increments `connectionGeneration`, so the
  generation leg still denies a late hint when only post-await `isStreaming`
  is removed. R203 independently proves the admission stream guard; R209
  removes both post-await legs to demonstrate the race fixture's discrimination.

These are disclosed backstop masking, not "all mutations RED" claims. No
production guard, decoder option or existing assertion was weakened to force a
red result. R206 (and original M13) may report a bounded-observation assertion
plus a thrown URLError; their xcodebuild processes must exit normally, not time out.

## Structural search evidence

Before editing:

    ast-grep outline ios/FleetNotifier/App/AppModel.swift --view expanded
    ast-grep run -l swift -p 'coordinator?.allowsLiveWork($$$ARGS)' ios/FleetNotifier/App/AppModel.swift

The outline command exited 0 (`/tmp/g546-r2-app-outline.log`).
The focused pattern exited 0 and returned the
four existing sibling calls at lines 1001, 2645, 3676 and 3810
(`/tmp/g546-r2-trust-sites.log`). This is invocation evidence, not skill availability.

## Delivery and unverified boundaries

The scratch was verified clean and removed after final GREEN. Each following command exited 0:

    git -C /tmp/g546-r2-scratch diff --exit-code > /tmp/g546-r2-scratch-clean.log 2>&1
    EXIT=0

    git worktree remove /tmp/g546-r2-scratch > /tmp/g546-r2-scratch-remove.log 2>&1
    EXIT=0

    git diff --exit-code 8b8e2fe6d80d32cf6eadbcab25d702e4a8c52ad0 -- ios/ > /tmp/g546-r2-source-identity.log 2>&1
    EXIT=0

Final commit, committed-diff check, normal feature-branch push and remote read-back are reported in the delivery message. No hash in this report purports to name its own evidence commit.

The client slice is ready for independent round-2 re-review, not full #546
acceptance. #544 remains unimplemented and unauthorized; no relay or fake relay,
APNs dispatch, deployment, daemon/Rust change, host/service/credential change,
PR or merge was performed. No hosted-CI result or actual device restoration,
background receipt, private-path reachability, battery impact or freshness was
verified. The physical-iPhone protocol remains NOT RUN with all results empty;
no new device-availability claim is made. These boundaries have not changed.
