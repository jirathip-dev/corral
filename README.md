# Corral

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](LICENSE-MIT)

**See every agent. Steer the fleet.**

**Corral is the control plane for your herdr fleet.** [Install
herdr](https://github.com/herdrdev/herdr) first: it is the runtime that
spawns and supervises agents in panes and worktrees. Corral gives that fleet
one live board, signed remote control from your phone (loopback by default,
or over your tailnet).

At the harness layer, Corral is **harness-agnostic**: Claude Code, Codex CLI,
OpenCode, and other agent harnesses become the same canonical record, so the
board and signed drive plane work the same regardless of which harness an
agent runs on.

Corral reads every worktree / agent / PR / CI fact into a snapshot read
model served over HTTP + SSE (loopback by default; tailnet/private
interfaces allowlisted, never public), and lets a registered device drive
the agents with typed, signed commands — prompt, interrupt, approve,
read_tail, kill, attach. The daemon is `corrald`, with a desktop fleet
board (`corrald-ui`, egui) and an iOS notifier app alongside it.

## Requirements

- **Rust toolchain** (pinned by `rust-toolchain.toml`).
- **herdr**, the runtime that supervises the agents in panes/worktrees. The
  herdr adapter feeds Corral's live agent state (see
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#stack-terminology-model--harness--runtime--control-plane)
  for the model→harness→runtime→control-plane layering). Without herdr,
  `corrald` still serves HTTP but shows no agents.

```
herdr socket ─┐                  ┌─ GET /snapshot, GET /events (SSE, Last-Event-ID resume)
git watcher ──┤ → integrator →   ├─ GET /history (event ring, ?since= &limit=)
                                 ├─ POST /register, /step-up, /grants, GET /host-key, /audit
                                 ├─ POST /device-token (APNs registration, signed)
                                 └─ POST /drive (Ed25519-signed, capability-gated)
```

## Install from release (macOS)

No Rust toolchain is needed to install a tagged macOS release. The installer
downloads the checksummed bundle, verifies its SHA-256 before touching the
machine, installs `corrald` under launchd, and stages the egui board:

```sh
bash <(curl -fsSL https://raw.githubusercontent.com/jirathip-dev/corral/main/scripts/install-corral.sh)
```

From a checkout, the same script can be run directly:

```sh
bash scripts/install-corral.sh
bash scripts/install-corral.sh --release v0.1.0
```

Release/tag resolution uses `gh`; a direct artifact URL bypasses it:

```sh
RELEASE_URL=https://example.com/corral-vX-macos.tar.gz \
  bash <(curl -fsSL https://raw.githubusercontent.com/jirathip-dev/corral/main/scripts/install-corral.sh)
```

Uninstall removes the launchd agents and the staged app, and keeps your config
and keys:

```sh
bash scripts/install-corral.sh --uninstall
```

The from-source setup below remains the primary development path.

## Herdr plugin

The root [`herdr-plugin.toml`](herdr-plugin.toml) provides real `setup` and
read-only `status` actions. From a clean checkout, link it locally and list
the actions:

```sh
herdr plugin link .
herdr plugin action list --plugin corral.control-plane
```

`setup` invokes `scripts/setup-corrald.sh`. `status` checks `/healthz`, then
prints only the agent count and snapshot cursors from `/snapshot`; it never
prints the snapshot body. These actions do not need a Herdr RPC callback. If a
future Corral action needs to call Herdr, use the injected `HERDR_BIN_PATH`
provided by Herdr rather than assuming `herdr` is on `PATH`.

## Quickstart

```sh
cargo build --release
CORRAL_CONFIG_DIR=/tmp/corral-dev ./target/release/corrald \
  --socket ~/.config/herdr/herdr.sock
curl -s http://127.0.0.1:8474/snapshot   # the fleet, JSON
```

Register a device (routing token + device public key; defaults to
read-only), then grant it a capability and check the audit log:

```sh
# The public key must be a real Ed25519 point — 32 random bytes are refused.
openssl genpkey -algorithm ED25519 -out /tmp/corral-dev-key.pem
PUBKEY=$(openssl pkey -in /tmp/corral-dev-key.pem -pubout -outform DER | tail -c 32 | base64)
TOKEN=$(cat /tmp/corral-dev/registration-token)

curl -s -X POST http://127.0.0.1:8474/register \
  -H 'Content-Type: application/json' \
  -d "{\"token\":\"$TOKEN\",\"public_key\":\"$PUBKEY\"}"
# → { "key_id": "dev_...", "grants": [], ... }

ADMIN=$(cat /tmp/corral-dev/admin-token)
curl -s -X POST http://127.0.0.1:8474/grants \
  -H 'Content-Type: application/json' -H "Authorization: Bearer $ADMIN" \
  -d '{"action":"set_grants","key_id":"dev_...","grants":["read_tail"]}'
curl -s -H "Authorization: Bearer $ADMIN" http://127.0.0.1:8474/audit
```

`Content-Type: application/json` is required on both POSTs — without it the
daemon refuses the body outright.

Every command above was run against a throwaway daemon and verified —
full walkthrough: [docs/QUICKSTART.md](docs/QUICKSTART.md).

## Security posture

- **Loopback by default, public refused** — `corrald` binds `127.0.0.1`
  by default; `--bind` also accepts tailnet (100.64/10), RFC 1918 private,
  and IPv6 unique-local addresses (#65) — public IPs and `0.0.0.0` are
  hard refusals. Writes are device-signed on every interface; the read
  plane (`/healthz`, `/snapshot`, `/events`, `/history`) is
  credential-free, so its boundary on a non-loopback bind — or a
  loopback bind fronted by Tailscale Serve — is the network itself:
  prefer a tailnet (WireGuard device auth) over a plain LAN.
- **Signed writes, default deny** — every `POST /drive` carries an
  Ed25519 device signature; a registered device has zero grants until the
  host promotes capabilities. No auto-approve.
- **Claim-based approvals** — an approval reply must echo the exact
  `prompt_hash` of the live prompt, killing the approve-the-wrong-question
  race.
- **Step-up for destructive payloads** — `rm -rf`, `push --force`,
  `curl | sh`, `~/.aws`, `~/.ssh`, `.env` need a 5-minute single-use
  biometric token. All writes land in a hash-chained audit log.

## Beyond the board

- **Event history + daily digest** — every status transition is appended at
  the store-apply choke point to a rotating JSONL ring. Query a window with
  `GET /history?since=<epoch-ms>&limit=<n>`, or take a per-agent daily
  summary offline with `corrald digest`. See
  [docs/OPERATIONS.md](docs/OPERATIONS.md).
- **Fleet registry** — `corrald fleet list|check` read, and `corrald fleet
  add|remove|pause|resume|models` atomically rewrite, the `fleets.json`
  registry (`$CORRAL_FLEETS_PATH`) that describes each fleet's repo, worktree
  dir, workers and per-role models. Corral reads a tolerant subset, so
  fleet-operations fields such as `reasoning_effort` and `admit` are accepted
  and preserved through rewrites; typos in fields Corral owns still fail
  loudly. `fleet switch|reap|prune` add the auth-gated process/worktree ops
  half. Full schema, safety gates and exit codes in
  `docs/corral/G35-registry.md`.
- **APNs notifier (iOS)** — blocked/done transitions push to a registered
  device, with canned lock-screen replies bound to the prompt's
  `prompt_hash` and a biometric step-up on destructive payloads. Armed with
  `CORRAL_APNS_*`; unconfigured, the daemon runs exactly as before.
  **Not device-verified yet** — see [Status](#status).

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --release
cargo test
cargo test -p corrald-client -- --ignored   # R1–R10 conformance vs a real corrald
cargo test -p corrald-ui --test live -- --ignored   # egui live tests
```

The workspace is **non-virtual** with
`default-members = [".", "crates/corrald-client", "clients/egui"]`, so a
bare `cargo clippy`/`build`/`test` at the root covers all three crates.

Module map, conventions (zero polling in the herdr adapter, additive-only
schema), and how to add a capability: [docs/DEVELOPING.md](docs/DEVELOPING.md).

## Development status

On `main`: P1–P3 (read model, data planes, drive plane, device-keypair
auth), P4 (shared `corrald-client` crate with R1–R10 conformance, the
`corrald-ui` desktop board, the iOS client), the read_tail content
round-trip, repo grouping, PR/issue binding, and the event-history ring +
daily digest.

Landing alongside this documentation: hosted CI and the fleet registry
commands.

**Honest verification status of the APNs notifier:** the daemon and iOS code
are written and unit-tested, but every test mocks the provider seam. Real
APNs delivery, the lock-screen reply round-trip, and the Face ID step-up
have **not** been exercised on hardware — they need a TestFlight build on a
real device. Treat the notifier as unproven end to end until that happens.

## Docs

- [docs/QUICKSTART.md](docs/QUICKSTART.md) — prerequisites, run the daemon, register a device, read the fleet
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — read model, signed drive plane, security boundaries
- [docs/OPERATIONS.md](docs/OPERATIONS.md) — device lifecycle, grants, remote access from iOS (Tailscale Serve), macOS keychain how-to, audit log, troubleshooting
- [docs/DEVELOPING.md](docs/DEVELOPING.md) — workspace layout, module map, quality gates, how to add a capability
- [docs/corral/visibility-topic-flip-checklist.md](docs/corral/visibility-topic-flip-checklist.md) — human-only public visibility and marketplace flip checklist
- [docs/corral/P4-conformance.md](docs/corral/P4-conformance.md) — normative wire contract, scenarios R1–R10
- [docs/corral/P1-brief.md](docs/corral/P1-brief.md), [P2](docs/corral/P2-brief.md), [P3](docs/corral/P3-brief.md) — historical phase briefs
- [docs/corral/P4-brief.md](docs/corral/P4-brief.md) — current phase brief (client stack)

> The P1–P4 briefs are historical internal process documents; they document
> *how* the design evolved, not how to use the current system.

## Status

**Pre-1.0, macOS-first.** The daemon and egui client run on macOS (and the
daemon on Linux); the iOS notifier requires an Apple developer account for
TestFlight. The launchd setup script, Keychain integration, and iOS app are
macOS stories. Linux support is partial (the `keyring` crate covers it) but
not the primary target.

## License

Dual-licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE).
