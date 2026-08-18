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
login and stays up (KeepAlive). Idempotent; safe to re-run.

```sh
scripts/setup-corrald.sh                     # loopback only (default 127.0.0.1:8474)
scripts/setup-corrald.sh --bind 100.67.222.5 # bind a Tailscale/private IP (desktop/daemon only)
scripts/setup-corrald.sh --uninstall         # remove the launchd agent (keeps config)
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

### `scripts/corrald-grant.sh`

Promote/revoke a device's drive capabilities with the admin token (never hand
the token to a device).

```sh
scripts/corrald-grant.sh --list                          # show registered devices + grants
scripts/corrald-grant.sh --key dev_<id> --caps read_tail,prompt
scripts/corrald-grant.sh --key dev_<id> --revoke
```

`CORRAL_BASE` overrides the daemon base URL (default `http://127.0.0.1:8474`).

## Remote access from iOS (Tailscale Serve)

The supported way to reach the daemon from the iOS client outside the
LAN is **real TLS via Tailscale Serve fronting a loopback-only daemon**
— not `--bind`. Serve improves CONFIDENTIALITY (real TLS; the daemon
process never listens beyond loopback), and it is the path ATS accepts
with no certificate plumbing of your own. It does NOT narrow exposure:
`tailscale serve` publishes to the WHOLE tailnet, and corral's read
plane (`/snapshot`, `/events`, `/history`, `/cost`) is credential-free
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
scheme is omitted, which lands on the ATS error above. Verified on a
live tailnet with `curl` (`200 verify=0`); the iOS DEVICE leg —
registration plus holding the `/events` SSE stream through the Serve
proxy — is pending the first TestFlight verification round (with #79),
any inaccuracy in this section will surface at that device leg, not
in the curl-verified part above.

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
  `/history`, `/cost` — credential-free; on a non-loopback bind — or a
  loopback bind fronted by Tailscale Serve — the tailnet/private network
  itself is the read boundary, so expose it only on networks whose every
  device may see fleet state — a plain LAN offers no device auth, prefer
  a tailnet).
- Drive capabilities are promoted by the host via `POST /grants`
  (admin token): `prompt`, `interrupt`, `approve`, `read_tail`, `kill`,
  `attach`. Default deny; no auto-approve.
- `GET /transcript` is NOT part of the credential-free read plane: it
  requires the `read_tail` grant (transcripts are a superset of tail
  content — same trust decision, same device registry). See "Transcript
  read-path" below.
- Grant/replace the whole set (verified):

```sh
ADMIN=$(cat <config-dir>/admin-token)
curl -s -X POST http://127.0.0.1:8474/grants \
  -H 'Content-Type: application/json' -H "Authorization: Bearer $ADMIN" \
  -d '{"action":"set_grants","key_id":"dev_...","grants":["prompt","read_tail"]}'
# → {"key_id":"dev_...","ok":true}
```

- Revoke (verified):

```sh
curl -s -X POST http://127.0.0.1:8474/grants \
  -H 'Content-Type: application/json' -H "Authorization: Bearer $ADMIN" \
  -d '{"action":"revoke","key_id":"dev_...","revoked":true}'
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

- Grows **only on drive writes**: executions and typed refusals at
  dispatch. GETs, authentication failures, and step-up failures are never
  appended.
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

## Transcript read-path (#63)

`GET /transcript?agent=<id>&cursor=<c>&limit=<n>` returns one
newest-first page of the agent's session transcript, redacted (D-083)
before it leaves the transcript module. It is an on-demand VIEW fetch —
never pushed (D5 stays intact), and the phone client does not call it in
this phase (D16: phone stays bounded-tail only).

Auth is the drive plane's `read_tail` trust decision on a GET: put the
exact `SignedDrive` JSON you would POST to `/drive` (capability
`read_tail`, `target` = the agent id) into the `x-corral-drive` header.
Because a transcript read is idempotent there is no replay-table claim
and no step-up — signature, key registry, expiry/revocation, and the
grant check are identical to `/drive`.

```sh
curl -s 'http://127.0.0.1:8474/transcript?agent=herdr:orch-corral&limit=20' \
  -H "x-corral-drive: $SIGNED_ENVELOPE_JSON"
# → {"agent":"...","store":"opencode","entries":[{"role":"assistant","text":"...","ts":1723...}],
#    "next_cursor":"oc.1723...9.6d73675f3031","skipped":0}
```

Follow `next_cursor` (opaque string) for older pages; `null` means the
store is exhausted. `skipped` counts torn rows/lines in the page's range
(honesty counter). Errors are typed: `bad_cursor` 400, `unknown_agent` /
`no_session` 404, `ambiguous_session` 409 **with the candidate list**
(the daemon never guesses between sessions that tie on recency),
`store_unreadable`/`sqlite3_unavailable`/`query_timeout` 503,
`store_shape` 502.

Session binding is by worktree: the agent's `workspace.worktree_path` is
matched against opencode `session.directory`, the Claude Code project
dir encoding under `~/.claude/projects`, and codex rollouts' first-line
`payload.cwd`; the most recent match wins. Store locations honour the
same env overrides as the cost meter (`$CORRAL_OPENCODE_DB`,
`$CORRAL_CLAUDE_DIR`, `$CORRAL_CODEX_DIR`). All reads are read-only
(opencode via `sqlite3 -readonly`).

## Cost / usage meter

`GET /cost` reports per-provider spend over rolling 5h / weekly / monthly
windows, read straight from each tool's own session store (read-only).

> **The default caps are invented.** Nobody has supplied the real
> opencode-go / claude / codex subscription limits, so every unset cap is a
> placeholder and the response marks it `cap_is_placeholder: true` (the UI
> prefixes such percentages with `~`). **Do not act on a percentage until
> you have set the real cap.** A meter that looks authoritative while
> resting on a guess is worse than no meter — the outage this feature
> exists to prevent was a silent credit exhaustion.

Set the real limits before trusting the alert:

```sh
CORRAL_COST_CAP_OPENCODE_5H_USD=...   CORRAL_COST_CAP_OPENCODE_WEEKLY_USD=...   CORRAL_COST_CAP_OPENCODE_MONTHLY_USD=...
CORRAL_COST_CAP_CLAUDE_5H_USD=...     CORRAL_COST_CAP_CLAUDE_WEEKLY_USD=...     CORRAL_COST_CAP_CLAUDE_MONTHLY_USD=...
CORRAL_COST_CAP_CODEX_5H_USD=...      CORRAL_COST_CAP_CODEX_WEEKLY_USD=...      CORRAL_COST_CAP_CODEX_MONTHLY_USD=...

CORRAL_COST_WARN_THRESHOLD_PCT=70     # window status -> warning at/above
CORRAL_COST_ALERT_THRESHOLD_PCT=90    # window status -> problem at/above
```

A provider whose store is absent reports `store_found: false` and renders
as "no store" — distinct from `$0.00`, which would wrongly read as "you
have spent nothing".

## Fleet registry

`corrald fleet` reads — and, since #35 slices 1–2, edits — the control-plane
registry describing each fleet's repo, local checkout, worktree dir,
orchestrator, workers and per-role models. `list`/`check` are read-only;
**`add`/`remove`** (slice 1) and **`pause`/`resume`/`models`** (slice 2)
rewrite the registry file (atomically, validated, with the repo resolved via
`gh` before an add). Nothing touches a running agent — `pause`/`resume` here
mutate the `paused` flag only; the ops half (halting working agents, the
auth-gated model-switch re-arm) is a later #35 slice.

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
corrald fleet list --registry <path>  # override the default
```

The registry path is `$CORRAL_FLEETS_PATH`, else
`$CORRAL_CONFIG_DIR/fleets.json` (default `~/.config/corral/fleets.json`);
a pre-existing legacy `~/.hermes/scripts/fleets.json` is honoured as a
fallback while the corral-owned file does not exist, with a stderr note
each time the fallback is taken (#66). Migrating a legacy machine: stop
anything that writes the registry first, then
`mkdir -p ~/.config/corral && mv ~/.hermes/scripts/fleets.json
~/.config/corral/fleets.json` — **`mv`, not `cp`**: a copy leaves the
legacy tooling writing one file while corrald reads the other, and the
two silently diverge. Every write command loads the registry before
writing, so a missing parent dir surfaces as the plain
`cannot read fleet registry <path>` error — bootstrap with
`mkdir -p ~/.config/corral` followed by
`echo '{"fleets": []}' > ~/.config/corral/fleets.json`.
Exit codes: **0** all good, **1** an operation failed — a fleet failed
`check`, a write command (`add`/`remove`/`pause`/`resume`/`models`)
refused (duplicate name, unresolvable repo, unknown name, no models to
inherit) or could not write (registry left byte-identical), or `watch`
found problems — for `watch` this INCLUDES an unreadable/invalid
registry, reported as a `PROBLEM:` line with exit 1 (monitor safety);
**2** usage error, or (every subcommand except `watch`) an
unreadable/unparseable/invalid registry.
Validation is strict on
purpose — unknown fields, empty required fields, whitespace inside
`name`/`gh_repo` and every `models.*` slot, a `gh_repo` that is not
`owner/repo`, a `local` starting with a bare `~`, and duplicate names all
fail loudly. Model map: required `orch`/`impl`/`review`, optional
`impl_alt`/`impl_alt2` fallback slots that `fleet models` can set or clear
(`--impl-alt ''`; `models all` applies to every fleet — `all` is a
reserved fleet name). Full schema and the per-command exit-code table:
`docs/corral/G35-registry.md`.

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
| `refusing to bind <addr>` | `--bind` must be loopback, private (RFC 1918), Tailscale/CGNAT 100.64/10, or IPv6 unique-local — public IPs and 0.0.0.0 are hard refusals |
| Daemon won't start, `auth plane init failed` | corrupt key material in the config dir — the daemon fails fast rather than silently re-keying. Inspect/remove the offending file (or start with a fresh `CORRAL_CONFIG_DIR`) |
| Daemon won't start, `failed to bind` | port already in use — pick another `--port`; `lsof -nP -iTCP:<port> -sTCP:LISTEN` to see who owns it |
| `GET /snapshot` shows no herdr agents | herdr socket missing/unreachable. The adapter warns and retries with backoff; HTTP keeps serving (verified). `corrald` must run on the same machine as herdr |
| UI can't connect | client defaults to `http://127.0.0.1:8474`; check the daemon port and that the client config (`$CORRAL_UI_CONFIG_DIR/config.json`) points at the right host |
| UI drive buttons do nothing / `not_granted` | device has no grant for that capability — promote on the host via `POST /grants` (or the UI's audit/grants view) |
| `409 hash_mismatch` on approve | the client's `prompt_hash` does not match the current prompt. Hash the exact untrimmed, redacted prompt string from the snapshot's `waiting_on.prompt` byte-for-byte — never raw pane text |
| `409 stale_approval` | approval_id refers to an earlier prompt the agent already moved past — fetch the live claim again |
| `403 step_up_required` | destructive payload needs a fresh `POST /step-up` token (single-use, 5 min) in `X-Step-Up-Token` |
| macOS Keychain re-prompts | binary was rebuilt — re-run `codesign -s - --force target/release/corrald-ui` (see keychain how-to above) |
| `git plane: worktree scan failed` warning | benign when `CORRAL_REPO_ROOT` has no `.git` (e.g. throwaway roots) |
