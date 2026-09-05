# Corral #387 — Chrome-only board header: no 'Fleet' navigation title

Real compiled capture of the #387 change on the iOS board
(`ios/FleetNotifier/UI/FleetViews.swift` + `App/FleetNotifierApp.swift`):

- The board navigation header is CHROME-ONLY. The `'Fleet'` navigation
  title is gone: the board declares an EMPTY title
  (`.navigationTitle("")`) locked to INLINE display mode
  (`.navigationBarTitleDisplayMode(.inline)`), so neither the top-of-board
  state nor the SCROLLED collapsed bar can render title text. The freed
  large-title band belongs to the board: content starts ~90 pt higher
  (measured below).
- The Settings GEAR stays top-right in every state — the same
  release-active >=44 pt gear Button #365 pinned, tinted by the active
  flavor's accent (mauve in Mocha AND Latte — #372) — with the DEBUG-only
  demo-overflow icon beside it (present in every prior lane's DEBUG
  captures; Release shows the gear alone).
- Repo filter chips, pull-to-refresh, offline banner, and status sections
  are untouched (only the navigation chrome + evidence-driver scroll
  plumbing changed).

The artifact is SYNTHETIC ONLY: the DEBUG `-corralDemoTitleEvidence`
driver over `DemoFleet.seed()` (fictional repos/agents, no live daemon, no
physical device). No live-fleet, physical-device, or TestFlight claim.

## Artifact

One deterministic launch records the sequence (the driver scrolls the real
board list through ScrollViewReader `.task(id:)` scroll requests — the
same recipe as the #379 settings scroll — because simctl cannot drag the
list; each phase writes a marker the host capture script polls, then
screenshots):

| Phase | File | Shows |
|---|---|---|
| 1 | `phase-1-board-mocha-top-390x844.png` | MOCHA at the TOP of the board: compact chrome-only bar + gear, pull-to-refresh hint + chips row directly beneath it — no title band |
| 2 | `phase-2-board-mocha-scrolled-390x844.png` | MOCHA SCROLLED (working/idle/done sections in view): the collapsed bar is still chrome-only — gear present, no title text |
| 3 | `phase-3-board-latte-scrolled-390x844.png` | LATTE at the SAME scrolled position after a live flavor flip — chrome-only bar on the light palette |
| 4 | `phase-4-board-latte-top-390x844.png` | LATTE back at the top of the board — chrome-only bar + gear |
| 5 | `phase-5-done-390x844.png` | Rest state after the sequence (latte top) |

All rendered 390x844 pt (raw 1179x2556 px iPhone 16 / iOS 26.5 @3x,
`sips -z 844 390`; 0.18 % aspect distortion from the 393x852 pt native —
same standard as issue-316/362/385/386).

## Header audit (Vision OCR + pixel geometry)

Local Vision OCR (`VNRecognizeTextRequest`, accurate, en-US) over every
raw frame, full-frame:

- ZERO case-insensitive 'Fleet' matches in ALL five phases (the pre-#387
  frame from the issue-386 evidence carries the word at confidence 1.00).
- OCR text-band geometry (OCR boxes in pt, 852 pt screen): the ONLY text above
  ~120 pt is the system status-bar clock (~21 pt); the nav-bar band
  (~59–120 pt) is EMPTY of text in every phase. The first board content
  (pull-to-refresh hint) starts at ~94 pt from the top — vs ~170 pt on the
  base #386 frame (title era) — i.e. content starts ~76–90 pt higher, the
  freed title band.
- Gear presence: the top-right toolbar region (160x230 raw px) holds a
  374-px mauve accent cluster in every phase — the gear glyph in the
  active flavor's accent (Mocha #CBA6F7 / Latte #8839EF tolerances) —
  top AND scrolled, Mocha AND Latte.
- Frames were also vision-inspected: phase-1/4 (top) show an empty
  left/center bar with the gear capsule at top-right and the chips row
  directly beneath; phase-2/3 (scrolled) show mid-board status sections
  with the same chrome-only bar + gear above them.

OCR audit output + the exact analysis commands are in `capture.log`.

## Capture commands (capture.log)

Fresh simulator (no keychain identity → no notification-permission alert
race), DEBUG build from this branch, `-corralDemoTitleEvidence` launch,
marker polling, `simctl io screenshot` per phase, `sips` resize to 390x844.

SHA-256s: `SHA256SUMS.txt` (all artifacts above).

Evidence conventions follow docs/design/evidence/issue-362, issue-385 and
issue-386.
