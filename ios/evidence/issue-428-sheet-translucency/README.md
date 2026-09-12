# 428 evidence — runtime masking surfaces fixed: themed form cells + layered native-glass backdrop

Frames for the #428 change (390x844 px, iPhone 16 @3x 1179x2556 downscaled
with `sips -z 844 390` — the repo's standard device class; 0.18 % aspect
distortion from the 393x852 pt native, same as the #416 set). All frames
are SYNTHETIC DEBUG demo captures over `DemoFleet.seed()` / the #401
multi-host seed (fictional repos/agents; no live daemon, no physical
device, no TestFlight claim).

## What the lane found (measured at the base head, a55018f)

Two surfaces masked the approved translucent treatment AT RUNTIME (the
same screenshots the physical report describes — rendered-pixel analysis,
not source reading):

1. **Native inset-grouped FORM cells in Settings / Add Host / the
   fingerprint sheet**: iOS paints those cells with the SYSTEM grouped
   surface (white in the light schemes, neutral #2c2c2e-class gray in the
   dark ones) regardless of the theme. Measured on the base frames: the
   Settings cells dominate at (44-46, 44-46, 49-52) in Mocha and
   (208-213, 211-219) in Latte — never the active flavor's base token —
   and the LATTE LARGE sheet was 100 % system white (#ffffff/#fdfdfe).
   That is the "non-translucent/non-themed surface" and AC2's
   "system-default white/black/grouped surface".
2. **The iOS 26 backdrop rendered ONLY `.clear` glass**: a surface with no
   material response of its own — it samples what sits behind the sheet
   (the dimmed board) and adds nothing. Measured: the backdrop regions in
   the sheets were neutral gray (31-33, 32-33, 36-37) in Mocha —
   flavor-less and visually indistinguishable from an opaque base paint —
   which is why the sheet read as a flat dark slab with no blur/material
   response on the device (glass over a dimmed dark board samples dark).

## What changed (one shared contract across the three sheets)

- `TranslucentSheetBackdrop` iOS 26 branch: native `.regular` Liquid
  Glass OVER the SAME tinted-material recipe the <26 fallback runs
  (ultraThinMaterial + base at the locked `fallbackTintAlpha`, glass tint
  at the locked `glassTintOpacity`). The frost and the flavor no longer
  depend on what the glass samples; `.clear` glass is gone. <26 fallback
  unchanged; both locked alpha bands and the WCAG floor math unchanged.
- A shared `themedRowSurface(_:)` helper paints every Form/List row of the
  Settings, Add Host, and fingerprint-confirmation sections with the
  active flavor's BASE token (the same surface the board rows use) —
  native grouped chrome (rounding/insets/separators) is preserved, the
  system white/gray cells are gone.
- Recent Output's content surfaces (header, output cards, non-loaded
  state backings) are UNCHANGED — they are the approved opaque text
  layers (AC4); their backdrop slivers now show the tinted-material+glass
  surface instead of the flat neutral.

## Frame map (all 390x844)

Medium detent (board visible above/behind — the underlay proof), native
glass unless `-fallback`:

| File | Shows |
|---|---|
| phase-1-board-mocha-390x844.png | Busy MOCHA board alone (A/B control) |
| phase-2-recents-mocha-390x844.png | Recent Output sheet (medium) over the busy board |
| phase-3-recents-latte-390x844.png | Recent Output sheet, Latte |
| phase-4-board-latte-390x844.png | Busy LATTE board alone (A/B control) |
| phase-5-settings-mocha-390x844.png | Settings sheet (evidence medium detent) |
| phase-6-settings-latte-390x844.png | Settings sheet, Latte |
| *-fallback-*.png (pairs above) | The SAME scenes on the forced <26 material branch |

Release presentation path (system LARGE detent — what release actually
shows; the busy board cannot stay in view behind a full-height sheet):

| File | Shows |
|---|---|
| phase-5-settings-mocha-large-390x844.png | Settings at LARGE, Mocha |
| phase-6-settings-latte-large-390x844.png | Settings at LARGE, Latte |
| phase-5-settings-mocha-large-fallback-390x844.png | LARGE, forced fallback |
| phase-6-settings-latte-large-fallback-390x844.png | LARGE, forced fallback |

Frappé/Macchiato spot run (AC3 — no flavor-independent system surface):

| File | Shows |
|---|---|
| phase-416-5-settings-frappe-390x844.png (+ -fallback) | Settings medium, Frappé |
| phase-416-6-settings-macchiato-390x844.png (+ -fallback) | Settings medium, Macchiato |
| phase-416-2-recents-frappe-390x844.png (+ -fallback) | Recent Output medium, Frappé |
| phase-416-3-recents-macchiato-390x844.png (+ -fallback) | Recent Output medium, Macchiato |

Add Host (multi-host seed, medium):

| File | Shows |
|---|---|
| phase-7-addhost-entry-mocha-390x844.png (+ -fallback) | Add Host entry, Mocha |
| phase-8-addhost-confirm-latte-390x844.png (+ -fallback) | Fingerprint confirm, Latte |

## Discriminating gate (AC7): rendered pixels, not source strings

`translucency-analysis.py` asserts on the ACTUAL rendered frames:

- form cells are the flavor's BASE token (dominant tone == base ± 6), no
  system grouped surface (white / #2c2c2e / #1c1c1e) above 3 %;
- backdrop regions carry the flavor cast (dark flavors: b-r >= 7;
  Latte: frosted-base luminance floors per scene) — the old neutral
  flavor-less surface fails;
- the glass and forced-fallback branches still differ measurably in
  backdrop-owned regions (the material response survived the rework);
- modes: medium, large (release), spots (Frappé/Macchiato), addhost.

`analysis.txt` records the raw runs:

- GREEN at the fixed head (this directory): medium 8/8 + A/B, large 6/6,
  spots 8/8, add-host 4/4 — all RC 0;
- RED at the base head a55018f (frames staged under /tmp during the
  lane): medium 1/8 (7 FAIL), large 1/6 (5 FAIL) — RC 1 — the same
  script and regions fail on the pre-fix captures, proving the gate
  discriminates the ACTUAL rendered result.

## Branch forcing + honest limits (same as #416)

- Native glass: the app's normal iOS 26 path — nothing forced.
- Fallback material: this host has ONLY the iOS 26.5 runtime, so the
  17-25 branch cannot run natively. `-corral416ForceFallbackBackdrop`
  (DEBUG-only launch argument) makes the shared backdrop take the SAME
  `else` branch an iOS 17-25 runtime executes — identical source; the
  availability check is the only difference.
- The iOS 26.5 SIMULATOR never composites the presenting view BEHIND the
  sheet card (probe-verified in #416) — "board content through the sheet
  surface" cannot be pixel-shown here for ANY recipe. The faithful
  on-host proof = the surfaces that mask/allow the treatment (cells,
  chrome, backdrop material response) + the A/B branches; physical
  iPhone pixel evidence is the issue's HUMAN gate and was not fabricated
  in this lane.
- The spot-flavor runs reuse the #416 driver phases via the DEBUG
  `-corral428SpotFlavors` launch argument (Frappé/Macchiato substitute
  for Mocha/Latte; Release never contains the argument's effect — the
  substitution properties are DEBUG-gated to `false`).

SHA-256s: `SHA256SUMS.txt` (all artifacts above). Capture commands:
`capture.log`.

## Correction pass (bounded #428 repair): sheet background unmasked + gate G5

Owner verdict on build 21: the sheet still read as a flat dark slab; the
investigation found the opaque coverage left only thin margins on the
shared backdrop and that THIS host cannot composite the presenter behind
the sheet card (see below). This pass removes the remaining masking
layer over the sheet BACKGROUND:

- `RecentOutputSheet.header`: the opaque base backing now hugs the
  caption row; the band's vertical padding stays on the shared backdrop.
- `RecentOutputSheet` loading/empty/error panels: the opaque base backing
  now hugs the tier instead of painting the whole content area.
- The locked `SheetBackdrop` alphas and their WCAG floor math are
  UNCHANGED (no floor was lowered; the `theme.base` backings under every
  text tier are unchanged).
- `-corral428MaskSheetBackground` (DEBUG-only, Release never contains
  its effect) paints the pre-repair opaque coverage back over the card
  as the gate's masking-layer RED control.

### Correction frames (`correction/`, medium detent, iPhone 16 @3x → 390x844)

| File | Shows |
|---|---|
| phase-416-2-recents-mocha / -3-recents-latte | candidate: in-sheet ring + header band on the backdrop |
| phase-416-2-recents-mocha-fallback | the forced <26 material branch, same scene |
| phase-416-5-settings-mocha (+ -fallback) | Settings sheet (shared contract), glass vs fallback |
| phase-416-1-board-mocha | busy-board A/B control |
| phase-416-2-recents-mocha-masked, -3-recents-latte-masked | masking-layer revert (RED) |
| masked/ | the same RED frames under standard names for the gate run |

### Gate G5 (rendered pixels, inside the card rect)

`translucency-analysis.py` now checks the recents sheet's background
regions INSIDE the card (left/right in-card ring + the strip under the
last block): they must measure as the TRANSLUCENT backdrop (max-channel
delta from the opaque base paint >= 4.5) — a region painted base reads
as base exactly. Runs (raw logs in `correction/analysis-correction.txt`):

- GREEN: candidate frames, default mode — **12/12, RC 0** (mocha ring
  max-delta 5.0/5.9, latte 9.3/13.3);
- CONTROL: masked frames via `--mode=masked` — **4/4, RC 0** (the ring
  measures (30,30,46)/(239,241,245) = the paint exactly: the check bites);
- RED: the SAME masked frames under the default mode — **3/8, RC 1**
  (ring max-delta 0.0/0.1/0.3 → FAIL): the masking-layer revert breaks
  the rendered gate; the candidate frames restore it.

### What this host still cannot show (unchanged, re-verified)

- The iOS 26.5 simulator never composites the presenting board behind
  the sheet card (#416 probe; re-confirmed here by cross-era measurement:
  the in-card ring column correlates ~0 with the board while the
  OUTSIDE screen-edge sliver correlates 0.87-0.94 — the sliver is the
  presenter, not read-through; the #385 correlation gate had measured
  that sliver). In-card board-content read-through is therefore not
  pixel-showable on this host for ANY recipe; the discriminating
  rendered facts are the surface that allows it + the masking revert.
- Physical-device appearance remains OPEN (the human gate): the locked
  tint share (base @ 0.8 + material + native glass over the system-dimmed
  presenter) is unchanged by this pass and can only be judged on an
  iPhone.
