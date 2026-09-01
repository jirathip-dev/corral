Corral #319 + #320 grouped R2 implementation evidence

Source of truth

- Approved design: design-output commit b968263a622e8cde50dbcad991ad3137f9ec4761, `corral/319-320-status-semantics/revision-2-grouped/`.
- Native implementation uses the existing FleetNotifier and corrald-ui renderers. The presentation classifier is derived from canonical state plus structured identity/activity labels; it does not inspect transcript output.
- Demo data is explicitly synthetic and is only used by the DEBUG capture paths.

Committed native evidence

| Surface | Artifact | Rendered size | Result |
| iOS FleetNotifier | `ios-r2-status-390x844.png` | 1170 x 2532 px, iPhone 14 / iOS 26.5 / 390 x 844 pt | PASS |

The iOS frame was captured from a freshly created simulator with
`-demoMode -corralStatusPresentation`, then the simulator was shut down and
deleted. The frame visibly contains the compact status chips (`All 5`, `Needs
you 1`, `Supervising 1`, `Finished 2`) and the ordered native sections:
Needs you (1), Working (1), Supervising (1), Finished (2). The supervising row
shows `Finished`, `Polling · every 60s`, and its native disclosure chevron.
There is no auth/loading/notification overlay and no lifecycle `Done` or
`Review` label. This is simulator evidence only; it makes no physical-device
claim.

SHA-256

- `ios-r2-status-390x844.png`: `de97157a738fa060c016f5ef08a6434e0b67b8b295faf13ced9941116a800c15`

Egui evidence boundary

The real release `corrald-ui` binary was built successfully and the bundled
synthetic fixture was exercised by the full egui package suite. A committed
egui PNG is intentionally absent: the host's CoreGraphics/Screen Recording
capture path could enumerate the 1320 x 892 native window but refused both
window and region image capture (`could not create image from window` /
`could not create image from rect`), while the app's exact frontmost probe
remained fail-closed. HTML or the approved design PNG is not substituted for
native egui evidence.

Supporting egui proof is the native package test result (`205 passed`) plus the
classifier/detail-panel tests, but those tests are not claimed as a screenshot.
