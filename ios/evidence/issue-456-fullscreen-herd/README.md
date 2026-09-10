# 456 evidence — full-screen native Herd shell (r1, accessibility repair)

Runtime frames for the #456 layout slice (ranch behind every safe area,
floating top scope + Settings, floating bottom paddock navigation, no opaque
Board header or duplicated toolbar), **re-captured after the r1 accessibility
Dynamic Type repair**.

Every frame is the REAL app on owned simulators with the deterministic DEBUG
`-corral456FullScreenEvidence` driver (fictional demo fixtures only — no live
daemon, no real hosts, no private rows, no physical-device or TestFlight
claim). Captured 2026-09-10 from the r1 tree
(`6150ab1bc0db5232faec1e8f64de8a448212fa46`); the pre-repair frames remain in
git history at `82740197443f764ef390d80330e907183c2871e4`.

## What changed vs the r0 evidence set

The r0 README claimed the AX frames showed "nothing overlaps the status bar"
while `phase-10/11/12` actually showed the scope label rendered above its pill.
This r1 set replaces those frames with re-captures from the repaired tree and
adds `measurement.log`: the discriminating pixel probe (chrome text in the band
between the status bar and the pill's top edge, left of the Dynamic Island),
re-validated on the historical r0 frames in the same run:

- r0 `phase-10/11/12` (the frames the FAIL cited): **728 px** of label text
  above the pill each (`LABEL_SPILLS_ABOVE_PILL=True`),
- r0 default-size control: 0 px,
- **all r1 AX frames and controls: 0 px.**

## Devices

- `Corral456R1` — iPhone 16, iOS 26.5 (393x852 pt = 1179x2556 px @3x), created
  for the #456 lane. Frames downscaled with `sips -z 844 390` (the repo's
  evidence convention; 0.18 % aspect distortion from the native 393x852 pt).
- `Corral456R1SE` — iPhone SE (3rd generation), iOS 26.5 (375x667 pt =
  750x1334 px @2x), created for the r1 capture. Frames downscaled with
  `sips -z 667 375`.
- Both simulators were shut down after capture (no erase/delete); the shared
  `iPhone 16` device and other lanes' simulators were never touched.

## Frames

| File | What it proves |
|---|---|
| phase-1-herd-day-390x844.png | Default text size: full-screen Day ranch behind the status bar, Dynamic Island and home indicator; floating `Filters / All repositories` pill + native gear; truthful five-count summary; `! FRONT RAIL · 2 BLOCKED`; paddock `atlas-vector — 2 here · 2 at rail`; floating `Previous · 1 / 3 · Next`. No board header strip, no nav-bar toolbar. |
| phase-2-herd-night-390x844.png | Same single scene, Night lighting, same controls. |
| phase-3-next-paddock-390x844.png | Bottom navigation moved to paddock 2 via the same `movePage` action the Next button invokes; paddock identity follows. |
| phase-4-scope-sheet-390x844.png | The floating scope control opens the REAL filter sheet (`REPOSITORY SCOPE`, counts, `Clear repository`, `Reset all filters`) over the ranch. |
| phase-5-settings-sheet-390x844.png | The floating gear opens the REAL Settings sheet (Appearance Board/Herd picker from #458, themes, Herd environment). |
| phase-6-long-names-390x844.png | 8 synthetic agents with ~45-char repository and horse names: repo title + horse names truncate, counters stay legible, floating chrome intact. |
| phase-7-empty-scope-390x844.png | Empty fleet: all five counts `0`, rail `0 BLOCKED`, `No agents in this scope`, floating controls still present. |
| phase-8-offline-outage-390x844.png | `-corralHerdOffline`: outage banner `Source disconnected · last-known agents` with `Open Board` + `Retry`, muted horses with `last known` status, counts `12 unknown`, rail `LAST KNOWN` — over the full-screen ranch. |
| phase-9-board-no-regression-390x844.png | Board (list) surface unchanged: pinned Filters control + pull-to-refresh hint, Settings gear, `blocked (2)` / `working (3)` sections. |
| phase-10-ax-day-390x844.png | **AX-XXXL repair witness**: the scope pill grows with the type — `Filters` fully inside its own pill, `All repositories` visible, counts card clear of the pill, `! FRONT RAIL · 2 BLOCKED`, floating `Previous · 1 / 3 · Next` — nothing in the status-bar/Dynamic Island band. |
| phase-11-ax-night-390x844.png | Same AX-XXXL composition at Night. |
| phase-12-ax-long-names-390x844.png | AX-XXXL + long repository/horse names: truncation holds, controls remain reachable. |
| phase-13-ax-empty-scope-390x844.png | AX-XXXL empty scope: five truthful `0` counts and the empty state inside the safe area. |
| phase-14-se-day-375x667.png | iPhone SE default size: ranch full-screen behind the status bar and bottom edge (home-button device), floating chrome and `Previous · 1 / 3 · Next` fit. |
| phase-15-se-night-375x667.png | Same SE scene at Night. |
| phase-16-se-long-names-375x667.png | SE + long names: truncation holds at 375 pt. |
| phase-17-se-empty-scope-375x667.png | SE empty scope: counts and empty state. |
| phase-18-se-ax-day-375x667.png | SE at AX-XXXL: chrome and navigation stay inside the safe area with >= 44 pt targets; the rail + paddock column scrolls between them (a small phone cannot show the whole column at this size without hiding content). Labels truncate (`Previo…`) rather than overlap. |

## Honest limits

- **Physical iPhone evidence remains the explicit unverified gate.** These are
  simulator frames; they do not prove real-device full-screen/contrast,
  notch/home-indicator ergonomics on hardware, or True Tone/OLED contrast.
- Touch injection is unavailable on `simctl`: the driver invokes the SAME
  actions the controls call (`movePage`, `showFilters`, `showSettings`), and
  the controls' >= 44 pt targets, labels, wiring and runtime layout are pinned
  by `FullScreenHerdShellWiringTests` +
  `FullScreenHerdShellAccessibilityLayoutTests`. No frame here is an injected
  tap.
- `measurement.log` is a luminance probe; the Night AX frame has a dark
  background, so it is reported as not measurable there (its Day/name/empty
  siblings and the in-test layout tests cover the same layout path).
- VoiceOver traversal, scroll physics and real Notch cut-out rendering were
  not exercised.
- The cover composition scales the approved 390x640 native world uniformly to
  cover the screen and crops the horizontal overflow (see `.report-456-r1.md`):
  the prototype's extended browser viewBox would have needed the fenced
  `RanchEnvironment.swift`; the in-fence composition keeps the world
  undistorted but shows a slightly tighter crop at the left and right edges on
  tall phones than the browser prototype.
- The AX frames use the simulator's system content-size setting; they are not
  a VoiceOver/contrast certification.

## Reproduce

See `capture.log` for the exact simulators, build, install, launch and capture
commands plus every raw exit code. The driver is DEBUG-only
(`-corral456FullScreenEvidence`; `-corralHerdEvidence` for the seeded fictional
fleet; `-corralHerdOffline` for the outage frame) and writes
`Documents/ux-evidence/<phase>.marker` while the host script screenshots each
phase.
