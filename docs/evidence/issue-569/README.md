# #569 — sheet polish: Board repo hue chip + chips, no state slabs (evidence)

Scope: iOS client-only sheet polish (owner decision "drop the panel background", one identity for the
repo). Base = `integration` `23bbfa66aa96b5419eebf0d8c30391f4f8991278`; the code head this evidence
covers is named in `.report.md`.

## What these frames are

Simulator-native XCTest renders (`SheetPolishTests`), not device screenshots: the test hosts the REAL
`RecentOutputSheet` over the REAL `FleetView` board in a 390x844 pt `UIWindow` and screenshots the window
at 3x (1170x2532 px) — the same evidence path #558 used.

- Simulator: lane-created `iPhone 16`, iOS 26.5, `6F816FE1-14D9-4602-BC6E-E48F4C12DEC2` (owned by this
  lane; the lane does not touch other lanes' simulators).
- The XCTest renderer does **not** composite `presentationBackground` materials, so the sheet surface in
  these frames is the system sheet fill (`#ffffff` light / `#1c1c1e` dark) rather than the in-app
  flavored translucent backdrop. The in-app guarantee for the translucent surface is the repo's locked
  worst-case WCAG model (below), which is how #385/#416/#428 reasoned about it; the frames' measured
  ratios are the "actual rendered backdrop" numbers for the render path that produced them.

## Frames (`frames/`, sha256 + bytes in `captures.json`)

16 state frames — every one shows the sheet's **header rows (state dot + state label + repo hue chip +
branch + pane capsule) → the #558 worktree/GitHub block → the state's copy**, and the test asserts the
frame's OCR contains `corral`, `abcdef1`, `sheet-lane`, `passing` plus the state's own copy:

| state | copy asserted | files |
| --- | --- | --- |
| loading | `Loading recent output` | `{day,night}-{default,ax3}-loading.png` |
| empty | `No output yet` | `{day,night}-{default,ax3}-empty.png` |
| error | `timeout` | `{day,night}-{default,ax3}-error.png` |
| permission | `Read Tail isn't granted` | `{day,night}-{default,ax3}-permission.png` |

8 chip frames — `chip-{latte,mocha}-{corral,sendmeter,synergy-apps,other}.png`: the sheet's header band
for each fixture repo/orphan, i.e. the Board-resolution hue chip with the pane capsule and the loaded
stream below.

`default` = `.large` Dynamic Type, `ax3` = `.accessibility3`. Day = Latte, Night = Mocha. Reduce Motion
is ON in the harness (`reduceMotionProvider: { true }`).

## Measured contrast (method + numbers)

`SheetPolishTests.testRenderedSheetStatesDayNightNormalAndLargeText` locates the copy's ink pixels in the
presented sheet region, takes the **median of the non-ink pixels on the ink's own rows** as the actual
rendered backdrop, and computes the WCAG ratio with the repo's own `SheetBackdrop.contrastRatio`. One
`G569_CONTRAST` line per state × flavor × size (in `measurements.log`); every one asserts `>= 4.5`
(the error's warning glyph asserts `>= 3.0`, it is non-text).

| state | Day ink → backdrop | ratio | Night ink → backdrop | ratio |
| --- | --- | --- | --- | --- |
| loading / empty / error / permission | `#4c4f69` → `#ffffff` | **7.99** | `#cdd6f4` → `#1c1c1e` | **11.77** |
| error warning glyph (non-text) | `#d20f39` → `#ffffff` | **5.43** | `#f38ba8` → `#1c1c1e` | **7.35** |

`testStateCopyTierHoldsAAOverTheWorstCaseTintedBackdrop` prints the same tiers under the #416 locked
model (`SheetBackdrop.worstContrast(ink:tint: base@0.8, over: every palette token)`, the bound that
covers the real translucent backdrop):

```
G569_TIER flavor=latte text=5.04 tailMuted=3.95 tailQuiet=2.50 red=3.43 mauve=3.42
G569_TIER flavor=mocha text=6.44 tailMuted=4.19 tailQuiet=1.91 red=4.02 mauve=4.59
```

So the copy's `text` tier clears AA on both axes (5.04 / 6.44 worst-case; 7.99 / 11.77 rendered) while
the pre-#569 muted/dim tiers and red-as-text cannot — the legibility debt the #428 opaque backing used
to pay. No state kept a backing.

## Hue match (Board chip ⇄ sheet chip)

`testSheetRepoChipPaintsTheBoardHueForThreeReposAndOther` requires the sheet's header band to paint the
Board chip's exact dot hex as a solid block (rows with a single-colour run >= 8 px; a 6 pt dot is a solid
block, glyph antialiasing — which can land on another palette token by coincidence — never is), and no
other repo's hue to paint a solid block there.

| repo | Latte dot | Mocha dot |
| --- | --- | --- |
| `corral` | `#179299` | `#94e2d5` |
| `sendmeter` | `#df8e1d` | `#f9e2af` |
| `synergy-apps` | `#209fb5` | `#74c7ec` |
| Other/unknown (grey) | `#acb0be` | `#585b70` |

Raw `G569_BOARD_CHIP` / `G569_CHIP` lines (Board component render vs the sheet frame) are in
`measurements.log`.

## RED / GREEN witness (`red-witness/`)

A disposable scratch copy of the code head (`git archive HEAD | tar -x -C /tmp/g569-scratch`) with ONE
anchored mutation reverting the sheet's repo identity to the pre-#569 flat muted text:

- RED: `RAW EXIT 65`, `** TEST FAILED **`, `Executed 4 tests, with 18 failures`; the hue assertion reads
  `XCTAssertGreaterThan failed: ("0") is not greater than ("6") - the sheet's repo chip for corral must
  paint #179299 — a 6pt hue dot is a solid block; flat muted text has no dot` (all repos × both flavors).
- Restore: byte-identical (`cmp -s` true; sha256 `1890a0f2cd2e7f737cd7ac3641bc01e7ee2dc4cda98821a107ae8831cac1a295`
  for pristine == restored == the lane worktree file), then GREEN: `RAW EXIT 0`,
  `Executed 4 tests, 0 failures`, `** TEST SUCCEEDED **`.
- The real worktree was never mutated (`git status --short` after the battery: only the intended
  untracked evidence dir / the intended delivery files).

## Reproduce

```sh
# render the frames (lane-owned simulator; the test writes them to the app container)
HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test \
  -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier \
  -destination 'platform=iOS Simulator,id=6F816FE1-14D9-4602-BC6E-E48F4C12DEC2' \
  -destination-timeout 120 -derivedDataPath /tmp/g569-dd \
  -parallel-testing-enabled NO CODE_SIGNING_ALLOWED=NO \
  -only-testing:FleetNotifierTests/SheetPolishTests
# collect + hash them here
python3 docs/evidence/issue-569/collect.py --sim 6F816FE1-14D9-4602-BC6E-E48F4C12DEC2 \
  --log <run log> [--log ...]
```

The collector's default `--frames` mode copies a directory of already-rendered PNGs (used for this
package); `--sim` pulls them straight out of a booted simulator's app container.

## Fix round (review STOP-and-FIX) — frames + witness re-collected at `99234658`

The protected render check
`RecentWorktreeBlockTests.testRenderedSheetDayNightMediumLargeAndAX3` regressed at
the round-1 delivery head (`60c35e96`): its frame OCR read `abodef1` where the fixture
renders `abcdef1`. Cause: the padded `RepoLabelChip` capsule is 19.0 pt tall while the
pre-#569 caption row is 16 pt (sized by the caption-semibold state label), so the chip
grew the row 3 pt and moved the whole #558 worktree block 9 px down. The fix
(`RepoLabelChip.compact`) keeps the Board's capsule drawing but drops the chip's layout
padding again, so the block sits at the base y again. Full detail: `.report-fix.md`.

Consequences for this package (everything above stays as written for round 1):

- every frame in `frames/` and every sha256/byte count in `captures.json` was
  re-collected from the fixed head — the header geometry in these PNGs is base-accurate
  (chip drawn 19.0 pt, block rows at 435 / 502 / 581 / 642 / 706 / 773 px @3x on the
  390x844 pt window); `measurements.log` now carries the fixed-head `G569_*` telemetry;
- the RED/GREEN witness was re-run at the fixed head: same assertion bites
  (`RAW EXIT 65`, `Executed 4 tests, with 18 failures`, the `("0") is not greater than
  ("6")` hue assertion), restore byte-identical, GREEN exit 0;
- the restore hash cited above (`1890a0f2…`) was the round-1 head's file and is
  superseded by `ef21a8585f8cedf33fbdb2f5822d6fcb4b822bc16b852c6e7e54878167875535`
  (worktree == committed blob at `99234658`; log `/tmp/g569-fix-witness.log`).
