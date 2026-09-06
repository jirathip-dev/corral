# 427 evidence — Direction-A filter/header redesign (Filters control + scope sheet)

Runtime frames for the #427 Direction-A change (phone-sized, synthetic
DEBUG demo captures over the #401 three-profile seed — Host A live, Host B
offline-or-connecting with retained STALE rows, Host C key mismatch with
zero lanes; fictional `demo-host-*` hosts; no live daemon, no physical
device, no TestFlight claim). Recorded on a FRESH `Corral427B` simulator
(iPhone 16, iOS 26.5, UDID F9A6A113-44FB-4F51-9483-3FC7198C7253) with the
deterministic `-corral427FilterEvidence` / `-corral427ConnectingHostEvidence`
drivers (marker files in Documents/ux-evidence, >= 9 s hold per phase).

All frames are 1179x2556 px @3x iPhone-16 captures downscaled with
`sips -z 844 390` (the repo's standard device class; 0.18 % aspect
distortion from the native 393x852 pt — same as the #385/#401/#430 sets).
Two spot frames are additionally resized to 375x812 (`sips -z 812 375`;
0.12 % aspect distortion) for the narrower reference width.

## What changed (what these frames show)

- The two horizontal chip rows (the duplicate-unexplained-`All` defect)
  are GONE from the board in every mode; the top-left header area holds
  the compact `Filters` control (active-count label + selected summary)
  in the board's pinned chrome, and Settings keeps the top-right toolbar.
- The control opens the native filter sheet: Host scope FIRST (every
  host's TOTAL lane count + textual health), Repository scope second
  (counts rescoped to the selected host — D4 unchanged); explicit
  `All hosts` / `All repositories` rows; scoped `Clear host` /
  `Clear repository`; one `Reset all filters` footer; `Close filters` +
  drag handle; selections apply immediately and dismissal preserves them.
- Connecting/offline/stale host status stays TEXTUAL inside the sheet and
  on the board: the D7 compact banner now also names connecting hosts
  ("1 host key mismatch · 1 host connecting"), and retained rows keep
  their `stale · last seen Nm ago` labels.
- A filtered board with no matching rows shows the `No lanes match` state
  with the exact active summary + `Both scopes remain active and
  reversible.` + `Reset all filters`.
- The sheet uses the shared translucent backdrop (Liquid Glass on the
  iOS 26.5 runtime; the themed 80 % material fallback captured via the
  #416 `-corral416ForceFallbackBackdrop` launch — the SAME else-branch a
  pre-26 runtime executes).

## Frames (390x844 unless noted)

| File | Proves |
|---|---|
| `phase-1-filters-board-all-mocha-390x844.png` | populated multi-host board, All/All — `Filters` + `All hosts · All repositories`, no chip rows, D7 banner, rows with host badges + stale labels |
| `phase-2-filters-sheet-all-mocha-390x844.png` | the filter sheet at the default scope: drag handle, title + live summary, HOST SCOPE before REPOSITORY SCOPE, `All hosts`/`All repositories` selected with checks, host health text (`live`/`offline`/`key mismatch`), repo counts (2/1/2 lanes) |
| `phase-3-filters-sheet-host-only-mocha-390x844.png` | host-only: `Host A` checked, summary `Host A · All repositories`, `Clear host` enabled / `Clear repository` disabled |
| `phase-4-filters-board-host-only-mocha-390x844.png` | closed host-only board: `Filters · 1` + `Host A · All repositories`, ONLY Host A rows (blocked/working/idle 1 each) |
| `phase-5-filters-sheet-repo-only-mocha-390x844.png` | repository-only: `All hosts` + `demo-atlas` checked, `Filters · 1` + `All hosts · demo-atlas` behind |
| `phase-6-filters-sheet-both-mocha-390x844.png` | both active in the sheet: `Host A` + `demo-atlas` checks, both Clear controls enabled, repo counts rescoped to Host A (1/1) |
| `phase-7-filters-board-both-mocha-390x844.png` | closed both-active board: `Filters · 2` + `Host A · demo-atlas`, only the single matching `working (1)` row |
| `phase-8-filters-sheet-host-b-offline-mocha-390x844.png` | offline/stale host: `Host B` checked as `2 lanes · offline`, repo scope rescoped to B's retained rows (`All repositories 2 lanes`), board keeps the D7 banner |
| `phase-9-filters-board-zero-results-mocha-390x844.png` | zero results: zero-lane key-mismatch `Host C` selected — `Filters · 1` + `Host C · All repositories`, `No lanes match` + summary echo + `Both scopes remain active and reversible.` + `Reset all filters` |
| `phase-10-filters-board-all-latte-390x844.png` | none-active (All) closed board after the live flavor flip — light-palette control, banner, rows |
| `phase-11-filters-sheet-both-latte-390x844.png` | both-active sheet on Latte (light palette contrast pair) |
| `phase-2-filters-sheet-all-mocha-375x812.png` | 375x812 spot: default-scope sheet at the narrow reference width — no horizontal clipping |
| `phase-6-filters-sheet-both-mocha-375x812.png` | 375x812 spot: both-active sheet at the narrow reference width |
| `phase-1-ax-sheet-all-mocha-390x844.png` | Dynamic Type: the whole UI at the accessibility content size (launch arg `-UIPreferredContentSizeCategoryName UICTContentSizeCategoryAccessibilityLarge`) — larger rows, wrapping scope headers, repository options reachable by scrolling under the pinned `Reset all filters` |
| `phase-3-ax-sheet-both-mocha-390x844.png` | Dynamic Type with both scopes active |
| `phase-2-filters-sheet-all-mocha-fallback-390x844.png` | themed fallback backdrop (the #416 forced-fallback branch a pre-iOS-26 runtime executes): tinted material over the visible board, content fully readable |
| `phase-6-filters-sheet-both-mocha-fallback-390x844.png` | fallback backdrop with both scopes active |
| `phase-1-connecting-board-all-mocha-390x844.png` | connecting host, closed board: D7 banner `1 host key mismatch · 1 host connecting`, both hosts' rows (Host B retained stale) |
| `phase-2-connecting-sheet-all-mocha-390x844.png` | connecting host in the sheet: `Host B — 2 lanes · connecting` textual health |
| `phase-3-connecting-sheet-host-b-mocha-390x844.png` | connecting host selected (Host B checked, `connecting` health, retained rows behind) |

## Accessibility / VoiceOver / hit targets

This repo has no UI-test target, so VoiceOver labels and >= 44 pt targets
ride the unit suite (`FilterHeaderRedesignTests` + the updated wiring
pins): the sheet rows carry explicit `All hosts` / `All repositories`
visible + VoiceOver labels, distinct `Clear host` / `Clear repository` /
`Reset all filters` / `Close filters` / `Settings` labels, `isSelected`
traits, >= 50 pt rows and >= 44 pt controls — all pinned over the bundled
FleetViews source; Dynamic Type wrapping + scroll reachability are shown
in the `-ax-` frames above.

## Capture commands (capture.log)

DEBUG build per run (`HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1
xcodebuild build ... -derivedDataPath /tmp/fn427-ev`), one launch per
driver with marker polling (`/tmp/fn427-capture-b.sh` — screenshot per
new phase marker, >= 2 s after detection) + `simctl io screenshot` +
`sips` resize. Full details in `capture.log`.

SHA-256s: `SHA256SUMS.txt`. Conventions follow
ios/evidence/issue-415/416/430.

## Prototype-vs-delivery note (variant A look/interaction)

- The approved prototype's trigger sits in the nav bar itself; iOS 26's
  inline nav bar compresses wide custom leading items to ellipses, so the
  control renders in the board's own pinned chrome strip (the top-left
  header area directly under the nav bar) with Settings alone top-right —
  same hierarchy and copy (`Filters` / `Filters · N` + the selected
  summary), no chip rows, native bottom sheet with handle + visible board
  context.
- Sheet look follows DESIGN-SYSTEM.md + variant-a.html: host scope first
  with `N lanes · health` text on every host, repo scope second with
  `N lane(s)`, 12 % accent tint + trailing check for the selected row,
  uppercase scope captions with scoped clear actions, one visible
  `Reset all filters`, 78 %-fraction detent (drag to full for large
  lists / Dynamic Type).
- D7 banner copy stays the board's compact aggregate form ("1 host
  offline", "1 host key mismatch", ...) extended with "1 host
  connecting"; the prototype's scenario copy ("macbook-air connecting ·
  Bazzite live", "retained lanes are stale") was illustrative — the ACs
  bind textual availability, which the aggregate line + per-row stale
  labels provide. Retained-stale ages stay on the board rows (C6), not on
  the sheet rows.
- Zero-results copy matches the prototype exactly (heading, summary echo,
  reversibility line, reset action); it renders only while a filter is
  active.
- Dynamic Type capture: the app has no UI-test target, so the
  accessibility size is forced through the DEBUG launch argument
  `-UIPreferredContentSizeCategoryName UICTContentSizeCategoryAccessibilityLarge`
  (UIKit argument domain; the same mechanism XCUITest uses) — a
  synthetic stand-in, noted here for reviewers.
