# Issue #328 + #329 + #330 — Recent-output V3 corrections, simulator evidence

Captured 2026-09-01 on the host's iPhone 16 simulator (iOS 26.5, UDID
59DDC0C5-891E-4EC0-91AF-4F50DF68D793). Native device is 393x852 pt
(1179x2556 px @3x); every PNG below is resized with
`sips -z 844 390` to the 390x844 evidence standard (0.18% aspect
distortion, stated per lane convention). Debug build from the lane's
`xcodebuild build` (derivedDataPath /tmp/fn-dd-328), installed via
`simctl install`, launched with the DEBUG-only demo routes.

All content is the privacy-safe demo fixture (fictional repos, generic
prose, no private output, paths, identities, or transcripts).

## Boundaries exercised

### a-collapsed-conversation-harness-390x844.png — #330 conversation + #328 count (#329 collapsed default)

Launch: `-corralDemoDetail` (featured demo session).

- Conversation is NON-EMPTY: assistant message, user message ("Please
  verify the diff too."), assistant code block, expanded tool diff —
  a supported live session's exchange renders in Conversation.
- "Harness activity · 3 outside conversation": the demo harness holds 4
  System/Unknown blocks including a divider-only system block; the
  divider is EXCLUDED from the count (3 events) — #328 AC1/AC2.
- Harness is COLLAPSED by default — #329 AC5.
- Composer ("Send") pinned at the bottom of the Recent-output panel.
- OCR audit (local Vision, en-US): conversation rows present, count
  label "Harness activity · 3 outside conversation", no dash-only
  cards anywhere.

### b-expanded-harness-bounded-390x844.png — #329 bounded expanded payload + #328 divider rendering

Launch: `-corralDemoDetail -corralDemoHarnessExpanded` (DEBUG-only
pre-expansion; simctl cannot inject the disclosure tap).

- Expanded harness renders 3 real cards (Diagnostic + two Unknown
  activity), each multi-line; the long payloads are contained by the
  harness region's own bounded vertical scroll — the payload ends
  above the pinned composer and inside the panel (no overflow beneath
  chrome or over the composer) — #329 AC1.
- The collapse control ("Harness activity · 3 outside conversation")
  stays visible and reachable at the top of the expanded region —
  #329 AC2 (scroll-to-last-line + collapse reachable; the swipe
  gesture itself cannot be injected on simctl and remains a
  human/device gate).
- The divider-only system block renders as a thin rule between the
  diagnostic and unknown cards — NO dash-only Diagnostic card —
  #328 AC1/AC3.
- OCR audit confirms the three cards + count label; no divider text
  card.

### c-empty-conversation-honest-390x844.png — #330 AC5 honest empty state

Launch: `-corralDemoDetail -corralDemoHarnessOnly` (DEBUG-only route to
the harness-only demo session).

- Session "demo-session: harness-only" routed ("Unattributed window",
  "terminal chrome only in this window").
- Conversation region shows the explicit honest empty state:
  "No attributed conversation in this window" — never an unexplained
  blank region.
- "Harness activity · 3 outside conversation" remains reachable below
  (4 System/Unknown blocks, divider-only chrome block excluded from
  the count).
- Composer pinned at the bottom.

## Interaction-evidence classification (honest)

simctl provides no touch injection (documented lane constraint), so the
expand TAP, the harness inner-scroll SWIPE, and the collapse TAP are
NOT exercised on the simulator. The observable boundaries are captured:
collapsed default (A), pre-expanded bounded payload with reachable
collapse control and pinned composer (B), and the honest empty state
(C). The tap/scroll/collapse interaction loop remains a human/device
gate; the layout contracts are pinned by the focused iOS suites
(ContextSplitV3Tests + RecentOutputDefectWiringTests) and the mutation
proofs.

## Capture commands

```
xcrun simctl launch <udid> com.corral.fleetnotifier -corralDemoDetail
sleep 12; xcrun simctl io <udid> screenshot a-...-raw.png
xcrun simctl terminate <udid> com.corral.fleetnotifier
... (same for the -corralDemoHarnessExpanded and -corralDemoHarnessOnly variants)
sips -z 844 390 <raw> --out <name>-390x844.png
```

SHA-256 (resized PNGs):
- 59688961edb16667e8192dcb147ab70eb2f3328cd0228a42443bcb40c1cc13b4  a-collapsed-conversation-harness-390x844.png
- be61a4dc1f937938f39d33dcc03c113cdcd90e85b1d8efb86d5f0efd02a30946  b-expanded-harness-bounded-390x844.png
- a66fc5afe42cd6e5551ca12a11e03d58e9b0001fe27bef86a6d113c645a36586  c-empty-conversation-honest-390x844.png
