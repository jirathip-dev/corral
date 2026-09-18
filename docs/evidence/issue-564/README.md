# Issue 564 evidence — iOS decodes and preserves per-worktree git freshness

This directory holds the lane's rendered and raw receipts for
`g564-freshness` (implementation and gate report: `../../../.report.md`).
Everything here is simulator/raster evidence from the production `WorkspaceLine`
view plus raw subprocess logs; it is not device or deployment evidence.

## What was delivered

1. `ios/FleetNotifier/Models/Models.swift` decodes and preserves the daemon's
   `git_worktree_facts` map (`<canonical worktree path> -> {fact_age_ms, stale}`,
   #492's additive wire fields) on **Snapshot** and **Delta**, and projects each
   row's verdict onto `Workspace.gitFactFreshness` as exactly three states:
   `fresh` / `stale` / `noFact`. A retained `WorktreeFactTable` expresses the
   per-host row retention a delta implies: a full frame replaces, a delta
   upserts only the rows it carries (omission is never a freshness claim), and
   an absent row is `noFact`.
2. `ios/FleetNotifier/UI/FleetViews.swift` gates the Board card's positive git
   signals (`dirty`, `↑ahead↓behind`) on `fresh`; a stale or absent fact renders
   neither, with no placeholder and no invented stale/clean affordance. The
   row's height and leading identity band are unchanged (measured below).

## Reproduce

From the repository root. Heavy legs are serialized under `flock /tmp/g564.lock`;
simulator lifecycle is owned by `hermes-sim-task` (one private temporary
simulator per invocation, deleted by the wrapper). The scripts used by the lane
are quoted verbatim in `commands.md`.

```sh
swiftc -parse ios/FleetNotifier/Models/Models.swift ios/FleetNotifier/UI/FleetViews.swift \
  ios/FleetNotifierTests/WorktreeFreshnessTests.swift ios/FleetNotifierTests/WorktreeFreshnessConsumerTests.swift
python3 ios/check-release-demo.py            # source mode
python3 ios/check-release-demo.py --self-test
xcodegen generate --spec ios/project.yml && git diff --exit-code -- ios/FleetNotifier.xcodeproj/project.pbxproj
# focused consumer class at the PRE-FIX head (base worktree + this one test file):
#   hermes-sim-task -c "bash <wrapper> /tmp/g564-base /tmp/g564-basedd \
#     -only-testing:FleetNotifierTests/WorktreeFreshnessConsumerTests"
# focused classes at the head, then the full suite:
#   hermes-sim-task -c "bash <wrapper> <worktree> /tmp/g564-dd -only-testing:FleetNotifierTests"
swiftc -o /tmp/g564/wireprobe/probe ios/FleetNotifier/Models/Models.swift /tmp/g564/wireprobe/main.swift
```

## Receipts in this directory

| File | What it is | Observed |
| --- | --- | --- |
| `base-focused.log.gz` | Pre-fix head `ff193c71`, only the base-compilable consumer test file | `Executed 6 tests, with 8 failures (0 unexpected)`, raw exit 65 |
| `head-focused.log.gz` | Head, both new classes (`WorktreeFreshnessTests`, `WorktreeFreshnessConsumerTests`) | `Executed 15 tests, with 0 failures`, raw exit 0 |
| `head-full.log.gz` | Head, full unit suite (`-only-testing:FleetNotifierTests`) | `Executed 680 tests, with 1 test skipped and 0 failures (0 unexpected)`, raw exit 0 |
| `head-focused-run1-ocr-assertion-fix.log.gz` | First head run: 2 failures from this file's own OCR assertion (`g564-lane` reads back as `g564-1ane`) | Fixed in-lane by asserting the OCR-stable basename `lane-a`; no production or expectation was weakened. Raw exit 65 |
| `release-source.log`, `commands.md` | Release-source check + the exact invocation ledger | see `commands.md` |
| `wire-probe-real-daemon-fixtures.log` | Production `Snapshot` decoder run over FOUR real daemon-emitted snapshots committed by #556 (`docs/evidence/issue-556/rust/fixture-{fresh,aged,stopped,absent}.json`) | per-row `fresh` / `stale` / `noFact` verdicts match the daemon's own `stale` flags and null ages |
| `564-*.png` | Renders of the production Board card (`WorkspaceLine`) through `ImageRenderer` at scale 3, 390 pt wide | see the hash table below |
| `anti-slop-compare.py` | Advisory finding-set comparison (relative path + rule + message) | see `anti-slop-delta.json` when present |

## Render hash table (harvested from the test host app's Documents)

| Image | Input | SHA-256 |
| --- | --- | --- |
| `564-fresh-fact.png` | frame with `{"fact_age_ms":7,"stale":false}` | `d3b3fec2…89941cb0` |
| `564-fresh-reference.png` | same row, no positive fact | `cebb14cf…536da320b` |
| `564-geometry-fresh.png` | fresh frame (geometry leg) | `d3b3fec2…89941cb0` |
| `564-stale-fact.png` | frame with `{"fact_age_ms":120000,"stale":true}` | `cebb14cf…536da320b` |
| `564-nofact-no-map.png` | frame without any facts map | `cebb14cf…536da320b` |
| `564-nofact-no-row.png` | frame whose map lacks this path | `cebb14cf…536da320b` |
| `564-absent-reference.png`, `564-nofact-reference.png`, `564-geometry-stale.png`, `564-geometry-absent.png` | the no-positive-fact reference variants | `cebb14cf…536da320b` |

Full digests: `commands.md`. Interpretation: every stale / no-fact render is
**byte-identical** to the no-positive-fact reference, and only the fresh frame
paints the signal — the card cannot present an unbacked fact as current, and no
new affordance (badge, wording, spacer) was invented. The two images were also
inspected directly: the stale row reads `corral · g564-lane · lane-a` with an
empty trailing band; the fresh row adds `dirty` (peach) and `↑2↓3`.

Measured geometry (`G564_GEOMETRY` in `head-full.log.gz`): the captured row is
36.333333333333336 pt high in **all three** states at 390 pt width; the leading
identity band (first 90 pt) is byte-identical between the fresh, stale and
absent renders, so suppression removes the trailing band only.

## OCR note

`VNRecognizeTextRequest` reads the monospaced branch name `g564-lane` as
`g564-1ane`; the tests therefore assert the OCR-stable basename `lane-a` when
proving "the scan actually read the row" (and assert the absence of `dirty` on
the same scan). The stale scans read `corral • g564-1ane • lane-a` (no `dirty`);
the fresh scan reads `corral • g564-1ane • lane-a dirty 12V3` (`↑2↓3` reads back
as `12V3`).

## Non-claims

No physical-device evidence, no hosted-CI evidence, no live-daemon/SSE
observation of the freshness gate on a real fleet, no release/TestFlight, no
PR/merge/issue mutation. The images are fixture-backed simulator renders of the
production view; the wire probe is the production decoder over committed
daemon-emitted snapshots.
