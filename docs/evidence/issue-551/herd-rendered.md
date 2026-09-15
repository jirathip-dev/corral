# Issue 551 round 2 — the Herd surface, before and after the recast fix

Two REAL rendered frames of the production Herd surface (1179x2556 px, iPhone 16
@3x), captured through the app's own DEBUG recorded-evidence driver — not
mockups, not recomposed images. They are the answer to the round-2 blocker
("the retained-board evidence and its pin sit on the wrong surface"): the
surface the owner photographed, in the disconnected posture this issue is about.

## Provenance

| Frame | Committed file | Source head | sha256 |
| --- | --- | --- | --- |
| BEFORE (round-1 head, recast present) | `herd-retained-before-r1.png` | `d7c4e77` (round-1 gated head) | `fb13f2d856ef8b3bee23a88f5e0ede3bc0df174a20e4cedf1114f8a99993eae1` |
| AFTER (round-2 head, recast fixed) | `herd-retained-after-r2.png` | `627573a` (round-2 fix commit) | `1f4c8a532f08a292b60613fae7d31e1f2ff7ef93f7c3301ba036fc9c26aedd95` |

Identical method for both, from a clean tree at each head:

```
# in a worktree at <head> (the BEFORE frame used a detached worktree at d7c4e77)
xcodebuild build -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier \
  -configuration Debug -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath /tmp/g551r2-herd-<label>-dd
xcrun simctl install 59DDC0C5-891E-4EC0-91AF-4F50DF68D793 \
  /tmp/g551r2-herd-<label>-dd/Build/Products/Debug-iphonesimulator/FleetNotifier.app
xcrun simctl terminate 59DDC0C5-… com.corral.fleetnotifier      # + clear Documents/ux-evidence
xcrun simctl launch 59DDC0C5-… com.corral.fleetnotifier \
  -corralHerdEvidence -corral456FullScreenEvidence -corralHerdOffline
# poll <container>/Documents/ux-evidence/456-offline-fullscreen.marker, then:
xcrun simctl io 59DDC0C5-… screenshot herd-retained-<label>.png
```

`-corralHerdEvidence` enters the debug demo + fictional seed and selects the Herd
presentation (`ios/FleetNotifier/Demo/HerdEvidence.swift:9-14`);
`-corral456FullScreenEvidence` runs the recorded full-screen driver and writes
the marker (`ios/FleetNotifier/UI/Herd/HerdView.swift:639-648`);
`-corralHerdOffline` forces the disconnected posture
(`ios/FleetNotifier/UI/FleetViews.swift:1326-1332`). The screenshot is taken 2 s
AFTER the marker file appears, inside that phase's 9 s hold — the documented
marker-then-hold capture discipline. Driver script:
`/tmp/g551r2-herd-capture.sh` (run under `flock /tmp/n.lock`), log
`/tmp/g551r2-herd-both.out`.

## What the frames show (read directly from the pixels)

**BEFORE (`herd-retained-before-r1.png`, round-1 head):** the counts strip reads
`! 0   ○ 0   ◦ 0   ✓ 0   ? 12` — every row of the fictional 12-lane fleet
counted as `unknown` — and each caption reads
`? unknown · last known blocked` / `? unknown · last known idle` /
`? unknown · last known done`. The compact indicator reads `Offline`. This is the
owner's `? N unknown` strip, on the owner's surface, produced by
`HerdHorse.state`'s recast.

**AFTER (`herd-retained-after-r2.png`, round-2 head):** the SAME launch, seed and
posture — the strip now reads `! 2   ○ 4   ◦ 3   ✓ 2   ? 1`, i.e. the fleet's
last-known distribution (2 blocked, 4 working, 3 idle, 2 done, 1 genuinely
unknown), the captions read `! blocked · last known`, `◦ idle · last known`,
`✓ done · last known`, the two retained blocked horses still stand at the front
rail (`2 at rail`), and the indicators still read `Offline`. Nothing is labelled
Live: the horses render their last-known poses statically, desaturated, with the
`last known` caption, while the connection indicator carries the honest
reconnecting/offline truth.

The two frames differ in every state-bearing element and share the same scene,
seed, lighting and posture — the difference is the fix.

## Limits

These are DEBUG demo frames through the app's fictional-seed evidence driver on
a simulator: they exercise the production `HerdView`/`HerdModel` presentation
path and the production disconnected posture, but they are not live-device
evidence and not a timing measurement. The physical-iPhone numbers and any
production-host claim remain the owner's gate
(`docs/evidence/issue-551/device-protocol.md`).
