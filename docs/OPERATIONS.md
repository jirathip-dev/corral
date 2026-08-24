# Corral Operations

Running and maintaining `corrald`: device lifecycle, grants, remote
access from iOS (Tailscale Serve), the macOS Keychain how-to, audit
semantics, and troubleshooting. Commands marked
"verified" were run against a throwaway daemon on 2026-08-16.

## One-shot setup (scripts/)

For a new machine / public install, two scripts make the daemon turnkey:

### `scripts/setup-corrald.sh`

Builds the release binary, creates `~/.config/corral` (keys, tokens), and
installs a **launchd agent** (`com.corral.corrald`) so the daemon runs at
login and stays up (KeepAlive). It also installs the release desktop client:
`/Applications/Corral.app` on macOS or `~/.local/bin/corrald-ui` plus a
`.desktop` entry on Linux. Idempotent; safe to re-run.

```sh
scripts/setup-corrald.sh                     # loopback only (default 127.0.0.1:8474)
scripts/setup-corrald.sh --bind 100.67.222.5 # bind a Tailscale/private IP (desktop/daemon only)
scripts/setup-corrald.sh --uninstall         # remove all three launchd agents (keeps config)
```

`--bind <tailnet-ip>` serves DESKTOP clients on the tailnet; the iOS
client cannot use it (ATS refuses plain HTTP to the CGNAT range) — for
iOS, keep the daemon loopback-only and use Tailscale Serve instead: see
"Remote access from iOS (Tailscale Serve)" below.

`launchctl bootstrap` is intentionally NOT run from inside an automation
gateway (same reason as the herdr-server agent): run the script from your
own Terminal. If the daemon is already up under launchd, the script's
`bootout` + `bootstrap` applies a changed config (a plain `launchctl
kickstart -k` only restarts the process; it does not re-read a rewritten
plist).

The macOS desktop install is handled by `scripts/install-corral-ui.sh`. It
stages the complete `Corral.app` beside `/Applications/Corral.app`, builds and
round-trips `Contents/Resources/Corral.icns` from the checked-in
full-bleed opaque `assets/icon/corral-icon-macos.png` (macOS applies the
squircle mask), validates the executable, and writes
`CFBundleIconFile=Corral` in `Info.plist` before touching the live
destination. Linux stages the binary, the 256px
`assets/icon/corral-icon-256.png`, and the
`~/.local/share/applications/corral.desktop` entry together. Only a validated
staged payload is committed; the desktop entry quotes and escapes executable
paths containing spaces, `%`, quotes, `$`, backticks, or backslashes according
to desktop-entry syntax. A failed copy, converter, plist check, or final rename leaves the
existing installation in place and rolls back any partial commit. If rollback
restoration itself fails, the installer keeps and reports the exact rollback
directory so the old payload remains recoverable. A missing icon or failed
macOS icon conversion is an installation error rather than a silent fallback.
The Linux `Exec` value uses command quoting followed by desktop-entry
general-string escaping: quotes, `$`, and backticks receive doubled on-disk
backslashes, a literal backslash receives four, and `%` is written as `%%`.
Newline- or carriage-return-containing requested prefixes and executable paths
containing `=` fail before destination directories are created. Install
prefixes and the macOS app parent are physically
canonicalized before safety checks; root/`..` paths and symlinked `bin` or
`share` payload parents are rejected, while a safe symlinked prefix is
resolved to its canonical target. Staging and final renames stay beside the
resolved destination on one filesystem. Linux and Other explicitly compare
the device of every target parent with the staging prefix before mutation and
again before commit; device mismatches fail rather than pretending a
cross-filesystem rename is atomic. If payload restoration fails, the old
payload is retained in the reported rollback directory; if only rollback
directory cleanup fails after a failed install, an existing empty or partial
rollback directory is named for inspection, a missing root is reported
without a path or recoverability claim, and an uninspectable root is reported
as indeterminate with its path. After a fresh install, the cleanup-failure
diagnostic instead names an empty rollback directory for inspection. After a replacement, the
diagnostic checks the expected rollback paths after failed cleanup and reports
all, some, or none of the prior Linux/Other backups (or the macOS previous
path) without promising recoverability from a pre-cleanup count. An existing
empty rollback directory is named for inspection; a missing root has no
inspection or recoverability claim; and an existing but uninspectable root is
reported as indeterminate. The multi-file Linux commit is rollback-based
rather than one single atomic operation.

`--uninstall` removes all three launchd agents and leaves the config directory; it
does not remove an installed desktop app. Use
`scripts/install-corral.sh --uninstall` to also remove the staged release
files and the `Corral.app` desktop payload.

Prebuilt tagged releases can be installed without a Rust toolchain using
`scripts/install-corral.sh`; see the README install section. In that mode
`scripts/setup-corrald.sh --from-release <binary>` skips the cargo build and
uses the bundled `corrald` and `corrald-ui` binaries instead.

### `scripts/corrald-grant.sh`

Promote/revoke a device's drive capabilities with the admin token (never hand
the token to a device).

```sh
scripts/corrald-grant.sh --list                          # show registered devices + grants
scripts/corrald-grant.sh --key dev_<id> --caps read_tail,prompt
scripts/corrald-grant.sh --key dev_<id> --revoke
```

`CORRAL_BASE` overrides the daemon base URL (default `http://127.0.0.1:8474`).
The desktop board exposes the same list/grant/revoke operations in
**Settings → Device grants**; enter (or let the UI read) the same host
`admin-token`. The CLI remains a supported alternate path.

## Remote access from iOS (Tailscale Serve)

The supported way to reach the daemon from the iOS client outside the
LAN is **real TLS via Tailscale Serve fronting a loopback-only daemon**
— not `--bind`. Serve improves CONFIDENTIALITY (real TLS; the daemon
process never listens beyond loopback), and it is the path ATS accepts
with no certificate plumbing of your own. It does NOT narrow exposure:
`tailscale serve` publishes to the WHOLE tailnet, and corral's read
plane (`/snapshot`, `/events`, `/history`) is credential-free
— every device on the tailnet can read full fleet state, exactly as
with a tailnet `--bind`. Use it only on a tailnet whose every device
may see fleet state (same rule as binding beyond loopback).

### Why `--bind <tailnet-ip>` cannot work for iOS

ATS classifies Tailscale's 100.64.0.0/10 (CGNAT) addresses as public
internet — `NSAllowsLocalNetworking` covers `.local` and RFC 1918, not
the tailnet — so a plain-HTTP daemon on a tailnet IP is refused:

```
[register_failed] The resource could not be loaded because the App
Transport Security policy requires the use of a secure connection.
```

An `NSExceptionDomains` carve-out for `ts.net` does NOT fix it — iOS
still forces a TLS upgrade for the MagicDNS hostname:

```
[register_failed] A TLS error caused the secure connection to fail.
```

(The build-3 carve-out that tried this is gone; the generated
Info.plist ships with no insecure-HTTP exception.)

### One-time tailnet prerequisite: enable HTTPS Certificates

`tailscale cert` fails (and `tailscale serve` hangs) until HTTPS
Certificates is enabled for the tailnet:

```
500 Internal Server Error: your Tailscale account does not support
getting TLS certs
```

Enable it in the Tailscale **admin console → DNS → HTTPS Certificates**
(tailnet-wide, one-time, admin-console-only — there is no CLI
equivalent). Verify:

```sh
tailscale status --json | grep -i certdomains   # empty before, populated after
```

### The working setup

On the iPhone first (`curl` from the host passes even when the phone
is not on the tailnet, so this is easy to miss):
install the Tailscale app, sign into the SAME tailnet, and leave it
connected. MagicDNS must be on for `<host>.<tailnet>.ts.net` to resolve
on the device (it is required for HTTPS certs anyway).

```sh
# 1. one-time, in the Tailscale admin console: DNS -> enable HTTPS Certificates

# 2. (optional) issue the cert — run OUTSIDE the repo checkout: it
#    writes an unencrypted private .key next to you. Serve provisions
#    the cert itself on first request; this step mainly turns the
#    missing-HTTPS-certs failure into an immediate, legible error.
tailscale cert <host>.<tailnet>.ts.net

# 3. front the loopback daemon with real TLS (--bg needs Tailscale
#    >= 1.58; the Mac App Store build does not put `tailscale` on PATH —
#    use /Applications/Tailscale.app/Contents/MacOS/Tailscale)
tailscale serve --bg --https=443 http://127.0.0.1:8474

# 4. verify a valid chain (no -k)
curl -s -o /dev/null -w '%{http_code} verify=%{ssl_verify_result}\n' \
  https://<host>.<tailnet>.ts.net/healthz     # -> 200 verify=0
```

The daemon stays **loopback-only** (plain `scripts/setup-corrald.sh`,
no `--bind`). In the app, the host is the plain HTTPS origin with no
port:

```
https://<host>.<tailnet>.ts.net
```

To inspect or tear down the Serve config later (do NOT run these as
part of the setup — `reset` undoes step 3):

```sh
tailscale serve status
tailscale serve reset
```

Include the `https://` scheme — the app assumes `http://` when the
scheme is omitted, which lands on the ATS error above. This repository's
local gate does not claim physical-device or TestFlight verification. When a
Release build is distributed, its only product path is the real registration,
`/events` SSE stream, and signed drive plane; the Debug-only seeded demo is
not part of that build. Validate the Release artifact with
`ios/check-release-demo.py` before any later hardware or TestFlight pass.

### The FleetNotifier app (iOS)

Capabilities, grant-gated per device (see "Grants model" below):

- **Live board** from the `/events` SSE stream, resuming from a
  persisted `Last-Event-ID` cursor. A stale cursor is dropped when the
  store is empty — a cursor is only a valid delta-base for state you
  actually hold (a reset device that resumes deltas-only would otherwise
  never see a snapshot).
- **Recent output** (`read_tail`): bounded live tail served via signed
  `/drive`, segmented in `corrald` into blocks (user / agent / tool / system);
  clients render the blocks without re-segmenting.
- **Older transcript pages** (`read_tail`): paged, newest-first history via
  signed `GET /transcript`, using the same grant. The iOS app folds both into
  the single Recent-output surface (live bottom + older history on scroll-up).
- **Interrupt** (`interrupt`): signed `/drive` stop for a live agent.
- **Kill** (`kill`) and **Attach** (`attach`): signed `/drive` controls.
  Kill is destructive by capability and takes the same Face ID step-up
  path as other destructive commands; Attach does not weaken that gate.
- **Prompt** (`prompt`) and **Approve / Deny / Continue** (`approve`):
  canned replies answer a waiting agent in-app; the same actions work
  from the lock-screen notification, validated against the live claim
  (`prompt_hash` / `stale_approval` refusals surface as typed banners).
- Disabled controls stay visible and distinguish the two gates: a missing
  grant says `requires the <cap> grant — ask the host.`, while an agent
  that does not advertise the capability says `<cap>: not available for
  this agent.`
- If a target disappears or moves while a drive is in flight, the daemon
  returns `stale_agent` (409 before dispatch when observed). The app removes
  the stale row, shows a refresh banner, and fetches one fresh snapshot; the
  SSE stream then supplies the authoritative replacement row.
- Biometric step-up (Face ID) runs in-app for destructive payloads and
  Kill; the lock-screen path is bounded to non-destructive canned replies.

Promote the phone's key (never hand the admin token to the device):

```sh
scripts/corrald-grant.sh --key <key_id> --caps read_tail,prompt,interrupt,approve
```

`kill` and `attach` are deliberately not in that baseline promotion. Add
them explicitly only after a host-side grant decision:

```sh
scripts/corrald-grant.sh --key <key_id> --caps read_tail,prompt,interrupt,approve,kill,attach
```

Diagnosing a stuck board: since #92 every stream-layer failure sets a
visible `.error` banner (os.Logger line + dismissible text) instead of
an infinite spinner — check the banner first, then the Troubleshooting
table below.

## Device lifecycle

### Registration

A device (client) generates an Ed25519 keypair and enrolls its public
key with the host. The registration token in the config dir
(`registration-token`, routing-only) gates enrollment; it never
authenticates writes.

```sh
PUBKEY=$(openssl pkey -in <device-key.pem> -pubout -outform DER | tail -c 32 | base64)
curl -s -X POST http://127.0.0.1:8474/register \
  -H 'Content-Type: application/json' \
  -d "{\"token\":\"$(cat <config-dir>/registration-token)\",\"public_key\":\"$PUBKEY\"}"
```

Verified response: `{"key_id":"dev_...","grants":[],"expiry_ts":...,
"revoked":false,...}` — **empty grants = read-only by default**. The
desktop client (`corrald-ui`) auto-registers on localhost by reading the
same `registration-token` file (same user).

Device keys expire 90 days after registration (`expiry_ts`). Expiry and
revocation are checked on every `verify` — re-register with a fresh key
before expiry.

### Grants model

- Registered device: read plane only (`/healthz`, `/snapshot`, `/events`,
  `/history` — credential-free; on a non-loopback bind — or a
  loopback bind fronted by Tailscale Serve — the tailnet/private network
  itself is the read boundary, so expose it only on networks whose every
  device may see fleet state — a plain LAN offers no device auth, prefer
  a tailnet).
- Drive capabilities are promoted by the host via `POST /grants`
  (admin token): `prompt`, `interrupt`, `approve`, `read_tail`, `kill`,
  `attach`, plus the fleet-level `start_worktree`. Default deny; no
  auto-approve.
- `GET /transcript` is NOT part of the credential-free read plane: it
  requires the `read_tail` grant (transcripts are a superset of tail
  content — same trust decision, same device registry). NOTE: this
  widens what `read_tail` reaches — see "Grant scope" under "Transcript
  read-path" below and re-review existing grants.
- `GET /issues` (#113) is part of the credential-free read plane: it
  exposes only the public repo-level issue metadata the gh poller already
  fetches (number, state, title, labels, url) and no per-agent
  transcript/tail content. Same network rule as `/snapshot`/`/events`:
  serve it only on loopback or a private/tailnet interface. Creating a
  worktree from an issue is a WRITE and separately requires the
  `start_worktree` drive grant — this GET never mutates GitHub.
- Grant/replace the whole set (verified):

```sh
ADMIN=$(cat <config-dir>/admin-token)
curl -s -X POST http://127.0.0.1:8474/grants \
  -H 'Content-Type: application/json' -H "Authorization: Bearer $ADMIN" \
  -d '{"action":"set_grants","key_id":"dev_...","grants":["prompt","read_tail"]}'
# → {"key_id":"dev_...","ok":true}
```

- Inspect registered devices/grants for the board's selector (host admin
  only). The projection contains `key_id`, `grants`, `revoked`,
  `expiry_ts`, and `created_ts`; it deliberately omits public keys and APNs
  push tokens. With no `key_id` it lists every registered device; with
  `?key_id=<id>` it narrows to one:

```sh
curl -s -H "Authorization: Bearer $ADMIN" \
  'http://127.0.0.1:8474/grants?key_id=dev_...'
# → {"ok":true,"devices":[{"key_id":"dev_...","grants":["prompt","read_tail"],"revoked":false,...}]}
```

The desktop **Settings → Device grants** editor uses that read surface for
its device selector, applies through the same `POST /grants` `set_grants`
body, and exposes `--revoke` as an explicit button. Checking or unchecking
capabilities replaces the full set; all unchecked is read-only.

- Revoke (verified):

```sh
curl -s -X POST http://127.0.0.1:8474/grants \
  -H 'Content-Type: application/json' -H "Authorization: Bearer $ADMIN" \
  -d '{"action":"revoke","key_id":"dev_...","revoked":true}'
```

- Signed self-service grants read (#101): a device can refresh its OWN
  grants via `POST /grants-read` — it signs `{key_id, request, ts}` with
  its existing Ed25519 key (freshness `|now - ts| < 60s`); no admin token,
  no new key material. The response is the key's CURRENT grants + expiry,
  so a promotion reaches the phone after relaunch/foreground without a
  device reset. The iOS app calls this on cold launch and on every
  foreground (`scenePhase == .active`); a failed refresh keeps the cached
  grants. Verified:

```sh
# KEY_B64 = base64 of the device's Ed25519 public key (see "Registering a
# device" below); SIGNATURE = the device's signature over the exact JSON
# {"key_id":"dev_...","request":"grants-read","ts":<unix-now>}
curl -s -X POST http://127.0.0.1:8474/grants-read \
  -H 'Content-Type: application/json' \
  -d '{"key_id":"dev_...","signature":"<sig-b64>","request":{"key_id":"dev_...","request":"grants-read","ts":1780000000}}'
# → {"ok":true,"key_id":"dev_...","grants":["read_tail","prompt","interrupt","approve"],"expiry_ts":...,"revoked":false}
```

Revocation takes effect on the next write — every drive is verified per
request, there are no long-lived sessions to cut short. A revoked/expired
key cannot mint step-up tokens either.

### Rotation

| Credential | How |
|---|---|
| Host identity (X25519, `host-key`) | stop daemon, delete `host-key`, restart; new key published by `GET /host-key`; device auth unaffected |
| Registration token | delete `registration-token`, restart; existing devices keep working, new enrollments need the new token |
| Admin token | delete `admin-token`, restart |
| Device keys | re-register before expiry; or revoke via `POST /grants` |

## macOS Keychain how-to (hit live 2026-08-16)

The dev binary is unsigned, and the client stores its device keypair via
the macOS Keychain (`keyring` crate, service `corrald-ui`). A fresh
`cargo build` changes the (unsigned) identity, so macOS re-prompts
"corrald-ui wants to access key…" on **every** launch of an existing
device. Fix after every rebuild:

```sh
codesign -s - --force target/release/corrald-ui
```

Ad-hoc signing gives a stable CDHash, so the next launch prompts once
and "Always Allow" sticks. First-run/new devices don't prompt (keychain
adds are silent) — prompts appear when **reading** an item created by an
older binary identity. If prompts return, the binary was rebuilt without
re-signing.

## Audit log

`<config-dir>/audit.log` — one JSON line per entry, SHA-256 hash-chained
(`prev` = previous entry's hash, `hash` = own hash; genesis anchor
`corral-audit-genesis-v1`). Read it with the admin token (verified):

```sh
curl -s -H "Authorization: Bearer $ADMIN" http://127.0.0.1:8474/audit
# {"entries":[...],"head":"<sha256>","valid":true,...}
```

- Grows on **drive writes** (executions and typed refusals at dispatch)
  and on **served `/transcript` pages** (#63 — one entry per page,
  capability `read_tail:transcript`, the agent as target). Authentication failures
  and step-up failures are never appended.
- `valid` is the live chain-integrity verdict: any tampered or inserted
  line breaks the chain. Known limit: a wholesale truncation of trailing
  entries is not detectable without an external anchor (a W4 follow-up).
- A crash mid-append is repaired at next open as a flagged tombstone line.

## Event history and the daily digest

Separate from the audit log: the **event ring** (D23) records agent
status-transition events — not just drive writes — appended at the
store-apply choke point. It is the "what did the fleet actually do"
record. On disk it is rotating append-only JSONL
(`seg-<seq>-<start_ts>.jsonl`) under `<config-dir>/history`.

Read a window over HTTP:

```sh
curl -s 'http://127.0.0.1:8474/history?since=1784210400000&limit=500'
# {"events":[...]}
```

`since` is epoch millis; `limit` defaults to 1000 and is capped at 5000.

Or take a per-agent daily digest offline, straight off the ring — no
running daemon required (D33):

```sh
corrald digest                          # last 24h
corrald digest --since 1784210400000    # explicit window
corrald digest --config-dir <path>      # non-default config dir
```

## Log rotation

`corrald` is chatty by design; `~/.config/corral/corrald-launchd.log` (the
launchd `StandardOutPath`/`StandardErrorPath` target) has been observed at
hundreds of MB. `scripts/setup-corrald.sh` installs a third user-level launchd
agent, `com.corral.corrald-rotate`, that runs `scripts/rotate-corral-logs.sh`
every 30 minutes to keep the log size-capped without sudo.

Behavior:

- Cap: **50 MiB** (`CORRAL_LOG_MAX_BYTES`); a log under the cap is left alone.
- History: **2 gzipped generations** — the live log rolls to `.1.gz`, the old
  `.1.gz` to `.2.gz`, and `.2.gz` is dropped on the next rotation
  (`CORRAL_LOG_KEEP`).
- Race safety: launchd holds the daemon's log fd open, so rotation renames the
  live file to `.1` and then restarts the daemon with
  `launchctl kickstart -k gui/$(id -u)/com.corral.corrald` (KeepAlive relaunches
  it and launchd reopens the path at offset 0). The renamed inode is only
  gzipped after that restart, so it is never compressed while the daemon can
  still write to it.
- The live log file is **never deleted**; a fresh empty file is created in its
  place, and the daemon-not-loaded case (config dir exists, job absent) is a
  graceful no-restart that still leaves the path present.
- `corral-update.log` (written by the run-and-exit update agent) gets the same
  cap/rotation treatment; it is not held open persistently, so it needs no
  restart.

Inspect the daemon log and its generations:

```sh
tail -f ~/.config/corral/corrald-launchd.log
gzip -dc ~/.config/corral/corrald-launchd.log.1.gz | less
```

`scripts/setup-corrald.sh --uninstall` removes `com.corral.corrald-rotate`
along with the daemon and update agents. Run the rotator manually (e.g. with a
temporary cap) to force a rotation:

```sh
CORRAL_LOG_MAX_BYTES=1024 scripts/rotate-corral-logs.sh
```

## Transcript read-path (#63)

`GET /transcript?agent=<id>` returns one newest-first page of the
agent's session transcript, redacted (D-083) before it leaves the
transcript module. It is an on-demand VIEW fetch — never pushed (D5
stays intact). Both the desktop egui board and the iOS FleetNotifier
app page through it; the iOS Recent-output surface merges the live bounded
tail (bottom) with the paged older history (top) via the transcript cursor.

Auth is the drive plane's `read_tail` trust decision on a GET: put the
exact `SignedDrive` JSON you would POST to `/drive` (capability
`read_tail`, `target` = the agent id) into the `x-corral-drive` header.
Signature, key registry, expiry/revocation, and the grant check are
identical to `/drive`; there is no step-up (reads are never destructive)
and no replay-table claim — **but `/transcript` is replay-BOUNDED, not
replay-table'd**. The envelope payload is transcript-scoped: the client
signs `{"ts": <unix seconds>, "cursor": <opaque|absent>, "limit":
<1..=50|absent>}` inside the envelope, and the signature covers the
whole envelope (payload included) — so **one signature buys exactly ONE
page**, and only while `|now - ts| <= 60s` (the same freshness rule as
`/step-up` and `/device-token`). There is no URL knob: `cursor`/`limit`
query parameters are refused outright, so a captured header replays at
most the one page it was signed for, for 60 seconds — never a different
page, and never more than 60 seconds of drift on the newest page (a
cursor-less capture replays whatever is newest at service time). Paging
means re-signing per page with the new cursor. A suspected header leak
is still a key-revocation event (revocation is checked on every call),
and every served page leaves its own audit entry (below).

```sh
curl -s 'http://127.0.0.1:8474/transcript?agent=herdr:ses_abc123' \
  -H "x-corral-drive: $SIGNED_ENVELOPE_JSON"
# SIGNED_ENVELOPE_JSON is a SignedDrive whose envelope payload is
# {"ts": <unix seconds>, "cursor": <absent for page 1>, "limit": 20}
# → {"agent":"...","store":"opencode","session":"opencode:ses_abc123",
#    "bind":"session_id","stores_unavailable":[],
#    "entries":[{"role":"assistant","text":"...","ts":1723...}],
#    "next_cursor":"oc.1723...9.6d73675f3031.9f2ab4c1d0e37a55","skipped":0}
```

Follow `next_cursor` (opaque string) for older pages — re-signing the
header with the new cursor; `null` means the store is exhausted. The
cursor is fingerprinted to the bound session and can only ever read the
file it was issued for; the bind is memoized for the life of the page
sequence (fingerprint-verified), so a newer session appearing
mid-sequence does not invalidate the cursor — it keeps paging the file
it was issued for, and the next cursor-less request picks the new
newest session. The memo is a bounded per-daemon cache (LRU, 64
entries), not a promise: if the entry is evicted or the daemon
restarts, a mid-sequence cursor falls through to a fresh bind and
becomes `bad_cursor` — page 1 again re-establishes the sequence.
`limit` is signed into the header and clamped to 1..=50 regardless of
what is asked (asking for 500 yields 50). `skipped` counts torn
rows/lines in the page's range; `session` names the bound session so a
client can pin it; `bind` says which rung answered (`session_id` =
exact, `worktree` = best-effort — same-tool agents sharing a worktree
without session-id hints share that rung); `stores_unavailable` lists
store kinds that errored during binding (a complete-looking page from
one store does not prove the others were consultable).

Errors are typed (`{"kind": ..., "message": ...}`): auth —
`missing_signature` 400, `bad_signature` 401, `unknown_key` 404,
`expired`/`revoked`/`not_granted` 403, and `bad_request` 400 for a
malformed header (including a missing or stale `ts` — |now - ts| > 60s
— or a `cursor`/`limit` query string), a capability other than
`read_tail`, or an envelope target that does not match `?agent`; read
path — `bad_cursor` 400, `unknown_agent`/`no_session` 404,
`ambiguous_session` 409 **with the candidate list** (the daemon never
guesses between sessions that tie on recency),
`store_unreadable`/`sqlite3_unavailable`/`query_timeout` 503,
`store_shape` 502. Concurrency: `/transcript` serves are capped at 8
per daemon; an over-cap request queues briefly (2s) then gets `busy`
503 — the ONE error a client should retry on.

Every SERVED page appends an audit entry (capability
`read_tail:transcript` — distinguishable from a bounded `/drive`
`read_tail` — the agent as target) to the same hash-chained log as drive
writes — check
`GET /audit` for the read trail. Auth failures are never audited.

### Grant scope — recorded decision (#63)

Gating `/transcript` on `read_tail` follows the issue spec ("transcripts
are a superset of tail content — the same trust decision") and it
**widens what an existing grant reaches**: `read_tail` was specified as
a 200-line / 32 KiB bounded tail of one pane (D5); with `/transcript`
the same grant pages the FULL session history of any agent on the host
(grants are per-device, not per-agent). **Operators: re-review existing
`read_tail` grants before deploying this** — every device holding the
grant gains history access with no re-consent. The reviewed alternative
— a separate `read_transcript` capability, which would leave existing
grants untouched — is recorded in the #63 review file for the merge
decision.

Session binding prefers the EXACT session id carried in the herdr agent
id (`herdr:<session-id>` — claude jsonl filename, opencode
`session.id`, codex rollout filename); only pane-derived ids fall back
to worktree matching (`workspace.worktree_path` against opencode
`session.directory`, the Claude Code project-dir encoding — with the
recorded in-file cwd verified, since the encoding is lossy — and codex
rollouts' first-line `payload.cwd`), restricted to the agent's own
tool's store when the tool is recognized (`opencode`/`claude`/`codex` —
an agent reporting any other tool string consults all three stores),
most recent match wins. Path comparisons use raw-then-
canonical matching (symlinked `$HOME` safe). Store locations honour the
transcript-reader env overrides (`$CORRAL_OPENCODE_DB`,
`$CORRAL_CLAUDE_DIR`, `$CORRAL_CODEX_DIR`). All reads are read-only
(opencode via `sqlite3 -readonly`).

## Fleet registry

`corrald fleet` reads and edits the control-plane registry describing each
fleet's repo, local checkout, worktree dir, orchestrator, workers and
per-role models. `list`/`check`/`watch` are read-only; **`add`/`remove`**
and **`pause`/`resume`/`models`** rewrite the registry file (atomically,
validated, with the repo resolved via `gh` before an add). Registry mutation
never touches a running agent. The destructive #35 surface is also present:
`switch` re-arms the orchestrator after an auth gate, `reap` clears verified
finished/paused-idle panes, and `prune` removes only provably-dead worktrees.
Those commands never rewrite the registry.

```sh
corrald fleet list                    # one greppable line per fleet
corrald fleet check                   # validate + verify each local checkout
corrald fleet add <name> --gh <o/r>   # insert a fleet (WRITES the registry)
corrald fleet remove <name>           # drop a fleet (WRITES the registry)
corrald fleet watch                   # read-only health pass (cron-able)
corrald fleet pause <name>            # set paused (WRITES; idempotent)
corrald fleet resume <name>           # clear paused (WRITES; idempotent)
corrald fleet models <name> --impl m  # update only the model slots named
corrald fleet models all --impl m     # ... applied to every fleet
corrald fleet models <name> --impl-alt ''   # CLEAR the optional alt slot
corrald fleet switch <name>           # auth-gated orchestrator re-arm
corrald fleet reap <fleet|all>        # dry-run; --apply kills verified panes
corrald fleet prune [--apply|--yes]   # dry-run; --apply/--yes removes dead trees
corrald fleet list --registry <path>  # override the default
```

The registry path is `$CORRAL_FLEETS_PATH`, else
`$CORRAL_CONFIG_DIR/fleets.json` (default `~/.config/corral/fleets.json`);
a pre-existing legacy fleet registry is honoured as a fallback while the
corral-owned file does not exist, with a stderr note each time the fallback
is taken (#66). Migrating a legacy machine: stop anything that writes the
registry first, then move that legacy file to
`$CORRAL_CONFIG_DIR/fleets.json` — **`mv`, not `cp`**: a copy leaves the
legacy tooling writing one file while corrald reads the other, and the
two silently diverge. Every write command loads the registry before
writing, so a missing parent dir surfaces as the plain
`cannot read fleet registry <path>` error — bootstrap with
`mkdir -p ~/.config/corral` followed by
`echo '{"fleets": []}' > ~/.config/corral/fleets.json`.
Exit codes: **0** all good, **1** an operation failed — a fleet failed
`check`, a registry write (`add`/`remove`/`pause`/`resume`/`models`)
refused or could not write (registry left byte-identical), `watch` found
problems (for `watch` this INCLUDES an unreadable/invalid registry, reported
as a `PROBLEM:` line with exit 1 for monitor safety), or `switch`/`reap`/
`prune` hit an operational refusal/failure (unauthenticated runtime,
shrink guard, failed identity check, git/gh/herdr failure);
**2** usage error, or (every subcommand except `watch`) an
unreadable/unparseable/invalid registry.
Validation is strict on
purpose — empty required fields, whitespace inside
`name`/`gh_repo` and every `models.*` slot, a `gh_repo` that is not
`owner/repo`, a `local` starting with a bare `~`, and duplicate names all
fail loudly. Unknown fleet-operations fields (`models.reasoning_effort`,
top-level `admit`, and future additions) are accepted by the subset reader
and preserved through corral rewrites; an unknown key that is one edit away
from a Corral-owned field — including an adjacent transposition (`pausd`,
`imp1_alt`, `puased`, `imlp_alt`) — is still refused, so a typo never silently
defaults or drops a gate. Model map: required `orch`/`impl`/`review`, optional
`impl_alt`/`impl_alt2` fallback slots that `fleet models` can set or clear
(`--impl-alt ''`; `models all` applies to every fleet — `all` is a
reserved fleet name). Full schema and the per-command exit-code table:
`docs/corral/G35-registry.md`.

## Workspace/repo attribution

The daemon's board grouping uses canonical workspace facts, not the name of
an orchestrator pane. `CORRAL_REPO_ROOT` is always a known primary checkout;
entries in the fleet registry add more known primary roots from their
`local` paths, with repository identity taken from the corresponding
`gh_repo` slug. Registry identity wins when a fleet `local` canonicalizes to
`CORRAL_REPO_ROOT`; the configured directory basename is only the fallback.
The linked-worktree root is `CORRAL_WORKTREES_ROOT` (default
`~/.herdr/worktrees`) and keeps the established `<repo>/<label>` layout.

The git plane supplies branch facts for each recognized root/worktree. On a
supervised plane restart, the old branch cache and stored branch fields for
recognized agents are cleared before replacement-plane probes begin; present
paths are repopulated by fresh git facts, while vanished paths cannot retain
their old branch. Repo identity and the other workspace/GitHub fields are
preserved, and unknown paths are not reconciled. Repo and branch matching
accepts raw or symlink/canonical path spellings, so a Herdr cwd under a
symlinked `$HOME` still joins the git record. The label, pane title, display
name, and terminal text are not branch or repo sources.
An agent whose `worktree_path` matches none of the configured roots or Herdr
worktree layout is intentionally left with `repo: null` and remains in
`(no repo)`; do not repair that bucket by guessing from a pane label.

## Issue-linked and issue-free worktrees (#113, slice 1)

The desktop UI's Issues tab renders the daemon's read-only
`GET /issues` view (keyed by repo/fleet name) and can start a worktree two
explicit ways, both gated by confirmation in the UI and the `start_worktree`
grant on the host:

- **Issue-linked**: from a selected, open issue. The daemon validates the
  issue against the SAME fetched set the browser renders, creates exactly one
  branch/worktree under `<home>/.herdr/worktrees/<fleet.worktree_dir>`, and
  carries the issue number in the branch (`issue-<N>-…`). A closed or missing
  issue is a typed refusal (`issue_closed` / `issue_not_found`) — it never
  falls through to the free path.
- **Issue-free**: a separate, clearly-marked control takes a user-chosen
  label; the branch is prefixed `w2/free-` and never carries an issue number.
  It is only reachable by explicitly choosing it.

The git step and the herdr handoff are injectable seams
(`src/fleet/worktree.rs`). Slice-1 keeps the handoff typed but deferred:
the worktree/branch is created, the launcher reports `deferred`, and the
agent-spawn RPC is a later slice. Duplicate taps/retries are idempotent on
`request_id` — a second request returns `already_started`, never a second
worktree. `error_kind`s: `unknown_fleet`, `issue_not_found`, `issue_closed`,
`already_started`, `invalid_name`, `git_failure`, `launch_failure`.

To make the desktop browser able to start worktrees, grant the device the
capability (read-only default denies it):

```sh
scripts/corrald-grant.sh --key <key_id> --caps start_worktree
```

See `docs/corral/G35-registry.md` for the registry fields the worktree
creation reads (`name`, `worktree_dir`); READ-ONLY GitHub access is
unchanged — this slice never issues a GitHub write.

## Security model summary

- Non-public binds only (loopback default; private/RFC 1918, Tailscale
  100.64/10, and IPv6 unique-local permitted — #65); `corrald` exits
  if asked to bind a public/routable address.
- Three credentials, never one: registration token (routing only),
  per-device Ed25519 keypair (authenticates writes; host identity is
  X25519), per-capability grants (read-only default, promoted on host).
- Claim-based approvals: replies echo `approval_id` + exact
  `prompt_hash` of the live prompt; mismatch is refused (409
  `hash_mismatch`) before dispatch — kills the approve-the-wrong-question
  race.
- Biometric step-up for destructive payloads (`rm -rf`, `push --force`,
  `curl | sh`, `~/.aws`, `~/.ssh`, `.env`): 5-minute single-use token via
  `POST /step-up`, presented as `X-Step-Up-Token`. Deterrent layer, not
  a security boundary — pattern detection is deliberately conservative.
- Default deny, no auto-approve; secrets redacted at the adapter
  boundary before leaving the machine; key material `0600`/`0700`;
  release binary exposes no secret accessors.

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `App Transport Security policy requires the use of a secure connection` (iOS) | the app is pointed at a plain-HTTP tailnet bind — iOS treats 100.64/10 as public internet. Use Tailscale Serve: see "Remote access from iOS" above |
| `A TLS error caused the secure connection to fail` (iOS) | a ts.net ATS exception can't fix plain HTTP (iOS forces TLS on MagicDNS). Use Tailscale Serve with real certs: see "Remote access from iOS" above |
| Board spins forever with no banner (builds ≤ 4) | pre-#92/#90 defects: `URLSession.bytes.lines` drops SSE frame terminators (zero frames ever complete) and the nested-`ObservableObject` board never re-rendered. Rebuild ≥ build 5; current builds show a typed `.error` banner, not a spinner |
| Recent output / prompt / interrupt / Kill / Attach / approve controls missing on the phone | the device key has no grants — promote via `POST /grants` or `scripts/corrald-grant.sh --key <key_id> --caps read_tail,prompt,interrupt,approve`; add `kill,attach` explicitly for those two controls. Registration is idempotent per key and never upgrades grants; resetting the device mints a NEW read-only key |
| Recent output / prompt / interrupt / Kill / Attach / approve controls still missing after a promotion | relaunching or foregrounding the app now refreshes grants from the daemon (`POST /grants-read`, signed — no device reset needed). If the controls still don't appear after a foreground refresh, the promotion did not reach this key (check `POST /grants` targeted the right `key_id`) |
| `[register_failed]` with a bare host | the app assumes `http://` when the scheme is omitted — include `https://` (see "Remote access from iOS" above) |
| `refusing to bind <addr>` | `--bind` must be loopback, private (RFC 1918), Tailscale/CGNAT 100.64/10, or IPv6 unique-local — public IPs and 0.0.0.0 are hard refusals |
| Daemon won't start, `auth plane init failed` | corrupt key material in the config dir — the daemon fails fast rather than silently re-keying. Inspect/remove the offending file (or start with a fresh `CORRAL_CONFIG_DIR`) |
| Daemon won't start, `failed to bind` | port already in use — pick another `--port`; `lsof -nP -iTCP:<port> -sTCP:LISTEN` to see who owns it |
| `GET /snapshot` shows no herdr agents | herdr socket missing/unreachable. The adapter warns and retries with backoff; HTTP keeps serving (verified). `corrald` must run on the same machine as herdr |
| Daemon log storms: repeated `events.subscribe` `REQUEST_TIMEOUT` + re-bootstrap, fd count climbing | herdr replays pane state BEFORE answering `subscribe`; the reader never blocks on event delivery. A full bounded channel is a deterministic resynchronization signal: the reader drains the pending subscribe response, retires the stream, then a successfully subscribed global stream re-bootstraps only after the shared capped outage backoff. Accepted-then-closed global streams use that same ladder, so repeated closes cannot reset to an immediate resubscribe; the ladder resets only after a meaningful stable interval. Connect/subscribe failures use capped exponential backoff (30s maximum) and emit one WARN per outage; each pane retry task owns its live forwarder, cancels it on removal/replacement, and remains active until herdr recovers. A dropped client aborts the reader so no descriptor is leaked (#105/#117) |
| UI can't connect | client defaults to `http://127.0.0.1:8474`; check the daemon port and that the client config (`$CORRAL_UI_CONFIG_DIR/config.json`) points at the right host |
| UI drive buttons do nothing / `not_granted` | device has no grant for that capability — promote on the host via `POST /grants` (or Settings → Device grants) |
| `409 hash_mismatch` on approve | the client's `prompt_hash` does not match the current prompt. Hash the exact untrimmed, redacted prompt string from the snapshot's `waiting_on.prompt` byte-for-byte — never raw pane text |
| `409 stale_approval` | approval_id refers to an earlier prompt the agent already moved past — fetch the live claim again |
| `403 step_up_required` | destructive payload needs a fresh `POST /step-up` token (single-use, 5 min) in `X-Step-Up-Token` |
| macOS Keychain re-prompts | binary was rebuilt — re-run `codesign -s - --force target/release/corrald-ui` (see keychain how-to above) |
| `git plane: worktree scan failed` warning | benign when `CORRAL_REPO_ROOT` has no `.git` (e.g. throwaway roots) |
