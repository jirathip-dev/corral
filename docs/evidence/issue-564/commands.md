# #564 exact gate receipts

Captured subprocess statuses (not pipeline statuses). Base `ff193c718c745b70ef7d7bd4487668c73bd71bfb`
(= `origin/integration` at lane start). Implementation commit `b688b6fe`; this evidence is committed after it.

| Gate | Command | Exit | Log |
| --- | --- | ---: | --- |
| parse | `swiftc -parse ios/FleetNotifier/Models/Models.swift ios/FleetNotifier/UI/FleetViews.swift ios/FleetNotifierTests/WorktreeFreshnessTests.swift ios/FleetNotifierTests/WorktreeFreshnessConsumerTests.swift` | 0 | (stdout empty) |
| xcodegen | `xcodegen generate --spec ios/project.yml` | 0 | `git diff --stat`: 8 added lines (the two new test files) |
| release-source | `python3 ios/check-release-demo.py` | 0 | `release-source.log` — `release-demo check: PASS` |
| release-self-test | `python3 ios/check-release-demo.py --self-test` | 0 | `release-demo check: PASS (Debug source preserved; Release boundary verified)` |
| focused HEAD (GREEN) | `hermes-sim-task -c "bash /tmp/g564/run_tests.sh <wt> /tmp/g564-dd -only-testing:FleetNotifierTests/WorktreeFreshnessTests -only-testing:FleetNotifierTests/WorktreeFreshnessConsumerTests"` | 0 | `head-focused.log.gz` — `Executed 15 tests, with 0 failures` |
| full HEAD suite | `hermes-sim-task -c "bash /tmp/g564/run_tests.sh <wt> /tmp/g564-dd -only-testing:FleetNotifierTests"` | 0 | `head-full.log.gz` — `Executed 680 tests, with 1 test skipped and 0 failures (0 unexpected)` |
| focused BASE (RED) | `hermes-sim-task -c "bash /tmp/g564/run_tests.sh /tmp/g564-base /tmp/g564-basedd -only-testing:FleetNotifierTests/WorktreeFreshnessConsumerTests"` | 65 | `base-focused.log.gz` — `Executed 6 tests, with 8 failures (0 unexpected)` at `ff193c71` |
| xcodegen drift | `git diff --exit-code -- ios/FleetNotifier.xcodeproj/project.pbxproj` after a fresh `xcodegen generate` | 0 | regenerated project committed |
| anti-slop base | `swift run --package-path ios/tools/anti-slop-swift --scratch-path /tmp/g564-slop anti-slop /tmp/g564/slop-base/...` | 0 | zero findings; see `anti-slop-delta.json` |
| anti-slop head | same tool over the head `Models.swift`, `FleetViews.swift` + both new test files | 0 | zero findings |
| anti-slop control | same tool over `ios/FleetNotifier ios/FleetNotifierTests` (positive control that the tool reports) | 1 | `anti-slop-control-repo-wide.log.gz` — `[anti-slop] 24 violations across 55 files (17 rules)`, none in the four lane files |
| wire probe | `swiftc -o /tmp/g564/wireprobe/probe ios/FleetNotifier/Models/Models.swift /tmp/g564/wireprobe/main.swift` then the probe over four real daemon fixtures | 0 (each) | `wire-probe-real-daemon-fixtures.log` |
| mutation M1 | gate removed from `FleetViews.swift:526`, focused consumer class | 65 | `mutation.log.gz` — `Executed 6 tests, with 6 failures`; restore sha256 + `git diff --exit-code` 0 |
| mutation M2 | snapshot resolution removed from `Models.swift:362-364`, model + consumer classes | 65 | `mutation.log.gz` — `Executed 15 tests, with 5 failures`; same restore proof |
| source fence | `git diff --stat ff193c71..b688b6fe -- src/ Cargo.toml` | 0 | EMPTY (no daemon/wire change) |

`<wt>` = `/Users/jirathip/.herdr/worktrees/corral/impl564-freshness`. The base scratch tree is a
`git worktree add --detach /tmp/g564-base ff193c718c745b70ef7d7bd4487668c73bd71bfb` checkout holding ONLY
`WorktreeFreshnessConsumerTests.swift` (the base-compilable test file; the model suite uses the new API and cannot
compile at base). Simulator lifecycle is owned by `hermes-sim-task` (one private temporary simulator per
invocation, deleted by the wrapper); heavy legs ran under `flock /tmp/g564.lock`; the base scratch worktree and its
derived data were removed after the RED run.

## Render hashes (harvested from the host app container after the full-suite run)

| Image | SHA-256 |
| --- | --- |
| `564-stale-fact.png` | `cebb14cfba7572d0fa4097eb032868db7377ccf129beaeea55e4729536da320b` |
| `564-nofact-no-map.png` | `cebb14cfba7572d0fa4097eb032868db7377ccf129beaeea55e4729536da320b` |
| `564-nofact-no-row.png` | `cebb14cfba7572d0fa4097eb032868db7377ccf129beaeea55e4729536da320b` |
| `564-absent-reference.png` | `cebb14cfba7572d0fa4097eb032868db7377ccf129beaeea55e4729536da320b` |
| `564-fresh-reference.png` | `cebb14cfba7572d0fa4097eb032868db7377ccf129beaeea55e4729536da320b` |
| `564-nofact-reference.png` | `cebb14cfba7572d0fa4097eb032868db7377ccf129beaeea55e4729536da320b` |
| `564-geometry-stale.png` | `cebb14cfba7572d0fa4097eb032868db7377ccf129beaeea55e4729536da320b` |
| `564-geometry-absent.png` | `cebb14cfba7572d0fa4097eb032868db7377ccf129beaeea55e4729536da320b` |
| `564-fresh-fact.png` | `d3b3fec280f6c0fbfd0b5703beecc8a0319b39c68dd4b2d0d5994a7d89941cb0` |
| `564-geometry-fresh.png` | `d3b3fec280f6c0fbfd0b5703beecc8a0319b39c68dd4b2d0d5994a7d89941cb0` |

Every stale / no-fact render is byte-identical to the no-positive-fact reference; only the fresh frame paints the
signal. Renders are 390 pt wide at scale 3 (1170 px), written by the production `WorkspaceLine` through
`ImageRenderer` in the test host app; the captured row measured 36.333333333333336 pt high in all three states and
the leading 90 pt band was byte-identical across fresh/stale/absent (`G564_GEOMETRY` in `head-full.log.gz`).
