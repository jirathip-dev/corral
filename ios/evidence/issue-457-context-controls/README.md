# 457 evidence — one shared context-aware filter sheet + contextual Herd controls

Runtime frames for the #457 slice: the floating Herd scope trigger, the HUD
counts and the ONE shared filter sheet take the ranch Day/Night surface
treatment while the sheet keeps its explicit presentation context (Herd vs
Board); the Board treatment and the global theme preference are unchanged.

## Frame map (committed set, 27 frames, 1x downscales)

iphone16-457-1…12 — iPhone 16 (iOS 26.5), -corral457ContextEvidence driver:
  1  herd day trigger       closed chrome (pill + counts + gear) over the Day ranch
  2  sheet day all          open sheet, default scope (All hosts / All repos)
  3  sheet day selected     Host A · demo-atlas selected
  4  sheet day scrolled     scrolled past the first section
  5  sheet day latte        flavor switched to Latte for the board — sheet unchanged
  6  sheet night latte      Night · Latte
  7  sheet night frappé     Night · Frappé
  8  sheet night macchiato  Night · Macchiato
  9  sheet night mocha      Night · Mocha
  10 sheet night scrolled   Night · scrolled
  11 herd night trigger     closed chrome · Night
  12 herd done              post-sheet chrome

iphone16-opaque-* — -corral457ForceOpaqueChrome (the same opaque branch a
  Reduce Transparency device executes): day trigger, day sheet, night end.
iphone16-hc-* — simulator Increase Contrast genuinely enabled
  (simctl ui increase_contrast enabled): day trigger, day sheet, night end.
iphone16-ax5-* — content_size accessibility-extra-extra-extra-large: day
  sheet, night end.
iphonese-* — iPhone SE 3rd gen: day trigger, day sheet, night end.
iphonese-ax5-* — iPhone SE at AX-XXXL: day sheet, night end.
iphone16-457-20…21 — -corral457BoardShot: the Board keeps its Catppuccin
  sheet + toolbar (no-regression shot).

## What the pixels show (measured with PIL on the raw 3x frames)

- Day chrome (ranch treatment): pill 213,216,202 with dark-ranch ink 35,54,47.
- Day sheet: 221,224,205 (ranch cream) with the shared sections; scrolled rows
  (457-4/457-10) show the REPOSITORY SCOPE list; AX-XXXL frames keep the sheet
  readable (summary truncates by design, no clipped rows).
- Night: sheet 29,43,51 at BOTH Frappé (457-7) and Mocha (457-9) — the flavor
  switch moves the board theme, never the ranch context (the sheet is the
  ranch-night surface, not a Catppuccin surface).
- Board (457-20…21): sheet 44,44,61 — the Catppuccin surface, unchanged.
- Fallback: Increase Contrast (real setting) and -corral457ForceOpaqueChrome
  both render the opaque day chrome 246,245,225 vs the glass composite
  213,216,202 (and the same opaque sheet backdrop). At night the two
  composites land within ~1/255 on this runtime (the runtime's night glass is
  near-opaque); the branch itself is asserted by the render-composite tests
  and the MUT2 probe (fallback ignored → composite 208 vs 246), so the
  night-frame equality is a runtime property, not a missing fallback.
- The sheet-open frames dim the ranch behind the sheet (the standard modal
  dimming); the Day/Night ranch remains visible around the chrome. This
  dimming is the real presentation — no compositing tricks.

## Reproduce

    # once per sim, before capturing (clears a pending notification alert the
    # unit suite can leave behind — see capture.log PRECONDITION):
    xcrun simctl shutdown $R1; xcrun simctl shutdown $SE
    xcrun simctl boot $R1; xcrun simctl boot $SE
    bash ios/evidence/issue-457-context-controls/capture-all.sh

Raw 3x frames land in /tmp/g457-raw-<prefix>/ (regenerable; not committed).
The committed set above is the curated 1x downscale; SHA256SUMS.txt pins it.

## Scope

Physical translucency/theme fidelity on a real iPhone stays the remaining
human gate — the physical phone keeps the app open; these are simulator
frames of the actual app build.
