# Corral Quickstart

Get `corrald` running and read a device end-to-end in ~10 minutes. Corral
is a read-only fleet monitor since #354: everything a client can do is a
signed READ (`read_tail` recents) or a credential-free GET; there is no
drive/approve/step-up surface anywhere.

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
addresses (#65) — public IPs and `0.0.0.0` are refused. The read plane
(`/snapshot`, `/events`, `/history`, `/issues`) is credential-free on
whatever interface you bind, so go beyond loopback only on a network
(ideally a tailnet) whose devices may all see fleet state.
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
| `--cors-origin` | none | exact browser origin allowed to read the credential-free read plane (repeatable; `*` refused) |

Default config dirs: daemon `$HOME/.config/corral`, client
`$HOME/.config/corral/ui` — override with `CORRAL_CONFIG_DIR` /
`CORRAL_UI_CONFIG_DIR`.

Check it is up:

```sh
curl -s http://127.0.0.1:8474/healthz   # → ok
curl -s http://127.0.0.1:8474/host-key
# → {"algorithm":"X25519","public_key":"...","note":"..."}
```

## 3. Read the fleet

The read plane is credential-free on loopback:

```sh
curl -s http://127.0.0.1:8474/snapshot
```

`{"schema_version":5,"rev":<n>,"generated_at":<ms>,"agents":{...}}` — one
entry per agent with state (herdr RAW vocabulary: working / idle / blocked
/ unknown — no "done" from herdr 0.8.2), waiting_on, capabilities, and
workspace facts. Live updates (resume from a `rev` via `Last-Event-ID`):

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
 "note":"default grants are empty (read-only); the #354 daemon is read-only and grant administration over HTTP was removed",
 "revoked":false}
```

A new device is **read-only**: `grants` is empty, and the only capability
names that can ever be granted are the signed reads (`read_tail`, plus the
daemon-retained `read_diff`). There is no HTTP grant route.

> The desktop client (`corrald-ui`, P4 W2) auto-registers on localhost —
> it reads the daemon's `registration-token` file for the same user, so
> no curl is needed. See the UI section below.

## 5. Grant the read capability (out-of-band)

Grant administration is out-of-band since #354 — the host-admin `POST
/grants` route and `scripts/corrald-grant.sh` are gone. The registry
(`registry.json`, 0600, in the config dir) is loaded once at daemon start:

```sh
# 1. stop corrald
# 2. edit <config-dir>/registry.json — set the device's "grants" array:
#      { ..., "grants": ["read_tail"], "revoked": false, ... }
# 3. start corrald again
```

`read_tail` unlocks the Recent-output (recents) surface — the only signed
drive any client sends. Revoke by setting `"revoked": true` the same way.
Never hand the `admin-token` (or the `registration-token`) to a device.

## 6. Drive (signed read)

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
to the adapter and returns the bounded tail (`lines` + segmented `blocks`
+ `source_rev`; unknown agents are refused at dispatch with `200
{"ok":false,"error":"unknown agent: ..."}` and **are** audited). Naming a
removed capability (`prompt`, `interrupt`, `approve`, `kill`, `attach`,
`start_worktree`, `read_issues`) is refused with `400 unknown_capability`
before the authorizer.

## 7. Read the audit log

The hash-chained log grows only on signed drive dispatches (the reads) —
never on GETs or auth failures. Only the host admin can read it:

```sh
curl -s -H "Authorization: Bearer ***" http://127.0.0.1:8474/audit
```

`{"entries":[...],"head":"<sha256>","valid":true,"note":"..."}` — each
entry carries `prev` + `hash`; `valid` is the chain integrity verdict.

## 8. The desktop UI (`corrald-ui`)

On `main` as the `clients/egui` workspace member:

```sh
cargo run -p corrald-ui --release
```

A dark-dashboard **read-only** fleet board speaking corrald's HTTP/SSE
surface directly: Board (repo groups with raw herdr state chips; blocked
pinned top; rows = name/repo/state/time-in-state/branch + pane ref) and
Settings (connection only). Tapping a row opens the recents v1 live tail
via the signed `read_tail` drive (device keys in the macOS Keychain /
0600 file fallback). There is no Issues tab, no audit pane, no grant
editor, and no drive/approval UI. It **auto-registers on localhost** by
reading the daemon's `registration-token` for the same user, so steps 4
and 5 above are only needed for other clients. The WASM build (`#215`,
mobile layout per `#304`) renders the same read-only board from a bundled
synthetic fixture — no signing, no keyring, no `/drive`.

macOS dev builds need one ad-hoc re-sign to stop Keychain re-prompts —
[OPERATIONS.md](OPERATIONS.md) has the how-to.

## 9. The iOS app (FleetNotifier)

SwiftUI client (`ios/` in this repo, bundle `com.corral.fleetnotifier`) that
speaks the same HTTP/SSE surface: read-only board (repo groups, raw state
chips, blocked pinned top, last-known rows under an offline banner), the
recents v1 live tail via signed `read_tail`, and state-change notifications
(start / blocked / episode-end-to-idle; global on/off in Settings). Real
APNs delivery awaits the host-side provisioning checkpoint; simulator/DEBUG
verification uses the local notification bridge. Release/distribution
builds use only the real registration, SSE, and signed-read path; the
Debug-only seeded demo is not a TestFlight or App Review path. This guide
does not claim physical-device or TestFlight verification.

Registering from the phone is steps 4 and 5 above, with two phone-specific
rules:

- **Include the `https://` scheme in the host.** The app assumes `http://`
  when the scheme is omitted, and ATS refuses plain HTTP to a tailnet
  hostname — see "Remote access from iOS (Tailscale Serve)" in
  OPERATIONS.md.
- **A fresh registration is read-only** (`grants: []`), and registration is
  idempotent per device key — re-registering never upgrades grants. Give
  the phone's key `read_tail` out-of-band (step 5) to unlock recents.

What the app shows:

- **Live board** from the `/events` SSE stream: repo groups; raw herdr
  state chips (working / idle / blocked / unknown); rows show
  name, repo, state, time-in-state, branch, and a small pane ref.
- **Recent output** (`read_tail`): bounded live tail (≤200 lines,
  segmented blocks) via signed `/drive` — live tail only, no load-earlier.
- **Notifications**: on working-entry, blocked, and episode end (active →
  idle, once per episode); tap deep-links to the row with recents open.
- **Settings**: connection + notification pairing only. No action
  controls, no Issues/Terminal/Diff UI, no device/grant admin.

Board never renders but the daemon is healthy? Every stream-layer failure
now surfaces as a dismissible banner instead of a silent spinner (the
`bytes.lines` frame-terminator defect and the nested-`ObservableObject`
render defect are fixed as of build 5). The Troubleshooting table in
OPERATIONS.md has the full checklist.

## Next

- One-shot setup (build + launchd + first run):
  `scripts/setup-corrald.sh` — see [OPERATIONS.md](OPERATIONS.md#one-shot-setup)
- Security model and device lifecycle: [OPERATIONS.md](OPERATIONS.md)
- Hacking on the daemon: [DEVELOPING.md](DEVELOPING.md)
