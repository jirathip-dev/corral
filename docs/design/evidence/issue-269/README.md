# issue-269 — post-removal board (Fleets tab removed)

## WHAT

egui board after the #269 Fleets-tab removal: the top-right tab bar shows
Issues / Board (plus the setup button) — there is NO Fleets tab entry and no
hidden path (`CORRAL_UI_SCREENSHOT_TAB=registry` mapping removed).

## Capture

- Surface: `clients/egui` wasm read-only web build (demo mode), the same
  `web.rs` tab-bar + `ui/` code as the desktop board.
- Build: `cd clients/egui && wasm-pack build --target web --out-dir pkg
  --out-name corral-web` (wasm32-unknown-unknown), then
  `cp web/index.html pkg/`; served via `python3 -m http.server 8489`
  inside `pkg/`.
- Shot: chrome-headless-shell (host playwright cache, no browser install)
  `--screenshot --window-size=1280,800 --virtual-time-budget=15000
  --hide-scrollbars http://127.0.0.1:8489/seed.html` where `seed.html`
  pre-seeds `corral_web_setup_v1` (`demo` mode) then redirects to `/` —
  the app's own localStorage setup key; the app itself opens the setup
  panel only when that key is absent (first run).
- Binary:
  `$HOME/Library/Caches/ms-playwright/chromium_headless_shell-1234/chrome-headless-shell-mac-arm64/chrome-headless-shell`

## Artifacts

- `post-removal-board.png` — 1280x800 PNG
- SHA-256: `ecb84afc06a1afe009dc08c3ca6002152bb3f06fe8a70f4e6536c7d8ce4dc93a`

## Variant note (native)

The native desktop tab strip is covered by the code change and the updated
unit test `workspace_navigation_has_exactly_three_tabs_and_demotes_audit`
(app.rs), plus clippy/test gates; no native window capture was run for this
lane (the design-gate native harness lives in scripts/ and is not an
implementer-lane tool).
