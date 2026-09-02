# corrald-ui — Corral P4 client (egui, desktop + read-only web)

Dark-dashboard **read-only** fleet board for corrald (`docs/corral/P4-brief.md`;
#354 v2: no Corral-invented wording — the state chips are herdr's raw token
labels from `contracts/state-tokens.json`). One Rust codebase, macOS + Linux,
**plus a read-only WebAssembly build** (`#215`, mobile layout per `#304`): the
same board rendered in a browser from corrald's credential-free read plane
(`/snapshot`, `/events` SSE with `Last-Event-ID` resume) or from a bundled
demo fixture — no writes, no signing keys, no keyring. On the desktop the
client also registers (`POST /register`, signed) so the recents panel can use
its one retained drive capability, the signed `read_tail` live tail.
`docs/corral/P4-conformance.md` is the normative wire contract.

## What it does

- **v2 read-only board (no search, no filter chips)** — repo groups with
  status chips; rows show name / repo / state / time-in-state / branch and a
  small pane ref. Attention order is pinned by the raw state tokens: blocked
  first, then working → idle → unknown. Last-known board + offline banner when
  the daemon is unreachable; live SSE updates with snapshot/delta replay.
- **Recents v1 bottom sheet** — LIVE TAIL ONLY: the daemon-capped ≤200-line
  `read_tail` result, auto-scrolled; the desktop board refreshes it with the
  cached source revision so it never serves a stale tail. No load-earlier, no
  conversation/harness partition.
- **Two tabs only** — Board | Settings. Settings is connection-only: host
  URL, paste-token or localhost auto-register, and the #310 identity recovery
  block (user-initiated after an actual server-side `bad_signature`
  rejection). No issues browser, no drive/approval/step-up UI, no
  grant-admin, no audit pane, no push/notifications.
- **Desktop identity** — device Ed25519 keypair (keychain / 0600 file
  fallback); registration is read-only by default (zero grants). Recents v1
  renders from the agent's advertised `read_tail` capability AND the device
  grant ledger; a `not_granted` refusal surfaces as a typed banner. On host
  fingerprint change or a wiped key the board shows a passive notice and
  Settings offers re-register — startup never mutates identity.
- **WASM demo by default** — the wasm bundles a privacy-scrubbed synthetic
  fixture (read-only snapshot + canned SSE deltas + a recents tail) and
  renders out of the box with no corrald anywhere. The first-open panel also
  offers a live daemon URL (default `http://127.0.0.1:8474`); mode persists to
  browser storage. The browser never signs a drive.

## Build + run

```sh
cargo build --release -p corrald-ui
./target/release/corrald-ui                      # default http://127.0.0.1:8474
CORRAL_UI_CONFIG_DIR=~/.config/corral/ui ./target/release/corrald-ui
RUST_LOG=info ./target/release/corrald-ui        # SSE connect is logged
```

Client config lives in `$CORRAL_UI_CONFIG_DIR` (default
`~/.config/corral/ui`): `config.json` (host URL + registration record),
`keys/` (0600 key file fallback), and keychain entries under `corrald-ui`.
For unattended scratch runs, `CORRAL_UI_DISABLE_KEYRING=1` selects the 0600
file fallback and prevents a macOS Keychain prompt from blocking the native
event loop.

### Evidence capture (verification aid, env-gated)

```sh
CORRAL_UI_SCREENSHOT=/tmp/board.png \
CORRAL_UI_SCREENSHOT_DELAY_MS=8000 \
./target/release/corrald-ui
```

Requests a wgpu viewport screenshot after the delay, writes the PNG, and
exits. Never active by default; evidence is synthetic-only (see
`evidence/`). The full native design-gate lifecycle is run from the repo root
with `bash scripts/test-design-gate-egui-integration.sh` (read-only verify).

## Read-only boundary (never widened)

| Surface | Web | Desktop |
|---|---|---|
| `GET /snapshot`, `GET /events` SSE | yes | yes |
| `POST /register` (signed read auth) | **no** | yes |
| signed `/drive` `read_tail` (recents v1) | **no** | yes |
| `/host-key`, keyring, device signing | **no** | yes |
| Issues / audit / grants-admin / step-up / any mutating drive | **no** | **no** |

The browser build renders the same board from the bundled fixture or the
daemon's read plane; recents in demo mode come from the fixture, and in live
mode the drill-in explains that signed `read_tail` is desktop-only. The
mutating drive surface, issues projection and admin routes have no client
code at all — the #354 RED/GREEN source probes pin that.

## Web build — read-only board + GitHub Pages demo (#215, #304)

The same egui board compiled to `wasm32-unknown-unknown` (eframe
glow/WebGL, no tokio runtime, no `keyring`, no `/drive`):

```sh
rustup target add wasm32-unknown-unknown     # once
cargo install wasm-pack                       # once (or use trunk)
cd clients/egui
wasm-pack build --target web --out-dir pkg --out-name corral-web
cp web/index.html pkg/                        # static shell + canvas
```

`pkg/` is the deployable static site: `index.html`, `corral-web.js`,
`corral-web_bg.wasm`. Deploy by copying the contents of `pkg/` into the
Pages host. To demo LIVE data, run corrald on the viewing machine with its
origin allowlisted (`corrald --cors-origin https://<user>.github.io`); the
CORS policy covers only the credential-free read plane
(`/healthz`, `/snapshot`, `/events`, `/history`, `/issues`) — see `src/api/cors.rs`.

The mobile demo layout is 390×844 per #304. The bundled fixture and any
built `pkg/` are scanned by `scripts/check-demo-privacy.py` (identity /
URL / path rules, with a `--self-test` that proves each rule bites); rebuild
the fixture only through a synthetic generator that reuses the approved
`demo:pNN:*` identities, and run the scanner before committing.

## Conformance + tests

```sh
cargo test -p corrald-ui                # unit + in-process conformance + probes
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

- `tests/conformance.rs` — canonical envelope bytes vs the daemon's own
  seams, DeviceAuthorizer round-trip (read-only default → NotGranted →
  granted verify), refusal-kind ↔ HTTP table.
- `tests/identity_recovery.rs` — reinstall-style recovery restores the
  signed read plane (grant ledger, recents drive) without admin routes.
- `tests/live.rs` — `#[ignore]`d live probes against a real scratch corrald
  (register → read-only refusal → SSE resume; re-register failure/success
  ordering). Needs a scratch daemon, e.g.:
  `CORRAL_CONFIG_DIR=$(mktemp -d) ./target/release/corrald --port 8574`
  then `CORRALD_URL=... cargo test -p corrald-ui --test live -- --ignored`.
- Unit probes in `app.rs`/`register.rs` pin the closed surface: no mutating
  drive identifiers anywhere in production code, two tabs only, Settings
  actions are exactly the connection/recovery set, and the Board arm's
  recents hydration/refresh wiring stays revision-aware (these probes are
  mutation-tested RED/GREEN in the lane).

## Layout

```
src/
  main.rs      entrypoints: eframe native (wgpu) + wasm web (glow)
  app.rs       app state, channels, registration, SSE loop, recents wiring
  web.rs       read-only wasm board: setup panel, demo fixture, live SSE
  demo.rs      bundled demo fixture loader (assets/demo-fixture.json)
  model.rs     wire mirrors of src/core/model.rs + rendering helpers
  protocol.rs  snapshot/SSE/register client + SSE parser
  drive.rs     signed read_tail envelope, typed refusals, retries
  keys.rs      keypair generation/storage (keychain + 0600 fallback)
  theme.rs     dark-dashboard palette + raw state-token chips (colors only)
  state.rs     fleet cache, grant ledger, toasts, registration records
  ui/          board (v2 repo-group board + recents v1) and register/settings
tests/
  conformance.rs  in-process conformance vs the daemon's own seams
  identity_recovery.rs  #310 recovery on the read-only daemon
  live.rs         #[ignore] live probe against a real corrald
assets/         demo-fixture.json (privacy-scrubbed synthetic board)
evidence/       synthetic demo captures (390×844 per #304)
```
