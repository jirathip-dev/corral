# 528 evidence — consolidated connection status + one-row Filters/Settings (Board & Herd)

Runtime frames for the #528 chrome consolidation (phone-sized, synthetic DEBUG
demo captures over the #401 three-profile seed — Host A live, Host B
offline-with-retained-stale-rows, Host C connecting in the primary matrix;
fictional `demo-host-*` hosts; no live daemon, no physical device, no
TestFlight claim). Recorded on the FRESH `Corral528` simulator (iPhone 16,
iOS 26.5, UDID 46C711A9-E5FC-4147-82F2-7553D33E9B1B) with the deterministic
`-corral528ConnectionEvidence` / `-corral528HerdEvidence` drivers (marker
files in `Documents/ux-evidence`, phase markers written AFTER the state
settles, >= 9 s hold per phase, one launch per driver). The fix round r2
(review conditions 1 + 2) re-captured the whole matrix on the fresh
`Corral528Fix` simulator (iPhone 16, iOS 26.5, UDID
DC56A639-72B2-498E-9621-E35AC1349166 — the original `Corral528` simulator was
host-cleaned between rounds) and added the `*-before-*` frames for the
popover-contrast pair.

All frames are 1179x2556 px @3x iPhone-16 captures downscaled with
`sips -z 844 390` (the repo's standard 390x844 device class; 0.18 % aspect
distortion from the native 393x852 pt — same convention as the #385/#401/
#416/#427/#456 sets).

## What changed (what these frames show)

- ONE compact connection indicator — the dot + short label — in BOTH modes,
  next to the Filters control: connected (`Live`), connecting (subtle pulse,
  `Connecting`), disconnected (`Offline` / the multi-host aggregate), and the
  partial multi-host aggregate (`1 host offline · 1 host connecting`). No
  routine banner, no repeated "connecting" copy, no separate outage strip.
- Board Filters, the indicator and the Settings gear share ONE horizontal
  row (the Herd-aligned layout); the gear moved out of the nav-bar toolbar
  into that row (still >= 44 pt, still labelled `Settings`).
- Tapping the indicator reveals the per-host detail popover (which host is
  offline / connecting / key-mismatched) plus the last-known provenance line
  while the retained board is not live. On Herd that popover rides the repo's
  AA-pinned ranch chrome surface — the same Day/Night treatment as the
  floating chrome bars around it — so it stays legible over the bright sky
  (review condition 2; see the `-before` / current pair for `528-9` /
  `528-ax-4`).
- Removed: the Herd disconnect panel (the source-disconnected copy with its
  board/retry recovery actions), the Board pull-to-refresh instruction AND
  gesture, the routine connecting/offline board line, and the separate D7
  outage strip.
- Stale-as-unknown stays truthful: retained rows keep their `stale · last
  seen Nm ago` labels, the indicator never claims `Live` while a host is
  unreachable, and the reveal says "Showing last-known fleet data."

## Frames (390x844)

| File | Proves |
|---|---|
| `528-1-board-partial-390x844.png` | Board, partial: indicator `1 host offline · 1 host connecting`, ONE-row chrome (Filters + indicator + gear), retained Host B rows stale-labelled |
| `528-2-board-detail-390x844.png` | The revealed per-host detail popover: Host A live / Host B offline / Host C connecting + the last-known provenance line |
| `528-3-board-disconnected-390x844.png` | Total disconnection: indicator `3 hosts offline` (no host live), board keeps the last-known fleet, rows stale |
| `528-4-board-recovered-390x844.png` | Recovery: the fleet returns live (`Live`) AND the re-applied delta changed a retained row (demo-orbit-blocked → working) — no gesture involved |
| `528-5-board-empty-connecting-390x844.png` | Empty/loading: a genuinely empty aggregate (every host's rows emptied) while Host A is still connecting — the indicator reads `1 host connecting`, the board renders no rows and no last-known note (review condition 1) |
| `528-6-board-partial-latte-390x844.png` | Latte (light palette) partial frame — the indicator + one-row chrome in the light flavor |
| `528-7-board-done-390x844.png` | Rest frame at the end of the board run (driver settled, no further phase) |
| `528-8-herd-partial-390x844.png` | Herd floating chrome, partial: the SAME indicator state on the ranch, counts card intact, no outage panel |
| `528-9-herd-detail-390x844.png` | Herd: the per-host detail reveal over the ranch, now on the AA-pinned ranch chrome surface (review condition 2) |
| `528-9-before-390x844.png` | The pre-fix Herd reveal (committed at the reviewed head `ba83728`): the same content on the translucent popover material, low-contrast over the bright sky |
| `528-10-herd-disconnected-390x844.png` | Herd, total disconnection: all horses render `unknown · last known …`, front rail reads `LAST KNOWN`, indicator `3 hosts offline` |
| `528-11-herd-recovered-390x844.png` | Herd recovery: indicator `Live` |
| `528-12-herd-empty-connecting-390x844.png` | Herd empty scope with the connecting indicator: every host's rows emptied, so the scope renders `No agents in this scope` with no front rail and the indicator reads `1 host connecting` (review condition 1) |
| `528-13-herd-done-390x844.png` | Rest frame at the end of the Herd run — by design the terminal marker fires on the SAME empty state as `528-12`, so the two files are byte-identical (the sha256 repeat is expected for this pair, not a stale capture; every other frame is distinct) |
| `528-ax-1-board-partial-detail-390x844.png` | Board at the accessibility content size: the one-row chrome + the revealed detail (large text, nothing clipped) |
| `528-ax-2-board-done-390x844.png` | Rest frame ending the board accessibility run |
| `528-ax-4-herd-partial-detail-390x844.png` | Herd at the accessibility content size: floating chrome + detail reveal on the ranch chrome surface (review condition 2) |
| `528-ax-4-before-390x844.png` | The pre-fix accessibility-size reveal at the reviewed head — the low-contrast pair for the comparison above |
| `528-ax-5-herd-done-390x844.png` | Rest frame ending the Herd accessibility run |

## Accessibility / VoiceOver / hit targets

This repo has no UI-test target, so VoiceOver labels and >= 44 pt targets
ride the unit suite (`ConnectionChromeModelTests` /
`ConnectionChromeWiringTests`): the indicator is one >= 44 pt button whose
`accessibilityLabel` is the full status description naming EVERY host
("Connection status: Partially connected. Host A: live, Host B: offline,
Host C: connecting. Showing last-known fleet data."), its hint names the
reveal, and the detail list combines one label per host. Reduce Motion
renders the connecting state static. Dynamic Type frames are captured at
`UICTContentSizeCategoryAccessibilityLarge` (the XCUITest argument-domain
mechanism; synthetic stand-in, same as the #427 set).

## Capture commands

DEBUG build per run, one launch per driver with marker polling
(`/tmp/i528-capture.sh` — screenshot per new phase marker, >= 2 s after
detection, inside the phase's >= 9 s hold) + `simctl io screenshot` + `sips`
resize. Full details in `capture.log`.

SHA-256s: `SHA256SUMS.txt`. Conventions follow ios/evidence/issue-427/416/456.
Every frame has a distinct sha256 (no stale duplicate captures); the two
standard runs are repeated AFTER the large-text pair so the a11y run's shared
phase-1 markers (`528-1`, `528-8`) cannot leave an AX composition under a
standard-size name (`capture.log` records the ordering).

## Prototype-vs-delivery notes

- The indicator's label is the compact aggregate form the D7 line used
  ("1 host offline", "1 host connecting", "3 hosts offline") — the spec's
  illustrative scenario copy ("macbook-air connecting · Bazzite live") is
  not used; the ACs bind textual availability, which the aggregate label +
  the per-host reveal provide.
- Per-row stale ages stay on the board rows (C6); the indicator carries the
  aggregate and the reveal line carries the provenance — never color alone.
