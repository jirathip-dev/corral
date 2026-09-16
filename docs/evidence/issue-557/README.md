# #557 — minimal Herd git caption

Implements the owner scope under amendment 1, continuing above the preserved
blocker receipt `ca7400dc4d688da705c39f0d9c9b8b925f999c48`.

## Limitation: no freshness guarantee

The current iOS Snapshot decoder at `Models.swift:192` discards
`git_worktree_facts`. This caption renders whatever `workspace.dirty` and
`workspace.behind` the current model delivers, including potentially stale
values. Freshness-aware suppression is deferred to **#564**. Host connectivity
is not used as a substitute. No model, store, wire, or Board-card change.

## Simulator renders, not physical-device evidence

The existing `HerdRailZoneTests` harness hosts the production `HerdView` in a
390×844-point `UIWindow` attached to the simulator's actual `UIWindowScene`.
`drawHierarchy` captures at scale 1, producing 390×844 PNGs without resizing or
cropping. The disposable `hermes-sim-task` device was an iPhone 17 Pro simulator
on the installed iOS 26.5 runtime; the window is the harness's explicit size,
not a claim about that device's native screen dimensions. `images.json` records
the actual simulator identity and SHA-256 of every image.

| State | Positive-fact image | No-git control |
| --- | --- | --- |
| Day / default type | `day-blocked-large-git.png` | `day-blocked-large.png` |
| Night / default type | `night-blocked-large-git.png` | `night-blocked-large.png` |
| Day / AX3 | `day-blocked-accessibility3-git.png` | `day-blocked-accessibility3.png` |
| Night / AX3 | `night-blocked-accessibility3-git.png` | `night-blocked-accessibility3.png` |

`day-blocked-accessibility3-git-scrolled.png` is the actual vertical scroll
(offset 200 pt) exposing the paddock, not a crop. All fixtures are fictional.
The rail horse has dirty=true and behind=3; field horses have no git facts.

Pixel inspection: the peach dot and `↓3` remain on the name line in Day/Night
and AX3. They do not wrap or escape the caption. AX3 intentionally ellipsizes
the horse name under the existing single-line rail policy (`rail-ho…`); the
full name remains in the accessibility label. The status is on its existing
second line. The AX3 repo chip wraps as before; that is not the horse caption.
No permission alert or other modal obscures these captures. The fixture's
connection indicator reads Offline because the standalone view has no live
transport; it does not control git-marker rendering.

## Geometry and regression pins

The new test compares **all frame-probe names and CGRect values** between
absent and positive git facts in Day/Night × default/AX3. All four pairs were
identical, including the rail zone, horse cards, paddock origin and HUD.
Coordinates below are in the existing `herdRailLayout` coordinate space:

| Type | Rail card (x, y, width, height) | Rail zone height | First paddock row y |
| --- | --- | --- | --- |
| Default | (12, 123.3333, 164, 156.6667) | 175.6667 | 307 |
| AX3 | (12, 277.6667, 240, 213.3333) | 259.6667 | 545.3333 |

Explicitly exercised unchanged pins:
- #548: `HerdRailZoneTests.testReservedZoneKeepsFirstHorseRowFixed` — empty,
  blocked and last-known states share the first-row origin and reserved height.
- #552: `testRepositoryChipSharesHUDRhythmAndKeepsIntrinsicChrome` — both HUD
  gaps are 6 pt in Day/Night at default, AX3 and AX5.
- #552: `testRepositoryChipUsesPrimaryNameAndSecondaryCountFonts` — the existing
  font-role and intrinsic-chrome source assertions remain unchanged.

The focused invocation executed 32 tests with zero failures. `gates.jsonl`
contains exact expanded commands, raw exit statuses and timing. Full raw logs
are retained under `/tmp/g557-*.log`; compressed copies accompany closeout.

## VoiceOver and regression discrimination

The production horse button's accessibility label uses words, for example:
`rail-horse, blocked, Other, dirty worktree, 3 commits behind`.
One commit uses `1 commit behind`. Absent/default facts and ahead-only facts
append nothing. The word construction and actual button/marker call sites are
pinned by `HerdTests.testHorseGitAccessibilityWordsAndAbsence` and
`testHorseCaptionLabelWiresPositiveGitMarkers`. These are XCTest value and
bundled-production-source tests, not a spoken VoiceOver session or XCUI tree
inspection. The rendered screenshots independently show the visible markers.

## Reproduce

From the repository root, use the host's shared lock for every heavy stage:

```sh
flock /tmp/n.lock python3 docs/evidence/issue-557/verify.py static
flock /tmp/n.lock hermes-sim-task --name g557-caption --shell 'python3 docs/evidence/issue-557/verify.py focused "$SIMULATOR_UDID"'
HERDR_XCODEBUILD_DIRECT=1 flock /tmp/n.lock python3 docs/evidence/issue-557/verify.py builds
flock /tmp/n.lock python3 docs/evidence/issue-557/verify.py slop
flock /tmp/n.lock hermes-sim-task --name g557-full --shell 'python3 docs/evidence/issue-557/verify.py full "$SIMULATOR_UDID"'
# Commit the production file first; the restore check compares against Git.
flock /tmp/n.lock hermes-sim-task --name g557-mutation --shell 'python3 docs/evidence/issue-557/verify.py mutation "$SIMULATOR_UDID"'
```

Each command inside the driver has a bounded deadline and a preceding
`df -h /`. DerivedData and the linter scratch build stay under `/tmp/g557-dd`.
The wrapper owns and deletes its disposable simulator. No installed app,
live daemon, physical device, CI workflow, PR, merge or release was exercised.
