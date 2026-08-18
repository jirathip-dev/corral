# Corral Operations

Running and maintaining `corrald`: device lifecycle, grants, the macOS
Keychain how-to, audit semantics, and troubleshooting. Commands marked
"verified" were run against a throwaway daemon on 2026-08-16.

## One-shot setup (scripts/)

For a new machine / public install, two scripts make the daemon turnkey:

### `scripts/setup-corrald.sh`

Builds the release binary, creates `~/.config/corral` (keys, tokens), and
installs a **launchd agent** (`com.jirathip.corrald`) so the daemon runs at
login and stays up (KeepAlive). Idempotent; safe to re-run.

```sh
scripts/setup-corrald.sh                     # loopback only (default 127.0.0.1:8474)
scripts/setup-corrald.sh --bind 100.67.222.5 # bind a Tailscale/private IP (needs #65)
scripts/setup-corrald.sh --uninstall         # remove the launchd agent (keeps config)
```

`launchctl bootstrap` is intentionally NOT run from inside the Hermes gateway
(same reason as herdr-server): run the script from your own Terminal, or if
the daemon is already up under launchd, `launchctl kickstart -k` reloads it.

### `scripts/corrald-grant.sh`

Promote/revoke a device's drive capabilities with the admin token (never hand
the token to a device).

```sh
scripts/corrald-grant.sh --list                          # show registered devices + grants
scripts/corrald-grant.sh --key dev_<id> --caps read_tail,prompt
scripts/corrald-grant.sh --key dev_<id> --revoke
```

`CORRAL_BASE` overrides the daemon base URL (default `http://127.0.0.1:8474`).

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

- Registered device: read plane only (`/snapshot`, `/events` —
  loopback-local, credential-free).
- Drive capabilities are promoted by the host via `POST /grants`
  (admin token): `prompt`, `interrupt`, `approve`, `read_tail`, `kill`,
  `attach`. Default deny; no auto-approve.
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

`corrald fleet` reads — and, since #35 slice 1, edits — the control-plane
registry describing each fleet's repo, local checkout, worktree dir,
orchestrator, workers and per-role models. `list`/`check` are read-only;
**`add`/`remove` rewrite the registry file** (atomically, validated, with
the repo resolved via `gh` before anything is written). Nothing touches a
running agent.

```sh
corrald fleet list                    # one greppable line per fleet
corrald fleet check                   # validate + verify each local checkout
corrald fleet add <name> --gh <o/r>   # insert a fleet (WRITES the registry)
corrald fleet remove <name>           # drop a fleet (WRITES the registry)
corrald fleet list --registry <path>  # override the default
```

The registry path is `$CORRAL_FLEETS_PATH`, else
`~/.hermes/scripts/fleets.json`. Exit codes: **0** all good, **1** at least
one fleet failed `check`, **2** usage or parse/validation error. Validation
is strict on purpose — unknown fields, empty required fields, whitespace
inside `name`/`gh_repo`, a `gh_repo` that is not `owner/repo`, a `local`
starting with a bare `~`, and duplicate names all fail loudly. Full schema:
`docs/corral/G35-registry.md`, which lands with the registry PR.

## Security model summary

- Loopback-only binding; `corrald` exits if asked to bind a routable
  address.
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
| `refusing to bind <addr>` | `--bind` must be loopback (`127.0.0.1`); this is a hard refusal |
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
