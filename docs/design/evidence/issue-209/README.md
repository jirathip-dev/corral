# Issue #209 evidence (Devices / Grants surface — board)

This bundle records the approved #250 V2 master/detail Device access design
for the egui board, on top of the base ledger surfaces that already existed
(`GET /grants` + `POST /grants` host-admin routes and the grant management
plumbing in Settings).

## What is captured

- `prototype.png` — the in-repo approved prototype render
  (`docs/design/corral-ux-device-grants-prototype.html`, headless Chrome via
  the machine-cached chrome-headless-shell). Shows the approved surface:
  THIS DEVICE (this computer) vs REMOTE DEVICES (other machines) grouping,
  per-capability toggles with plain-language descriptions, Re-register /
  Refresh grants on the THIS-device card, Revoke / Re-grant on remote cards,
  the Restore strip path (state-reregistered variant), and the #249
  bad-signature trust-check note.
- `prototype-view.html` — the stage copy the renderer used (source wrapped
  with the design-gate target styling).
- `capture.log` — the prototype renderer log.

## NOT captured here — live-after.png

The live egui frame (`live-after.png`) was NOT produced by this lane. The
capture reached the app launch and hung on the app's fail-closed
native-window probe: `accessibility_probe_ok=false` with
`probe_error="hidden=unavailable,frontmost=unavailable,windows=-25204"`
(the raw AX API path of `scripts/native-window-probe.swift` is denied for
this session's responsible app — the known 2026-08-26 design-gate wake race
documented in `macos-automation-tcc-gates`; osascript System Events control
works, the compiled probe's AXUIElement path does not). A live capture
therefore requires the Authorization/Accessibility grant on the
responsible app (Guy-owned host setting) and a re-run of:

```text
CHROME_BIN=$HOME/Library/Caches/ms-playwright/chromium_headless_shell-1234/chrome-headless-shell-mac-arm64/chrome-headless-shell \
  scripts/design-gate-evidence.sh --issue 209 --surface egui \
  --prototype docs/design/corral-ux-device-grants-prototype.html \
  --host-url http://127.0.0.1:8474 --egui-tab settings \
  --no-build --delay-ms 12000 \
  --egui-wake-command /tmp/g209-wake.sh \
  --output-root docs/design/evidence --force
```

(`/tmp/g209-wake.sh` performs an exact-PID NSRunningApplication activation;
a repeat-run with this wake plus the granted Accessibility row matches the
pattern of the successful issue-206 publisher run — bounded retries until
`probe_ok=true` + `reason_code=dispatch_ready`.)

## Design source

- Approved design: #250 V2 (variant-2-master-detail, locked by Guy
  2026-08-27) — the in-repo prototype above implements it in the repo's
  design tokens.
- Prototype source SHA-256: `d7e8cfc8298a8239632b79c49c563da520f6fe126319e41ca494f50a899af727`
