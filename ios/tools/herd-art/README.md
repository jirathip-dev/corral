# Native Herd (#444, correction 1)

Board and Herd are presentations of the existing BoardModel projections. The
Board's reconciled repository/host scope feeds Herd; taps use the existing
model-owned Recent Output request. No protocol, capability or store was added.
Board remains the default, dense operational presentation.

## Shipping drawing

`UI/Herd/HerdArt.swift` draws the original V1 horse grammar with native Path
operations. The only anatomy redesign is the grazing neck, separate head/jaw,
paired ears and grounded muzzle, with small grazing-only knee accents.
Coat, breed, mane, tack and accessory are the original name-derived SHA-256 axes.
Standing, working, blocked, done and unknown retain the original pose vocabulary.
A stationary minimum 156-point-wide button contains 132×100-point art. Roaming
is bounded to ±4 points and never moves the button or label.

`RanchEnvironment.swift` draws sky, native cloud/star particles, Milky Way,
moonlight, hills, barn, trees, fences, ground grain, shadows and grass. Nothing
loads scene or horse images. The only approved app-target artwork is the four
#463 Treatment-A horse app icons; `check-native-art.py` pins their exact bytes.

## Shipping app icons (#463)

The shipping catalog carries exactly four approved #462 Treatment-A masters:

| Set | Master | Role |
| --- | --- | --- |
| `AppIcon` | `bay-1024.png` | primary/default; nil `alternateIconName` restores it |
| `Palomino` | `palomino-1024.png` | stable alternate |
| `Black` | `black-1024.png` | stable alternate |
| `Grey` | `grey-1024.png` | stable alternate |

The immutable approved masters and their approval metadata live outside
`Assets.xcassets/**.appiconset` in `appicon-masters/treatment-a/` and
`appicon-approval.json` (design commit, canonical `horsesvg.py` SHA-256, exact
four-name allowlist, per-master SHA-256, forbidden legacy Original and
Treatment-B hashes). `app-icons.py` materializes and checks the catalog,
including opaque RGB 1024x1024 structure and the XcodeGen wiring; it never
redraws or re-exports art. The legacy Original and every Treatment B master
are forbidden shipping bytes.

The independently composed planes use the #442 handoff ratios:

| Plane | Ratio |
| --- | ---: |
| Sky/stars | 0.05 |
| Hills | 0.12 |
| Barn/trees | 0.22 |
| Ground/rear fences | 0.40 |
| Foreground grass | 0.65 |
| Horses/labels (native pager) | 1.00 |
| Global blocked rail | 0.00 |

Positive native scroll offsets move world layers left. World-indexed geometry
covers the viewport without loading tiles. Reduce Motion disables ambient clocks,
roaming and environmental parallax; direct, non-animated repository navigation
remains available. It deliberately selects standing/alert static poses, not a
frozen intermediate frame. The clock stops when obscured, dismissed, backgrounded
or entirely disconnected. Last-known blocked agents retain their rail position
but display explicit unknown/disconnected status and do not animate.

Settings → Herd environment offers Day/Night/Auto independently of Catppuccin.
Auto uses only an already-authorized cached CLLocation (valid coordinates,
0–10 km accuracy, no future timestamp, at most 24 hours old). It never requests
permission or starts location updates. Unavailable/denied/invalid/polar samples
use local Day 07:00 inclusive through 19:00 exclusive. The next boundary,
midnight, significant clock changes, time-zone changes and foreground entry
refresh the decision. Solar calculations use Gregorian dates even if the device
uses a Buddhist or other display calendar.

## Non-shipping source material

Reference commit: `28ff3cbf02429b92f22dae2c8a7578ff19abda5e`.
Original procedure: `docs/design/evidence/issue-442/scripts/horsesvg.py`.
Environment guidance: `docs/design/evidence/issue-442/r2-v1-premium/NATIVE-HANDOFF.md`
and that directory's `layers/geometry.json`, Day/Night reference renders.
The R2 horse/rim/grounding/lineup artwork is not consumed.

`horsesvg.py` is the unchanged original procedure. `export.py` preserves the
pre-correction discrimination tooling: it compiles an offline vector-operation
corpus for inspection, not for the application. `probe-native.py` compiles the
old grazing procedure into temporary Swift operations solely for assertion-RED
and actual-size native before-image evidence. None of this directory, the
comparison output, or the #442 reference PNG/SVG/JSON files is in the app target.

## Reproducible checks

From the repository root, with a concrete available simulator UDID:

    python3 ios/tools/herd-art/app-icons.py --check
    python3 ios/tools/herd-art/app-icons.py --write          # deterministic regeneration
    python3 ios/tools/herd-art/test-app-icons.py --output /tmp/app-icon-proofs
    python3 ios/tools/herd-art/check-native-art.py
    python3 ios/tools/herd-art/check-native-art.py --bundle /path/to/FleetNotifier.app
    python3 ios/tools/herd-art/test-native-art.py --app /path/to/FleetNotifier.app --output /tmp/herd-art-proofs
    flock /tmp/corral-heavy-gate.lock python3 ios/tools/herd-art/probe-native.py --udid UDID --output /tmp/herd-native-proofs
    flock /tmp/corral-heavy-gate.lock python3 ios/tools/herd-art/capture.py UDID /path/to/Debug/FleetNotifier.app /tmp/herd-native-evidence

`app-icons.py` is the #463 source-pinned generator/checker: `--write`
materializes the approved masters into the four appiconsets and `--check`
fails closed on catalog, master, approval-metadata or `ios/project.yml`
drift. `test-app-icons.py` mutates disposable copies (missing/misnamed
alternate, legacy Original/Treatment-B swap, duplicate Bay alternate, wrong
hash/dimension/alpha, stale output/project config) and requires each to RED.

`check-release-demo.py` also invokes the native source guard and, with `--binary`,
the actual app-product guard. The manifest digest covers all Herd app sources;
DEBUG fixture types/flags must be absent from Release. The bundle gate checks
Mach-O identity, the real file inventory, bitmap signatures and compiled
Assets.car rendition names. Source checks reject missing/unreadable roots,
non-Swift renderer inputs, symbol aliases and module-qualified/multiline image
loader calls. Guard probes mutate disposable copies only.

Native XCTest classes: `HerdTests` and `HerdEnvironmentTests`. The native probe
requires assertion failures, not compile failures: old grazing, removed real-view
`.onDisappear` cancellation and a shrunken tap zone, followed by byte-verified
restoration and GREEN. Source-wiring fixtures are copied afresh into the test
bundle by XcodeGen's existing test pre-build phase.

The opt-in DEBUG driver uses the real FleetView/HerdView with fictional agents.
It changes only environment state for Day→Night, emits the same scene UUID,
agent IDs, filters, selection and paddock, then navigates, enables Reduce Motion,
shows Auto fallback/dense data and dismisses. A delayed DEBUG observation verifies
no further clock ticks after dismissal. JSON markers record the real scroll
position and the transforms supplied to each native plane. The horse transform
is the native pager's direct offset, including under Reduce Motion.

`capture.py --offline` exercises the explicitly labelled disconnected fixture;
`--first-only` captures just the native old-grazing comparison frame. Capture
checks/reboots only its named simulator because Xcode can leave it shut down.
Screenshots and machine-readable output remain outside the app and source tree.
They demonstrate simulator rendering and runtime state, not physical touch feel,
real authorization changes, device battery consumption or a live fleet audit.
