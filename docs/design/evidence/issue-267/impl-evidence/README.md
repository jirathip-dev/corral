# #267 issue browser — impl evidence (shipped surface)

Shipped-surface screenshots for the approved V3 read-only issue browser
(PR/issue screenshot-to-issue evidence). Design-gate prototypes live in the
parent dir (`variant-3-inline-list-390x844.png` etc.).

## Capture environment

- Simulator: iPhone 16, iOS 26.5 (`com.apple.CoreSimulator.SimRuntime.iOS-26-5`),
  UDID 59DDC0C5-891E-4EC0-91AF-4F50DF68D793 (393x852 pt = 1179x2556 @3x).
- App: Debug build of `ios/FleetNotifier` at this commit (scheme
  `FleetNotifier`), installed via `simctl install`, launched with the
  DEBUG-only demo launch args below (opt-in local routes, never in Release).
- Raws captured with `xcrun simctl io <udid> screenshot`, then resized to the
  390x844 evidence standard with `sips -z 844 390` (0.18% aspect distortion).

## Shots

| File | Launch args | Content |
| --- | --- | --- |
| `fleet-issues-button-390x844.png` | `-demoMode` | Fleet screen toolbar: teal "Issues" button (icon + label) next to the slider menu — the approved entry point |
| `issues-list-390x844.png` | `-corralDemoIssues` | Issues screen: open/closed chips in pinned chrome (open default), read-only subline, flat newest-first rows (#N + title + OPEN pill) |
| `issues-detail-390x844.png` | `-corralDemoIssues -corralDemoIssuesDetail 267` | #267 expanded inline: OPEN + label pills, `corral · #267` meta, body, `──── 18 earlier comments · Load earlier ────` divider, newest-first comments, `▴ collapse` |

Demo data is the seeded `DemoFleet.seedIssues()`: real repo data
(jirathip-dev/corral, -sendmeter, -plush-meadow at capture time); comment
text is illustrative demo copy — the LIVE surface renders verbatim daemon
data (gh poller window: body + newest-first 30 comments + total count).

## Command log

```
xcrun simctl boot 59DDC0C5-891E-4EC0-91AF-4F50DF68D793
HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild build \
  -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier \
  -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/fn-dd-g267 build
xcrun simctl install <udid> /tmp/fn-dd-g267/Build/Products/Debug-iphonesimulator/FleetNotifier.app
xcrun simctl launch <udid> com.corral.fleetnotifier -demoMode      # entry point
sleep 6 && xcrun simctl io <udid> screenshot fleet-issues-button-raw.png
xcrun simctl launch <udid> com.corral.fleetnotifier -corralDemoIssues
sleep 6 && xcrun simctl io <udid> screenshot issues-list-raw.png
xcrun simctl launch <udid> com.corral.fleetnotifier -corralDemoIssues -corralDemoIssuesDetail 267
sleep 6 && xcrun simctl io <udid> screenshot issues-detail-raw.png
sips -z 844 390 fleet-issues-button-raw.png --out fleet-issues-button-390x844.png
sips -z 844 390 issues-list-raw.png --out issues-list-390x844.png
sips -z 844 390 issues-detail-raw.png --out issues-detail-390x844.png
```

## SHA-256

```
224cf4f597a97b685a5ea8767fa3d10f8c171bfa225727c24c2ab17574126e01  fleet-issues-button-390x844.png
03b6245e8aa664df02ca9dab02b9d0897b04e328ff21b03e7c1fd77b1b82cde2  issues-list-390x844.png
b52e6f0e646735764045f1aa982d229bf561c23cee8bb7a8ef80287d4a1899ae  issues-detail-390x844.png
```
