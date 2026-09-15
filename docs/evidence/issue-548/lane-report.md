# Corral #548 — reserved Herd rail zone (selected design A)

Branch: `g548-herd-rail-zone`
Worktree: `/Users/jirathip/.herdr/worktrees/corral/impl548-herd-rail-zone`
Pinned base: `ed24e6f57075acf1e5b082c7cd29a931dfbfbd62`
Tested implementation commit: `e1beb6d262e45ae9e9b4b34746a15bc977fffa9a`

This replaces the inherited #379 report, which remains available at the pinned base. The final evidence/report commit changes no app, test, or pin bytes from the tested implementation commit; `docs/evidence/issue-548/tested-sources.json` records their SHA-256 values. No integration refresh was requested: this lane remains on the exact briefed base.

## Implementation map

- `ios/FleetNotifier/UI/Herd/HerdView.swift:326-355`: one intrinsic, occupancy-independent rail reservation. It measures the existing 108pt art slot plus the shared actual card caption at the current Dynamic Type size. The longest rail status, `? unknown · last known blocked`, reserves room for a disconnected blocked horse. A host caption line is included in multi-host scopes. There is no guessed total-height constant.
- `HerdView.swift:358-389`: compact left-aligned `ranchChromeSurface` repository chip under the HUD, followed by the reserved zone and paddock. Accessibility sizes scroll this entire column between the pinned HUD and navigation. The old spacer and empty label/placeholder fence are gone.
- `HerdView.swift:391-418,449-461`: preserve repository paging; avoid nested vertical scrolling on the accessibility path; share caption metrics between measurement and actual cards. Rail name/host lines are bounded to one line, with full existing accessibility text retained. Rail width remains 164pt normally and is 240pt at accessibility sizes to accommodate the caption.
- `HerdView.swift:464-497`: keep the original `Button { select(horse) }`, per-card fence/flag, disconnected disablement, art and motion calls, accessibility label and recent-output hint. The non-empty container retains `Global blocked front rail`; the empty zone is accessibility-hidden. No new colors or HUD counter changes.
- `HerdView.swift:501-523`: repository suffix is present only for a positive rail count; DEBUG-only frame preferences measure the production views, not a parallel geometry model.
- `ios/FleetNotifierTests/HerdTests.swift:263-407`: one deterministic rendered-layout test covering Day/Night × empty/blocked/last-known × default/accessibility3, including a real accessibility scroll. Two companion tests check empty-chrome/callback wiring and zero/positive suffix behavior. Existing shell wiring assertions were updated for the reservation name/removal of the spacer.
- `ios/check-release-demo.py`: only the approved Release-source digest and dated #548 comment changed. No checker weakening or test-source pin change.
- `docs/evidence/issue-548/`: reproducible capture/export/comparison/probe tooling and recorded artifacts. No new Swift files or project changes.

`untouched.json` records byte-identical base/head hashes for HerdModel, RanchEnvironment, HerdArt, HerdAmbientPolicy, HerdEnvironmentChoice and the other lane's FleetNotifierTests.swift. The source diff is confined to the three allowed iOS files above.

## Measured layout evidence

Method: real production `HerdView` in a scene-attached `UIWindow`, `GeometryReader` preference frames at the first field card (`field-0`) and reservation, and XCTest attachments. Motion is disabled through the existing theme provider. These are native UIKit/SwiftUI window captures, not HTML or redrawn mockups. `UIGraphicsImageRenderer` uses scale 1, producing original 390×844 PNGs; no post-capture cropping/resizing is used for the evidence images. They are point-resolution app-window images, not @3x OS/status-bar screenshots.

The true 390×844-point iPhone 14 simulator capture used installed iOS 26.5, template `6F4F7E87-2886-49B4-AD01-FF261D7705C7`. Its safe-area top was 47pt. Day and Night both measured:

| Type | Empty first row in window | Blocked | Last-known | Reserved height | Actual longest card |
| --- | ---: | ---: | ---: | ---: | ---: |
| large | 337.333333pt | 337.333333pt | 337.333333pt | 175.666667pt | 175.666667pt |
| accessibility3 | 524.333333pt | 524.333333pt | 524.333333pt | 259.666667pt | 259.666667pt |

First-row origins are exactly equal across each three-state measurement set, not judged by eye. Card top-alignment is checked with 0.01pt tolerance for floating-point rounding; occupancy-origin equality is a separate exact assertion. The reservation equals the measured longest actual rail card at both tested sizes.

Accessibility scroll: the real scroll view moved by 200pt; first-row local Y moved from 477.333333 to 277.333333pt. The chip and blocked card are fully visible in the initial accessibility frame; the scrolled frame exposes subsequent paddock content. The pre-existing Filters scope label can ellipsize at this size; it was not modified. No new chip/rail chrome is clipped at the captured sizes.

### Paired before/after control

Base and candidate were BOTH measured on the brief's iPhone 16 simulator `59DDC0C5-891E-4EC0-91AF-4F50DF68D793`, with the SAME 390×844 window and 59pt top safe-area inset. This paired control is separate from the iPhone 14 evidence above; do not subtract positions across different simulators.

| Build | Empty first row in window | Blocked | Last-known |
| --- | ---: | ---: | ---: |
| Base ed24e6f, Day and Night | 361.000000pt | 546.000000pt | 546.000000pt |
| Candidate, Day and Night | 349.333333pt | 349.333333pt | 349.333333pt |

The empty-state first row is 11.666667pt higher than base. In local Herd coordinates this is 302.000000 → 290.333333pt. The base control demonstrates an occupancy-dependent jump; the candidate does not.

The separate `/tmp/g548-base` worktree remains pinned to ed24e6f, with only measurement instrumentation and its fixture added. `base-measurement.patch` preserves those changes (`git apply --unidiff-zero` in a fresh base checkout). Reverse-apply validation against the instrumented scratch tree exited 0. This base run is a measurement control, NOT a claimed assertion-RED run of the new tests.

The empty Day/Night images visibly contain neither FRONT RAIL label nor placeholder top fence. The lower ranch scenery is source-byte-identical. Additionally, the unobscured lower-fence edge crop `[0,328,10,388]` in the paired empty frames has zero changed pixels in both themes. This is a bounded scenery comparison, not a claim that foreground cards never occlude the fence or that whole screenshots are identical.

`python3 docs/evidence/issue-548/verify.py` exited 0, log `/tmp/g548-verify-evidence.log`; full numbers are in `verification.json`. It uses already-installed Pillow 12.3.0 for the crop comparison; no package or app dependency was installed/changed.

## Evidence inventory

Original files: `/tmp/g548-evidence/native/`.

- `day-empty-large.png`, `day-blocked-large.png`, `day-last-known-large.png`
- `night-empty-large.png`, `night-blocked-large.png`, `night-last-known-large.png`
- `day-blocked-accessibility3.png`, `day-blocked-accessibility3-scrolled.png`
- Additional generated accessibility frames for the other states/themes are retained.

All 13 native attachments and the four Day/Night paired empty control images are archived under `docs/evidence/issue-548/{native,base,head}/`. `images.json` lists all 17 original PNG paths, dimensions, simulator names and SHA-256 values. `capture.json` files preserve raw measurement maps and xcresult provenance; non-archived control states still point to their original `/tmp` captures. All six default native states and both accessibility frames named above were visually inspected.

Existing-tool help was run:

- `python3 ios/tools/herd-art/capture.py --help` → 0, `/tmp/g548-capture-help-final.log`.
- `python3 ios/tools/herd-art/probe-native.py --help` → 0, `/tmp/g548-probe-help-final.log`.

The fixture needs the three exact rail populations without touching the forbidden App/Demo files. `capture-device.py` therefore reuses the existing capture tool's bounded simctl helper and the committed XCTest renderer. The original `-corralHerdEvidence` launch route was not modified or claimed as the fixture source.

Native capture invocation:

`flock /tmp/n.lock python3 docs/evidence/issue-548/capture-device.py`

Outer raw exit **1**, log `/tmp/g548-native-device.log`: tests and attachment export succeeded, then cleanup attempted to shut down an already-shutdown template (simctl 149). Deletion still succeeded and was independently verified absent by a fresh simctl device list (`/tmp/g548-native-deletion.log`, exit 0). No failed-build artifact was installed or substituted.

Successful child command (raw exit 0; 3 tests, 0 failures; `/tmp/g548-native-capture.log`):

`env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination platform=iOS\ Simulator,id=6F4F7E87-2886-49B4-AD01-FF261D7705C7 -derivedDataPath /tmp/g548-dd -only-testing:FleetNotifierTests/HerdRailZoneTests`

Export command (exit 0):

`python3 docs/evidence/issue-548/collect.py --log /tmp/g548-native-capture.log --output /tmp/g548-evidence/native`

It invoked `xcrun xcresulttool export attachments --test-id 'HerdRailZoneTests/testReservedZoneKeepsFirstHorseRowFixed()' --path /tmp/g548-dd/Logs/Test/Test-FleetNotifier-2569.09.15_8-08-47-+0700.xcresult --output-path /tmp/g548-evidence/native/attachments`, exit 0, 13 attachments. Complete child argv/device metadata are in `native-command.json`.

Cleanup now checks the owned template's state before shutdown. `python3 docs/evidence/issue-548/check-capture-cleanup.py` exited 0 (`/tmp/g548-cleanup-unit.log`), checking Booted/Shutdown paths with explicitly mocked simctl calls. This is not a second full capture run; the original outer exit 1 remains disclosed. No expensive recapture was needed after successful tests/export.

## Exact gates and raw results

All commands below ran from the worktree root unless stated. Heavy commands were serialized under `/tmp/n.lock`. `/tmp/g548-run.py` supplied bounded waits and recorded exact argv, cwd, raw exit, duration and log in `/tmp/g548-commands.jsonl` (archived as `commands.jsonl`). No piped display command supplied a gate verdict. No repo-owned justfile was found; the brief's direct commands are authoritative.

### G1 — focused, exit 0

`flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g548-dd -only-testing:FleetNotifierTests/HerdTests -only-testing:FleetNotifierTests/FullScreenHerdShellWiringTests -only-testing:FleetNotifierTests/FullScreenHerdShellAccessibilityLayoutTests -only-testing:FleetNotifierTests/HerdRailZoneTests`

`/tmp/g548-focused-r3.log`: `Executed 35 tests, with 0 failures`; `** TEST SUCCEEDED **`; 101.32s including runner overhead.

### G2 — full iOS suite, exit 0

`flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -destination 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793' -derivedDataPath /tmp/g548-dd -only-testing:FleetNotifierTests`

`/tmp/g548-full.log`: `Executed 617 tests, with 0 failures`; `** TEST SUCCEEDED **`; 188.13s. The actual current suite count is 617, not the brief's stale 181-test reference.

### G3 — Release boundary and standalone negative self-test, exits 0 / 0

- `python3 ios/check-release-demo.py` — `/tmp/g548-check-release-final.log`, 1.86s.
- `python3 ios/check-release-demo.py --self-test` — `/tmp/g548-self-test-final.log`, 112.71s; standalone 600s bound.

Both printed `source PASS: 7 native Swift renderer files; 8 unchanged approved icon inputs` and `release-demo check: PASS (Debug source preserved; Release boundary verified)`.

### G4 — Debug / Release / binary, exits 0 / 0 / 0

`flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild build -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -configuration Debug -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/g548-debug-dd`

`/tmp/g548-debug.log`, 41.91s, `** BUILD SUCCEEDED **`.

`flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild build -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier -configuration Release -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/g548-release-dd CODE_SIGNING_ALLOWED=NO`

`/tmp/g548-release.log`, 63.28s, `** BUILD SUCCEEDED **`.

`python3 ios/check-release-demo.py --binary /tmp/g548-release-dd/Build/Products/Release-iphonesimulator/FleetNotifier.app/FleetNotifier`

`/tmp/g548-binary.log`, 2.47s: `bundle PASS: Mach-O ...; 6 product files`, approved catalog names and `release-demo check: PASS (Debug source preserved; Release boundary verified)`.

### G5 — project-generation drift, exits 0 / 0

- `(cd ios && xcodegen generate --spec project.yml)` — `/tmp/g548-xcodegen-final.log`.
- `git diff --exit-code -- ios/` — `/tmp/g548-xcodegen-diff-final.log`, empty diff.

### G6 — advisory anti-slop, raw exit 1; zero added identities

`swift run --package-path ios/tools/anti-slop-swift anti-slop ios/FleetNotifier ios/FleetNotifierTests`

Final committed-source run: `/tmp/g548-aslop-committed.log`, exit 1, 57.40s. Base log: `/tmp/g548-aslop-base.log`. Both contain the same 24 diagnostic identities (path + complete diagnostic, with line/column movement ignored): 18 no-force-unwrap, 1 no-swallowed-errors, 4 no-shape-in-symbol-names, 1 no-force-try.

`python3 docs/evidence/issue-548/compare-slop.py` → exit 0, `/tmp/g548-aslop-delta-final.log`: `added: []`, `removed: []`, `PASS: zero added diagnostic identities`. No suppression, rule change or baseline reset was used.

### G7 — hygiene, exit 0

`git diff --check` → `/tmp/g548-hygiene.log`, exit 0. Additional committed-range/tip checks are recorded in the final delivery audit; a clean working-tree check alone is not treated as committed-diff proof.

### G8 — scratch mutation discrimination

`bash /tmp/g548-probes.sh` → raw exit 0, `/tmp/g548-probes.log`, 586.03s (including lock wait). The script changes to the implementation checkout and invokes `flock /tmp/n.lock python3 docs/evidence/issue-548/probes.py`.

The driver created the fresh detached `/tmp/g548-scratch` worktree at e1beb6d, never mutated the implementation checkout, and ran each focused xcodebuild with `HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1`, the brief's iPhone 16 destination, and `/tmp/g548-probe-dd`. `probe-results.json` records every exact argv, cwd, head, exit, duration and assertion excerpt.

| Mutation | Raw exit | Assertion observed | Log under /tmp/g548-evidence/probes/ |
| --- | ---: | --- | --- |
| Remove the empty-state reservation | 65 | occupancy produced two distinct first-row origins; reservation became 0pt | reservation.log |
| Restore empty FRONT RAIL label | 65 | empty rail must never bring back the label | label.log |
| Restore empty placeholder fence | 65 | empty rail must never bring back the placeholder fence | fence.log |
| Show suffix when count is zero | 65 | `canter · 15 here · 0 at rail` differs from `canter · 15 here` | zero-suffix.log |
| Omit suffix when count is one | 65 | `canter · 15 here` differs from `canter · 15 here · 1 at rail` | positive-suffix.log |
| Restored pristine three-test battery | 0 | `Executed 3 tests, with 0 failures`; `** TEST SUCCEEDED **` | restored-green.log |

These are assertion failures, not compiler/infrastructure REDs. Before and after EACH mutation the driver ran `shasum -a 256 /tmp/g548-scratch/ios/FleetNotifier/UI/Herd/HerdView.swift`; after each restore it also required `git diff --exit-code -- ios/FleetNotifier/UI/Herd/HerdView.swift` to exit 0. All 10 before/after hash lines equal `494cf16fb71c17e1b23866a39d649dd1a5a6e148c3eaec9baab72bb184a9a27a`, also matching the delivered implementation source. The scratch worktree was independently checked clean after the final GREEN. It remains available for inspection.

## Structural search evidence

`ast-grep run --pattern 'horseCaption($$$)' --lang swift ios/FleetNotifier/UI/Herd/HerdView.swift` → exit 0, `/tmp/g548-structural.log`: two call sites at lines 334 and 485, confirming reservation and real cards use the shared caption. Earlier exploration used ast-grep outlines before the focused reads. Ordinary text/log searches were used for literal labels and saved test output.

## Earlier diagnostics and non-claims

- Initial focused run `/tmp/g548-focused.log`: raw 65, an existing count-summary pixel-band assertion found 6 bands instead of 5. The affected assertion was not weakened; final focused and full runs passed. No unsupported host-load attribution is made.
- Second focused run `/tmp/g548-focused-r2.log`: raw 65 because top coordinates 106.66666666666666 and 106.66666666666669 failed a strict greater-than-or-equal comparison. It became a 0.01pt alignment equality; the separate exact occupancy-origin invariant remains intact and is mutation-probed.
- Base measurement setup had compiler/setup iterations before the successful r3 control. `/tmp/g548-base-measure-r3.log` is the authoritative successful base measurement (raw 0, 523.04s including lock wait).
- The native capture wrapper's cleanup-only exit 1 is disclosed above; its test/export children passed and all captured bytes were verified. The cleanup fix has unit coverage, not a full recapture claim.
- Native synthetic-fixture window evidence only: no physical iPhone, live fleet, network pairing, hosted CI, TestFlight or release operation. Physical-iPhone checks of empty and blocked states remain the OWNER's gate.
- Callback/sheet preservation and VoiceOver labels are source-wiring tested; no real user tap, spoken VoiceOver session or physical scroll gesture is claimed. The accessibility scroll measurement is a real native scroll-view offset change.
- No PR creation, issue mutation, merge, main/integration push or integration reconciliation. Independent exact-head review and any later integration pin recomputation belong to the orchestrator.

## Delivery audit

The untracked dispatch input `.hermes-context.md` was archived byte-identically to `/tmp/g548-hermes-context.md` (SHA-256 `0fdec2fb5302302f0af02bf6ab8a45f4b1e8b0b7c2bb615f496c0f27570d633a`) rather than committed or ignored through shared repository configuration. `.brief.md` remains in the worktree.

The final scoped delivery log is `/tmp/g548-delivery.log`: it records committed base-range/tip whitespace checks, source/image hash and file-fence validation, the normal `git push -u origin g548-herd-rail-zone`, `git rev-parse HEAD`, `git rev-parse origin/g548-herd-rail-zone`, remote `git ls-remote --heads` equality, and a genuinely empty `git status --porcelain`. Final delivery SHA values are reported outside this immutable report so they do not self-reference their own commit.

No implementation gate remains blocked. The capture-wrapper cleanup-only failure and the owner/CI/interaction non-claims remain disclosed above; they have not been relabeled as green full-capture or device evidence.
