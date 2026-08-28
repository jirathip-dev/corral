# corrald-ui — Corral P4 client (egui, desktop + read-only web)

Dark-dashboard fleet board for corrald (`docs/corral/P4-brief.md` W2). One
Rust codebase, macOS + Linux, **plus a read-only WebAssembly build** (`#215`):
the same board rendered in a browser from corrald's credential-free read
plane (`/snapshot`, `/events` SSE) or from a bundled demo fixture — no
writes, no signing keys, no keyring, no registration. Speaks corrald's HTTP
surface directly (`docs/corral/P4-conformance.md` is the normative contract).

## What it does

- **Live fleet board** — snapshot + SSE with `Last-Event-ID` resume (the
  daemon answers a stale cursor with a full snapshot or delta replay),
  reconnect with doubling backoff capped at 30s, `/snapshot` fallback on
  reconnect only (no daemon polling). The SSE connection carries NO total
  request timeout (a total deadline severed the stream every 60s); each
  chunk read has a 45s deadline — 3x the daemon's 15s keepalive cadence —
  so a genuinely dead socket still forces a reconnect.
- **Master/detail board** — a ~40/60 split. The left pane searches repo /
  branch / title / issue and filters with contract-state chips (All · Needs
  you · Review · Working · Idle); Cards default to a flat attention-ranked
  list with `State · relative age` (`<1m`, minutes/hours/days, `—` when the
  timestamp is unknown) per card and one collapsed `Idle (N)` tail; the age
  slot is reserved on narrow panes so identity/repo text drops first. The
  Table keeps its grouped-by-repo default. Empty state buckets never render,
  and a no-match query reports once in the active view. The right pane owns
  the selected agent's detail, drive controls, full waiting claim, and
  Recent output.
- **Cards | Table** — cards are the default view. The exact nine-column
  conformance table (drop DRIVE, narrow WAITING ON) remains reachable from
  the toolbar; full drive controls stay in the selected detail pane.
- **Dark-dashboard theme pass** — custom `egui::Visuals` (charcoal canvas
  `#0d1117`, teal accent, distinct hues for the four agent states and the
  four waiting-on kind badges: approve-tool / question / menu / crash).
- **Topology + PR/CI factors** — repo / branch / dirty / ahead-behind / PR /
  CI render in the Cards detail pane and in the exact Table columns.
- **Signed drive** — device Ed25519 keypair generated on first run,
  stored in the OS keychain (macOS Keychain / Linux kernel keyring via
  `keyring`); 0600 file fallback with a startup warning banner.
  Registration UX: paste the routing-only registration token, or
  auto-register on localhost (reads the daemon's `registration-token`
  file, same user). Read-only default: drive buttons render only from
  `agent.capabilities` AND the device's grant ledger; a `not_granted`
  refusal surfaces as a typed error banner and demotes the button
  (demotions persist; Settings → "refresh grants" re-registers the SAME
  key to re-learn the host's current grant set and re-enable re-granted
  capabilities). Every agent-advertised capability renders SOMETHING —
  a disabled button with the reason when the ledger lacks it.
- A known Herdr target that disappears or moves returns the typed
  `stale_agent` refusal. The board removes that row immediately and performs
  one snapshot refresh while SSE remains authoritative for the replacement.
- **Drive controls from capabilities** — prompt (Enter sends), interrupt,
  read_tail (bounded 200 lines; Cards performs at most one automatic fetch
  for the currently visible, attention-resolved detail card when the agent
  advertises the capability and the device grant allows it; later pages stay
  explicit via Load earlier, with no background or pane-wide prefetch), approve
  (choice buttons from `waiting_on.choices`; the claim echoes
  `approval_id` + `prompt_hash` byte-for-byte from the snapshot prompt),
  kill/attach (typed refusals surface when the source doesn't implement
  them).
- **Idempotent retries** — one `request_id` per logical action, reused
  across transport/5xx/`in_flight` retries; the daemon's replay table
  dedupes. Re-registration rotates the device key only AFTER the daemon
  accepts the new pubkey (a failed re-register leaves the old key + old
  key_id intact and driving). `403 step_up_required` triggers the transparent step-up flow:
  sign a `StepUpRequest` (fresh nonce/ts), mint via `POST /step-up`,
  retry the SAME envelope with `X-Step-Up-Token`. The daemon is the
  authority on destructive-pattern detection; the client only mirrors the
  pattern table for a pre-send hint.
- **Host administration** — Settings hosts `GET /audit` and the device
  grant editor. Both use the host `admin-token` (auto-read on localhost or
  keychain-stored), which never enters a device-signed drive flow. The
  audit pane renders the hash-chained log with the chain-validity verdict;
  the grant editor lists registered devices via `GET /grants`, replaces a
  selected device's full capability set via `POST /grants`, and exposes the
  same revoke action as `corrald-grant.sh --revoke`.
- **Fleet identities (no dedicated tab)** — the Issues tab resolves repo
  categories into exact fleet-name drive targets via the daemon's
  `GET /fleets` identity catalog (configless #237 — corral never reads or
  writes the fleet registry file). The dedicated Fleets tab was removed
  (#269); the private fleet surface belongs to the future fleet-ops
  sidecar plugin (#239). Mutations still run through `herdr-fleet` on the
  host, and `corrald fleet switch <name>` delegates the re-arm to the
  fleet-ops CLI. There is no fleets.json editing anywhere in the client.
- **Issues tab** — the repo-level `GET /issues` browser moved out of Board
  into its own top-level tab, keeping the issue-linked worktree action
  alongside the board's other tabs. Issue-free worktree creation is not
  exposed in this view.

## Build + run

```sh
cargo build --release -p corrald-ui
./target/release/corrald-ui                      # default http://127.0.0.1:8474
CORRAL_UI_CONFIG_DIR=~/.config/corral/ui ./target/release/corrald-ui
RUST_LOG=info ./target/release/corrald-ui        # SSE connect is logged
```

The desktop window icon is embedded at compile time from
`assets/icon/corral-icon-256.png` and supplied to eframe's native viewport.
Regenerate/check the repository icon outputs with the commands in
[`docs/DEVELOPING.md`](../../docs/DEVELOPING.md#icon-assets-and-packaging).

Client config lives in `$CORRAL_UI_CONFIG_DIR` (default
`~/.config/corral/ui`): `config.json` (host URL + registration record),
`keys/` (0600 key file fallback), and keychain entries under
`corrald-ui`.

### Evidence capture (verification aid, env-gated)

```sh
CORRAL_UI_SCREENSHOT=/tmp/board.png \
CORRAL_UI_SCREENSHOT_DELAY_MS=8000 \
./target/release/corrald-ui
```

Requests a wgpu viewport screenshot after the delay, writes the PNG, and
exits when the native seam completes. Never active by default. The design-gate
evidence harness accepts the file only after complete PNG/CRC validation, so a
writer that lingers is cleaned up with bounded TERM→KILL against its owned
direct child. (eframe's own wgpu screenshot event is
flaky without an explicit `device.poll`; this path polls with a bounded
wait and captures the presented surface.) On macOS, evidence mode also keeps
the eframe 0.36.1 root viewport visible/key during its hidden-first-frame
startup, and the harness compiles the exact-PID CoreGraphics probe. Dispatch is
fail-closed until the process, window, frontmost, key/main, and exact-PID
on-screen CoreGraphics observations all agree. macOS has no reliable public
independent Space-membership query in this probe, so evidence does not claim
an active-space result; the exact target window and aggregate non-target count
are recorded instead.

For unattended scratch runs, `CORRAL_UI_DISABLE_KEYRING=1` selects the
documented 0600 file fallback and prevents a macOS Keychain prompt from
blocking the native event loop; the integration harness sets it automatically.
Re-registration in this mode first reconciles and removes any stale keychain
entry, refusing to continue if that identity cannot be reconciled.

For native live-agent evidence, add `CORRAL_UI_SCREENSHOT_AGENT` with an agent
id observed in the daemon's `/snapshot` response. The app resolves that real
agent through the visible Cards selection, then performs the same one-shot,
capability- and grant-gated signed `read_tail` hydration used by the board; it
does not create demo data. Subsequent pages remain explicit Load earlier
requests:

```sh
CORRAL_UI_SCREENSHOT=/tmp/board-live.png \
CORRAL_UI_SCREENSHOT_DELAY_MS=20000 \
CORRAL_UI_SCREENSHOT_AGENT=herdr:<agent-id> \
./target/release/corrald-ui
```

For the full local lifecycle—real corrald, `/snapshot` target selection,
registered native UI, safe exact-pid window wake, and cleanup—run from the
repository root on macOS:

```sh
bash scripts/test-design-gate-egui-integration.sh                 # read-only verify
bash scripts/test-design-gate-egui-integration.sh --publish       # native regenerate/publish
```

## Conformance + tests

```sh
# Unit + in-process conformance against the daemon's own seams
# (dev-dependency corrald with test-utils: canonical envelope bytes,
# DeviceAuthorizer round-trip, step-up bytes):
cargo test -p corrald-ui

# LIVE probe against a real corrald (register → read-only refusal →
# grant → signed read_tail → idempotent replay → audit growth). Needs a
# scratch daemon, e.g.:
#   CORRAL_CONFIG_DIR=$(mktemp -d) ./target/release/corrald --port 8574 \
#       --socket ~/.config/herdr/herdr.sock
# then:
CORRALD_URL=http://127.0.0.1:8574 \
CORRAL_CONFIG_DIR=<scratch daemon config> \
CORRAL_UI_CONFIG_DIR=<scratch ui config> \
cargo test -p corrald-ui --test live -- --ignored --nocapture --test-threads=1
```

(``--test-threads=1``: the re-register probes share the host-scoped keyring
entry and the daemon registry, so they must not race.)

The live probe writes nothing to the GUI's keyring; it registers a fresh
ephemeral key so the read-only default is observed on every run. The
second probe (`live_reregister_failure_preserves_key_and_registration`)
verifies the F5 failure ordering: a failed re-register leaves the
persisted seed and registration untouched and the old key still drives.
The third (`live_reregister_success_rotates_the_in_memory_key`) verifies
the F5 success path: after a successful rotation the in-memory signing
key is reloaded to the new seed and the next signed drive verifies
against the daemon (and the old key under the new key_id is refused with
`bad_signature`). To make the GUI
itself boot straight to the board, register once (registration screen
in-app, or the probe flow) so `config.json` carries the matching
registration for the host fingerprint.

## Web build — read-only board + GitHub Pages demo (#215)

The same egui board compiled to `wasm32-unknown-unknown` (eframe
glow/WebGL, no tokio runtime, no `keyring`, no `/drive`):

- **Demo data by default.** The wasm bundles `assets/demo-fixture.json`
  (a representative snapshot + a short canned SSE delta sequence + the
  issue/fleet projections) and animates through it, so the deployed page
  shows a believable fleet board with **no corrald anywhere**.
- **Live daemon optional.** The first-open setup panel offers
  "Demo data | Live daemon" with the daemon base URL (default
  `http://127.0.0.1:8474`); both persist to browser storage and survive a
  refresh. Nothing is compiled into the wasm.

### Read-only boundary (never widened)

| Surface | Web | Desktop |
|---|---|---|
| `GET /snapshot`, `GET /events` SSE | yes | yes |
| `GET /issues`, `GET /fleets` (read projections) | yes | yes |
| `POST /drive`, `POST /step-up`, `POST /register`, `GET /grants` | **no** | yes |
| `/host-key`, `keyring`, device signing | **no** | yes |

Every write control is replaced by a disabled **read-only (web)**
indicator (`BoardActions::read_only`), and the drive callback is a no-op.
`read_tail` is a signed /drive capability, so the web build does not fetch
recent-output either — that stays desktop.

### Build (wasm-pack + target)

```sh
rustup target add wasm32-unknown-unknown     # once
cargo install wasm-pack                       # once (or use trunk)
cd clients/egui
wasm-pack build --target web --out-dir pkg --out-name corral-web
cp web/index.html pkg/                        # static shell + canvas
```

`pkg/` is the deployable static site: `index.html`, `corral-web.js`,
`corral-web_bg.wasm`. (A `trunk` setup works identically — `trunk build`
from `clients/egui` producing the same artifacts; the entrypoint waits for
`<canvas id="corral">`.)

### Local live check

```sh
cd clients/egui/pkg && python3 -m http.server 8000   # http://127.0.0.1:8000
# and, on the machine running corrald (loopback by default):
corrald --cors-origin http://127.0.0.1:8000
```

Point the web page at `http://127.0.0.1:8474` (or any host you can reach)
and switch to "Live daemon". If you serve the page from another machine,
use that machine's IP in `--cors-origin` and omit `--bind` only on a
permitted interface (never public — corrald refuses public binds).

### Daemon CORS flag (`--cors-origin` / `$CORRALD_CORS_ORIGIN`)

corrald's read plane is credential-free by design (#65), so the only
daemon-side change the browser needs is an `Access-Control-Allow-Origin` on
the read routes. The policy (see `src/api/cors.rs`):

- **Opt-in:** no flag → the daemon emits zero CORS headers, exactly as
  before.
- **Exact allowlist:** `--cors-origin https://user.github.io` may be
  repeated; `$CORRALD_CORS_ORIGIN=https://a,https://b` does the same. `*`
  is refused at startup. Matching is byte-exact, `scheme://host[:port]`,
  no trailing slash.
- **Read plane only.** The middleware sits only on `/healthz`,
  `/snapshot`, `/events`, `/history`, `/issues`, `/fleets`. The write
  plane (`/drive`, device-token, grants-read, auth routes) never emits
  CORS headers — a browser from another origin cannot complete a signed
  write even if the page tried.
- **Permitted binds only.** `--bind` is already restricted to loopback /
  RFC 1918 / Tailscale-CGNAT / ULA by `bind_permitted`, so CORS is only
  ever reachable where the read plane itself is allowed to be.

### Deploying the GitHub Pages demo

1. Build as above, then copy the **contents** of `pkg/` into a Pages
   host (a `gh-pages` branch of your Pages repo, or the repo root / docs
   folder depending on your Pages settings).
2. Open `https://<user>.github.io/<repo>/` — it renders the demo board
   instantly (demo fixture is compiled into the wasm; nothing runs on a
   server).
3. Deploy again by re-running step 1 on a new build and pushing.
4. To demo LIVE data, run corrald on the *viewing* machine and put its
   origin in the allowlist:
   `corrald --cors-origin https://<user>.github.io` (the full origin of
   the deployed page, scheme + host, no path).

Pages enablement itself (repo settings / custom domain) is a one-time
human step outside this repo; it requires no code changes on repeat
deploys.

## Layout

```
src/
  main.rs      entrypoints: eframe native (wgpu) + wasm web (glow)
  app.rs       app state, channels, registration, drive dispatch (native)
  web.rs       read-only wasm board: setup panel, demo fixture, live SSE
  demo.rs      bundled demo fixture loader (assets/demo-fixture.json)
  model.rs     wire mirrors of src/core/model.rs + rendering helpers
  protocol.rs  snapshot/SSE/register/admin-grants/audit client + SSE parser
  drive.rs     signed envelope, typing, idempotent retries, step-up
  keys.rs      keypair generation/storage (keychain + 0600 fallback)
  theme.rs     dark-dashboard palette (the only place colors live)
  state.rs     fleet cache, grant ledger, toasts, config records
  ui/          board (master/detail + cards/table), issues, audit,
               register/settings
tests/
  conformance.rs  in-process conformance vs the daemon's own seams
  live.rs         #[ignore] live probe against a real corrald
evidence/       live verification artifacts (screenshot, SSE logs, audit)
```
