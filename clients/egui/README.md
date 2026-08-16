# corrald-ui — Corral P4 desktop client (egui/wgpu)

Dark-dashboard fleet board for corrald (`docs/corral/P4-brief.md` W2). One
Rust codebase, macOS + Linux. Speaks corrald's HTTP surface directly
(`docs/corral/P4-conformance.md` is the normative contract).

## What it does

- **Live fleet board** — snapshot + SSE with `Last-Event-ID` resume (the
  daemon answers a stale cursor with a full snapshot or delta replay),
  reconnect with doubling backoff capped at 30s, `/snapshot` fallback on
  reconnect only (no daemon polling).
- **Dark-dashboard theme pass** — custom `egui::Visuals` (charcoal canvas
  `#0d1117`, teal accent, distinct hues for the four agent states and the
  four waiting-on kind badges: approve-tool / question / menu / crash).
- **Topology + PR/CI columns** — repo / branch / dirty / ahead-behind /
  PR / CI per agent, worktree detail on row expand.
- **Signed drive** — device Ed25519 keypair generated on first run,
  stored in the OS keychain (macOS Keychain / Linux kernel keyring via
  `keyring`); 0600 file fallback with a startup warning banner.
  Registration UX: paste the routing-only registration token, or
  auto-register on localhost (reads the daemon's `registration-token`
  file, same user). Read-only default: drive buttons render only from
  `agent.capabilities` AND the device's grant ledger; a `not_granted`
  refusal surfaces as a typed error banner and demotes the button.
- **Drive controls from capabilities** — prompt (Enter sends), interrupt,
  read_tail (bounded 200 lines, on tap only — never prefetch), approve
  (choice buttons from `waiting_on.choices`; the claim echoes
  `approval_id` + `prompt_hash` byte-for-byte from the snapshot prompt),
  kill/attach (typed refusals surface when the source doesn't implement
  them).
- **Idempotent retries** — one `request_id` per logical action, reused
  across transport/5xx/`in_flight` retries; the daemon's replay table
  dedupes. `403 step_up_required` triggers the transparent step-up flow:
  sign a `StepUpRequest` (fresh nonce/ts), mint via `POST /step-up`,
  retry the SAME envelope with `X-Step-Up-Token`. The daemon is the
  authority on destructive-pattern detection; the client only mirrors the
  pattern table for a pre-send hint.
- **Audit view** — `GET /audit` (host admin): auto-reads the host's
  `admin-token` on localhost or uses a keychain-stored token; renders the
  hash-chained log with the chain-validity verdict and auto-refreshes
  while visible.

## Build + run

```sh
cargo build --release -p corrald-ui
./target/release/corrald-ui                      # default http://127.0.0.1:8474
CORRAL_UI_CONFIG_DIR=~/.config/corral/ui ./target/release/corrald-ui
RUST_LOG=info ./target/release/corrald-ui        # SSE connect is logged
```

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
exits. Never active by default. (eframe's own wgpu screenshot event is
flaky without an explicit `device.poll`; this path polls with a bounded
wait and captures the presented surface.)

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
cargo test -p corrald-ui --test live -- --ignored --nocapture
```

The live probe writes nothing to the GUI's keyring; it registers a fresh
ephemeral key so the read-only default is observed on every run. To make
the GUI itself boot straight to the board, register once (registration
screen in-app, or the `tests/live.rs` flow) so `config.json` carries the
matching registration for the host fingerprint.

## Layout

```
src/
  main.rs      eframe entry (wgpu renderer) + tokio runtime
  app.rs       app state, channels, registration, drive dispatch
  model.rs     wire mirrors of src/core/model.rs + rendering helpers
  protocol.rs  snapshot/SSE/register/audit client + SSE parser
  drive.rs     signed envelope, typing, idempotent retries, step-up
  keys.rs      keypair generation/storage (keychain + 0600 fallback)
  theme.rs     dark-dashboard palette (the only place colors live)
  state.rs     fleet cache, grant ledger, toasts, config records
  ui/          board (fleet), audit, register/settings
tests/
  conformance.rs  in-process conformance vs the daemon's own seams
  live.rs         #[ignore] live probe against a real corrald
evidence/       live verification artifacts (screenshot, SSE logs, audit)
```
