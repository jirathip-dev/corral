# 416 evidence — perceptible translucent sheet treatment (Recent Output / Settings / Add Host)

Frames for the #416 fixed-parity change (390x844 px, iPhone 16 @3x
1179x2556 downscaled with `sips -z 844 390` — the repo's standard device
class; 0.18 % aspect distortion from the 393x852 pt native, same as the
#385 set). All frames are SYNTHETIC DEBUG demo captures over
`DemoFleet.seed()` / the #401 multi-host seed (fictional repos/agents; no
live daemon, no physical device, no TestFlight claim).

## What changed (shared treatment)

- `SheetBackdrop.glassTintOpacity` 0.3 -> 0.1 (band re-locked 0.05–0.2):
  the iOS 26+ NATIVE Liquid Glass keeps only a whisper of the flavor
  tint. The #385 measurement that justified 0.3 compared the dimmed
  region ABOVE the sheet against the board (not the sheet surface — see
  the mechanism appendix), and on-device glass renders darker still; a
  heavy tint paints clear glass into a flat solid.
- `SheetBackdrop.fallbackTintAlpha` 0.88 -> 0.8 (band re-locked 0.75–0.85):
  the <26 ultraThinMaterial blur now holds a fifth of the fallback
  surface instead of ~12 %. 0.8 is the LOWEST value that keeps the
  preserved worst-case WCAG lock green for every flavor (frappe is the
  tightest at ~4.97:1) — going lower would weaken the existing
  contrast checks, which #416 preserves.
- The whole-sheet opaque `.background(theme.base)` fills were removed
  from the Add Host sheet AND the fingerprint/key-confirmation sheet —
  both had `.translucentSheetBackdrop(theme.base)` applied but painted an
  opaque base layer over it, masking the backdrop completely (the
  masking-layer class this issue is about). The Settings sheet and the
  Recent Output sheet already had no whole-sheet fill; header/state/
  output-card surfaces stay opaque on purpose (AC3 hierarchy).

## Frame map

Native-glass branch (iOS 26+, the default on this 26.5 host):

| File | Shows |
|---|---|
| phase-1-board-mocha-390x844.png | Busy MOCHA board alone (A/B control) |
| phase-2-recents-mocha-390x844.png | Recent Output sheet (medium detent) over the busy board |
| phase-3-recents-latte-390x844.png | Recent Output sheet, live Latte flip |
| phase-4-board-latte-390x844.png | Busy LATTE board alone (A/B control) |
| phase-5-settings-mocha-390x844.png | Settings sheet at the EVIDENCE medium detent (see below) |
| phase-6-settings-latte-390x844.png | Settings sheet, Latte |
| phase-7-addhost-entry-mocha-390x844.png | Add Host entry form over the multi-host board |
| phase-8-addhost-confirm-latte-390x844.png | Add Host fingerprint-confirm phase, Latte |

Material-fallback branch (iOS 17–25 recipe, FORCED on this 26.5 host —
see below): the same eight scenes with a `-fallback` suffix.

## Branch forcing (honest capture notes)

- Native glass: the app's normal iOS 26 path — nothing forced.
- Fallback material: this host has ONLY the iOS 26.5 runtime, so the
  17–25 branch cannot run natively. `-corral416ForceFallbackBackdrop`
  (DEBUG-only launch argument, `Corral416Evidence.forceFallbackBackdrop`)
  makes the shared `TranslucentSheetBackdrop` take the SAME `else`
  branch an iOS 17–25 runtime executes — identical source; the
  availability check is the only difference. Frames therefore exercise
  the real fallback recipe (`.ultraThinMaterial` + base at
  `fallbackTintAlpha`) but on the 26.5 compositor.
- Medium detent: the Recent Output sheet's release detents are
  `[.medium, .large]` (opens medium). The Settings and Add Host sheets
  use the system LARGE detent in release; at LARGE on iOS 26 the system
  does not keep the presenting board in view, so the #416 evidence
  captures them at the MEDIUM detent via the DEBUG-only
  `-corral416MediumDetents` evidence argument (`MediumDetentsForEvidence`).
  Release launches keep the LARGE detent.

## What the simulator can and cannot prove (mechanism appendix)

Probes run during the lane (same DEBUG app, backdrop recipe selected by
launch argument) established a hard compositing fact of the iOS 26.5
SIMULATOR: UIKit never composites the presenting view BEHIND the sheet
card. With a fully transparent (`Color.clear`) presentation background the
region under the card still shows only the app window's flat backdrop —
no board rows appear in the inter-card gaps at any recipe (pixel-verified
row-band correlations ~0 vs the board frame; a solid-green probe backdrop
paints the surface green, proving the backdrop view owns the surface).
The board is rendered only ABOVE the sheet's top edge at the medium
detent (dimmed by the system scrim). Consequences:

- The re-locked glass tint (0.3 -> 0.1) is NOT pixel-visible on this sim
  (glass-vs-glass frames differ by <0.3 mean channel delta) — the sim
  renders clear glass over a flat backdrop. The change is real-device
  material: on-device Liquid Glass samples and blurs the content behind
  the sheet, where the tint level decides how much of it survives.
- The forced-fallback A/B IS measurable on the sim: `analysis.txt` shows
  the material branch rendering measurably lighter/frosted on identical
  geometry (mean deltas +9..+36 channel-sum vs the glass branch across
  both flavors and all three sheet surfaces), and the vision inspection
  of the latte fallback frame reads the surface as a frosted material
  rather than a flat paint fill.
- On iOS 17–25 (where the fallback actually runs) sheets composite and
  blur the presenter behind the card — the material is a real blur layer
  there, and 0.8 (vs the old 0.88) is the perceptible end of the tint
  that the preserved WCAG floor allows.

Vision inspection (fields/text readable, hierarchy intact): done on the
mocha + latte recents frames, the mocha + latte settings frames, and the
mocha/latte Add Host frames for BOTH branches — text tiers stay readable
over the treated surfaces (AC3) and the busy board stays in view above/
around the medium-detent sheets (AC4). The treated sheet surfaces never
carry an opaque whole-sheet paint in these frames (see the diff: the Add
Host / key-confirmation fills are gone); card interiors and the header
strip remain flat single-tone opaque surfaces (opaque-control rows in
`analysis.txt`).

## Verification gates at the reviewed head (see .report-416.md)

- Focused: SheetBackdropTests (5) + SheetTranslucencyWiringTests (4) —
  9 tests, 0 failures.
- Full iOS suite: 316 tests, 0 failures.
- RED/GREEN mutation probe: re-pointing the Add Host sheet to an opaque
  `.background(theme.base)` trips both new wiring assertions
  (missing modifier + applied opaque fill); restored byte-identical ->
  green.
- `python3 ios/check-release-demo.py --self-test` PASS (digests
  re-pinned for the #416 source/test sets).
- xcodegen NO_DRIFT + `git diff --check` clean.

SHA-256s: `SHA256SUMS.txt` (all artifacts above).
