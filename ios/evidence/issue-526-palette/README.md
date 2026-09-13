# 526 evidence — per-mode palette ownership (Board preset vs Herd environment)

Runtime frames for the #526 palette-ownership change (synthetic DEBUG demo
captures over the #401/#528 three-profile seed — Host A live, Host B
offline with retained stale rows, Host C connecting; fictional
`demo-host-*` hosts; no live daemon, no physical device, no TestFlight
claim). Captured on the fresh `Corral526` simulator (iPhone 16, iOS 26.5,
UDID `379E5468-9787-4D87-A9A0-75BEE15AB80C`) with the deterministic
`-corral526HerdDayEvidence` / `-corral526HerdNightEvidence` /
`-corral526BoardEvidence` / `-corral526AutoEvidence` drivers (marker files
in `Documents/ux-evidence`, phase markers written AFTER each state settles,
>= 9 s hold per phase, one launch per driver, `capture.sh` shoots 2 s after
a new marker appears).

The committed frames are 1179x2556 px @3x iPhone-16 captures downscaled
with `sips -z 844 390` (the repo's standard 390x844 device class; 0.18 %
aspect distortion from the native 393x852 pt — same convention as the
#385/#401/#416/#427/#456/#528 sets).

## What the frames prove (#526 ACs)

- **Board selected** → the four Catppuccin preset rows are visible; the
  Herd environment control is absent.
- **Herd selected** → the Board presets are absent; the Ranch-light
  Auto/Day/Night control is visible; App Icon and the shared
  host/notification controls stay available in BOTH modes.
- **Independent preferences** → the Herd frames are captured with the
  OPPOSITE saved Board preset (Herd Day with a dark Mocha Board preset;
  Herd Night with a light Latte Board preset): the Herd palette follows
  the environment and the saved Board preset appears nowhere on those
  frames.
- **Coordinated Herd chrome** → agent label cards, the front-rail header,
  the repository (paddock) header and the pagination pill/text/disabled
  inks all render the ranch Day/Night chrome; Settings, Recent Output and
  the Herd-context filter sheet resolve from the SAME effective Day/Night
  state.
- **Live re-resolution** → the environment flips night WHILE the Settings
  sheet is open (no stale palette on the open sheet), and the mode switch
  inside Settings updates the sheet both ways.
- **Auto transition** → with Auto selected, the lighting instant is moved
  from local noon to local night; the ranch AND the chrome flip together
  from the same resolved state.
- **Disconnected surfacing as #528 leaves it** → the compact connection
  indicator + retained/stale rows; no routine banner, no Open Board/Retry.

## Frames (390x844)

| File | Proves |
|---|---|
| `526-1-herd-day-opposite-board-mocha-390x844.png` | Herd Day chrome (cards, headers, pager) with the saved Board preset set to Mocha (dark); partial connection indicator |
| `526-2-settings-herd-day-390x844.png` | Settings in Herd mode, Day: Ranch light visible, Board presets absent, App Icon present |
| `526-3-settings-herd-night-open-sheet-390x844.png` | Environment switched to Night with the sheet OPEN — the sheet repaints (no stale palette) |
| `526-4-settings-board-selected-390x844.png` | Mode switched to Board inside Settings: presets return, environment control hidden |
| `526-5-settings-herd-selected-390x844.png` | Switched back to Herd inside Settings: Herd Day/Night chrome, Board presets hidden. **Byte-identical to 526-3 by construction** — the round trip restores exactly the same persisted state (Herd + Night), which is the point: switching modes never leaks the other mode's palette into the sheet |
| `526-6-herd-day-after-dismissal-390x844.png` | Dismissed back to the Herd Day surface |
| `526-7-recents-herd-day-390x844.png` | Recent Output opened from Herd (Day palette, shells + output surface) |
| `526-8-herd-night-opposite-board-latte-390x844.png` | Herd Night chrome with the saved Board preset set to Latte (light) |
| `526-9-settings-herd-night-390x844.png` | Settings in Herd Night |
| `526-10-recents-herd-night-390x844.png` | Recent Output opened from Herd in Night |
| `526-11-herd-disconnected-night-390x844.png` | Every demo host offline: compact indicator + retained/stale rows (no routine banner, no Open Board/Retry) |
| `526-12-board-mocha-controls-390x844.png` | Board with its Mocha preset (dark controls) |
| `526-13-board-latte-controls-390x844.png` | Board with its Latte preset (light controls) |
| `526-14-board-latte-filter-sheet-390x844.png` | Board-launched Filters sheet keeps the board Catppuccin treatment with a selected repo scope |
| `526-15-board-latte-settings-presets-390x844.png` | Board-launched Settings: presets visible, Herd environment hidden |
| `526-16-pagination-first-page-previous-disabled-390x844.png` | Pager page 1: Previous disabled (muted ranch ink), Next enabled |
| `526-17-pagination-last-page-next-disabled-390x844.png` | Pager last page: Next disabled, Previous enabled |
| `526-18-auto-day-390x844.png` | Auto resolving Day (lighting instant at local noon) |
| `526-19-auto-night-transition-390x844.png` | Auto transitioned to Night: ranch AND chrome flip together |

## Reproduce

    xcrun simctl create Corral526 "iPhone 16" com.apple.CoreSimulator.SimRuntime.iOS-26-5
    xcrun simctl boot <udid>
    HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 \
      xcodebuild build -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier \
      -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/i526-dd
    xcrun simctl install <udid> /tmp/i526-dd/Build/Products/Debug-iphonesimulator/FleetNotifier.app
    bash ios/evidence/issue-526-palette/capture.sh <udid> herdday 526-7-recents-herd-day -corral526HerdDayEvidence
    bash ios/evidence/issue-526-palette/capture.sh <udid> herdnight 526-11-herd-disconnected-night -corral526HerdNightEvidence
    bash ios/evidence/issue-526-palette/capture.sh <udid> board 526-15-board-latte-settings-presets -corral526BoardEvidence
    bash ios/evidence/issue-526-palette/capture.sh <udid> auto 526-19-auto-night-transition -corral526AutoEvidence
    sips -z 844 390 /tmp/g526-raw-<run>/<name>.png --out <name>-390x844.png
