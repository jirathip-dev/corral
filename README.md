# Corral

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](LICENSE-MIT)

**Corral is a control plane for fleets of AI coding agents.** If you run
several coding agents (Claude Code, Codex CLI, OpenCode) in git worktrees,
each with its own terminal, you need: one live board of what every agent is
doing, signed remote control from your phone (devices connect over
loopback today; tailnet binds land with #65), and cost visibility before a
provider bill surprises you. Corral gives you that.

Corral reads every worktree / agent / PR / CI fact into a snapshot read
model served over loopback HTTP + SSE, and lets a registered device drive
the agents with typed, signed commands — prompt, interrupt, approve,
read_tail, kill, attach. The daemon is `corrald`, with a desktop fleet
board (`corrald-ui`, egui) and an iOS notifier app alongside it.

**Runtime note:** agents are currently supervised by
[herdr](https://github.com/herdrdev/herdr) (the runtime that spawns
them in panes/worktrees). Corral's core model, drive plane, and HTTP
surface are runtime-neutral — see
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#stack-terminology-model--harness--runtime--control-plane)
for the model→harness→runtime→control-plane layering.

```
herdr socket ─┐                  ┌─ GET /snapshot, GET /events (SSE, Last-Event-ID resume)
git watcher ──┤ → integrator →   ├─ GET /history (event ring, ?since= &limit=)
gh (GraphQL) ─┘   read model     ├─ GET /cost   (per-provider spend, 5h/weekly/monthly)
                                 ├─ POST /register, /step-up, /grants, GET /host-key, /audit
                                 ├─ POST /device-token (APNs registration, signed)
                                 └─ POST /drive (Ed25519-signed, capability-gated)
```

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
  by default and refuses public/routable binds. Private/tailnet
  (100.x/10.x) binds are planned via #65.
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
- **Cost / usage meter** — per-provider spend over rolling 5h / weekly /
  monthly windows on `GET /cost`, with a board COST column and per-provider
  dashboard tiles. **The plan caps and the claude/codex pricing table are
  placeholders** until real subscription limits are supplied — see
  [Cost meter (G34)](#cost-meter-g34) below.
- **Fleet registry** — `corrald fleet list|check` read, and `corrald fleet
  add|remove` atomically rewrite, the `fleets.json` registry
  (`$CORRAL_FLEETS_PATH`) that describes each fleet's repo, worktree dir,
  workers and per-role models. Full schema and exit codes in
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

Landing alongside this documentation: hosted CI, the cost/usage meter, and
the fleet registry commands.

**Honest verification status of the APNs notifier:** the daemon and iOS code
are written and unit-tested, but every test mocks the provider seam. Real
APNs delivery, the lock-screen reply round-trip, and the Face ID step-up
have **not** been exercised on hardware — they need a TestFlight build on a
real device. Treat the notifier as unproven end to end until that happens.

## Cost meter (G34)

`GET /cost` reports per-provider (opencode/claude/codex) USD spend and %
of a configured cap over rolling 5h/weekly/monthly windows, read-only and
bounded from each provider's own session store — see `src/cost/mod.rs` for
the data flow and `docs/corral/DECISIONS.md` (D34) for the design writeup.
**The default caps are invented.** Real opencode-go / claude / codex
subscription limits have not been supplied, and the claude/codex pricing
table is a documented guess (neither provider exposes a cost field — only
tokens). So every unset cap is a placeholder: the API marks it
`cap_is_placeholder: true`, the desktop tiles prefix such percentages with
`~`, and a provider with no session store renders "no store" rather than
`$0.00` — which would read as "nothing spent", the opposite of the truth.
**Do not act on a percentage until you have set the real cap:**

```sh
CORRAL_COST_CAP_<PROVIDER>_<WINDOW>_USD   # e.g. CORRAL_COST_CAP_CLAUDE_WEEKLY_USD=100
CORRAL_COST_WARN_THRESHOLD_PCT=70         # window status -> warning at/above
CORRAL_COST_ALERT_THRESHOLD_PCT=90        # window status -> problem at/above
```

See `src/cost/config.rs` for the full variable list. The board's per-agent
cost column and a `tracing::warn!` watchdog before any window nears
exhaustion both come from the same meter.

## Docs

- [docs/QUICKSTART.md](docs/QUICKSTART.md) — prerequisites, run the daemon, register a device, read the fleet
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — read model, signed drive plane, security boundaries
- [docs/OPERATIONS.md](docs/OPERATIONS.md) — device lifecycle, grants, macOS keychain how-to, audit log, troubleshooting
- [docs/DEVELOPING.md](docs/DEVELOPING.md) — workspace layout, module map, quality gates, how to add a capability
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
