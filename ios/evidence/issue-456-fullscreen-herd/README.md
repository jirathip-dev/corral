# 456 evidence — full-screen native Herd shell

Runtime frames for the #456 layout slice (ranch behind every safe area,
floating top scope + Settings, floating bottom paddock navigation, no opaque
Board header or duplicated toolbar). All frames are the REAL app on owned
simulators with the deterministic DEBUG `-corral456FullScreenEvidence` driver
(fictional demo fixtures only — no live daemon, no real hosts, no private
rows, no physical-device or TestFlight claim).

Recorded 2026-09-10 from the final #456 tree (local head recorded in
`.report-456.md`; DEBUG build in `/tmp/corral-456-dd`, Release build in
`/tmp/corral-456-dd-release`):

- `Corral456B` — iPhone 16, iOS 26.5 (393x852 pt = 1179x2556 px @3x), created
  fresh for this evidence run. Frames downscaled with `sips -z 844 390`
  (0.18 % aspect distortion from the native 393x852 pt — the repo's standard
  device class).
- `Corral456SE` — iPhone SE (3rd generation), iOS 26.5 (375x667 pt =
  750x1334 px @2x), created fresh. Frames downscaled with `sips -z 667 375`.
- Both simulators were shut down (no erase/delete) after capture. The shared
  `iPhone 16` device and other lanes' simulators were never touched.

Why a fresh `Corral456B`: running the full XCTest suite on a simulator
launches the test host app, whose `startLive()` notification-setup path can
queue the unanswered OS notification prompt on that simulator (the #458
simulator-state lesson). The first `Corral456` capture set was discarded for
that reason and is NOT part of this evidence; `Corral456B` never ran tests,
so every frame here is alert-free. This is a simulator-state artifact of the
test host, not app behavior in normal use.

## Frames

| File | What it proves |
|---|---|
| phase-1-herd-day-fullscreen-390x844.png | Day ranch paints edge-to-edge behind the status bar/Dynamic Island and the bottom home indicator; floating `Filters / All repositories` scope pill + native gear button + truthful five-count summary; floating `Previous · 1 / 3 · Next`. No board header strip, no nav-bar toolbar. |
| phase-2-herd-night-fullscreen-390x844.png | Same single scene, Night lighting, same controls (environment independent of app flavor). |
| phase-3-next-paddock-390x844.png | Bottom navigation moved to paddock 2/3 via the same `movePage` action the Next button invokes; paddock identity (title, `n here · m at rail`) follows. |
| phase-4-scope-sheet-390x844.png | The floating scope control opens the REAL filter sheet (`REPOSITORY SCOPE`, counts, `Clear repository`, `Reset all filters`) over the ranch. |
| phase-5-settings-sheet-390x844.png | The floating gear opens the REAL Settings sheet: Appearance Board/Herd picker (#458 saved presentation intact, Herd selected), themes, Herd environment. |
| phase-6-long-names-390x844.png | 8 synthetic agents with ~45-char repositories and horse names: repo title truncates with `…`, horse names wrap to two lines with `…`, counters stay legible, `1 / 8` paddocks, floating chrome intact. |
| phase-7-empty-scope-390x844.png | Empty fleet: all five counts read `0`, rail reads `0 BLOCKED`, `No agents in this scope` empty state, floating controls still present. |
| phase-8-offline-outage-390x844.png | `-corralHerdOffline`: outage banner `Source disconnected · last-known agents` with `Open Board` + `Retry`, every horse muted/`?` with `last known` status, counts `12 unknown`, rail `LAST KNOWN` — all over the full-screen ranch. |
| phase-9-board-no-regression-390x844.png | Board (list) surface unchanged: pinned Filters control + pull-to-refresh hint, Settings gear top-right, `blocked (2)` / `working (3)` sections with the normal rows. |
| phase-10-ax-day-fullscreen-390x844.png | System content size `accessibility-extra-extra-extra-large`: HUD, rail and bottom bar remain readable, nothing overlaps the status bar or the home indicator, targets stay large. |
| phase-11-ax-long-names-390x844.png | AX-XXXL + long names: single-line truncation/wrapping holds, controls remain reachable. |
| phase-12-ax-empty-scope-390x844.png | AX-XXXL empty scope: counts and empty state readable over the ranch. |
| phase-13-se-day-fullscreen-375x667.png | iPhone SE small screen: ranch full-screen behind the status bar and screen bottom (home button device — no home indicator); all five counts fully visible (wrap fallback, no clipping); floating scope/gear and `Previous · 1 / 3 · Next` fit. |
| phase-14-se-night-fullscreen-375x667.png | Same SE scene at Night. |
| phase-15-se-long-names-375x667.png | SE + long names: truncation/wrapping holds at 375 pt with the floating bottom bar clear of the content. |

## Honest limits

- **Physical iPhone evidence remains the explicit unverified gate.** These are
  simulator frames; they do not prove real-device full-screen/contrast,
  notch/home-indicator ergonomics on hardware, or True Tone/OLED contrast.
- Touch injection is unavailable on `simctl`: the driver invokes the SAME
  actions the controls call (`movePage`, `showFilters`, `showSettings`), and
  the controls' >= 44 pt targets, labels and wiring are pinned by
  `FullScreenHerdShellWiringTests`. No frame here is an injected tap.
- VoiceOver traversal, scroll physics and real Notch cut-out rendering were
  not exercised.
- The cover composition scales the approved 390x640 native world uniformly to
  cover the screen and crops the horizontal overflow (see `.report-456.md`
  "composition rationale"): the prototype's extended browser viewBox would
  have needed the fenced `RanchEnvironment.swift`; the in-fence composition
  keeps the world undistorted but shows a slightly tighter crop at the left
  and right edges on tall phones than the browser prototype.
- The AX frames use the simulator's system content-size setting; they are not
  a VoiceOver/contrast certification.

## Reproduce

See `capture.log` for the exact simulator, build, install, launch and capture
commands. The driver is DEBUG-only (`-corral456FullScreenEvidence` plus
`-corralHerdEvidence` for the seeded fictional fleet; `-corralHerdOffline`
for the outage frame); it writes `Documents/ux-evidence/<phase>.marker` and
holds each phase long enough for the host script to shoot it.
