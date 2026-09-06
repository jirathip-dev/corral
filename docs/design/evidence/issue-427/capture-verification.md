# Capture and verification — Corral #427

## Final result

PASS.

- full evidence matrix: 168/168 exact-size PNGs;
- Dynamic Type/accessibility: 6/6 exact-size PNGs;
- A/B/C comparison: 1/1 at 1440×1080;
- total required captures: 175/175;
- rendered DOM cases: 60/60 PASS;
- contrast checks: 28/28 PASS;
- minimum measured contrast: 4.68:1 (Frappé text on surface1 status header);
- console errors: 0 across all DOM cases;
- horizontal overflow: 0 cases;
- interactive hit-target failures: 0 cases;
- scope-order failures: 0 cases;
- VoiceOver-scope-label failures: 0 cases;
- scroll-reach failures: 0 cases;
- deterministic interaction failures: 0 cases.

Machine-readable detail:

- `verification.json`
- `dom-verification.json`
- `contrast-checks.json`
- `asset-manifest.json`
- `capture.log`

## Visual inspection

Final contact sheets were inspected after the last full recapture:

- A at 390×844 and 375×812;
- B at 390×844 and 375×812;
- C at 390×844 and 375×812;
- paired Dynamic Type top/end states;
- final A/B/C comparison sheet;
- full-size Latte narrow/offline, Mocha narrow/zero, and Frappé connecting frames.

Observed and corrected before the final recapture:

1. moved global reset from the sheet title row to a pinned footer;
2. removed raw source-evidence wording from the visible plush-meadow option and documented the missing count instead;
3. moved selected checks to the native trailing edge;
4. separated Host and Repository scopes with a themed surface break;
5. removed the evidence watermark from the phone safe area; filenames carry labels;
6. replaced ambiguous working ellipsis text with three explicit state squares;
7. moved Latte status headers from surface1 to surface0 so text clears AA;
8. paired Dynamic Type top and scrolled-end captures to prove the final option remains reachable.

No blocking clipping or reachability defect remained after correction. Board content can continue below the viewport by design; the board is the product’s scroll surface, not lost content.

## 10-tell anti-slop audit

| Tell | Result |
|---|---|
| wrong surface archetype / hero treatment | PASS — Monitor surface, no hero |
| oversized display typography | PASS — SF hierarchy stays compact |
| symmetric feature-card composition | PASS — absent from product UI; comparison cards are evidence-only |
| decorative gradients / glow | PASS — none; material tint has a functional role |
| invented brand colors | PASS — exact Catppuccin tokens only |
| fake metrics / testimonials / filler | PASS — source evidence + documented fixture data only |
| excessive rounding and shadows | PASS — native sheet radius; no ornamental card field |
| everything becomes a pill | PASS — options are native rows, board hierarchy stays flat |
| gratuitous motion | PASS — immediate selection; reduced-motion override |
| generic copy / generic title | PASS — Corral vocabulary; removed `Fleet` title stays removed |

Composition tells 3, 8, and 10 do not fire; no recomposition was required after the final pass.
