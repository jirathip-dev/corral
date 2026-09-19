# Evidence — corral issue #574 (build-32 horizontal-pager lag)

Fixture/measurement bundle for the #574 lane (`g574-pager-perf`). The measurement
harness is the DEBUG-only, launch-arg-gated pager-perf battery:

- counters + fixture: `ios/FleetNotifier/Demo/HerdEvidence.swift` (`HerdPerf574`, `-corral574Perf`)
- increment sites: `ios/FleetNotifier/UI/Herd/HerdView.swift`, `ios/FleetNotifier/UI/Herd/RanchEnvironment.swift`
- gesture battery: `ios/FleetNotifierUITests/HerdPagerPerfTests.swift` (skipped unless `CORRAL574_PERF=1` is present in the test runner environment — it is NOT set on CI, so the shared scheme's bare `xcodebuild test` skips the battery)

## What is in here

| path | what |
| --- | --- |
| `REPORT.md` | the lane report: tables, verdict, gates, gaps |
| `dumps/<arm>/*.json` | raw per-phase counter dumps copied out of the app container (`Documents/herd-perf/`) |
| `hostload/<arm>.txt` | host load before/at-gesture-start/after each arm run (uptime, ncpu, top CPU consumers) |
| `gestures/<arm>.log` | the runner's full xcodebuild log for the gesture battery (phase markers `G574_DUMP_BEGIN/END`) |
| `parity/` | frozen-status-bar screenshots of the parked fixture before/after the fix + the pixel diff |
| `video/` | the fixed-head simulator video (see `REPORT.md` for path/size/duration/hash) and extracted frames |

## Arms

| arm | tree | code |
| --- | --- | --- |
| `A1-head` / `A2-head` | `g574-pager-perf` worktree @ pre-fix head | build-32 client code (base `integration` @ `7ac34bf`) |
| `B-base` | `/tmp/fn574-base` scratch checkout of `7d79581` (build 31) with the same instrumentation layer |
| `F-fixed` / `F2-fixed` / `F3-fixed` | `g574-pager-perf` worktree at the fixed head | fix 1 (scroll channel), fix 2 (enumerated pager index), fix 3 (hoisted field row stack) |

## Reproduction (per arm)

```sh
# 1. boot the lane's simulator (iPhone 17 Pro, iOS 26.5)
xcrun simctl boot <UDID> && xcrun simctl bootstatus <UDID> -b

# 2. build + run the gesture battery, collecting dumps + host load
bash run-arm.sh <arm-label> <worktree-root> <derived-data-dir>
#    RUN_INTERNALS: xcodebuild build-for-testing, then
#    TEST_RUNNER_CORRAL574_PERF=1 xcodebuild test-without-building \
#      -only-testing:FleetNotifierUITests/HerdPagerPerfTests
#    (the TEST_RUNNER_ prefix is how xcodebuild forwards CORRAL574_PERF to the
#    runner process; the app-under-test gets `-corralHerdEvidence -corral574Perf`)

# 3. aggregate
python3 gen-tables.py runs/<arm>
```

The fixture in every arm: `build-32 fleet size: 40 agents, 12 repositories,
155-worktree-fact frame shape, 3 blocked at rail` seeded deterministically by
`HerdEvidence.pagerFixture()`.
