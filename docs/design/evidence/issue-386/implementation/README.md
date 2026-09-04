# Corral #386 — Board hierarchy: thick collapsible status bars + demoted repo captions

Real compiled capture of the #386 change on the iOS board
(`ios/FleetNotifier/UI/FleetViews.swift` + `UI/BoardModel.swift`):

- STATUS SECTIONS became THICK COLLAPSIBLE BARS: a full-width toggle on the
  theme's surface1 tier (chrome around it stays mantle, so the bar
  contrasts per palette), state-colored mark + raw status name + TOTAL
  count in `headline.bold`, and a chevron that rotates to point right when
  collapsed. The WHOLE bar is the tap target (≥44 pt, pressed feedback);
  collapse is INSTANT (no animation — Reduce Motion unaffected); state is
  per board session only (`BoardModel.StatusSectionCollapse`, view-owned
  `@State`, never persisted — consistent with #373 blocks).
- REPO SUBGROUP HEADERS are DEMOTED captions: repo name + count in
  `caption2` subtext1 (small/secondary type) on the hue 9 %-over-mantle
  band; the small hue chip + rail stay. NOT collapsible (no disclosure
  anywhere on the caption) and always visible under an expanded status
  section.
- A collapsed section renders its BAR ALONE (counts stay on the bar) —
  including when EVERY section is collapsed (the all-collapsed frame
  below proves an empty list section still renders its header bar).

The artifact is SYNTHETIC ONLY: the DEBUG `-corralDemoCollapseEvidence`
driver over `DemoFleet.seed()` (fictional repos/agents, no live daemon, no
physical device). No live-fleet, physical-device, or TestFlight claim.

## Artifact

One deterministic launch records the sequence (the driver flips the same
state a bar tap sets — `sectionCollapse.collapse(...)`, idempotent so the
`.task(id:)` evidence hook's double-fire on demo entry cannot undo it —
and the same flavor the Appearance rows set; each phase writes a marker
the host capture script polls, then screenshots):

| Phase | File | Shows |
|---|---|---|
| 1 | `phase-1-board-mocha-390x844.png` | MOCHA: blocked (2) COLLAPSED (thick bar alone, chevron right) directly above working (3) EXPANDED (thick bar + small repo captions + agent rows) |
| 2 | `phase-2-board-latte-390x844.png` | LATTE: the same collapse state after a live flavor flip |
| 3 | `phase-3-all-collapsed-latte-390x844.png` | LATTE: ALL five sections collapsed — five thick bars and nothing else (empty sections keep their bars/counts) |
| 4 | `phase-4-done-390x844.png` | Rest state after the sequence (all collapsed, latte) — byte-identical to phase 3 (deterministic rendering) |

All rendered 390x844 pt (raw 1179x2556 px iPhone 16 / iOS 26.5 @3x,
`sips -z 844 390`; 0.18 % aspect distortion from the 393x852 pt native —
same standard as issue-316/362/385).

## Hierarchy quantification (collapse-analysis.py)

The status bar is the ONLY full-width surface1 surface on the board
(chrome strips are mantle, rows ride base, captions are hue-tinted
repoBand strips), so a row scan for the active flavor's surface1 token
isolates the bars (`analysis.txt`):

- phases 1 and 2 (Mocha AND Latte): two full bars of 48.3 pt each — the
  collapsed blocked bar and the expanded working bar have IDENTICAL
  thickness — plus the idle bar edge at the bottom of the frame.
- phase 3 (all collapsed): FIVE bars, each 48.3 pt, evenly spaced, with
  nothing between them — every collapsed section renders its bar + counts.
- Repo captions under the expanded working section render at `caption2`
  (11 pt system) in subtext1 — a small secondary tier vs the status
  bars' `headline.bold` (17 pt) on the measured 48.3 pt bars.

## Capture commands (capture.log)

Fresh simulator (no keychain identity → no notification-permission alert
race), DEBUG build from this branch, `-corralDemoCollapseEvidence` launch,
marker polling, `simctl io screenshot` per phase, `sips` resize to 390x844.

SHA-256s: `SHA256SUMS.txt` (all artifacts above).

Evidence conventions follow docs/design/evidence/issue-328-330, issue-362
and issue-385.
