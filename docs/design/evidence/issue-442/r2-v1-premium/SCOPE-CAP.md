# Guy-approved #442 R2 scope cap — execute now

This direction supersedes further visual-polish loops.

Finish the currently bounded V1 R2 direction at **premium cozy-game quality**, not an AAA/painterly-production bar. Deliver exactly the current three decision images and then STOP:

1. `v1-r2-day-390x844.png`
2. `v1-r2-night-390x844.png`
3. `v1-r2-idle-anatomy-study.png`

Do not add variants, rerun/rebuild the full 21-frame matrix, chase AAA rendering, or open another visual-fix round. Present the best honestly verified procedural/vector result and state its visual ceiling/limitations.

HTML/CSS/Python are disposable design proof, not shipping implementation. Keep the procedural/vector-only boundary: no Comfy Cloud, local ComfyUI, image-generation service, paid API, or new external art dependency.

## Required implementation-ready layer exports

Every visual component must remain procedural, editable, deterministic, and separately exportable. Retain the editable masters/generators and export separately addressable PNG and/or SVG layers for:

- sky + stars / Milky Way;
- distant hills;
- barn + trees;
- fences / blocked front rail;
- foreground grass;
- horse sprite and pose layers, including standing, weight shift, grazing, selected Reduce Motion static, and the identities used in Day/Night;
- any contact-shadow/grounding layers needed to preserve the composition.

For every exported layer, document exact pixel/viewBox dimensions, layer ordering/z-index, anchor/origin points, composition coordinates, parallax ratio, SHA-256, source generator/master, and exact reproduction command. The verifier must prove expected layer names/counts, dimensions/viewBoxes, hashes, and deterministic rerender equality.

Also preserve a clean square-safe horse selection/lineup as editable procedural source plus separately addressable SVG/PNG layer exports, so a future SEPARATE app-icon design issue can derive candidates from the approved horse identity instead of the current icon. Keep the horse silhouette, mane/coat/tack/accessory identity readable within a square safe area and document its anchors/bounds. This is source/handoff evidence only: do NOT replace, edit, or wire the app icon in #442. Future icon work requires its own issue, design gate, and Guy approval.

Keep exports inside `docs/design/evidence/issue-442/r2-v1-premium/` and include them in its manifest. These exports and the square-safe lineup are supporting source assets, not additional decision PNGs; the canonical top-level final PNG count remains exactly three.

## Required native handoff contract

Document enough exact values for a later native implementation issue to recreate composition and behavior:

- native HUD tokens and Catppuccin-independent Day/Night/Auto environment selection;
- spacing, safe-area, typography, tap-zone and nameplate/state-label geometry;
- raw state → mark/word/pose/signature-behavior mappings;
- blocked-front-rail placement and pennant/lantern treatment;
- horse identity axes and pose/state layer mapping;
- animation timings, Reduce Motion substitutions, bounded roaming zones, and optional ambience/haptic defaults;
- foreground/midground/background parallax ratios and swipe/scroll coupling;
- Day/Night lighting treatment and Auto sunrise/sunset permission/fallback behavior;
- shared Board/Herd filter/selection/detail-sheet invariants.

State explicitly: **SwiftUI with SpriteKit or Canvas will recreate the approved composition and behavior. Browser HTML/CSS/Python effects are reusable only where exported as PNG/SVG layers; all other procedural effects must be separately reimplemented in Swift.**

Finish the existing gate mechanics: reproducible build/capture, exactly three final decision PNGs, layer/source manifest, protected-control proof, mirror equivalence, docs-only commit/push, one issue comment beginning `V1 R2 DIRECTION GATE — awaiting Guy approval`, exact readback, and uncommitted `.report.md` final evidence.

No further visual-fix round after this instruction. Preserve all production Swift/Rust, PR, merge, integration/main, TestFlight, deploy, release, and paid-service fences. Stop at the human direction gate.
