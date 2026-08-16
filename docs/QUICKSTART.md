# Corral Quickstart

Get `corrald` running and drive a device end-to-end in ~10 minutes. Every
command in this guide was run and verified against a throwaway daemon
(2026-08-16).

## Prerequisites

- Rust toolchain (edition 2024 — `rustc` ≥ 1.85; verified with 1.97.1).
- `herdr` running on the same machine — `corrald` reads the fleet from
  the herdr unix socket (`~/.config/herdr/herdr.sock`). If herdr is down,
  `corrald` still serves HTTP; it just shows no herdr agents (see
  [OPERATIONS.md](OPERATIONS.md#troubleshooting)).

```sh
rustc --version   # ≥ 1.85
```

## 1. Build

```sh
cargo build --release
```

Result: `target/release/corrald`.

## 2. Run the daemon

`corrald` binds loopback only (default `127.0.0.1:8474`) and refuses any
routable `--bind`. Use a throwaway config dir for the first run — the
daemon mints `admin-token`, `host-key`, `registration-token`,
`registry.json`, `audit.log` (all `0600` under a `0700` dir) there:

```sh
CORRAL_CONFIG_DIR=/tmp/corral-dev ./target/release/corrald \
  --socket ~/.config/herdr/herdr.sock
```

Flags (`corrald --help`):

| Flag | Default | Meaning |
|---|---|---|
| `--socket`, `-s` | `~/.config/herdr/herdr.sock` | herdr API unix socket |
| `--port`, `-p` | `8474` | HTTP port |
| `--bind`, `-b` | `127.0.0.1` | bind address (loopback only) |

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

`{"schema_version":3,"rev":<n>,"generated_at":<ms>,"agents":{...}}` — one
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
config dir; never hand it to devices):

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

Not on `main` yet: P4 W2, branch `w2/egui-desktop`, unmerged. Build/run
instructions live in the branch's own README (read-only):

```sh
git show origin/w2/egui-desktop:clients/egui/README.md
```

The UI is a dark-dashboard fleet board speaking corrald's HTTP/SSE
surface directly, with signed drive controls, keychain-stored device keys
(macOS), and an audit view. macOS dev builds need one ad-hoc re-sign to
stop Keychain re-prompts — [OPERATIONS.md](docs/OPERATIONS.md) has the
how-to.

## Next

- Security model and device lifecycle: [docs/OPERATIONS.md](docs/OPERATIONS.md)
- Wire contract for client authors: [docs/corral/P4-conformance.md](docs/corral/P4-conformance.md)
- Hacking on the daemon: [docs/DEVELOPING.md](docs/DEVELOPING.md)
