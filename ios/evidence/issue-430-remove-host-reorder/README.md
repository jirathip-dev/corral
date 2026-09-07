# 430 evidence — Settings Hosts: reorder Edit control removed (static rows)

Frames for the #430 change (390x844 px, iPhone 16 @3x 1179x2556 downscaled
with `sips -z 844 390` — the repo's standard device class; 0.18 % aspect
distortion from the 393x852 pt native, same as the #385/#416 sets). All
frames are SYNTHETIC DEBUG demo captures over the #401 multi-host seed
(fictional `demo-host-*` hosts; no live daemon, no physical device, no
TestFlight claim), recorded on a fresh `Corral430` simulator (iPhone 16,
iOS 26.5, UDID 0F27110E-A497-4970-B8AF-9BBA6ADF3382) with the deterministic
`-corralDemoMultiHostSettingsEvidence` driver (three seeded profiles: Host A
live/active, Host B offline with retained stale rows + `host unreachable`
error, Host C key mismatch — the driver opens Settings, scrolls the Hosts
section into view, and holds ~9 s per phase behind marker files).

## What changed

- The Settings toolbar's conditional `EditButton` (`ToolbarItem
  .topBarLeading`, shown only with 2+ hosts) is removed — the leading slot
  now shows the stack's previous-title label instead of an Edit pill.
- The `Hosts` rows render STATICALLY in the persisted profile order: the
  `.onMove(perform:)` wiring, the view-level `moveHosts` helper, the
  model-level `AppModel.moveHosts`, and the store-level
  `HostProfileStore.moveProfile` order mutation are all removed.
- The drag-to-reorder footer copy is gone (single static footer keeps the
  pairing/fingerprint + remove-and-re-pair guidance).
- The persisted `order` field, `orderedProfiles` load ordering, and
  append-at-end add semantics are UNCHANGED (unit suite pins: order survives
  a document reload; a new host appends after existing profiles).

## Frames

| File | Shows |
|---|---|
| `phase-0-base-mh-settings-mocha-390x844.png` | BEFORE (#430 base head 40198936): Settings > Hosts in Mocha with the leading **Edit** pill present (2+ hosts) — the affordance this issue removes |
| `phase-1-mh-settings-mocha-390x844.png` | AFTER: same driver/state in Mocha — **no Edit item** in the nav bar; Host A/B/C rows with health + error text, fingerprint/key-id/grants-expiry metadata, per-host Notify toggles, Retry/Rename/Remove host actions |
| `phase-2-mh-settings-latte-390x844.png` | AFTER: same static Hosts list after the live flavor flip to Latte — no Edit in the light theme either |
| `phase-3-mh-settings-done-390x844.png` | AFTER: board restored when Settings closes (driver end state) |

The base frame is the identical driver run against the identical DEBUG build
recipe at the #430 BASE head (the RED-state tree) — the only difference is
the #430 diff itself, so the nav-bar comparison is apples-to-apples
(no-Edit is not a capture artifact).

## Audit (Vision OCR of the frames)

- Phase-0 base: leading nav slot = **Edit** pill; Settings title center; `?`
  help trailing. Phase-1/2 final: leading slot has **no Edit** — the stack's
  faded previous-title label (`Hosts`, chevron-less) renders where the Edit
  pill sat; `?` help stays trailing; titles unchanged.
- Host A row (live, `Active`), Host B row (offline + `stream disconnected —
  host unreachable`), Host C row (key mismatch, red guidance) are all
  visible with URL/fingerprint/key-id/grants-expiry readouts, per-host
  `Notify about this host` toggles, and Retry / Rename / Remove host actions
  in every Settings frame — remaining host actions independently reachable.
- No system alert overlays any frame (fresh simulator, no keychain identity
  — same posture as the #389/#401 evidence sets).

Accessibility/source guarantees ride the unit suite (this repo has no UI
test target): `MultiHostSurfaceWiringTests.testSettingsHostsRenderStatically
WithoutReorderChromeButKeepPerHostActions` is RED at the base head (`.onMove`
+ `moveHosts` + `EditButton` all present → 3 assertion failures, exit 65) and
GREEN after removal, asserting the remaining per-host actions/add-host/notify
anchors still render; `HostProfileWiringTests` keeps the Add-host/Remove-host
wiring pins.

## Capture commands (capture.log)

DEBUG build per side (`/tmp/fn430-ev` final head, `/tmp/fn430-base-app.app`
base head, both `HERDR_XCODEBUILD_DIRECT=1 xcodebuild build ... -derivedDataPath`),
one launch per side with marker polling + `simctl io screenshot` per phase +
`sips` resize to 390x844. Full details in `capture.log`.

SHA-256s: `SHA256SUMS.txt`. Conventions follow ios/evidence/issue-415/416.
