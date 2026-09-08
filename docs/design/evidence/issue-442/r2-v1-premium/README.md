# #442 — V1 R2 direction gate: premium cozy-game

Status: awaiting Guy approval. Scope-capped source/design handoff, not implementation approval.
The current three decision images are FROZEN by `decision-freeze.json`. No further visual-fix round.

- [Day · 390×844](v1-r2-day-390x844.png) · [interactive proof](day.html)
- [Night · 390×844](v1-r2-night-390x844.png) · [interactive proof](night.html)
- [Idle anatomy study · 600×760](v1-r2-idle-anatomy-study.png) · [proof](study.html)

## Native/source handoff

- [Native contract](NATIVE-HANDOFF.md): exact implementation proposals vs measured proof.
- [Exports](EXPORTS.md): separately addressable layers and square-safe future-icon SOURCE.
- [Layer inventory](layers/index.json): dimensions, viewBoxes, z, origins/anchors,
  composition coordinates, parallax ratios, SHA-256, generator and reproduction commands.
- [Measured geometry](layers/geometry.json): native-size rendered rects and HUD computed styles.
- [Editable square lineup](layers/square/lineup.svg): supporting SVG only, not a decision PNG.
- [Owner scope cap](SCOPE-CAP.md) and [honest visual limits](visual-review.md).

## Reproduce exactly

From this directory, with Python 3 + Pillow and the existing pinned Chromium headless-shell
in `~/Library/Caches/ms-playwright/chromium_headless_shell-1234/` plus macOS `sips`:

    python3 -B run-gates.py

Individual commands:

    python3 -B build.py
    python3 -B capture.py
    python3 -B export-layers.py
    python3 -B probe.py
    python3 -B verify-layers.py
    python3 -B verify.py --repro

`run-gates.py` saves raw exits and logs, then regenerates the manifest.
`verify-layers.py` independently asserts the exact expected layer inventory, all dimensions,
viewBoxes, hashes, required metadata, safe frames, unchanged frozen PNGs, and deterministic
asset/metadata regeneration. It also proves exact browser pixel equality between intact
Day/Night worlds and their reassembled exported vector layers at the SAME screen origin.
SVG turbulence samples change with device-space origin; different-offset comparisons are invalid.

After deliberate mirror copy:

    python3 -B verify.py --mirror /Users/jirathip/design-output/corral/442-herd-view/r2-v1-premium
    shasum -a 256 -c manifest.sha256

Manifest excludes itself and its independent check log; counts are printed by gates.
Exactly three canonical top-level decision PNGs. Supporting layers are SVG, not extra decisions.

## Boundaries

No production Swift/Rust, app-icon asset/wiring, PR, merge, integration/main, TestFlight,
deployment, release, paid API, or generation service changes. Future icon work requires
its own issue, design gate, and Guy approval. HTML/CSS/Python are disposable design proof.
