# Corral Architecture

`corrald` is a Rust daemon that collapses the herdr agent fleet into a
snapshot read model (served over loopback HTTP + SSE) and exposes a
signed, capability-gated write plane (`POST /drive`). Design
authority: `~/Projects/hermes-brain/plans/corral/DECISIONS.md`
(D1–D14).

## Read side: planes → integrator → store → HTTP/SSE

```
herdr socket ── herdr adapter (push: events.subscribe, zero polling)
git worktrees ─ GitPlane (fsevents push + 10s parallel sweep safety net)
GitHub ──────── GhPlane (one GraphQL round-trip per poll; SWR: no polling
              │  until the first SSE client ever connects)
              ▼
        plane_channel → Integrator (pure channel drain, supervised,
              │         re-armed with backoff on panic/exit)
              ▼
        Store (canonical agent records; coalesced deltas on a
              │  250ms foreground / 2s background tick)
              ▼
        GET /snapshot (monotonic rev), GET /events
              (SSE; resumes from Last-Event-ID — full snapshot when
               the cursor is stale, incremental {rev, upd, del} deltas
               otherwise)
        GET /history (D23 event ring: status transitions appended at
              the store-apply choke point, persisted as rotating JSONL;
              `?since=<epoch-millis>` and `?limit=` — 1000 default,
              5000 cap)
```

Every adapter normalizes into the canonical `Agent` record
(`src/core/model.rs`, `schema_version` 4, additive-only). The herdr
adapter is push-only — it subscribes once and converges on pushed
`pane_*` events, never a poll loop (grep-able standing rule; the gh
plane is the sanctioned exception, poll-by-design at one round-trip
per poll).

Secrets are redacted once, at the adapter boundary (`src/core/redact.rs`),
before any bytes leave the machine. The APNs path re-redacts anyway — see
[Trust boundaries](#trust-boundaries).

### Side readers (not part of the store)

Two subsystems answer from outside the agent read model, because their
inputs are files on disk rather than plane events:

- **Cost meter** (`src/cost/`, `GET /cost`) — reads each provider's own
  session store directly: `opencode.db` (SQLite, read-only, bounded — the
  file runs to double-digit GB, so no unbounded scans), Claude Code JSONL
  transcripts, and codex rollouts. Usage is aggregated from the stores'
  own timestamps into rolling 5h / weekly / monthly windows, then divided
  by a per-provider cap to produce a percentage and an `ok`/`warning`/
  `problem` status. A missing store yields `store_found: false`, never a
  zero that would read as "nothing spent". **Caps and the claude/codex
  pricing table are placeholders** until real plan limits are supplied;
  synthetic caps are flagged `cap_is_placeholder: true` so no client can
  present them as fact.
- **Fleet registry** (`src/fleet/`, `corrald fleet list|check`) — parses
  and validates the `fleets.json` control-plane registry. Read-only in
  this phase: no mutation, no process control, and the daemon runtime path
  never touches it (the subcommands dispatch before the tokio runtime is
  built).

## Write side: the signed drive plane

```
client (device Ed25519 keypair)           corrald
───────────────                           ──────
signed envelope {key_id, signature,       POST /drive
  envelope{request_id, capability,          1. parse envelope (unknown
  target, payload, rev}}                       capability → typed error)
                                           2. DeviceAuthorizer::verify
                                              default deny: unknown key,
                                              revoked, expired, bad
                                              signature, NotGranted
                                           3. payload parse (typed per
                                              capability)
                                           4. replay-table claim keyed
                                              by request_id (idempotent:
                                              exactly-once dispatch)
                                           5. approve claims: check
                                              approval_id + exact
                                              prompt_hash against the
                                              LIVE prompt
                                           6. step-up gate for
                                              destructive payloads
                                              (X-Step-Up-Token)
                                           7. dispatch to adapter
                                           8. audit append (hash-chained)
```

- Signatures cover `canonical_envelope_bytes` — the fixed-order JSON
  serialization of `DriveEnvelope` — so client and daemon agree without
  sharing serialization code. `crates/corrald-client` mirrors the wire
  types field-for-field and signs the identical bytes; the R1–R10
  conformance suite proves both sides against a real corrald.
- Writes are idempotent by `request_id`: the daemon stores the first
  response and serves it byte-identical on retry.
- Approvals are **claim-based** (D8): `approval_id = <agent_id>:<prompt_hash>`,
  and the reply's `prompt_hash` must match the agent's current, live
  prompt hash — the wrong-question race is refused with a typed 409
  before any dispatch.
- The daemon never sends keys by coordinates: the adapter resolves the
  canonical `agent_id` to its own transport target.

## Capabilities

`prompt`, `interrupt`, `approve`, `read_tail` (bounded 200 lines /
32 KiB, on tap only), `kill`, `attach` — the closed set in
`src/drive/mod.rs`. Anything else is refused with a typed error before
dispatch.

## Security model

- **Loopback only.** `corrald` refuses to bind a non-loopback address.
  The read plane is credential-free *because* it is loopback-local.
- **Three credentials, never one** (D13): registration token (routing
  only, gates `POST /register`), per-device Ed25519 keypair (authenticates
  writes; host identity is X25519, published by `GET /host-key`), and
  per-capability grants (read-only default, promoted by the host via
  `POST /grants` with the admin token). Expiry (90 days) + revocation are
  checked on every verify.
- **Default deny, no auto-approve.** A fresh device has zero grants.
- **Step-up** for destructive patterns (`rm -rf`, `push --force`,
  `curl | sh`, `~/.aws`, `~/.ssh`, `.env`): 5-minute single-use token
  minted by `POST /step-up` only after the device proves key possession.
- **Audit log**: append-only, SHA-256 hash-chained, `0600`; grows only on
  drive writes (executions + dispatch refusals) — never on GETs or auth
  failures.
- Key material is persisted `0600` under a `0700` dir; secrets are never
  logged; the release binary exposes no secret accessors.

### Trust boundaries

Four places where data crosses a trust line, and what guards each:

1. **herdr / git / gh → the read model.** Adapter output is untrusted text
   (agent panes contain whatever an agent printed). Redaction runs at the
   adapter boundary *before* facts enter the store — `sk-ant-*`, `ghp_*`,
   `AKIA*`, high-entropy strings, `.env`-shaped content.
2. **Device → daemon (writes).** Loopback is not authentication. Every
   `POST /drive` is Ed25519-signed over a canonical envelope, checked
   against a registered, unexpired, unrevoked key, then against that key's
   grants, then against the step-up gate for destructive payloads.
3. **Daemon → Apple (APNs egress).** This is the only path where fleet
   content leaves the machine. Payloads are re-redacted at build time —
   the adapter's redaction is not trusted to have been sufficient — and
   bounded to the APNs size limit.
4. **Lock screen → daemon.** A canned reply carries the `prompt_hash` of
   the notification it came from; the daemon refuses it if the live prompt
   has moved on. Destructive payloads still require biometric step-up, and
   the check is server-side — a compromised client cannot skip it.

The session stores the cost meter reads (`opencode.db`, claude JSONL,
codex rollouts) are opened **read-only and bounded**; they are inputs, and
corral never writes to another tool's state.

## Clients

- `crates/corrald-client` — shared client layer: typed read model,
  reconnecting SSE with resume, signed drive with idempotent retries,
  step-up flow, approval claims. No GUI.
- `clients/egui` (`corrald-ui`) — desktop fleet board (egui/wgpu), macOS +
  Linux. Device keys in the OS keychain; auto-register on localhost; drive
  buttons rendered from `agent.capabilities` + the device grant ledger;
  the COST column and per-provider cost tiles.
- `ios/FleetNotifier` — SwiftUI iOS client: SSE read model, signed drive,
  APNs registration, and canned lock-screen replies bound to
  `prompt_hash`. See the README's Status section for what is and is not
  verified on hardware.

## Layout on main

```
src/main.rs          binary: arg parsing (loopback enforced), auth plane,
                     planes supervisor, axum serve
src/lib.rs           library surface
src/adapters/        herdr (push), git_plane, gh_plane, Adapter trait
src/core/            canonical model, events (plane channel), store,
                     redaction
src/integrate/       plane-channel drain folding git/gh facts onto agents
src/drive/           frozen P3 contract: capabilities, envelope, signing,
                     authorizer/audit traits
src/approve/         claim-based approvals (prompt_hash checks)
src/auth/            host identity, device registry, authorizer, step-up,
                     hash-chained audit, HTTP routes
src/api/             router, /snapshot /events /history /cost /healthz,
                     POST /drive, POST /device-token
src/history/         D23 event ring (rotating JSONL) + D33 daily digest
src/cost/            per-provider spend readers (opencode.db, claude JSONL,
                     codex rollouts), rolling windows, pricing, caps
src/fleet/           fleets.json registry: parse + validate (read-only)
src/push/            APNs provider, payload build + redaction, transition
                     notifier
crates/corrald-client/  shared client layer + R1–R10 conformance suite
clients/egui/        corrald-ui desktop board (board, cost tiles, audit)
ios/FleetNotifier/   iOS client: SSE, drive, APNs, lock-screen replies
tests/               integration tests per module
```

Phase briefs (P1–P4) and the normative wire contract live in
`docs/corral/` — see the README docs index.
