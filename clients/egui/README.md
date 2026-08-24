# corrald-ui — Corral P4 desktop client (egui/wgpu)

Dark-dashboard fleet board for corrald (`docs/corral/P4-brief.md` W2). One
Rust codebase, macOS + Linux. Speaks corrald's HTTP surface directly
(`docs/corral/P4-conformance.md` is the normative contract).

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
  list with `State · relative age` per card and one collapsed `Idle (N)`
  tail, while the Table keeps its grouped-by-repo default. Empty state
  buckets never render, and a no-match query reports once in the active
  view. The right pane owns the selected agent's detail, drive controls,
  full waiting claim, Recent output, and transcript.
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
  read_tail (bounded 200 lines, on tap only — never prefetch), approve
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
- **Registry view** — `GET /fleet-registry` (non-auth read-only): a
  `Registry` tab fetches the same `fleets.json` source `/issues` uses and
  lists every fleet's repo, local path, orchestrator, workers, pause state,
  and model map including `reasoning_effort`; parse/transport failures render
  prominently with a manual refresh.
- **Issues tab** — the repo-level `GET /issues` browser moved out of Board
  into its own top-level tab, keeping the issue-linked and issue-free
  worktree actions alongside the board's other tabs.

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

## Layout

```
src/
  main.rs      eframe entry (wgpu renderer) + tokio runtime
  app.rs       app state, channels, registration, drive dispatch
  model.rs     wire mirrors of src/core/model.rs + rendering helpers
  protocol.rs  snapshot/SSE/register/admin-grants/audit client + SSE parser
  drive.rs     signed envelope, typing, idempotent retries, step-up
  keys.rs      keypair generation/storage (keychain + 0600 fallback)
  theme.rs     dark-dashboard palette (the only place colors live)
  state.rs     fleet cache, grant ledger, toasts, config records
  ui/          board (master/detail + cards/table), issues, audit,
               registry, register/settings
tests/
  conformance.rs  in-process conformance vs the daemon's own seams
  live.rs         #[ignore] live probe against a real corrald
evidence/       live verification artifacts (screenshot, SSE logs, audit)
```
