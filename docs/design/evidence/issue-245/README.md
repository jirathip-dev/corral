# #245 Fleet screen — compact spacing evidence

Approved design: **Variant 1 — compact** (see issue #245, DESIGN APPROVED
comment and `~/design-output/corral/245-fleet-screen-spacing/variant-1-compact.html`).

Screenshots are the real iOS FleetNotifier app in Debug demo mode
(`-demoMode` launch argument) on the iPhone 16 simulator (iOS 26.5),
captured with `xcrun simctl io <udid> screenshot` and scaled from 1179x2556
(native 393x852 pt) to the 390x844 pt evidence standard.

## Before / after (390x844)

- `before-390x844.png` — current pre-#245 screen: title-case section headers
  ("Needs you (4)"), a manual refresh button (⟳) next to the chips and a
  "Refresh fleet" toolbar menu item, and a ~98 pt blank band between the
  filter chrome and the first section header.
- `after-390x844.png` — approved Variant 1: lowercase section headers
  ("needs you (4)", "idle / done (N)", "results"), manual refresh removed
  (chrome ⟳ + toolbar menu item; pull-to-refresh + SSE remain), the
  "pull to refresh · updates stream in automatically" hint line added under
  the chips, and tightened spacing (chrome→first-section header band
  98 pt → 39 pt; header→row padding halved; rows tightened).
