# Corral #385 — Liquid Glass / translucent sheets evidence (RecentOutputSheet + Settings)

Real compiled capture of the #385 change: RecentOutputSheet and the Settings
sheet now float over a SHARED translucent backdrop
(`TranslucentSheetBackdrop` + `.translucentSheetBackdrop(_:)` in
FleetViews.swift):

- iOS 26+ (this host's runtime): Native Liquid Glass —
  `glassEffect(.clear.tint(theme.base @ SheetBackdrop.glassTintOpacity))`,
  availability-gated (`#available(iOS 26.0, *)`). The `.clear` style is used
  instead of `.regular`: over the system sheet scrim the regular glass reads
  as an opaque dark slab; clear glass keeps the board visible through the
  sheet (pixel-verified below).
- iOS 17–25: translucent fallback — `ultraThinMaterial` blur + the active
  flavor's base at `SheetBackdrop.fallbackTintAlpha` (0.88, inside the
  spec-locked 0.85–0.90 band). Deployment target is 17.0.
- Board rows/chips are untouched opaque theme surfaces (scope fence #371);
  only the two sheets' backgrounds became translucent.

The artifact is SYNTHETIC ONLY: the DEBUG `-corralDemoGlassEvidence` driver
over `DemoFleet.seed()` (fictional repos/agents, no live daemon, no physical
device). No live-fleet, physical-device, or TestFlight claim.

## Artifact

One deterministic launch records the full A/B sequence (the driver flips the
same state the gear button / row taps / Appearance rows set; each phase
writes a marker the host capture script polls, then screenshots):

| Phase | File | Shows |
|---|---|---|
| 1 | `phase-1-board-mocha-390x844.png` | Busy MOCHA board alone (baseline + A/B board for phase 2) |
| 2 | `phase-2-recents-mocha-390x844.png` | Mocha recents sheet over the busy board (translucent) |
| 3 | `phase-3-recents-latte-390x844.png` | Latte recents sheet (live flavor flip while presented) |
| 4 | `phase-4-board-latte-390x844.png` | Busy LATTE board alone (A/B control for phase 3) |
| 5 | `phase-5-settings-mocha-390x844.png` | Mocha Settings sheet (shared backdrop) |
| 6 | `phase-6-settings-latte-390x844.png` | Latte Settings sheet (shared backdrop) |
| 7 | `phase-7-done-390x844.png` | Rest state after the sequence (latte board, all sheets dismissed) |

All rendered 390x844 pt (raw 1179x2556 px iPhone 16 / iOS 26.5 @3x,
`sips -z 844 390`; 0.18 % aspect distortion from the 393x852 pt native —
same standard as issue-316/362).

## Translucency proof (recents A/B, both flavors)

`translucency-analysis.py` (stdlib + Pillow) correlates a column INSIDE the
recents sheet surface (its left margin, which no card covers) against the
SAME column of the board-only frame blurred by a Gaussian kernel. An opaque
surface cannot track the underlying content (flat column, ~0 correlation);
a translucent one does. Raw output in `analysis.txt`:

- `[recents Mocha]` sheet-surface column vs blurred board: corr=0.772,
  surface-stdev=5.77 vs board-stdev=27.48, tones=2 → TRANSLUCENT.
- `[recents Latte]` corr=0.892, surface-stdev=24.56 vs board-stdev=36.02,
  tones=4 → TRANSLUCENT.
- Opaque control (block-card interiors are one flat tone): mocha card
  stdev=9.6 tones=2, latte card stdev=0.0 tones=1.
- VERDICT: PASS.

Both flavors therefore meet AC "content visible beneath the sheet" in the
worst dark case (mocha) and light case (latte); the recents content tier
keeps its contrast because block cards and the header strip's caption row
retain their opaque token backing (see SheetBackdropTests worst-case WCAG
lock: every Catppuccin flavor's text over the 88 % tinted backdrop holds
>= 4.5:1 against any palette token the busy board can paint underneath).

Settings frames note: the Settings sheet presents at the system LARGE
detent. On iOS 26 large sheets, UIKit does not render the presenting view
behind the sheet card (only the dimmed strip above the sheet's top curve —
visible in phases 5/6), so the sheet surface over that region reads as the
glass over a uniform backdrop; the SAME shared backdrop modifier is wired on
both sheets (source-wiring test `SheetTranslucencyWiringTests` pins both
call sites + both paths). The medium-detent recents sheet is the A/B pixel
proof of the backdrop itself.

## <26 fallback path

This host has only the iOS 26.5 runtime (`xcrun simctl list runtimes`), so
the 17–25 tinted-material fallback cannot be runtime-captured here. It is
proven by: compile (availability-gated branch in
`TranslucentSheetBackdrop`, deployment target 17.0),
`SheetTranslucencyWiringTests.testBackdropCarriesBothTheGlassAndTheMaterialFallbackPaths`
(asserts the `.ultraThinMaterial` + `SheetBackdrop.fallbackTintAlpha`
branch exists in the compiled-source bundle), and
`SheetBackdropTests` (alpha 0.88 inside the spec band + worst-case WCAG).

## Capture commands (capture.log)

Fresh simulator (no keychain identity → no notification-permission alert
race), DEBUG build from this branch, `-corralDemoGlassEvidence` launch,
marker polling, `simctl io screenshot` per phase, `sips` resize to 390x844.

SHA-256s: `SHA256SUMS.txt` (all artifacts above).

Evidence conventions follow docs/design/evidence/issue-328-330 and
issue-362.
