# 456 evidence — full-screen native Herd shell (refreshed at the merged head)

Runtime frames for the #456 layout slice (ranch behind every safe area,
floating top scope + Settings, floating bottom paddock navigation, no opaque
Board header or duplicated toolbar), **re-captured after the #449 integration
refresh** (union of `origin/integration` 16daaca and the #456/#456-r1 work).

Every frame is the REAL app on owned simulators with the deterministic DEBUG
`-corral456FullScreenEvidence` driver (fictional demo fixtures only — no live
daemon, no real hosts, no private rows, no physical-device or TestFlight
claim). Captured 2026-09-10 from the merged tree
`25739631969b42b5d0473b190ec7be3da44a989a` (parents `1ab8a811…` and
`16daaca4…`). Earlier frame sets remain in git history: the pre-repair r0 set
at `8274019`, the r1 repaired set at `1ab8a811`.

## Why the frames were re-captured

#449 changes the rendered composition: `HerdProjection.paddocks` now sorts
repositories that contain a connected working agent first, so the first
paddock of the demo fixture is a working repository. The axes and totals are
unchanged; the paddock identity, the page order and the outage fallback are
not. `measurement.log` re-runs the discriminating probe on this set and still
validates it against the historical r0 frames.

- Default day: first paddock is now `cedar-tools` (working) — was
  `atlas-vector` in the r1 set.
- Next paddock: `maple-client` (page 2 of 3).
- Offline outage: every agent is `unknown`, so no repository qualifies as
  working and the alphabetical fallback shows `atlas-vector` first.

## Devices

- `Corral456R1` — iPhone 16, iOS 26.5 (393x852 pt = 1179x2556 px @3x). Frames
  downscaled with `sips -z 844 390` (the repo's evidence convention; 0.18 %
  aspect distortion from the native 393x852 pt).
- `Corral456R1SE` — iPhone SE (3rd generation), iOS 26.5 (375x667 pt =
  750x1334 px @2x), downscaled with `sips -z 667 375`.
- Both simulators were shut down after capture (no erase/delete); the shared
  `iPhone 16` device and other lanes' simulators were never touched.

## Frames

| File | What it proves |
|---|---|
| phase-1-herd-day-390x844.png | Default text size: full-screen Day ranch behind the status bar, Dynamic Island and home indicator; floating `Filters / All repositories` pill + native gear; truthful five-count summary (2 blocked / 4 working / 3 idle / 2 done / 1 unknown); `! FRONT RAIL · 2 BLOCKED` with `birch-clearing` + `oak-before-dark`; working-repository-first paddock `cedar-tools — 4 here · 0 at rail`; floating `Previous · 1 / 3 · Next`. No board header strip, no nav-bar toolbar. |
| phase-2-herd-night-390x844.png | Same single scene, Night lighting, same controls. |
| phase-3-next-paddock-390x844.png | Bottom navigation moved to paddock 2 via the same `movePage` action the Next button invokes: `maple-client — 4 here · 0 at rail`, position `2 / 3`. |
| phase-4-scope-sheet-390x844.png | The floating scope control opens the REAL filter sheet (`REPOSITORY SCOPE`, counts, `Clear repository`, `Reset all filters`) over the ranch. |
| phase-5-settings-sheet-390x844.png | The floating gear opens the REAL Settings sheet (Appearance Board/Herd picker from #458, themes, Herd environment). |
| phase-6-long-names-390x844.png | 8 synthetic agents in 8 ~45-char repositories: repo title and horse names truncate with `…`, counters (2/3/1/1/1) stay legible, `1 / 8` paddocks, floating chrome intact. |
| phase-7-empty-scope-390x844.png | Empty fleet: all five counts `0`, rail `0 BLOCKED`, `No agents in this scope`, floating controls still present. |
| phase-8-offline-outage-390x844.png | `-corralHerdOffline`: outage banner `Source disconnected · last-known agents` with `Open Board` + `Retry`, muted horses with `last known` status, counts `12 unknown`, rail `2 LAST KNOWN`, paddock `atlas-vector — 2 here · 2 at rail` (disconnected agents are never promoted by the #449 ordering). |
| phase-9-board-no-regression-390x844.png | Board (list) surface unchanged: pinned Filters control + pull-to-refresh hint, Settings gear, `blocked (2)` / `working (3)` sections. |
| phase-10-ax-day-390x844.png | **AX-XXXL repair witness (merged head)**: the scope pill grows with the type — `Filters` fully inside its own pill, `All repositories` visible, counts card clear of the pill, `! FRONT RAIL · 2 BLOCKED`, floating `Previous · 1 / 3 · Next` — nothing in the status-bar/Dynamic Island band (measurement.log: 0 px). |
| phase-11-ax-night-390x844.png | Same AX-XXXL composition at Night. |
| phase-12-ax-long-names-390x844.png | AX-XXXL + long repository/horse names: truncation holds, controls remain reachable. |
| phase-13-ax-empty-scope-390x844.png | AX-XXXL empty scope: five truthful `0` counts and the empty state inside the safe area. |
| phase-14-se-day-375x667.png | iPhone SE default size: ranch full-screen behind the status bar and bottom edge (home-button device), floating chrome and `Previous · 1 / 3 · Next` fit. |
| phase-15-se-night-375x667.png | Same SE scene at Night. |
| phase-16-se-long-names-375x667.png | SE + long names: truncation holds at 375 pt. |
| phase-17-se-empty-scope-375x667.png | SE empty scope: counts and empty state. |
| phase-18-se-ax-day-375x667.png | SE at AX-XXXL: chrome and navigation stay inside the safe area with >= 44 pt targets; the rail + paddock column scrolls between them (a small phone cannot show the whole column at this size without hiding content). Labels truncate (`Previo…`, `All repositori…`) rather than overlap. |

## Measured AX geometry (pre-empting a low-resolution misread)

On `phase-10-ax-day-390x844.png` the pill is materially present and contains
the label — it is a large translucent card over a bright ranch, so a
downscaled view can make it look like free-standing text:

- pill material at x=44 spans rows **65..189** (navy `(46,71,84)`), i.e. from
  the top safe-area margin (59 pt + 6 pt chrome padding) downwards;
- the label glyph rows measured over x 48..200 are **80..181** — strictly
  inside the pill, 15 rows below its top edge and 8 above its bottom edge;
- rows **191..196** are ranch pixels (`(99,165,197)`) — the 6 pt gap between
  the scope pill and the counts card, which starts at row 197;
- `measurement.log` counts **0** chrome-text pixels above the pill
  (status-bar/Dynamic Island band), versus 728 px on the historical r0 frames.

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
  cover the screen and crops the horizontal overflow (see
  `.report-456-refresh.md`): the prototype's extended browser viewBox would
  have needed the fenced `RanchEnvironment.swift`; the in-fence composition
  keeps the world undistorted but shows a slightly tighter crop at the left
  and right edges on tall phones than the browser prototype.
- The AX frames use the simulator's system content-size setting; they are not
  a VoiceOver/contrast certification.

## Reproduce

See `capture.log` for the exact simulators, build, install, launch and capture
commands plus every raw exit code. The driver is DEBUG-only
(`-corral456FullScreenEvidence`; `-corralHerdEvidence` for the seeded fictional
fleet; `-corralHerdOffline` for the outage frame) and writes
`Documents/ux-evidence/<phase>.marker` while the host script screenshots each
phase.
