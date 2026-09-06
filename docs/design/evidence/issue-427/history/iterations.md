# Iterations

## Round 1 — grounded structure

- Read the locked #427 specification, issue body, `FleetViews.swift`, `BoardModel.swift`, `AppTheme.swift`, theme tests, DemoFleet fixture, and #401 implementation screenshots.
- Locked the surface archetype to Monitor.
- Built A bottom sheet, B inline rail, and C Scope Path over the existing board hierarchy.

## Round 2 — visual QA correction

- Strengthened material/scrim separation while preserving visible board context.
- Simplified option metadata and removed raw evidence-debug copy.
- Moved global reset to a pinned sheet footer.
- Added stronger scope separation and native trailing checkmarks.
- Removed safe-area evidence watermark; retained full labels in filenames.
- Replaced an ambiguous ellipsis glyph with the existing three-square working mark.

## Round 3 — accessibility and contrast

- Detected Latte text/surface1 at 4.39:1.
- Rebound only the Latte thick status surface to the existing `surface0` token; the corrected pair passes AA.
- Added paired Dynamic Type top/end captures and runtime scroll-reach proof.
- Regenerated all 175 required images, rebuilt contact sheets, reran 60 DOM cases and 28 contrast checks.

Final: PASS, recommendation A, approval still required.
