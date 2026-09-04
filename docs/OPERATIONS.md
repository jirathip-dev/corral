# Corral Operations

Running and maintaining `corrald`: device lifecycle, grants, remote
access from iOS (Tailscale Serve), audit
semantics, and troubleshooting. Commands marked
"verified" were run against a throwaway daemon on 2026-08-16.

## One-shot setup (scripts/)

For a new machine / public install, two scripts make the daemon turnkey:

### `scripts/setup-corrald.sh`

Builds the release binary, creates `~/.config/corral` (keys, tokens), and
installs a **launchd agent** (`com.corral.corrald`) so the daemon runs at
login and stays up (KeepAlive). Idempotent; safe to re-run. (The pre-#376
desktop client install is gone: Corral is an iOS-only product.)

```sh
scripts/setup-corrald.sh                     # loopback only (default 127.0.0.1:8474)
scripts/setup-corrald.sh --bind 100.67.222.5 # bind a Tailscale/private IP (daemon only)
scripts/setup-corrald.sh --uninstall         # remove all three launchd agents (keeps config)
```

`--bind <tailnet-ip>` serves the credential-free read plane on the
tailnet (for scripts or other hosts); the iOS client cannot use it (ATS
refuses plain HTTP to the CGNAT range) — for iOS, keep the daemon
loopback-only and use Tailscale Serve instead: see "Remote access from
iOS (Tailscale Serve)" below.

`launchctl bootstrap` is intentionally NOT run from inside an automation
gateway (same reason as the herdr-server agent): run the script from your
own Terminal. If the daemon is already up under launchd, the script's
`bootout` + `bootstrap` applies a changed config (a plain `launchctl
kickstart -k` only restarts the process; it does not re-read a rewritten
plist).

`--uninstall` removes all three launchd agents (daemon, auto-update, log
rotation) and leaves the config directory in place. A release install is
removed with `scripts/install-corral.sh --uninstall`, which also removes
the staged release files.

Prebuilt tagged releases can be installed without a Rust toolchain using
`scripts/install-corral.sh`; see the README install section. In that mode
`scripts/setup-corrald.sh --from-release <binary>` skips the cargo build
and uses the bundled `corrald` binary instead.

### Grant provisioning (out-of-band since #354)

The host-admin grant surface (`POST /grants`, `GET /grants` and the
`scripts/corrald-grant.sh` helper) was removed with the mutating plane
in #354: no HTTP request — even one carrying the admin token — can grant,
list, or revoke device capabilities any more (the routes are absent:
404). The registry (`registry.json`, 0600, in the config dir) is the only
grant store, and it is loaded once at daemon start, so provisioning is:

1. stop `corrald`,
2. edit `<config-dir>/registry.json` — grant/replace the whole set by
   editing a device's `"grants"` array (`"read_tail"` is the capability
   clients use; `"read_diff"` also parses as a daemon-retained read — no
   other name parses), or revoke by setting `"revoked": true`,
3. start `corrald` again; the change applies from the first drive after
   restart.

Never hand the admin token (or the `registration-token`) to a device.
The device-facing, signed self-service grants read (`POST /grants-read`,
#101) is unchanged: a device refreshes its OWN current grants over its
existing Ed25519 key.

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
`/events` SSE stream, and signed read drives; the Debug-only seeded demo is
not part of that build. Validate the Release artifact with
`ios/check-release-demo.py` before any later hardware or TestFlight pass.

### The FleetNotifier app (iOS)

Read-only client, grant-gated per device (see "Grants model" below):

- **Live board** from the `/events` SSE stream, resuming from a
  persisted `Last-Event-ID` cursor. A stale cursor is dropped when the
  store is empty — a cursor is only a valid delta-base for state you
  actually hold (a reset device that resumes deltas-only would otherwise
  never see a snapshot). Board rows use the herdr RAW state vocabulary
  (working / idle / blocked / unknown; the herdr 0.8.2 wire can also
  carry `done` — recorded in the #324 live probe — which the board ranks
  and renders with `idle`, so a wire-`done` record reads as finished,
  never active), grouped by repo with blocked agents
  pinned to the top; last-known rows persist under an offline banner when
  the daemon is unreachable.
- **Recent output** (`read_tail`): the only signed drive the app sends —
  bounded live tail (≤200 lines, daemon-capped), segmented in `corrald`
  into blocks (user / agent / tool / system); the app renders the blocks
  without re-segmenting. Live tail only: no load-earlier, no conversation
  partition.
- **State-change notifications**: local notifications on `working` entry
  (episode start), `blocked`, and episode end (`working`/active → `idle`,
  the "done" transition — fires once per episode, deduped). Content is
  `agent · repo` / `state · branch`; tapping deep-links to the row with
  recents open; the only control is a global on/off in Settings. Real APNs
  delivery is wired but not provisioned: it needs the daemon-side APNs
  provisioning checkpoint (a `.p8` auth key from Guy + `CORRAL_APNS_*`
  env); simulator/DEBUG verification uses the local notification bridge.
- **Read-only since #354**: the mutating drive capabilities (`prompt`,
  `interrupt`, `approve`, `kill`, `attach`, `start_worktree`,
  `read_issues`) and the terminal/attach transport were removed from the
  daemon; the worktree-diff page was removed from the iOS client in #354
  (L2) while the daemon RETAINS the signed `read_diff` read path
  (bounded changed-files/diff — no bundled client dispatches it). A
  signed
  drive naming a removed capability is refused at the capability boundary
  (`400 unknown_capability`) before the authorizer, before any adapter
  dispatch, and before the audit log.
- If a target disappears or moves while a read drive is in flight, the
  daemon returns `stale_agent` (409 before dispatch when observed). The app
  removes the stale row, shows a refresh banner, and fetches one fresh
  snapshot; the SSE stream then supplies the authoritative replacement row.

Grant the phone's key out-of-band (never hand the admin token to the
device); see "Grant provisioning" above:

```sh
# 1. stop corrald, 2. in <config-dir>/registry.json set the device's
# grants: ["read_tail"] for recents (the only drive the app sends),
# 3. start corrald again.
```

Every other capability name is refused (`400 unknown_capability`) — the
daemon no longer accepts mutating grants, and the HTTP grant admin is
gone (#354).

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
"revoked":false,...}` — **empty grants = read-only by default**.

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
- Drive capabilities are provisioned out-of-band on `registry.json`
  (the host-admin `POST /grants` surface was removed in #354). The only
  capability names that parse are the two signed reads: `read_tail`
  (bounded recent output — the capability every client uses) and
  `read_diff` (#232: bounded worktree-diff read, retained daemon-side,
  no client dispatches it after the client cuts). Grants default empty,
  like every other grant. Every removed name (`prompt`, `interrupt`,
  `approve`, `kill`, `attach`, `start_worktree`, `read_issues`) is
  refused by the parser (`400 unknown_capability`). Default deny; no
  auto-approve.
- `GET /issues` (#113) is part of the credential-free read plane: it
  exposes only the public repo-level issue metadata the gh poller already
  fetches (number, state, title, labels, url) and no per-agent
  transcript/tail content. Same network rule as `/snapshot`/`/events`:
  serve it only on loopback or a private/tailnet interface. This GET never
  mutates GitHub (the `start_worktree` drive that consumed it was removed
  in #354).
- Grant/replace the whole set — out-of-band (verified procedure): stop
  the daemon, edit the device's `"grants"` array in `<config-dir>/registry.json`
  (`"read_tail"`, `"read_diff"` — unknown names are refused on load),
  restart. The registry is loaded once at startup; no HTTP route mutates
  it (#354).

```sh
# <config-dir>/registry.json (0600): "devices" -> "grants"
#   { ..., "grants": ["read_tail"], "revoked": false, ... }
# ("read_diff" also parses for a daemon-retained read no client uses)
```

- Revoke — out-of-band: stop the daemon, set the device's `"revoked":
  true` in `<config-dir>/registry.json`, restart.

- Signed self-service grants read (#101): a device can refresh its OWN
  grants via `POST /grants-read` — it signs `{key_id, request, ts}` with
  its existing Ed25519 key (freshness `|now - ts| < 60s`); no admin token,
  no new key material. The response is the key's CURRENT grants + expiry,
  so out-of-band grant changes reach the phone after the host restarts the
  daemon and the app relaunches/foregrounds — no device reset. The iOS app
  calls this on cold launch and on every
  foreground (`scenePhase == .active`); a failed refresh keeps the cached
  grants. Verified:

```sh
# KEY_B64 = base64 of the device's Ed25519 public key (see "Registering a
# device" below); SIGNATURE = the device's signature over the exact JSON
# {"key_id":"dev_...","request":"grants-read","ts":<unix-now>}
curl -s -X POST http://127.0.0.1:8474/grants-read \
  -H 'Content-Type: application/json' \
  -d '{"key_id":"dev_...","signature":"<sig-b64>","request":{"key_id":"dev_...","request":"grants-read","ts":1780000000}}'
# → {"ok":true,"key_id":"dev_...","grants":["read_tail"],"expiry_ts":...,"revoked":false}
```

Revocation is checked on every drive — per request, no long-lived
sessions — so once an out-of-band `"revoked": true` has been loaded
(daemon restart), the very next drive is refused with `revoked`.

### Rotation

| Credential | How |
|---|---|
| Host identity (X25519, `host-key`) | stop daemon, delete `host-key`, restart; new key published by `GET /host-key`; device auth unaffected |
| Registration token | delete `registration-token`, restart; existing devices keep working, new enrollments need the new token |
| Admin token | delete `admin-token`, restart |
| Device keys | re-register before expiry; or revoke out-of-band: stop the daemon, set `"revoked": true` in `registry.json`, restart |

## Audit log

`<config-dir>/audit.log` — one JSON line per entry, SHA-256 hash-chained
(`prev` = previous entry's hash, `hash` = own hash; genesis anchor
`corral-audit-genesis-v1`). Read it with the admin token (verified):

```sh
curl -s -H "Authorization: Bearer $ADMIN" http://127.0.0.1:8474/audit
# {"entries":[...],"head":"<sha256>","valid":true,...}
```

- Grows on **drive dispatches** (executions and typed refusals at
  dispatch). Authentication failures are never appended.
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

## Recent output read path (#167)

`read_tail` returns the bounded (200 lines / 32 KiB, D5) redacted tail of
one agent pane, segmented server-side into wire blocks (user / agent /
tool / system) served ADDITIVELY as `{"lines": [...], "blocks": [...]}`
on the signed `POST /drive` response. It is an on-demand VIEW fetch —
never pushed (D5 stays intact). The iOS FleetNotifier app renders the
Recent-output surface from this one payload: clients consume the
segmented `blocks` (with the legacy `lines` field as the
backward-compatible text view).

```sh
# signed POST /drive with capability read_tail (see the drive plane)
# → {"ok":true,"rev":42,"result":{"lines":["..."],"blocks":[{"kind":"tool",...}],"source_rev":59}}
```

### Provider read-tail revision contract (#324)

`read_tail` requests may carry a cached source revision and every response
carries the provider's current one:

- Request payload: `{"kind":"read_tail","lines":Option<u32>,
  "since_rev":Option<u64>}` — `since_rev` is the client's cached
  per-agent/output-source revision from a previous response.
- Response result: `{"lines":[...],"blocks":[...],"source_rev":Option<u64>}`
  — `source_rev` is the PROVIDER's revision of the agent's
  `recent_unwrapped` output source. It is monotonic per agent/output source
  and is never the fleet snapshot revision (the response envelope's `rev`).

Provider semantics (defined for `agent.read`, documented in
`src/adapters/herdr.rs`): a nonzero `read.revision` means the provider
tracks revisions. When a request carries `rev` equal to the provider's
current revision the window is UNCHANGED — the provider returns an empty
`text` (no page transferred) and Corral returns an explicit empty
`lines`/`blocks` result with the same `source_rev`. Any other revision
(first read, advanced output, or a provider wrap/restart that resets the
counter below the cached value) returns the bounded window plus the
provider's current revision. `revision` `0`/absent means a legacy provider:
the bounded full-page fallback applies and `source_rev` echoes the
client's cached revision (existing behavior).

Upstream blocker, recorded honestly: the live Herdr 0.8.2 provider does NOT
support revisions. Its socket `agent.read` returns `revision: 0` regardless
of `rev` (verified for no `rev`, `rev=1`, and `rev=999999` in #324), so the
incremental path only activates against providers that honor the contract;
Herdr 0.8.2 keeps the bounded full-page behavior. The regression/probe
evidence for the incremental path uses a simulated contract-honoring
provider (`docs/design/evidence/issue-324/`).

### The closed capability set (read-only since #354)

The signed `/drive` surface accepts exactly two capability names —
`read_tail` (above, used by every client for recents) and `read_diff`
(bounded worktree-diff read, #232). `read_diff` is retained daemon-side
and its grant still parses, but no bundled client dispatches it: the iOS
Diff page was removed in #354 L2 and the desktop client was removed in
#376. Every other name — `prompt`, `interrupt`,
`approve`, `kill`, `attach`, `start_worktree`, `read_issues` — is refused
with `400 unknown_capability` before the authorizer, before dispatch, and
before the audit log.

## Read-only API reference

What a client can do against a live daemon (routes verified against
`src/api/mod.rs` and `src/auth/http.rs` at the #354 cut):

| Route | Auth | Purpose |
|---|---|---|
| `GET /healthz` | none | liveness (`ok`) |
| `GET /host-key` | none | host X25519 identity |
| `POST /register` | registration token (routing only) | enroll a device Ed25519 key; response grants are empty (read-only default) |
| `GET /snapshot` | none (credential-free read) | full fleet snapshot, monotonic `rev` |
| `GET /events` | none (credential-free read) | SSE stream; resumes from `Last-Event-ID` |
| `GET /history` | none (credential-free read) | D23 event-ring window (`?since=<ms>&limit=`) |
| `GET /issues` | none (credential-free read) | repo-level issue metadata view (#113; no bundled client UI renders it) |
| `POST /drive` | device signature | signed read drive: `read_tail` (all clients) / `read_diff` (daemon-retained; no client dispatches it) |
| `POST /grants-read` | device signature | device refreshes its OWN grants (#101) |
| `POST /device-token` | device signature | APNs device-token registration (daemon push path) |
| `GET /audit` | admin Bearer token | hash-chained audit log (host-side read) |

There is no `/grants` admin route, no `/step-up`, no `/fleets`, and no
mutating drive arm; the daemon CLI is `corrald` (serve) plus
`corrald digest` (D33, offline) only.

## Fleet operations — configless (#237)

Corral is **configless**: it does not own, read, or write `fleets.json`.
The fleet registry is fleet-ops' opinionated config (per-role models,
admit, paused) and lives in `~/.config/fleet-operations/fleets.json`,
managed with the fleet-ops CLI `herdr-fleet` (`list|add|remove|check|
pause|resume|models|switch|doctor`). `corrald` carries no fleet CLI of its
own: registry writes and the orchestrator re-arm are the fleet-ops CLI's
job — `herdr-fleet switch <name>` is lanes-aware and validates the fleet
identity itself (hermes-lane profiles included).

```sh
herdr-fleet list                     # one greppable line per fleet
herdr-fleet check                    # validate fleets + local checkouts
herdr-fleet add <name> --gh <o/r>    # insert a fleet (fleet-ops owns writes)
herdr-fleet remove <name>            # drop a fleet (fleet-ops owns writes)
herdr-fleet pause <name>             # set paused (fleet-ops owns writes)
herdr-fleet models <name> --impl m   # update per-role models
herdr-fleet switch <name>            # re-arm the fleet's orchestrator
```

`herdr-fleet switch` exits 0 when the re-arm succeeded and
1 on any refusal/failure; its diagnostics stream through unchanged. The
legacy #35 registry surface (`list/check/add/remove/pause/resume/models/
watch/reap/prune` with `--registry`) is superseded — those commands were
the corral-owned read/write path that configless removes. Pane/worktree
cleanup uses `herdr` directly (`herdr pane close`, `herdr worktree
remove`, `git worktree prune`) and `fleet-watch` remains the fleet-ops
watcher. The iOS app exposes only its board, recents, and Settings
surfaces; it has no Issues or Fleets tab (the Issues browser was removed
with the #354 client cut). Fleet-ops surfaces (registry views, watch,
re-arm) live in the fleet-ops tooling itself — the `herdr-fleet` CLI and
`fleet-watch` — never in corrald's daemon or the iOS app.

## Workspace/repo attribution

The daemon's board grouping uses canonical workspace facts, not the name of
an orchestrator pane. Configless (#237): `CORRAL_REPO_ROOT` is the known
primary checkout and the git plane additionally discovers immediate
`~/Projects` checkouts; NO fleet-registry roots or `worktree_dir -> gh_repo`
aliases feed attribution. Repo categories are the live `workspace.repo`
values from the Herdr snapshot, period — display repo categories are never
actionable identities. The linked-worktree root is `CORRAL_WORKTREES_ROOT`
(default `~/.herdr/worktrees`) and keeps the established
`<worktree_dir>/<label>` layout; the directory component is an addressable
location, not repo identity. The GitHub facts plane folds PR/CI facts on the
same repo basename the agent carries, and `GET /issues` groups by those same
live Herdr `workspace.repo` categories — no fleet-ops CLI catalog
participates and no `/fleets` route exists.

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

Git-plane load is bounded at the subprocess boundary. FSEvents-triggered
probes, the 60-second status sweep, and topology rescans share four git
command permits; registry rescans are serialized because they also perform
filesystem canonicalization. A probe admitted to the permit pool is subject
to the normal five-second git child timeout; time spent queued before
admission is additional latency and can make total event time exceed five
seconds. A completed probe that exceeds the 200ms event budget still emits
the existing `git plane event over budget` warning. The permit pool protects
Herdr scheduling; it does not discard git facts, convert a timeout into
success, or restart the Store/SSE revision sequence. During a load
investigation, correlate the warning's `took_ms` with `event stream
closed`/`re-bootstrapping`; a warning alone is diagnostic, not a stream
failure.

When FSEvents reports an unknown path under `commondir/worktrees/`, the
watcher awaits the throttled topology rescan so the new worktree can be
debounced immediately. A concurrent 10s/60s safety rescan holds the same
serialization guard, so topology freshness can wait behind one in-flight
scan; the callback queue retains later frames and the one-shot 400ms retry
covers a `git worktree add` that registers just after the first scan. This is
the intentional freshness tradeoff: topology is reconciled before probing,
while scans never overlap and the event stream is not reset.

## Repo-level issues view (`GET /issues`, #113)

The daemon continues to serve the read-only `GET /issues` view — keyed by
repo, scoped to the current Herdr-owned workspace repositories (#332). The
gh poller's specs rebuild from those same live workspaces, and topology
changes prune stale categories, so every visible category is current. The
view is display-only end to end: the `start_worktree` drive (the only
consumer of a selected issue) and its grant were removed from the daemon
along with the other mutating capabilities in #354, so `GET /issues` never
starts or mutates anything. No bundled client renders an Issues tab since
#354 L2 removed it from the iOS app (the desktop client was removed in
#376) — the route remains a
credential-free read endpoint for read-only clients and scripts.

## Security model summary

- Non-public binds only (loopback default; private/RFC 1918, Tailscale
  100.64/10, and IPv6 unique-local permitted — #65); `corrald` exits
  if asked to bind a public/routable address.
- Three credentials, never one: registration token (routing only),
  per-device Ed25519 keypair (authenticates signed reads; host identity is
  X25519), per-capability grants (read-only default, promoted on host).
  With the #354 cut only the two read capabilities exist, so every
  authenticated drive is a read; mutating names are refused at the
  capability boundary before the authorizer.
- Waiting-agent state (`waiting_on`, still carrying `approval_id` /
  `prompt_hash` from the pre-cut schema) is READ state: the daemon
  records the blocked question from herdr's output and serves it in the
  snapshot/SSE stream. The clients surface the blocked STATE (chip +
  pin-to-top), not an answer flow — the daemon no longer accepts an
  approve reply; answering a waiting agent is a host-side action outside
  corrald.
- Default deny, no auto-approve; secrets redacted at the adapter
  boundary before leaving the machine; key material `0600`/`0700`;
  release binary exposes no secret accessors.

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `App Transport Security policy requires the use of a secure connection` (iOS) | the app is pointed at a plain-HTTP tailnet bind — iOS treats 100.64/10 as public internet. Use Tailscale Serve: see "Remote access from iOS" above |
| `A TLS error caused the secure connection to fail` (iOS) | a ts.net ATS exception can't fix plain HTTP (iOS forces TLS on MagicDNS). Use Tailscale Serve with real certs: see "Remote access from iOS" above |
| Board spins forever with no banner (builds ≤ 4) | pre-#92/#90 defects: `URLSession.bytes.lines` drops SSE frame terminators (zero frames ever complete) and the nested-`ObservableObject` board never re-rendered. Rebuild ≥ build 5; current builds show a typed `.error` banner, not a spinner |
| Recent output rows missing on the phone | the device key has no `read_tail` grant — provision out-of-band: stop the daemon, add `"read_tail"` to the device's `"grants"` array in `<config-dir>/registry.json`, restart. Registration is idempotent per key and never upgrades grants; resetting the device mints a NEW read-only key |
| Recent output rows still missing after provisioning | relaunching or foregrounding the app refreshes grants from the daemon (`POST /grants-read`, signed — no device reset needed). If the rows still don't appear after a foreground refresh, the grant did not reach this key (check the device's `"grants"` array in `registry.json` after the daemon restart) |
| `[register_failed]` with a bare host | the app assumes `http://` when the scheme is omitted — include `https://` (see "Remote access from iOS" above) |
| `refusing to bind <addr>` | `--bind` must be loopback, private (RFC 1918), Tailscale/CGNAT 100.64/10, or IPv6 unique-local — public IPs and 0.0.0.0 are hard refusals |
| Daemon won't start, `auth plane init failed` | corrupt key material in the config dir — the daemon fails fast rather than silently re-keying. Inspect/remove the offending file (or start with a fresh `CORRAL_CONFIG_DIR`) |
| Daemon won't start, `failed to bind` | port already in use — pick another `--port`; `lsof -nP -iTCP:<port> -sTCP:LISTEN` to see who owns it |
| `GET /snapshot` shows no herdr agents | herdr socket missing/unreachable. The adapter warns and retries with backoff; HTTP keeps serving (verified). `corrald` must run on the same machine as herdr |
| Daemon log storms: repeated `events.subscribe` `REQUEST_TIMEOUT` + re-bootstrap, fd count climbing | herdr replays pane state BEFORE answering `subscribe`; the reader never blocks on event delivery. A full bounded channel is a deterministic resynchronization signal: the reader drains the pending subscribe response, retires the stream, then a successfully subscribed global stream re-bootstraps only after the shared capped outage backoff. Accepted-then-closed global streams use that same ladder, so repeated closes cannot reset to an immediate resubscribe; the ladder resets only after a meaningful stable interval. Connect/subscribe failures use capped exponential backoff (30s maximum) and emit one WARN per outage; each pane retry task owns its live forwarder, cancels it on removal/replacement, and remains active until herdr recovers. A dropped client aborts the reader so no descriptor is leaked (#105/#117) |
| `400 unknown_capability` on a signed drive | the drive names a capability the daemon removed in the #354 cut (`prompt`, `interrupt`, `approve`, `kill`, `attach`, `start_worktree`, `read_issues`) — the remaining signed-read set is `read_tail` (client recents) and the daemon-retained `read_diff` |
| repeated `git plane event over budget` warnings | the four-command git budget preserves daemon scheduling, so an isolated slow probe is diagnostic and still correct; if warnings coincide with `event stream closed`/`re-bootstrapping`, inspect host git/filesystem load and the `took_ms` values. The daemon must not reset revisions merely because a git probe is slow |
| `git plane: worktree scan failed` warning | benign when `CORRAL_REPO_ROOT` has no `.git` (e.g. throwaway roots); the first failure is WARNed once, retries back off (10s → 60s → 5m), present sources retain their last-known worktrees/topology during a transient Git failure, and immediate `~/Projects` checkouts are refreshed by the 15-minute rediscovery pass |
