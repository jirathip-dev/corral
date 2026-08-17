# Corral

Corral is the control plane for the herdr agent fleet: it reads every
worktree / agent / PR / CI fact into a snapshot read model served over
loopback HTTP + SSE, and lets a registered device drive the agents with
typed, signed commands — prompt, interrupt, approve, read_tail, kill,
attach. The daemon is `corrald`; a desktop fleet board (`corrald-ui`) is
in flight on branch `w2/egui-desktop`.

```
herdr socket ─┐                  ┌─ GET /snapshot, GET /events (SSE, Last-Event-ID resume)
git watcher ──┤ → integrator →   ├─ GET /history (event ring, ?since= &limit=)
gh (GraphQL) ─┘   read model     ├─ POST /register, /step-up, /grants, GET /host-key, /audit
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

## Development

```sh
cargo build --release
cargo clippy --all-targets -- -D warnings
cargo test
cargo test -p corrald-client -- --ignored   # R1–R10 conformance vs a real corrald
```

Module map, conventions (zero polling in the herdr adapter, additive-only
schema), and how to add a capability: [docs/DEVELOPING.md](docs/DEVELOPING.md).

## Status

On `main`: P1–P3 (read model, data planes, drive plane, device-keypair
auth) + P4 W1 (shared `corrald-client` crate with R1–R10 conformance).
In flight: P4 W2 desktop UI (`corrald-ui`, branch `w2/egui-desktop`,
unmerged), P4 W3 iOS app (branch `w3/ios-fleet-notifier`).

## Docs

- [docs/QUICKSTART.md](docs/QUICKSTART.md) — prerequisites, run the daemon, register a device, read the fleet
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — read model, signed drive plane, security boundaries
- [docs/OPERATIONS.md](docs/OPERATIONS.md) — device lifecycle, grants, macOS keychain how-to, audit log, troubleshooting
- [docs/DEVELOPING.md](docs/DEVELOPING.md) — workspace layout, module map, quality gates, how to add a capability
- [docs/corral/P4-conformance.md](docs/corral/P4-conformance.md) — normative wire contract, scenarios R1–R10
- [docs/corral/P1-brief.md](docs/corral/P1-brief.md), [P2](docs/corral/P2-brief.md), [P3](docs/corral/P3-brief.md) — historical phase briefs
- [docs/corral/P4-brief.md](docs/corral/P4-brief.md) — current phase brief (client stack)
