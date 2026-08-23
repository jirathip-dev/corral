# Corral Quickstart

Get `corrald` running and drive a device end-to-end in ~10 minutes. Every
command in this guide was run and verified against a throwaway daemon
(2026-08-16).

## Prerequisites

- Rust toolchain — **pinned by `rust-toolchain.toml`** (currently 1.97.1).
  You do not choose a version: rustup reads that file and installs it on the
  first `cargo` command in this repo (#48).
- `herdr` running on the same machine — `corrald` reads the fleet from
  the herdr unix socket (`~/.config/herdr/herdr.sock`). If herdr is down,
  `corrald` still serves HTTP; it just shows no herdr agents (see
  [OPERATIONS.md](OPERATIONS.md#troubleshooting)).

```sh
rustc --version   # prints the pinned version; rustup fetches it if absent
```

## 1. Build

```sh
cargo build --release
```

Result: `target/release/corrald`.

## 2. Run the daemon

`corrald` binds loopback by default (`127.0.0.1:8474`); `--bind` also
accepts tailnet (100.64/10), RFC 1918 private, and IPv6 unique-local
addresses (#65) — public IPs and `0.0.0.0` are refused. Reads are
credential-free on whatever interface you bind, so go beyond loopback only
on a network (ideally a tailnet) whose devices may all see fleet state.
(For the iOS client, don't bind beyond loopback at all — front the
loopback daemon with real TLS via Tailscale Serve, which exposes the
read plane to the same tailnet-wide audience as a tailnet bind: see
"Remote access from iOS (Tailscale Serve)" in docs/OPERATIONS.md.)
Use a throwaway config dir for the first run — the daemon mints
`admin-token`, `host-key`, `registration-token`,
`audit.log` (all `0600` under a `0700` dir) plus a `history/` directory
there. `registry.json` appears on the **first device registration**, not at
startup:

```sh
CORRAL_CONFIG_DIR=/tmp/corral-dev ./target/release/corrald \
  --socket ~/.config/herdr/herdr.sock
```

Flags (`corrald --help`):

| Flag | Default | Meaning |
|---|---|---|
| `--socket`, `-s` | `~/.config/herdr/herdr.sock` | herdr API unix socket |
| `--port`, `-p` | `8474` | HTTP port |
| `--bind`, `-b` | `127.0.0.1` | bind address (loopback / tailnet / private / IPv6 ULA; public and 0.0.0.0 refused) |

Default config dirs: daemon `$HOME/.config/corral`, client
`$HOME/.config/corral/ui` — override with `CORRAL_CONFIG_DIR` /
`CORRAL_UI_CONFIG_DIR`.

Check it is up:

```sh
curl -s http://127.0.0.1:8474/healthz   # → ok
curl -s http://127.0.0.1:8474/host-key
# → {"algorithm":"X25519","public_key":"...","note":"host identity is an
#    X25519 key; device writes are signed with per-device Ed25519 keys"}
```

## 3. Read the fleet

The read plane is credential-free on loopback:

```sh
curl -s http://127.0.0.1:8474/snapshot
```

`{"schema_version":5,"rev":<n>,"generated_at":<ms>,"agents":{...}}` — one
entry per agent with state, waiting_on, capabilities, and workspace
facts. Live updates (resume from a `rev` via `Last-Event-ID`):

```sh
curl -sN http://127.0.0.1:8474/events
```

## 4. Register a device

A device proves itself with an Ed25519 keypair; the registration token
(routing only) gates the enrollment. Generate a dev key and register:

```sh
openssl genpkey -algorithm ED25519 -out /tmp/corral-dev-key.pem
PUBKEY=$(openssl pkey -in /tmp/corral-dev-key.pem -pubout -outform DER | tail -c 32 | base64)
TOKEN=$(cat /tmp/corral-dev/registration-token)
curl -s -X POST http://127.0.0.1:8474/register \
  -H 'Content-Type: application/json' \
  -d "{\"token\":\"$TOKEN\",\"public_key\":\"$PUBKEY\"}"
```

Result (verified):

```json
{"algorithm":"Ed25519","expiry_ts":...,"grants":[],
 "key_id":"dev_0b1a066ae2c26abe4830241d68ebfc33",
 "note":"default grants are empty (read-only): drive capabilities are promoted by the host",
 "revoked":false}
```

A new device is **read-only**: `grants` is empty. The read plane needs no
grants; every drive capability must be granted by the host.

> The desktop client (`corrald-ui`, P4 W2) auto-registers on localhost —
> it reads the daemon's `registration-token` file for the same user, so
> no curl is needed. See the UI section below.

## 5. Grant a capability

The host promotes capabilities with its `admin-token` (read it from the
config dir; never hand it to devices). The desktop UI's **Settings →
Device grants** provides the same action with the same host-admin boundary:

```sh
ADMIN=$(cat /tmp/corral-dev/admin-token)
curl -s -X POST http://127.0.0.1:8474/grants \
  -H 'Content-Type: application/json' -H "Authorization: Bearer $ADMIN" \
  -d '{"action":"set_grants","key_id":"dev_0b1a066ae2c26abe4830241d68ebfc33","grants":["read_tail"]}'
# → {"key_id":"dev_...","ok":true}
```

Revoke anytime with `{"action":"revoke","key_id":"...","revoked":true}`.

## 6. Drive (signed)

A drive command is the envelope signed with the device key. The signature
covers the exact canonical JSON bytes (fixed field order). Sign with the
dev key and POST it:

```sh
ENV='{"request_id":"smoke-1","capability":"read_tail","target":"<agent_id>","payload":{"kind":"read_tail","lines":null}}'
printf '%s' "$ENV" > /tmp/env.json
openssl pkeyutl -sign -inkey /tmp/corral-dev-key.pem -rawin -in /tmp/env.json -out /tmp/env.sig
curl -s -w '\nHTTP %{http_code}\n' -X POST http://127.0.0.1:8474/drive \
  -H 'Content-Type: application/json' \
  -d "{\"key_id\":\"dev_...\",\"signature\":\"$(base64 < /tmp/env.sig)\",\"envelope\":$ENV}"
```

Without the grant the daemon refuses (verified): `403`
`{"kind":"not_granted","message":"capability not granted: read_tail",...}`
and the audit log stays untouched. With the grant, the command dispatches
to the adapter (unknown agents are refused at dispatch with `200
{"ok":false,"error":"unknown agent: ..."}` and **are** audited).

## 7. Read the audit log

The hash-chained log grows only on drive writes — never on reads or auth
failures. Only the host admin can read it:

```sh
curl -s -H "Authorization: Bearer $ADMIN" http://127.0.0.1:8474/audit
```

`{"entries":[...],"head":"<sha256>","valid":true,"note":"..."}` — each
entry carries `prev` + `hash`; `valid` is the chain integrity verdict.

## 8. The desktop UI (`corrald-ui`)

On `main` as the `clients/egui` workspace member:

```sh
cargo run -p corrald-ui --release
```

A dark-dashboard fleet board speaking corrald's HTTP/SSE surface directly,
with signed drive controls, keychain-stored device keys (macOS), a host
audit view, and the Settings device-grant editor. It **auto-registers on localhost**
by reading the daemon's `registration-token` for the same user, so steps 4
and 5 above are only needed for other clients.

macOS dev builds need one ad-hoc re-sign to stop Keychain re-prompts —
[OPERATIONS.md](OPERATIONS.md) has the how-to.

## 9. The iOS app (FleetNotifier)

SwiftUI client (`ios/` in this repo, bundle `com.corral.fleetnotifier`) that
speaks the same HTTP/SSE surface as the desktop UI: live fleet board,
per-agent workspace lines, and signed drive controls. Release/distribution
builds use only the real registration, SSE, and signed-drive path; the
Debug-only seeded demo is not a TestFlight or App Review path. This guide does
not claim physical-device or TestFlight verification.

Registering from the phone is steps 4 and 5 above, with two phone-specific
rules:

- **Include the `https://` scheme in the host.** The app assumes `http://`
  when the scheme is omitted, and ATS refuses plain HTTP to a tailnet
  hostname — see "Remote access from iOS (Tailscale Serve)" in
  OPERATIONS.md.
- **A fresh registration is read-only** (`grants: []`), and registration is
  idempotent per device key — re-registering never upgrades grants. Promote
  the phone's key with step 5:

```sh
scripts/corrald-grant.sh --key <phone-key-id> --caps read_tail,prompt,interrupt,approve
```

The baseline phone promotion intentionally stops at the safe read/reply
set. Add `kill,attach` explicitly only when the host has approved those
controls; `read_tail` also unlocks the iOS Full chat view.

What the app shows:

- **Live board** from the `/events` SSE stream: agent rows ordered
  blocked > working > done > idle, each with state, title/session,
  repo·branch·worktree, issue chips, CI glyph, tool.
- **Tail 200** (`read_tail`): bounded 200-line tail via signed `/drive`.
- **Full chat** (`read_tail`): signed, paged `/transcript` view, separate
  from the bounded Tail 200 view.
- **Prompt** (`prompt`): free-text prompt to an agent.
- **Interrupt** (`interrupt`), **Kill** (`kill`), and **Attach**
  (`attach`): signed write controls; Kill uses Face ID step-up.
- **Approve / Deny / Continue** (`approve`): canned replies to a waiting
  agent, including from the lock-screen notification.
- Rows without the matching grant render without those controls.

Board never renders but the daemon is healthy? Every stream-layer failure
now surfaces as a dismissible banner instead of a silent spinner (the
`bytes.lines` frame-terminator defect and the nested-`ObservableObject`
render defect are fixed as of build 5). The Troubleshooting table in
OPERATIONS.md has the full checklist.

## Next

- One-shot setup (build + launchd + first run):
  `scripts/setup-corrald.sh` — see [OPERATIONS.md](OPERATIONS.md#one-shot-setup)
- Security model and device lifecycle: [OPERATIONS.md](OPERATIONS.md)
- Wire contract for client authors: [corral/P4-conformance.md](corral/P4-conformance.md)
- Hacking on the daemon: [DEVELOPING.md](DEVELOPING.md)
