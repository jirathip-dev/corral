# Corral #388 — Settings Connection inputs: theme-styled fields + paired state

Real compiled capture of the #388 change on the iOS Settings sheet
(`ios/FleetNotifier/UI/FleetViews.swift` + `App/AppModel.swift` +
`App/FleetNotifierApp.swift`):

- The Connection Host + Registration-token inputs no longer render as the
  default SQUARE near-black boxes: both render through the shared
  `ConnectionField` surface — `theme.surface1` fill (never near-black),
  `theme.text` ink, `theme.subtext0` placeholder, 10 pt continuous corner
  radius, and a 1 pt hairline `surface2` border that tints to the accent
  while focused. Every color is a Catppuccin token, so the fields follow
  the active flavor on all four palettes — Latte light included.
- Once the device is REGISTERED (`AppModel.isRegistered`, i.e. it holds the
  daemon-issued identity key id), the Connection section hides the
  Registration-token field entirely: the Host field stays (still editable,
  still themed — a paired device can re-point), a status row reads
  `Device registered · Key ID dev_xxx · read-only signed`, and a small
  **Re-register** action reveals the token field again. Remove device in
  the Device section clears the identity and the unpaired form returns
  naturally.

The artifact is SYNTHETIC ONLY: the DEBUG `-corralDemoConnectionInputsEvidence`
driver over `DemoFleet.seed()` (fictional repos/agents, no live daemon, no
physical device; the paired phases seed the same in-memory `keyId` a
successful register() would store — no keychain, no networking; mode stays
`.demo`). No live-fleet, physical-device, or TestFlight claim.

## Artifact

One deterministic launch records the sequence. Each phase writes a marker
file the host capture script polls, then the script screenshots (markers
are written 2.5 s AFTER the phase's state settles and each phase HOLDS 9 s
so the capture always lands inside the phase it names — a 4 s window raced
the cold-sim screenshot latency and frames drifted into the next phase's
flavor):

| Phase | File | Shows |
|---|---|---|
| 1 | `phase-1-settings-macchiato-unpaired-390x844.png` | MACCHIATO, UNPAIRED: host + Registration-token fields on the themed surface1 surface + Register action |
| 2 | `phase-2-settings-mocha-unpaired-390x844.png` | MOCHA, UNPAIRED: same themed fields on the darkest palette |
| 3 | `phase-3-settings-latte-unpaired-390x844.png` | LATTE light, UNPAIRED: themed fields on the light palette (surface1 `#bcc0cc`) |
| 4 | `phase-4-settings-latte-paired-390x844.png` | LATTE light, PAIRED: host field + `Device registered · Key ID dev_3f88a1b2c3d4 · read-only signed` status row + Re-register — NO token field |
| 5 | `phase-5-settings-mocha-paired-390x844.png` | MOCHA, PAIRED: status row + Re-register, no token field |
| 6 | `phase-6-settings-macchiato-paired-390x844.png` | MACCHIATO, PAIRED: status row + Re-register, no token field |
| 7 | `phase-7-done-390x844.png` | Rest state after the sequence (macchiato, sheet closed) |

All rendered 390x844 pt (raw 1179x2556 px iPhone 16 / iOS 26.5 @3x,
`sips -z 844 390`; 0.18 % aspect distortion from the 393x852 pt native —
same standard as issue-316/362/385/386). Captured on a FRESH simulator
(`Corral388`, iPhone 16, iOS 26.5 — no keychain identity, so no
notification-permission alert race and no leftover pairing).

## Audit (Vision OCR + pixel geometry)

Local Vision OCR / vision inspection of every 390x844 frame:

- UNPAIRED frames (1–3): the Connection section lists the Host field
  (`127.0.0.1:8474`), a `Registration token` placeholder field, and the
  `Register device (read-only)` row — both fields rounded with a visible
  surface fill and hairline border (Vision: "rounded dark-gray boxes, not
  sharp black squares" on Mocha/Macchiato; "rounded light-gray fill" on
  Latte).
- PAIRED frames (4–6): the status row reads `Device registered · Key ID
  dev_3f88a1b2c3d4 · read-only signed` (the driver's demo key id, shown in
  the same 16-char compact spelling as the Device-section read-out), the
  Re-register action is present, and NO Registration token field / NO
  Register action renders on any palette.
- Flavor truth per frame is pinned by the Appearance checkmark row
  (Latte/Macchiato/Mocha as labeled) plus whole-frame luminance: Latte
  phases ~234-237 vs dark phases ~48-51.
- Pixel scan of the Connection band (raw frames, y 1100–1400): ZERO
  near-black pixels (all channels < 24) in every dark frame — the fields
  are surface1-toned, not the pre-#388 near-black rounded-border chrome.

The paired frames' key-id + status copy and the driver's six markers are
additionally pinned by the unit suite (`ConnectionSectionWiringTests`,
`ConnectionRegistrationModelTests` in `ios/FleetNotifierTests/`).

## Capture commands (capture.log)

Fresh simulator `Corral388` (iPhone 16, iOS 26.5), DEBUG build from this
branch, `-corralDemoConnectionInputsEvidence` launch, marker polling,
`simctl io screenshot` per phase, `sips` resize to 390x844. Full timings in
`capture.log`.

SHA-256s: `SHA256SUMS.txt` (all artifacts above).

Evidence conventions follow docs/design/evidence/issue-362, issue-385 and
issue-386.
