# Corral

Corral is the control plane for the herdr agent fleet: it reads every
worktree / agent / PR / CI fact into a snapshot read model served over
loopback HTTP + SSE, and lets a registered device drive the agents with
typed, signed commands — prompt, interrupt, approve, read_tail, kill,
attach. The daemon is `corrald`, with a desktop fleet board (`corrald-ui`,
egui) and an iOS notifier app alongside it.

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
TOKEN=$(cat /tmp/corral-dev/registration-token)
curl -s -X POST http://127.0.0.1:8474/register \
  -d "{\"token\":\"$TOKEN\",\"public_key\":\"<base64 32-byte Ed25519 pubkey>\"}"
# → { "key_id": "dev_...", "grants": [], ... }
curl -s -H "Authorization: Bearer $(cat /tmp/corral-dev/admin-token)" \
  -X POST http://127.0.0.1:8474/grants \
  -d '{"action":"set_grants","key_id":"dev_...","grants":["read_tail"]}'
curl -s -H "Authorization: Bearer $(cat /tmp/corral-dev/admin-token)" \
  http://127.0.0.1:8474/audit
```

Every command above was run against a throwaway daemon and verified —
full walkthrough: [docs/QUICKSTART.md](docs/QUICKSTART.md).

## Security posture

- **Loopback only** — `corrald` refuses to bind any routable interface.
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
- **Cost / usage meter** — per-provider spend (opencode, claude, codex) over
  rolling 5h / weekly / monthly windows on `GET /cost`, with a board COST
  column and per-provider dashboard tiles.
  **The plan caps and the claude/codex pricing table are placeholders** —
  real subscription limits have not been supplied, the API marks every
  synthetic cap `cap_is_placeholder: true`, and the UI prefixes those
  percentages with `~`. Set `CORRAL_COST_CAP_*` before trusting the alert.
- **Fleet registry** — `corrald fleet list` / `corrald fleet check` read the
  `fleets.json` registry (`$CORRAL_FLEETS_PATH`) that describes each fleet's
  repo, worktree dir, workers and per-role models. Read-only in this phase;
  see [docs/corral/G35-registry.md](docs/corral/G35-registry.md).
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

## Status

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

## Docs

- [docs/QUICKSTART.md](docs/QUICKSTART.md) — prerequisites, run the daemon, register a device, read the fleet
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — read model, signed drive plane, security boundaries
- [docs/OPERATIONS.md](docs/OPERATIONS.md) — device lifecycle, grants, macOS keychain how-to, audit log, troubleshooting
- [docs/DEVELOPING.md](docs/DEVELOPING.md) — workspace layout, module map, quality gates, how to add a capability
- [docs/corral/P4-conformance.md](docs/corral/P4-conformance.md) — normative wire contract, scenarios R1–R10
- [docs/corral/P1-brief.md](docs/corral/P1-brief.md), [P2](docs/corral/P2-brief.md), [P3](docs/corral/P3-brief.md) — historical phase briefs
- [docs/corral/P4-brief.md](docs/corral/P4-brief.md) — current phase brief (client stack)
