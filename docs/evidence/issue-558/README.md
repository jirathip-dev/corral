# #558 Recent-output sheet worktree block

Scope: client-only sheet block and the three optional Workspace decodes authorized by Amendment 1. The blocker receipt at `43dc8ac64163367e7e31aaa131278bd9c303e25c` remains in history. Base integration: `74c96a1d57528083e67df7fd24de9764e21373f9`.

## Rendered evidence

These are simulator-native XCTest renders, not physical-device or live-server evidence. `RecentWorktreeBlockTests` decodes a fictional Agent JSON payload, injects it through the existing `AppModel.enterDemo` / `FleetStore.seedDemo` path, and hosts the real `RecentOutputSheet` over `FleetView`. UIKit selects the real medium/large page-sheet detent; the production header row, detents, motions, and recent-output list are untouched. The existing demo supplies the tail text.

Simulator: iPhone 16, iOS 26.5, `59DDC0C5-891E-4EC0-91AF-4F50DF68D793`. The test's UIWindow is 390×844 points; its unresized native captures are 1170×2532 pixels at 3×. This is a controlled test-window size, not a claim that the simulator device's physical screen is 390×844. GitHub component renders are 358×220 points, 1074×660 pixels, made with SwiftUI ImageRenderer on the simulator.

- `day-{medium,large}-default.png`, `night-{medium,large}-default.png`: all four facts rows, long branch middle truncation, long subject limited to two lines, PR/verdict/first-three closing issues plus `+2`.
- `day-{medium,large}-ax3.png`, `night-{medium,large}-ax3.png`: accessibility3. The GitHub row naturally wraps below the PR/verdict when the horizontal version cannot fit; all three issue numbers and `+2` remain visible. At medium AX3 the facts consume the available sheet body; expand to large to see the tail. No change to the existing header's accessibility-size ellipsizing is claimed.
- `day-medium-absent.png`, `night-medium-absent.png`: older payload with no git/GitHub facts. The block is absent; the original caption strip directly precedes recent output. No placeholder, dash, clean label, or zero count is invented.
- `github-passing.png`, `github-failing.png`, `github-pending.png`: the three CI verdicts in words using the existing green/red/yellow tokens. All include `#558` and `closes #45, #46, #47 +2`.

`captures.json` records every capture's SHA-256, dimensions, tested-source digests, and provenance. Thirteen captures were counted and verified. Images were visually inspected; the tests additionally recognize rendered text and assert the overflow count remains visible.

## Fixture and regression coverage

All coverage lives in the existing `ios/FleetNotifierTests/FleetNotifierTests.swift` file, class `RecentWorktreeBlockTests`; no new Swift file or project change.

- Exact single-label value assertion includes branch, dirty, ahead/behind, seven-character SHA, first-line subject, basename, PR number, CI words, and capped closes-list. The same row inventory drives the renderer and accessibility label.
- Omitted and explicit-null `head_sha`, `head_subject`, and `issues` decode to nil; missing facts produce no rows or label.
- Each commit field renders independently; zero counts and false dirty do not create facts. Worktree basename reuses the Board's existing helper, including duplicate-branch suppression.
- Bound success/failure/pending, unknown and absent CI, unbound orphan CI/issues, and server-cleared stale projection are tested. Unknown and absent CI produce byte-identical component pixels; unbound orphan values produce the same empty pixels as an empty workspace.
- Long and doubled-long subjects have the same two-line height, matching an independent two-line caption reference at both default and AX3 sizes. Measured heights: default 36.333333333333336 pt; AX3 92 pt.
- The server freshness predicate remains at `src/core/store.rs:620-633`. This lane inspects that existing predicate and tests its absent-binding output in the client; it does not run a live stale-binding integration or claim client age handling (#564).

Initial focused run: exit 65, one newly-authored assertion incorrectly estimated two-line height as twice one-line height plus 1 pt. Actual font leading made that estimate wrong; the test now compares an independently rendered two-line caption. Visual inspection also found that the initial horizontal GitHub layout hid `+2` at AX3; the adaptive layout fixes that, with an added rendered `+2` assertion. No pre-existing assertion or expectation was edited. Final focused run: exit 0, 7 tests, 0 failures (`/tmp/g558-focused-adaptive.log`).

## Reproduce

From the repository root, with a booted simulator that this lane owns and the host's heavy-command lock held:

```sh
HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test \
  -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier \
  -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' \
  -destination-timeout 60 -derivedDataPath /tmp/g558-dd \
  -parallel-testing-enabled NO \
  -only-testing:FleetNotifierTests/RecentWorktreeBlockTests CODE_SIGNING_ALLOWED=NO
python3 docs/evidence/issue-558/verify.py
```

The actual lane commands are serialized with `flock /tmp/n.lock`; bounded command logs and raw exits live under `/tmp/g558-*.log`. The manual simulator path avoids the host wrapper's disposable-device deletion, which this brief forbids. Only the named simulator was booted/shut down; no device was deleted.

The verifier checks the original header/Board/detent/motion/list bytes, every existing assertion, append-only report history, generated digest pins, allowed changed paths, and all capture hashes. It does not substitute for native tests or visual inspection. See the appended `.report.md` entry and final delivery message for native gates and mutation results.
