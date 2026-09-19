# #574 — build-32 horizontal-pager lag: measurement, fix, re-measurement

Lane `impl574-pager-perf`, branch `g574-pager-perf`, base `origin/integration` @
`7ac34bfea63a9dbeb9d7c321c4c7e9085224c046` (pre-fix head after the preserved
instrumentation commit: `4dc19bf`). Harness: Hermes `fleet-impl` profile
(lane-local model axis is profile-owned). Repo `jirathip-dev/corral`, issue #574.

Owner's report (authoritative): **horizontal (repository) paging lags; vertical
horse scrolling is fine.** build 31 (`7d79581`) fine, build 32 (`4efebcf7`) laggy.

Preflight: `git diff --name-only 4efebcf7 7ac34bf -- ios/` = **0 files** — the
lane base carries build-32's client code exactly. Measured on the lane's own
simulator `impl574-perf` (iPhone 17 Pro, iOS 26.5), internal-disk derived data
(`/tmp/fn574-dd`), host load recorded per leg (this Mac is shared with sibling
fleets; 1-min load > `hw.ncpu` = 10 is reported as contended).

## Fixture and method

Deterministic DEBUG fleet seeded by the `-corralHerdEvidence -corral574Perf`
launch: `build-32 fleet size: 40 agents, 12 repositories, 155-worktree-fact
frame shape, 3 blocked at rail`, pages in the documented deterministic order
(page 1 = atlas-vector … page 7 = elder-docs), scene clock parked
(`evidenceElapsed = 12`) so the deltas are gesture-attributable. The battery
(`ios/FleetNotifierUITests/HerdPagerPerfTests.swift`, skipped unless
`CORRAL574_PERF=1` reaches the runner) drives **real input only**:
6 press-drag page turns right (t1), 6 left (t2), the same 6+6 via the
navigation buttons (t3), 3+3 vertical swipes on page 1's field (t4, control),
and the recents sheet open/scroll/close (t5). Counter dumps are taken at the
end of each phase (raw JSON in `dumps/`, copied from the app container).

Arms: **A1/A2** = pre-fix head (build-32 code) in the lane worktree; **B** =
scratch checkout of `7d79581` (build 31) with the same instrumentation layer
(`/tmp/fn574-base`); **F/F2/F3** = the fixed head (F3 = final). A/B/A order was
run in one sitting: A1 → B → A2.

## A/B/A (pre-fix) — the regression is in the per-scroll-tick work

Per-scroll-tick = counter delta ÷ `scrollChanges` delta (the brief's derived
metric). t1/t2 are the drag legs; both show the same profile.

| counter / tick | A2 (build 32) t1 / t2 | B (build 31) t1 / t2 |
| --- | --- | --- |
| `herdViewBody` | 1.47 / 1.57 | 1.21 / 1.24 |
| `paddockProjections` | 23.92 / 25.50 | 20.26 / 21.12 |
| `pagerPageGeometry` | 9.21 / 9.79 | 8.14 / 8.68 |
| `rowBuilds` | **79.32 / 79.67** | **22.76 / 21.73** |
| `artBounds` | **37.79 / 37.48** | n/a (code absent at `7d79581`) |
| `edgeGroupBodies` | **37.45 / 37.48** | n/a |
| `edgeOpacity` | 37.45 / 37.48 | n/a |
| `captionResolutions` | 2.94 / 3.14 | 2.42 / 2.49 |
| `ranchBody` / `ranchPlanePaints` | 1.00 / **6.00** | 1.00 / **6.00** |

A1 reproduces A2 within ~2 % on every counter (herdBody 1.49/1.42, projections
24.27/23.42, rowBuilds 78.24/74.72, artBounds 37.24/35.23, edges 36.88/35.23,
pageGeom 9.37/9.21) — the harness is stable, not noisy. Host at gesture start:
A1 load1 12.16 (contended), A2 9.10, B 4.59.

Reading: every horizontal tick re-runs `HerdView.body` because the pager offset
lived in `@State var scroll` on `HerdView`; at build 32 the rebuilt pages carry
#568's **eager** row stacks (`VStack`, deliberate for the snap) so each rebuild
re-evaluates every realized row — and per row that means `herdArtBounds` (which
draws the whole vector art to reduce its ink bounds), the row's `HerdEdgeGroup`
body and its two content passes. Build 31's rows are a `LazyVGrid` (visible
cells only), which is why the same tick cascade is ~4× cheaper there. The six
`RanchEnvironment` planes repaint 6× per tick in **both** arms (shared cost, not
the regression: build 31 is the owner's "fine" reference and repaints the same).

## Fix (only what the numbers implicate)

`ios/FleetNotifier/UI/Herd/HerdView.swift`:

1. **Scroll-offset channel.** The pager offset moved off `@State var scroll`
   into `HerdScrollChannel` (published by `HerdScrollTracking`), observed only
   by the new `HerdRanchLayer`, which owns the same `RanchEnvironment` inputs as
   before — so the parallax, per-plane offsets and scene are unchanged while a
   tick no longer invalidates the herd body. `scroll` remains a read-only
   accessor for the evidence recorders.
2. **Enumerated pager pages.** `ForEach(Array(paddocks.enumerated()), id:.element.id)`
   — the per-page background no longer calls `paddocks.firstIndex` (which
   re-derived the whole grouping/sort) for every page on every geometry update.
3. **Hoisted eager row stack.** `HerdFieldRowStack` (new, file-private) carries
   the rows out of `HerdPaddockScroll`'s `GeometryReader` body. A GeometryReader
   re-evaluates its content whenever the geometry it provides changes — inside
   the pager that is every frame of a horizontal slide — and the old inline
   rows were rebuilt per frame with a changing `visible.frame(in:.global)`. The
   stack now receives a canonical viewport (`x` pinned to 0; only `minY/maxY/
   height` are read downstream) so a horizontal slide leaves its inputs
   unchanged. Vertical scrolling still updates rows through the measured row
   frames, and #568's edge overlay still paints from the current layout pass
   (no delayed state opacity — its semantics are untouched).

## Re-measurement (fixed head)

| counter / tick | A2 (pre-fix) | B (build 31) | F3 (fixed) |
| --- | --- | --- | --- |
| `herdViewBody` | 1.47 / 1.57 | 1.21 / 1.24 | **0.36 / 0.30** |
| `paddockProjections` | 23.92 / 25.50 | 20.26 / 21.12 | **3.57 / 3.00** |
| `pagerPageGeometry` | 9.21 / 9.79 | 8.14 / 8.68 | **3.91 / 3.98** |
| `rowBuilds` | 79.32 / 79.67 | 22.76 / 21.73 | **29.09 / 25.41** |
| `artBounds` | 37.79 / 37.48 | n/a | **6.74 / 4.94** |
| `edgeGroupBodies` | 37.45 / 37.48 | n/a | **6.50 / 4.94** |
| `edgeOpacity` | 37.45 / 37.48 | n/a | 21.51 / 19.57 |
| `ranchBody` / `ranchPlanePaints` | 1.00 / 6.00 | 1.00 / 6.00 | 1.00 / 6.00 |

Per page turn (per leg, 6 turns): row builds **A2 701 → F3 339**, vs **B 341** —
the build-32 row-rebuild regression is closed to build-31 parity; the remaining
additions per turn are the #568 feature's own per-frame work (art bounds 79,
edge group bodies 76, edge-opacity evaluations 251) which build 31 simply does
not have. Per-tick reductions A2 → F3: rows −63 %, art bounds −82 %, edge group
bodies −82 %, projections −85 %, herd body −76 %, page geometry −57 %. The
vertical control leg stays pager-flat (0 scroll ticks) in every arm.

F2 (fixes 1+2 only) isolates fix 3: rows 49.2 → 29.1, artBounds 24.3 → 6.7,
edgeGroupBodies 23.9 → 6.5 per tick. F and F2 ran at gesture-start load1 5.81
and 21.31; F3 at 106.55 (heavily contended, other fleets' Xcode builds) — the
counter ratios agree across all three, consistent with the brief's note that
counter deltas stay valid under load.

Raw per-phase tables for A2 / B / F3 are in `REPORT-tables.md` (generated from
`dumps/` by the same script as the tables above); per-phase cumulative and
per-tick tables for every arm are reproducible with
`python3 gen-tables.py dumps/<arm>`.

## Residual cost and honest limits

- **Unchanged / shared:** `ranchPlanePaints` 6 and `ranchBody` 1 per tick — the
  six `RanchEnvironment` procedural planes still repaint on every tick in both
  arms (build 31 included; the owner's fine reference). They were not
  implicated as the regression and were left untouched.
- `edgeOpacity` ~20/tick remains: #568's overlay paints its content from the
  current layout pass (deliberately, to avoid a delayed-state-opacity fragment
  during fast reversals); that per-frame evaluation cannot be removed without
  changing #568's edge semantics.
- **Not verified:** device/touch feel, the owner's "feels" verdict, physical
  iPhone behaviour, absolute frame timing (shared host). Counter deltas and
  their ratios are the claims made; contended runs are labelled above. The
  simulator video is a capture of the fixed head, not a device recording.

## Gates (exact commands + raw exits)

Run from the worktree; the raw logs are committed under `gates-logs/` in this
bundle (originals also at `/tmp/fn574-logs/gates/`).

| # | command (canonical, from `.brief.md` §7) | raw exit | notes |
| --- | --- | --- | --- |
| 1 | `cd ios && xcodegen generate --spec project.yml && git -C .. diff --exit-code -- ios/` | `XCODEGEN_EXIT=0`; `DIFF_EXIT=0` post-commit | regeneration changed no tracked file (the pre-commit `DIFF_EXIT=1` was the lane's own uncommitted edits) |
| 2 | `python3 ios/check-release-demo.py` | `RELEASE_DEMO_EXIT=0` | "release-demo check: PASS" — digest re-pinned over the measured-pager sources |
| 3 | `xcodebuild test … -only-testing:FleetNotifierUITests/HerdPagerPerfTests` (no marker) | `PERF_SKIP_EXIT=0` | the battery skips on CI's bare `xcodebuild test` |
| 4 | `xcodebuild test … -only-testing:FleetNotifierTests` | `UNIT_EXIT=0` | `Executed 684 tests, with 1 test skipped and 0 failures`. First run at the pre-pin head failed 3 source-wiring pins that quoted the old ranch mount; the pins were updated to the new mount and strengthened (not weakened) — see `HerdTests.swift` / `HerdWindTests.swift` in the diff |
| 5 | `xcodebuild test … -only-testing:FleetNotifierUITests/HerdEdgeGestureTests` | `EDGE_UI_EXIT=0` | 7 tests, 0 failures — the #568 edge/settle suite stays green at the fixed head |
| 6 | `swift run --package-path ios/tools/anti-slop-swift anti-slop ios/FleetNotifier ios/FleetNotifierTests` | advisory (`ANTISLOP_EXIT=1`, 24 violations across 56 files) | zero violations in the lane's changed files: the branch's only new violation (`HerdEvidence.pagerRepoShape`, `no-shape-in-symbol-names`) was renamed to `pagerRepoSizes`; the remaining set is pre-existing (`FleetNotifierTests.swift` etc.) |
| 7 | `git diff --check origin/integration..HEAD` | `DIFFCHECK_COMMITTED_EXIT=0` | no whitespace errors |

## Video (fixed head)

The fixed-head battery was recorded end-to-end on the lane simulator
(`xcrun simctl io recordVideo --codec h264`) while the perf battery drove the
real gestures — 6 press-drag page turns right, 6 left, the 6+6 button turns,
the vertical control swipes and the recents sheet open/scroll/close.

- `/tmp/fn574-logs/video/fixed-head.mp4` — duration **145.74 s**, size
  89,334,588 bytes, sha256 `7a8da7af62810c3f5d4e70703a53cc999fb5738dea2ced06755f9f79a62012ea`
  (full-resolution original; kept on the lane disk, not committed for size).
- `video/fixed-head-small.mp4` — the same recording re-encoded 402x874
  (crf 31), 1.1 MB, sha256 `dd330a2363eb4478378ae8d5ec9f8f53c71668d626c3755c89747e5df6ea9fa0` (committed).
- `video/frame-{1,2,3,4}-*.png` — full-resolution stills at 26 s / 49 s / 80 s /
  116 s (page turns mid-battery, button turns, vertical leg, sheet leg).

## Parity (painted content unchanged)

`parity/before-page1.png` (pre-fix head) vs `parity/after-page1.png` (fixed
head), both captured on the parked perf fixture with the status-bar clock
frozen (`simctl status_bar override --time 9:41`):

- both 1206x2622; **PIXEL IDENTICAL** (byte-compare of the RGB buffers) —
  sha256 `e96d68c9eb229ee2e7351b9d6eb7e7c046b3ad422846052ece4e805bd228c53f`
  for both files; `parity/pixel-diff.txt` is the raw output.

