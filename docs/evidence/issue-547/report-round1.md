# #547 — BLOCKED at contract preflight; no implementation delivered

Date: 2026-09-15.
Worktree: `/Users/jirathip/.herdr/worktrees/corral/impl547-foreground-reconnect`
Branch: `g547-foreground-reconnect`
Verified starting HEAD: `ed24e6f57075acf1e5b082c7cd29a931dfbfbd62`.

This is a blocker report, NOT an implementation PASS or a review-ready fix.
The existing serial reconnect remains unchanged. The baseline tests below do
not establish #547 acceptance. No app, test, checker, project, or daemon source
was edited. This report replaces the inherited unrelated #379 `.report.md`;
that prior report remains available at the exact starting commit above.

## Blocking contract conflicts

1. A truthful release-active `row visible` timestamp needs a renderer hook,
   outside the allowed file list. `ios/FleetNotifier/UI/FleetViews.swift` owns
   the single-host `agentRow` at lines 1506–1523 and composite `hostAgentRow`
   at lines 1643–1675. Neither has a visibility callback into the model. The
   section/row loops at lines 1531–1558 can omit collapsed rows entirely.
   `FleetStore.onAgentsChanged` is a data-application callback, NOT proof of
   visibility; `HostBoardProjection.boardRows` is likewise only a projection.
   Logging either as `row visible`, or using an arbitrary delayed main-queue
   callback, would fabricate the required measurement. The file fence allows
   neither editing `FleetViews.swift` nor replacing this stage with a proxy.

   Required re-brief: permit the narrowly scoped production row-visibility
   hook(s) in `FleetViews.swift`, with regression coverage, or explicitly
   defer the row-visible instrumentation criterion. No unsupported runtime
   view introspection/swizzling was substituted.

2. General parallel opening of pinned streams conflicts with mandatory
   existing tests in an out-of-fence file:
   `ios/FleetNotifierTests/PreflightRetryTests.swift`.
   - Coordinator mismatch: lines 270–278 require ZERO `/events` requests.
   - Coordinator unreachable/repeated-start: lines 321–324 and 362 require
     ZERO `/events` requests while the key-check ladder runs.
   - Active unreachable/repeated-start: line 710 requires ZERO `/events`.
   - Active mismatch: lines 728–736 require ZERO `/events` requests.
   - Active pull/preflight: line 762 requires ZERO `/events` before verification.

   Both classes are explicitly required by G1 and included in G2. They passed
   in the real baseline execution below. These are request-count assertions,
   not verify-before-apply assertions: a correctly buffered speculative stream
   would still contradict them. Updating only the allowed monolithic test
   file cannot update these contracts.

   Required re-brief: allow narrowly replacing obsolete serial-request
   assertions in `PreflightRetryTests.swift` with verified-buffering assertions,
   preserving mismatch terminality, no application/no Live before verification,
   read authorization, retry cadence and single ownership. Alternatively,
   explicitly choose a foreground-reconnect-only optimization that leaves
   never-verified cold starts serial. That is a narrower behavior split than
   general parallel opening; it was not silently chosen to evade the tests.

No source work was started across these boundaries. No existing checks were
weakened, disabled, skipped conditionally, or patched outside the fence.

## Files delivered

- `.report.md`: this explicit blocker and baseline-verification report.
- `docs/evidence/issue-547/device-protocol.md`: unexecuted capture handoff with
  sample counts and per-stage results template. It explicitly identifies the
  missing instrumentation and owner-provided reproducible Bazzite workload.
  It is NOT claimed to satisfy the exact loaded-host protocol criterion yet.

## Inspection evidence

The issue body and routed comment were read with the brief's exact commands:

    gh issue view 547 --json body -q .body
    gh issue view 547 --json comments -q '.comments[] | .author.login + ": " + .body'

The batch exited 0 and confirmed #547 precedes #545, no daemon change, and no
main/release/TestFlight promotion. The worktree and branch matched the brief.
No `justfile`, `Justfile`, root `CLAUDE.md`, `ios/AGENTS.md`, or `ios/CLAUDE.md`
was present at the exact checked paths; reading root `AGENTS.md` returned
file-not-found. No just recipe was invented.

Structural inspection used the following commands (not merely skill loading):

    ast-grep outline ios/FleetNotifier/App/FleetStore.swift ios/FleetNotifier/Profiles/HostStreamCoordinator.swift ios/FleetNotifier/App/AppModel.swift > /tmp/g547-outline.log 2>&1

Raw `OUTLINE_EXIT=0`; found FleetStore's connect/ingest/disconnect and per-host
startSessionIfNeeded/openStream plus AppModel startLive/stopLive/scene seam.

    ast-grep run -l swift -p 'func stream($$$ARGS) { $$$BODY }' ios/FleetNotifier/Network/CorraldClient.swift > /tmp/g547-stream-ast.log 2>&1

Raw `STREAM_AST_EXIT=0`; matched `CorraldClient.stream` at lines 169–273.
Its callbacks already expose SSE HTTP 200 and incoming frames; these network
stages do not require editing the network file.

    ast-grep outline ios/FleetNotifier/App/AppModel.swift --view expanded > /tmp/g547-app-outline.log
    ast-grep outline ios/FleetNotifier/UI/FleetViews.swift --view expanded > /tmp/g547-view-outline.log
    ast-grep outline ios/FleetNotifierTests/PreflightRetryTests.swift > /tmp/g547-preflight-outline.log

The batch's captured `AST_EXIT=0` is the final outline's raw status, not separate
status proof for every preceding command. The saved outputs identified the
view-row functions and the two required preflight test classes. Matched ranges
were then read directly. Exact-text searches located their serial assertions.

## Real baseline gates

All commands ran from the named worktree. The Xcode run used the brief's heavy
lock, exact simulator, direct-runner environment, and derived-data directory.

### G1: existing focused baseline — exit 0

    flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 \
      xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier \
      -destination "platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793" \
      -derivedDataPath /tmp/g547-dd \
      -only-testing:FleetNotifierTests/ScenePhaseLifecycleTests \
      -only-testing:FleetNotifierTests/EpochRecoveryTests \
      -only-testing:FleetNotifierTests/PreflightRetryCoordinatorTests \
      -only-testing:FleetNotifierTests/PreflightRetryActiveHostTests \
      -only-testing:FleetNotifierTests/SSEStreamRegressionTests \
      > /tmp/g547-focused.log 2>&1; echo FOCUSED_EXIT=$?

Raw output: `FOCUSED_EXIT=0`.
Log: `/tmp/g547-focused.log`.
Xcode result: `** TEST SUCCEEDED **`.
Test result: `Executed 31 tests, with 0 failures (0 unexpected) in 20.595 (20.614) seconds`.
Classes: EpochRecovery 7; active preflight 6; coordinator preflight 9; SSE
regression 2; scene lifecycle 7. These are test-execution durations, not
reconnect-stage performance measurements.

Deviation from the template: the `<YourNewClasses>` placeholder was omitted
because no new class exists; this is only the existing baseline portion of G1.
No new regression, buffered-frame test, or mutation discrimination is claimed.

### G3 source boundary — exit 0

    python3 ios/check-release-demo.py > /tmp/g547-check-release.log 2>&1; echo CHECK_RELEASE_EXIT=$?

Raw output: `CHECK_RELEASE_EXIT=0`.
Log output:

    source PASS: 7 native Swift renderer files; 8 unchanged approved icon inputs
    release-demo check: PASS (Debug source preserved; Release boundary verified)

### G3 self-test — first attempt timed out; separate completed run exit 0

    python3 ios/check-release-demo.py --self-test > /tmp/g547-self-test.log 2>&1; echo SELF_TEST_EXIT=$?

This was initially the next command in a 120-second terminal batch. The tool
returned exit 124 (`Command timed out after 120s`); no `SELF_TEST_EXIT` marker
was emitted and `/tmp/g547-self-test.log` was empty. There is NO raw checker
status for this incomplete invocation. The following diff-check in that batch
was not reached. The subsequent process snapshot showed no remaining
`check-release-demo.py` process.

One standalone retry with a 600-second terminal deadline completed:

    python3 ios/check-release-demo.py --self-test > /tmp/g547-self-test-retry.log 2>&1; echo SELF_TEST_EXIT=$?

Raw output: `SELF_TEST_EXIT=0`.
Log: `/tmp/g547-self-test-retry.log`.
Output was the same two PASS lines as the source boundary check. This completed
invocation, not the timeout, is the baseline self-test evidence.

### G7 hygiene — exit 0 before report edits

    git diff --check > /tmp/g547-diffcheck.log 2>&1; echo DIFFCHECK_EXIT=$?

Raw output: `DIFFCHECK_EXIT=0`; empty log. Delivery hygiene is checked again
when staging the documentation-only report; its status is reported at closeout.

### Gates not run

- G2 full iOS suite: NOT RUN (no implementable candidate within the fence).
- G4 separate Debug/Release builds and Release binary proof: NOT RUN. G1
  compiled the baseline Debug test target, not the separate required G4 builds.
- G5 XcodeGen/drift: NOT RUN; no project regeneration or new Swift files.
- G6 anti-slop base/head: NOT RUN. No Swift diff, but no scanner result claimed.
- G8 mutation battery: NOT RUN; no implementation or new tests to discriminate.
  Verify-before-apply, parallel-open and cap probes are all missing; no scratch
  worktree or restore-hash proof is claimed.
- Digest re-pin: not applicable to this documentation-only change; app/test
  source bytes were not changed and existing source gate passed.

## Timing inventory and acceptance status

No #547 instrumentation shipped. All seven requested stage marks (path-ready,
key request, key response, SSE HTTP 200, first frame received, applied, row
visible) remain to be implemented. There are zero collected timing samples;
all per-stage medians are MISSING on simulator and physical device. The test
suite duration above must not be used as any stage median.

Delta resume: source inspection confirms `CorraldClient.stream` sets
`Last-Event-ID` and `Corral-Epoch` at lines 195–198. FleetStore.connect retains
an in-memory populated delta base and its cursor (lines 590–598), and #450's
snapshot-only epoch authority is in accepts/applyStreamFrame. The existing
EpochRecoveryTests and scene cursor/header tests passed. That is NOT the new
required foreground parallel-buffer delta-resume proof, which remains missing.

Required work status:

1. Timing instrumentation and measured per-stage medians: NOT IMPLEMENTED.
   Physical-iPhone before/after Mac/loaded-Bazzite data is HUMAN/DEVICE-GATED;
   row visibility additionally needs file-fence authorization. Protocol handoff
   provided, but exact owner workload missing.
2. Parallel stream plus fail-closed verification: NOT IMPLEMENTED.
3. New delta-resume regression and parallel-path proof: NOT IMPLEMENTED.
4. Retained-board visible/reconnecting UI proof: NOT CAPTURED. No rendered UI
   or updated-row visibility is claimed from model inspection.
5. New per-host slow/healthy independence proof: NOT IMPLEMENTED. Existing
   baseline coordinator tests passed, not the new concurrent-preflight proof.
6. Cap plus expiry bounded buffering: NOT IMPLEMENTED.
7. Required RED/GREEN probes: NOT RUN.

## Non-claims / handoff

- No physical device attached: `xcrun devicectl list devices` returned
  `No devices found.` No simulator timing, real-host, loaded-Bazzite,
  authenticated delta transport, or physical-device result is claimed.
- No hosted CI, PR, independent review, issue mutation, merge, main/release
  promotion, TestFlight, APNs/background-execution, daemon or Rust change.
- The test simulator is now verified Shutdown. A cleanup shutdown request
  found it already Shutdown (CoreSimulator error 405); the subsequent
  `xcrun simctl list devices available` confirmed that state. No other
  simulator was stopped.
- `.hermes-context.md` was already untracked at entry and is preserved, not
  deleted or hidden by editing shared Git excludes. Consequently the brief's
  completely clean `git status --porcelain` criterion is NOT met; this
  pre-existing control file must be accounted for by the orchestrator.
- The branch delivery is documentation/blocker-only. Exact local/remote SHAs
  and push verification are reported in the final message. Do not route this
  as a completed implementation or promote it as #547 acceptance.
