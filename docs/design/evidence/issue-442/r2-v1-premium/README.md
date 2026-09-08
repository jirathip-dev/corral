# V1 R2 — premium Pasture Panorama direction gate

V1 is the accepted control. This single R2 visual refinement is NOT yet approved by Guy.
No production implementation, Swift/Rust changes, PR, merge, integration/main change,
TestFlight, deployment, or release. These are static direction images, not a new app.

## Exactly three decision images

- [Day](v1-r2-day-390x844.png): 390 × 844.
- [Night](v1-r2-night-390x844.png): 390 × 844.
- [Idle anatomy / pose study](v1-r2-idle-anatomy-study.png): 600 × 760.

Every study specimen is 148 × 112 CSS/output pixels, the exact paddock sprite size.
Front-rail sprites now use that same size. Day and Night retain identical horse SVGs,
fixture, positions, labels and layout. Environment illumination changes independently.
The first visible repository remains atlas-vector. The fictional 12-agent fixture
still spans four horizontally traversable repository paddocks.

## Surface and small design system

Surface: Monitor, preserving V1's side-on front rail plus horizontal repository paddocks.
System-font native hierarchy; 22px app heading, 15px repository heading, 11px persistent
horse labels. Compact 10px aggregate summaries fit all five raw states without clipping.
Existing Mocha operational token vocabulary is retained; native glass is navy with
restrained translucent highlights, 9–12px corners, no fantasy ornaments.
Natural coat/mane/tack colors are stable identity assets, not Catppuccin tokens.
World art uses atmospheric blue/indigo, warm directional Day illumination, cool
moon-facing Night rims, wood grain/undersides and grounded grass planes.

## Day / Night / Auto contract

Day/Night/Auto environment selection is independent of Catppuccin palette.
With existing device-location permission, Auto uses local sunrise/sunset. Location
permission is never mandatory and is not requested by this artifact. If permission
or a valid solar calculation is unavailable, use fixed device-local 07:00–19:00 Day,
otherwise Night; reevaluate when the device timezone/local day changes. A user can
always choose Day or Night explicitly. Only clear Day and clear Night are rendered.
Weather and seasonal packs are deferred.

## Motion and operational boundary

Future parallax is driven by horizontal swipe/scroll: foreground rail/grass fastest,
midground horses/paddock/barn slower, distant ridges/sky slowest. Only very slow ambient
layer drift is allowed. No gyroscope requirement or autonomous cinematic camera.
Horses may roam only within stable bounded tappable zones in their own repository.
Idle language: calm standing, weight shift, small ear/tail movement, occasional grazing.
Working: restrained purposeful step; blocked: alert at the physical front rail;
done: relaxed settled stand; unknown: cautious still pose plus explicit ? unknown.
Disconnected: intentional static unknown/last-known presentation with an explicit
outage indication, never fabricated live activity. Existing control contract remains.
Reduce Motion deliberately selects calm standing for idle, planted alert for blocked,
settled working/done, and still unknown; no roaming, parallax, ambient drift or pulse.
The study's selected RM pose is deliberately regenerated standing, not a frozen frame.
Silent by default. Ranch ambience and haptics, if implemented later, must be opt-in.

The fixed Day/Night HTML capture stages are intentionally static: their native-style
Board/Herd, Filters and detail affordances are shown, not newly wired. The accepted
control remains the authority for the shared Board/Herd model, filters, selection and
same native agent-detail/recent-output sheet. This gate does not claim new functional
coverage or rebuild the previous evidence matrix.

## Reproduction (macOS)

From this canonical R2 directory:

    PYTHONDONTWRITEBYTECODE=1 python3 run-gates.py
    shasum -a 256 -c manifest.sha256
    PYTHONDONTWRITEBYTECODE=1 python3 verify.py --mirror /Users/jirathip/design-output/corral/442-herd-view/r2-v1-premium

Requires Python 3 standard library, installed Playwright-cache chrome-headless-shell,
and macOS sips. No new installation, remote generation or external art required.
`capture.py` chooses the lexically latest cached headless-shell; shipping logs pin
build 1234. Captures use 2× device scale then sips to exact requested dimensions.
Reproducibility compares fresh temporary HTML and PNG bytes, not just dimensions.
Temporary capture/rerender directories are automatically removed.
For a standalone art rerender from the mirror use `python3 -B build.py` then
`python3 -B capture.py`; control/scope verification must run in the Corral worktree.

## Evidence integrity

`protected-control-baseline.txt` preserves the previous session's Git blob baseline.
`control-sha256.json` separately hashes all 79 pre-existing tracked control files from
exact HEAD ecd3938a72cdfce256128c7d437dcd589141baf5. No protected file is overwritten.
`control-input.html` and `fixtures.py` are byte-exact copies of accepted inputs.
`art.py`, `build.py`, `capture.py`, `probe.py`, and `verify.py` retain the full process.

Manifest entries cover all payloads and build/capture/browser/verify/exit logs.
The manifest excludes itself and `logs/manifest-check.log` (the independent check's
self-referential output). Raw package count and manifest entry count therefore differ
by two. Both manifest-listed PNG count and raw canonical PNG count must equal three.
`verify.py --mirror` compares raw path sets before every file's SHA-256, including
those two excluded files. Delivery/head/comment readback logs live in uncommitted
root `.report.md`, avoiding self-referential commit hashes inside the package.
