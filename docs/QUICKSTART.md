# Corral Quickstart

Get `corrald` running and read a device end-to-end in ~10 minutes. Corral
is a read-only fleet monitor since #354: everything a client can do is a
signed READ (`read_tail` recents) or a credential-free GET; there is no
drive/approve/step-up surface anywhere.

## Prerequisites

The release install (steps 1-2, the normal path) needs **no Rust toolchain**:

- **macOS**, or **x86_64 Linux with a systemd user manager** — a desktop
  login session provides one; headless/SSH hosts enable it once with
  `loginctl enable-linger "$USER"` (see [LINUX.md](LINUX.md)). Any other
  platform or architecture has no published bundle, see the artifact table
  in step 1.
- `curl` and `tar`. `gh` is only needed to resolve a release; passing
  `--url <bundle-url>` installs an explicit bundle without `gh`.
- `herdr` running on the same machine — `corrald` reads the fleet from
  the herdr unix socket (`~/.config/herdr/herdr.sock`). If herdr is down,
  `corrald` still serves HTTP; it just shows no herdr agents (see
  [OPERATIONS.md](OPERATIONS.md#troubleshooting)).

Building from source instead is a separate, developer path — step 9.

## 1. Install the prebuilt daemon (checksummed release)

From a checkout:

```sh
bash scripts/install-corral.sh                    # latest release
bash scripts/install-corral.sh --release v0.4.2   # or pin a release tag
```

Without a checkout (same script, fetched from the repository):

```sh
bash <(curl -fsSL https://raw.githubusercontent.com/jirathip-dev/corral/main/scripts/install-corral.sh)
```

What the installer does, in order:

1. Resolves the release bundle for your platform (or uses the `--url` you
   gave) and downloads it with its published `.sha256`, refusing a
   mismatched or malformed checksum **before** creating any install state.
2. Stages and validates the bundle, then swaps it into
   `~/.local/share/corral/release` (`release.previous` holds the outgoing
   version until the new one health-checks, then it is removed).
3. Installs and starts the per-user service — `com.corral.corrald` under
   launchd on macOS (KeepAlive), `corrald.service` under `systemd --user`
   on Linux — running the daemon on loopback `127.0.0.1:8474` against
   `~/.config/herdr/herdr.sock`.
4. Health-checks `http://127.0.0.1:8474/healthz`; on failure the installer
   exits non-zero and the release directory is rolled back (removed on a
   fresh install) — on Linux the service is stopped first.

Your keys and device registry live in `$CORRAL_CONFIG_DIR` (default
`~/.config/corral`, `0600` files under a `0700` dir) and are **never
touched** by install, update, or uninstall.

Published release artifacts (read back from the actual releases):

| Host | Bundle in the release |
|---|---|
| macOS | `corral-<tag>-macos.tar.gz` + `.sha256` |
| Linux x86_64 | `corral-<tag>-linux-x86_64.tar.gz` + `.sha256` |

`v0.4.2` is the first release that publishes the Linux bundle; earlier tags
(`v0.3.0` and older) are macOS-only — on Linux the installer fails for those
tags with `release v0.3.0 is missing corral-v0.3.0-linux-x86_64.tar.gz or
corral-v0.3.0-linux-x86_64.tar.gz.sha256`.
Any other platform or architecture (Windows, Linux aarch64, …) has no
published bundle: the installer refuses those hosts up front (exit 2) and
never relabels another platform's artifact or disguises a source build as
prebuilt. Linux runbook and guardrails: [LINUX.md](LINUX.md).

Installer flags (`scripts/install-corral.sh`): `--release <tag>` /
`RELEASE_TAG` and `--url <bundle-url>` / `RELEASE_URL` are mutually
exclusive; `--bind`/`--port` change the service's loopback defaults
(advanced — the default setup passes no flags); `--uninstall` removes the
service and release files but keeps `$CORRAL_CONFIG_DIR`; `--self-test`
runs the platform-neutral path-safety self-test.

## 2. Verify the managed service

```sh
curl -s http://127.0.0.1:8474/healthz    # → ok
curl -s http://127.0.0.1:8474/host-key
# → {"algorithm":"X25519","public_key":"...","note":"..."}
```

macOS: `launchctl print gui/$(id -u)/com.corral.corrald` (logs:
`$CORRAL_CONFIG_DIR/corrald-launchd.log`). Linux:
`systemctl --user status corrald.service` (logs:
`journalctl --user -u corrald`).

## 3. Read the fleet

The read plane is credential-free on loopback:

```sh
curl -s http://127.0.0.1:8474/snapshot
```

`{"schema_version":5,"rev":<n>,"generated_at":<ms>,"agents":{...}}` — one
entry per agent with state (herdr RAW vocabulary: working / idle / blocked
/ unknown; the 0.8.2 wire can also carry `done`, ranked and rendered with
`idle` on the boards as the finished state), waiting_on, capabilities, and
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

## 5. Grant the read capability (host-approved enrollment; out-of-band fallback)

Since #485 the host owner grants `read_tail` through a **local-only
pairing flow** — no registry edits, no restart. Owner operations live on a
unix socket (`<config-dir>/owner.sock`, mode 0600 in the 0700 config dir,
peer-credential checked) and are deliberately **never HTTP routes**.

1. **Owner mints** a short-lived single-use code (the reply carries the
   versioned v1 QR payload):

   ```sh
   printf '%s' '{"op":"mint","endpoint":"https://<host>.<tailnet>.ts.net"}' \
     | nc -U "$HOME/.config/corral/owner.sock"
   ```

2. **Device redeems** the payload's `code` with its own Ed25519 public key
   (this creates a *pending request* — redeem alone grants nothing and can
   never make a record):

   ```sh
   curl -s -X POST http://127.0.0.1:8474/enroll/redeem \
     -H 'Content-Type: application/json' \
     -d "{\"v\":1,\"code\":\"<code>\",\"public_key\":\"$PUBKEY\"}"
   ```

3. **Owner checks and approves** — the pending listing shows the full
   `key_id`; approve only the key your device shows (the `name` label is
   device-supplied and unverified):

   ```sh
   printf '%s' '{"op":"pending"}' | nc -U "$HOME/.config/corral/owner.sock"
   printf '%s' '{"op":"approve","enrollment_id":"enr_..."}' \
     | nc -U "$HOME/.config/corral/owner.sock"
   ```

The approved record carries `read_tail` **exactly** (never `read_diff`,
never control), takes effect live, and is committed to disk *before* it is
published. A revoked device returns only through a fresh explicit
owner-approved pairing like the one above — never automatically:

```sh
printf '%s' '{"op":"revoke","key_id":"dev_..."}' \
  | nc -U "$HOME/.config/corral/owner.sock"
```

The device polls `POST /enroll/status` (which never answers 409, so a lost
redeem response cannot brick pairing) and may fall back to its signed
`POST /grants-read` if the daemon restarted mid-pairing. Full contract:
[docs/enrollment-v1.md](enrollment-v1.md).

**Fallback (unchanged):** the out-of-band registry edit remains valid for
recovery — stop corrald, edit `<config-dir>/registry.json`'s `"grants"`
array to `["read_tail"]` (or `"revoked": true`), start corrald again.
Never hand the `admin-token` (or the `registration-token`) to a device.

The base setup (board, live states, recents withheld) needs no grant at
all; only the recents surface does.

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

## 8. The iOS app (FleetNotifier)

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

Notifications are optional in normal setup: the board and recents work
without granting notification permission and without any APNs credentials.
The app itself raises the OS notification-permission prompt on the first
live board today — explicit opt-in, no-prompt behavior is follow-up
[#487](https://github.com/jirathip-dev/corral/issues/487); host-approved QR
pairing is follow-up [#486](https://github.com/jirathip-dev/corral/issues/486).

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

## 9. Developer: build and run from source

Rust toolchain — **pinned by `rust-toolchain.toml`** (currently 1.97.1).
You do not choose a version: rustup reads that file and installs it on the
first `cargo` command in this repo (#48).

```sh
rustc --version   # prints the pinned version; rustup fetches it if absent
```

Build:

```sh
cargo build --release
```

Result: `target/release/corrald`.

Run the daemon in the foreground:

```sh
CORRAL_CONFIG_DIR=/tmp/corral-dev ./target/release/corrald \
  --socket ~/.config/herdr/herdr.sock
```

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
startup.

Flags (`corrald --help`):

| Flag | Default | Meaning |
|---|---|---|
| `--socket`, `-s` | `~/.config/herdr/herdr.sock` | herdr API unix socket |
| `--port`, `-p` | `8474` | HTTP port |
| `--bind`, `-b` | `127.0.0.1` | bind address (loopback / tailnet / private / IPv6 ULA; public and 0.0.0.0 refused) |
| `--cors-origin` | none | exact browser origin allowed to read the credential-free read plane (repeatable; `*` refused) |

Default config dir: daemon `$HOME/.config/corral` — override with
`CORRAL_CONFIG_DIR`.

To install a source build as the managed launchd service (no release
bundle involved), `scripts/setup-corrald.sh` builds and installs the
`com.corral.corrald` agent (KeepAlive, port 8474) and is idempotent:

```sh
bash scripts/setup-corrald.sh
```

Workspace layout, quality gates, and hosted CI: [DEVELOPING.md](DEVELOPING.md).

## Next

- Linux x86_64 (Bazzite) release install, systemd unit, Tailscale Serve:
  [LINUX.md](LINUX.md)
- Security model and device lifecycle: [OPERATIONS.md](OPERATIONS.md)
- Hacking on the daemon: [DEVELOPING.md](DEVELOPING.md)
