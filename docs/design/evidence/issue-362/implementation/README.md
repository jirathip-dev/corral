# Corral #362 — iOS status-section board implementation evidence

Real compiled capture of the #362 board flip: the iOS home board now groups
agents into raw herdr STATUS sections (Blocked → Working → Idle → Unknown;
a Done section renders only when herdr reports done), with repo as row
metadata instead of the #354 repo-group sections.

The artifact is SYNTHETIC ONLY: the DEBUG `-demoMode` board over
`DemoFleet.seed()` (fictional repos, fictional agent ids, no live daemon,
no physical device). It makes no live-fleet, physical-device, or
TestFlight claim.

## Artifact

| Surface | File | Rendered size | Result |
|---|---|---|---|
| iOS FleetNotifier | `ios-status-sections-390x844.png` | 390x844 pt (raw 1179x2556 px iPhone 16 / iOS 26.5 @3x, `sips -z 844 390`) | PASS |

## What the frame shows

The board renders the status-section projection through
`BoardModel.sections(agents)`:

- Section headers in the locked order with raw status name + count:
  `blocked (1)` → `working (3)` → `idle (1)` → `unknown (1)`. Blocked is
  the FIRST section (attention-first; no cross-repo promotion).
- Rows carry repo as SMALL ROW METADATA only — the `working` section mixes
  `demo-orbit` (PR 9025, dirty), `demo-atlas` (demo-output), and the
  orphan `demo-orphan` (repo = nil, `w25:p1`) — no repo sections exist.
- Row anatomy unchanged from the row contract: state chip + time-in-state,
  agent name, pane ref (`w21:p1` …) + tool chip, and line 2
  `repo · branch` (+ PR/dirty badge).
- The demo seed deliberately has NO `done` agent (herdr 0.8.2 finished
  panes fall back to idle), so no Done section renders — the raw-status
  "done renders only when reported" semantics is proven by the focused
  BoardModel unit tests (`testDoneGetsItsOwnSectionOnlyWhenHerdrReportsIt`,
  `testDoneSectionPositionIsRankTieNotTimestampDriven`), not by a live or
  demo screenshot.
- No auth/setup overlay, no notification alert, no search field, no
  filter chips, no action controls.

## Capture commands (capture.log)

Fresh simulator (no keychain identity → no notification-permission alert
race), DEBUG build from this branch, `-demoMode` launch, `simctl io
screenshot`, `sips` resize to 390x844.

SHA-256

- `ios-status-sections-390x844.png`: `0a570bcda085d4eb279caf9da6431be2a48b49dda785e21b19cd5d23f8427388`
- `ios-status-sections-raw.png` (1179x2556, pre-resize): `c84fffac2d8fc38fb840fec5c8a9c3e943d324a246496ff13f36c69e31422ed5`

Evidence conventions follow docs/design/evidence/issue-319-320 and
issue-316.
