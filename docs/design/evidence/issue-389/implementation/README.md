# Corral #389 — iOS Push Notifications: aps-environment entitlement + denied-permission guidance

Real compiled evidence of the #389 change on the iOS Settings sheet
(`ios/FleetNotifier/UI/FleetViews.swift`, `App/AppModel.swift`,
`Notifications/NotificationPermission.swift`, `Notifications/AppDelegate.swift`,
`ios/project.yml` + regenerated `FleetNotifier.xcodeproj`):

- The FleetNotifier target now carries the **aps-environment** entitlement —
  `ios/FleetNotifier/FleetNotifier.entitlements` (development) for Debug and
  `ios/FleetNotifier/FleetNotifier-Release.entitlements` (production) for
  Release, wired through per-config `CODE_SIGN_ENTITLEMENTS` in `project.yml`.
  Xcode AUTOMATIC signing (team `9244PWFYD7`) auto-provisions; NO Apple-portal
  action was taken (routed note). See `capture.log` for the signed-artifact
  probes: the Debug simulator product's embedded `__TEXT,__entitlements`
  section carries `aps-environment = development` (Release: `production`);
  re-signing the product with the Xcode-generated `.xcent` shows the same
  entitlement through `codesign -d --entitlements -`.
- When the OS notification permission is **denied/restricted**, the Settings →
  Notifications section no longer silently fails: the toggle displays OFF and
  the section shows WHY (`Corral can't alert you — notifications are off for
  this app in iOS Settings.`) plus an **Open iOS Settings** action
  (`UIApplication.openSettingsURLString`). `.notDetermined` still prompts on
  enable (alert + sound); a grant enables, a denial lands in the guidance
  state. The status refreshes when Settings appears and on every foreground.
- Keep: `requestAuthorization` on first enable (startLive), the
  `receiveDeviceToken → POST /device-token` upload path, and the DEBUG local
  notification bridge (unit-tested: `DeviceTokenUploadTests`).
- Unit + source-wiring coverage: `NotificationPermissionMappingTests`,
  `NotificationEnableModelTests`, `SettingsNotificationWiringTests`,
  `DeviceTokenUploadTests` (token callback uploads a signed body once;
  duplicate callbacks suppressed).

The artifact is **SYNTHETIC ONLY**: the DEBUG
`-corralDemoDeniedNotificationsEvidence` driver over `DemoFleet.seed()`
(fictional repos/agents, no live daemon, no physical device) FORCES the denied
notification posture in demo mode — a simulator cannot be denied
notifications (`simctl privacy` has no notifications service and the OS alert
cannot be answered without touch injection) — so the frame is the synthetic
stand-in the unit suite pins. No live-fleet, physical-device, or TestFlight
claim.

## Artifact

One deterministic launch records the sequence. Each phase writes a marker
file the host capture script polls, then the script screenshots (markers are
written 2.5 s AFTER the phase's state settles and each phase HOLDS 9 s so the
capture always lands inside the phase it names):

| Phase | File | Shows |
|---|---|---|
| 1 | `phase-1-denied-mocha-board-390x844.png` | MOCHA demo board (blocked/working sections, filter chips) before Settings opens |
| 2 | `phase-2-denied-settings-notifications-390x844.png` | MOCHA Settings sheet SCROLLED to the Notifications section (the DEBUG scroll task drives the form — simctl cannot drag): toggle OFF + `Corral can't alert you — notifications are off for this app in iOS Settings.` + **Open iOS Settings** action |
| 3 | `phase-3-denied-done-390x844.png` | Rest state after the sequence (Mocha board, sheet closed) |

All rendered 390x844 pt (raw 1179x2556 px iPhone 16 / iOS 26.5 @3x,
`sips -z 844 390`; 0.18 % aspect distortion from the 393x852 pt native — same
standard as issue-316/362/385/386/388). Captured on a FRESH simulator
(`Corral389`, iPhone 16, iOS 26.5 — no keychain identity, so no
notification-permission alert race and no leftover pairing).

## Audit (Vision OCR + pixel geometry)

Local Vision OCR / vision inspection of the frames:

- Phase 2 (the AC frame): the Notifications section is FULLY visible at the
  form's bottom (nothing cut off) — `State-change notifications` toggle OFF,
  the orange struck-bell `Corral can't alert you — notifications are off for
  this app in iOS Settings.` row, and the purple `Open iOS Settings` action;
  the Device section (Key ID `—`, Keychain storage, Read-only signed device,
  `Not paired`, Remove device) sits above it. Flavor truth: the Mocha
  checkmark in the Appearance row + dark luminance (~48-51) — Mocha as
  labeled. The switch shows OFF because the toggle's displayed value derives
  from the permission posture (`notificationsEnabled && !showsBlockedGuidance`).
- Phases 1/3: the demo board in Mocha (blocked/working rows) with no sheet.

The denied posture, the guidance copy, the Open-iOS-Settings action, and the
driver's three markers are additionally pinned by the unit suite
(`SettingsNotificationWiringTests`, `NotificationEnableModelTests`,
`NotificationPermissionMappingTests` in `ios/FleetNotifierTests/`).

## Capture commands (capture.log)

Fresh simulator `Corral389` (iPhone 16, iOS 26.5), DEBUG build from this
branch, `-corralDemoDeniedNotificationsEvidence` launch, marker polling,
`simctl io screenshot` per phase, `sips` resize to 390x844, plus the
Debug/Release codesign + `__entitlements`-section probes. Full details in
`capture.log`.

SHA-256s: `SHA256SUMS.txt` (all artifacts above).

Evidence conventions follow docs/design/evidence/issue-362/385/386/388.
