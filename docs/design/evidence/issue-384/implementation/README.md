# Corral #384 — Per-row repo labels hidden while a repo filter pill is active

Real compiled capture of the #384 change on the iOS board
(`ios/FleetNotifier/UI/FleetViews.swift`):

- While ANY repo pill is active (not 'All') the per-row repo NAME label is
  removed from agent rows: the row's `WorkspaceLine` no longer renders the
  `RepoLabelChip` capsule — only a COLOR-ONLY hue dot echo remains (the
  active pill + the #371/#386 subgroup caption still name the repo, so the
  identity channel is never lost).
- The echo keeps the label chip's EXACT vertical footprint: it carries the
  chip's caption2 text line box (transparent same-font spacer) + the chip's
  2 pt vertical padding, so rows keep their height — NO layout jump when
  the pill toggles (measured 0 px delta, see analysis.txt).
- Tapping 'All' restores the per-row repo label chips INSTANTLY: the hide
  flag re-derives on every body evaluation from the same pure
  `BoardModel.reconcile(model.repoFilter, …)` that drives the sections — no
  extra state, no timer.
- Subgroup/section behavior is untouched (#371 subgroups + #386 thick
  collapsible bars + demoted captions keep working); branch, basename,
  pane ref, badges, time-in-state all stay on the rows.

The artifact is SYNTHETIC ONLY: the DEBUG `-corralDemoRepoLabelEvidence`
driver over `DemoFleet.seed()` (fictional repos/agents, no live daemon, no
physical device). No live-fleet, physical-device, or TestFlight claim.

## Artifact

One deterministic launch records the sequence (the driver flips the same
`model.repoFilter` state the chips row sets and the same flavor the
Appearance rows set — simctl cannot inject touches; each phase writes a
marker the host capture script polls, then screenshots):

| Phase | File | Shows |
|---|---|---|
| 1 | `phase-1-board-mocha-all-390x844.png` | MOCHA 'All': every row shows its per-row repo label chip (demo-garden / demo-orbit / demo-atlas) |
| 2 | `phase-2-board-mocha-filtered-390x844.png` | MOCHA demo-atlas pill ACTIVE: only demo-atlas rows, NO repo name labels — just the color-only hue dot; row heights unchanged |
| 3 | `phase-3-board-mocha-restored-all-390x844.png` | MOCHA: 'All' restored — repo label chips back (instant restore) |
| 4 | `phase-4-board-latte-all-390x844.png` | LATTE 'All': rows with repo label chips |
| 5 | `phase-5-board-latte-filtered-390x844.png` | LATTE demo-atlas pill ACTIVE: rows without repo name labels (color dot only) |
| 6 | `phase-6-board-latte-restored-all-390x844.png` | LATTE: 'All' restored — chips back |
| 7 | `phase-7-done-390x844.png` | Rest state (latte, All) |

All rendered 390x844 pt (raw 1179x2556 px iPhone 16 / iOS 26.5 @3x,
`sips -z 844 390`; 0.18 % aspect distortion from the 393x852 pt native —
same standard as issue-362/385/386).

## No-layout-jump quantification (row-geometry.py -> analysis.txt)

The demo-output row's line-2 trailing badge sits at an IDENTICAL offset
below the line-1 state chip in both filter states, both flavors:
span(chip top -> badge bottom) = 132 px in every frame; All - filtered =
0 px. Rows keep their exact height; only the label disappears.

## Capture commands (capture.log)

Fresh simulator (no keychain identity → no notification-permission alert
race), DEBUG build from this branch, `-corralDemoRepoLabelEvidence`
launch, marker polling, `simctl io screenshot` per phase, `sips` resize to
390x844.

SHA-256s: `SHA256SUMS.txt` (all artifacts above).

Evidence conventions follow docs/design/evidence/issue-362, issue-385 and
issue-386.
