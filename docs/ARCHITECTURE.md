# Corral Architecture

`corrald` is a Rust daemon that collapses the herdr agent fleet into a
snapshot read model (served over HTTP + SSE — loopback by default,
tailnet/private interfaces allowlisted, never public) and exposes a
signed, capability-gated read plane (`POST /drive`, read-only since
#354). Design authority: `docs/corral/DECISIONS.md` (D1–D14).

## Stack terminology (model → harness → runtime → control plane)

Precise terms matter when discussing "agnosticism". Four layers, bottom up:

| Layer | What it is | Examples | Corral's relationship |
|---|---|---|---|
| **Model** | the LLM itself | deepseek-v4-flash, fable, opus | configured, not coupled |
| **Harness** | the wrapper that gives a model tools (shell, files, browser) | **Claude Code, Codex CLI, OpenCode** | **interchangeable** — the adapter passes harness kinds through verbatim (`tool: tool.unwrap_or("unknown")`); no per-harness logic exists, which is the stronger form of agnosticism |
| **Runtime** | the layer that spawns/supervises harnesses in panes/worktrees | **herdr** | **the coupling point** — `src/adapters/herdr.rs` reads the herdr unix socket for the live agent feed |
| **Control plane** | the daemon on top | **corrald** | this repo |

**Harness-agnostic vs runtime-bound.** Corral is already
*harness-agnostic*: it does not care whether an agent is claude, codex, or
opencode — all flow through the same canonical `Agent` record. It is
*runtime-bound* to herdr at the adapter: without the herdr socket, the read
model has no live agent feed (the daemon still serves HTTP, just no
agents).

The core model, signed drive plane, and HTTP surface are runtime-neutral; the
current runtime coupling is isolated in the herdr adapter described above.

The canonical board model contains no provider-specific pricing or quota
fields. Provider session stores are never consulted: the only agent output
a client can read is the bounded `read_tail` pane tail, redacted and
segmented server-side into blocks, plus (since #232) the bounded
`read_diff` worktree diff page (changed-files list + paged unified diff +
diffstat), computed via libgit2 from herdr-owned worktrees.

**Implication for a "no-herdr" mode.** The adapter is the runtime coupling
point. A second-runtime / no-runtime mode would add an `Adapter`
implementation that derives agents from git worktrees + a mapping file (or
another runtime's socket) instead of herdr's. That is a documented limitation
today, not a bug.

## Read side: planes → integrator → store → HTTP/SSE

```
herdr socket ── herdr adapter (push: events.subscribe + bounded catalog refresh)
git worktrees ─ GitPlane (fsevents push + 10s topology / 60s status safety net;
              │  missing-source backoff + live 15m rediscovery; four-command budget)
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
(`src/core/model.rs`, `schema_version` 5, versioned for additive and breaking
changes). The herdr adapter is event-first: it subscribes to
`events.subscribe` and converges on pushed `pane_*` events, while a bounded
trusted `agent.list` reconciliation every 2s covers the silent-but-open
stream failure mode. The gh plane remains the sanctioned poller, at one
GraphQL round-trip per poll. The reader is the ONLY task that reads the
herdr socket, so a slow event consumer can never block it. A full events
channel retires
the stream as a resynchronization condition; the reader still drains a
pre-subscribe response. Subscription/connect failures retry in the stream
task with exponential backoff capped at 30 seconds; only the first failure
of an outage is WARN-logged. An accepted global stream that closes belongs to
the same outage domain: its close notification, re-bootstrap, and
resubscribe are delayed by that capped ladder. The ladder resets only after a
meaningful stable stream interval, so a successful subscribe response alone
cannot create a hot loop. A pane stream reconnects from its owning retry task
after the capped delay, and keeps its live forwarder owned until the stream
closes or the pane is removed or recreated. Pane removal or replacement
cancels that generation before a new one can attach; subscribe failures never
trigger a global re-bootstrap. Dropping the client aborts the reader so a
failed connection never leaks its descriptor (#105).

GitPlane scheduling is bounded independently of the Herdr stream: every
GitPlane git subprocess, whether started by an fsevents debounce, a status
sweep, or a registry rescan, must acquire one of four shared command permits.
Registry rescans are serialized because their topology reconciliation also
performs synchronous filesystem canonicalization. A probe admitted to the
budget keeps its normal five-second child timeout and still reports an
over-budget warning; time spent waiting for a permit is additional total
latency, not part of that child timeout. The budget is backpressure, not a
failure suppression or a revision reset. This keeps git-plane load from
consuming the executor needed by the Herdr reader and leaves Store/SSE
revisions owned solely by the existing coalescer.

A repo/container source whose `git worktree list` scan fails is tracked
independently of the command budget. The first failure emits one WARN for
that continuous failure period; retries use a 10s, 60s, then 5m backoff and
repeated failures stay at DEBUG. After 15m, the regular topology path stops
touching the source until the cold rediscovery pass retries it. A present but
temporarily failing source retains its last-known worktrees and commondir
topology, so suppression cannot manufacture `WorktreeRemoved` events; an
actually missing worktree directory is still removed. A source being
recreated re-arms immediately, and a successful scan emits a recovery log and
clears the failure period, so a later outage gets one new warning.

The cold rediscovery pass scans immediate Git checkouts under `~/Projects`
in addition to the fixed fallback root (configless #237: no `fleets.json`
registry read — the fleet registry stays fleet-ops' config and its physical
checkouts are discovered live). It replaces the primary-source set before
reconciling, so removed roots disappear and new Project checkouts produce
`WorktreeAdded` without a daemon restart. Healthy FSEvents, topology, and
status behavior remains unchanged; only unavailable-source handling and the
slow source-discovery safety net are special-cased.

An unknown `commondir/worktrees/` FSEvents path makes the watcher await its
throttled topology rescan so the newly discovered worktree can be debounced
immediately. The same serialized rescan guard is shared with the 10s topology
and 60s status safety paths: a safety rescan already in flight can delay
topology freshness, but event frames remain queued and the one-shot 400ms
retry covers registration that races the first scan. This is a bounded
freshness tradeoff in exchange for reconciling topology before probing it;
the watcher/safety overlap is covered by a deterministic regression test.

The gh plane also publishes the *repo-level* issue set it fetches into a
read-only `GET /issues` view (`src/api/issues.rs`), which the desktop board's
Issues tab renders — separate from the per-agent `closingIssuesReferences`
join in the snapshot. The view is scoped strictly to categories represented by
current Herdr adapter workspaces; its GitHub-origin poll specs rebuild from
those owned checkout/worktree facts, and topology changes prune stale issue
categories. GitHub stays READ-ONLY: no issue create/edit/close surface
exists (the signed `read_issues` drive arm and the `start_worktree` drive
that consumed a selected issue were removed in #354).

The host admin boundary also exposes `GET /grants` (#137): an admin-token
gated projection of registered device key ids, current grants, revocation,
and expiry/created timestamps for the desktop Settings grant editor. It
never returns device public keys or APNs push tokens, and `POST /grants`
remains the only mutation path. The admin token is sent only to these
host-side administration endpoints (`GET /grants`, `POST /grants`,
`GET /audit`) and is never embedded in a device-signed drive flow.

Secrets are redacted once, at the adapter boundary (`src/core/redact.rs`),
before any bytes leave the machine. The APNs path re-redacts anyway — see
[Trust boundaries](#trust-boundaries).

### Side readers (not part of the store)

Two subsystems answer from outside the agent read model, because their
inputs are files on disk rather than plane events:

- **Fleet identities** (`src/fleet/`, `corrald fleet switch <name>`,
  `GET /fleets`) — CONFIGLESS (#237): corral does not own, read, or write
  `fleets.json`. The fleet registry is fleet-ops' opinionated config
  (`herdr-fleet list|add|remove|pause|resume|models|switch` on
  `~/.config/fleet-operations/fleets.json`). `src/fleet/cli.rs` is the
  fleet-ops CLI validated identity path the daemon and `fleet switch`
  shell out to; the workspace `worktree`/`switch` modules build on it.
  The old registry subcommands (`list|check|watch|add|remove|pause|resume|
  models|reap|prune` with `--registry`) are superseded and removed.

### Workspace attribution

Repo/branch grouping is one shared read-side fact flow. Configless (#237):
the daemon seeds explicit primary checkout roots from `CORRAL_REPO_ROOT` and
the git plane discovers immediate `~/Projects` checkouts — NO registry
roots or aliases exist. Repo names are path-derived identities of those
live checkouts; the board categories are the live `workspace.repo` values.
The git plane probes those roots and the Herdr linked-worktree root; the
integrator records branch facts by canonical worktree path, and the Herdr
adapter reads the same facts while building a fresh agent record.

Path identity is raw-then-canonical, including symlinked `$HOME` and missing
path tails. A primary checkout must match a known root exactly. A linked
worktree uses the established `<worktrees_root>/<worktree_dir>/<label>`
layout. The first path component stays a directory-name fallback (no
registry alias), and the `<label>` is only a path component—never branch
identity. The GitHub facts plane folds PR and CI facts on the same repo
basename the fleet-ops validated list carries, and issue grouping keys are
the fleet-ops CLI validated fleet names. Branches come from git HEAD facts;
display names, pane labels, and terminal titles never participate. A
supervised git-plane restart clears the previous generation's branch cache
and the branch field on already-stored recognized agents, then repopulates
present paths from fresh probes. Repo identity and the other
workspace/GitHub fields survive that boundary; this prevents a missed
removal event from reviving a vanished worktree's old branch. Paths that
match neither source are not reconciled and remain `workspace.repo: null`,
therefore staying in the `(no repo)` orphan bucket. The `GET /fleets` view
serves the validated identity catalog only — no `fleets.json` projection
exists.

## Signed read plane (`POST /drive`, read-only since #354)

```
client (device Ed25519 keypair)           corrald
───────────────                           ──────
signed envelope {key_id, signature,       POST /drive
  envelope{request_id, capability,          1. parse envelope (a removed
  target, payload, rev}}                       capability name → typed
                                              400 unknown_capability,
                                              before the authorizer)
                                           2. DeviceAuthorizer::verify
                                              default deny: unknown key,
                                              revoked, expired, bad
                                              signature, NotGranted
                                           3. payload parse (typed per
                                              capability)
                                           4. replay-table claim keyed
                                              by request_id (idempotent:
                                              exactly-once dispatch)
                                           5. read dispatch to the
                                              adapter seam (read_tail /
                                              read_diff)
                                           6. audit append (hash-chained)
```

- Signatures cover `canonical_envelope_bytes` — the fixed-order JSON
  serialization of `DriveEnvelope` — so client and daemon agree without
  sharing serialization code. `crates/corrald-client` mirrors the wire
  types field-for-field and signs the identical bytes; the R1–R10
  conformance suite proves both sides against a real corrald.
- Reads are idempotent by `request_id`: the daemon stores the first
  response and serves it byte-identical on retry.
- The daemon never sends keys by coordinates: the adapter resolves the
  canonical `agent_id` to its own transport target.
- Herdr keeps the canonical agent-to-pane target and its reverse mapping under
  one state lock. A stable session moving panes evicts the old pane, and a
  disappeared or moved target leaves a stale-agent tombstone. Read dispatch
  observes that tombstone as `stale_agent` (HTTP 409 before replay claim when
  possible; a typed refusal if the adapter loses the race). Desktop and iOS
  clients remove the stale row immediately and refresh their snapshot; the
  live SSE stream remains the authority for the replacement row.
- Trusted catalog reconciliation is the second eviction rule: a stored herdr
  session absent from the fresh `agent.list` is evicted and tombstoned even
  when its old pane is still listed without a session id.
  A single session-less view is debounced to protect a live lane: the previous
  explicit id survives one omitted `agent_session`, and only two consecutive
  session-less catalog refreshes corroborate the demotion.

## Capabilities

The #354 read-only cut closed the drive plane to two signed reads:

`read_tail` (bounded live tail served as segmented blocks on `/drive`) and
`read_diff` (#232: bounded worktree diff page — diffstat + changed-files
list + paged unified diff, computed via libgit2, never a git subprocess,
restricted to herdr-owned worktree paths) — the closed set in
`src/drive/mod.rs`. Every mutating capability (`prompt`, `interrupt`,
`approve`, `kill`, `attach`, `start_worktree`, `read_issues`) and the
terminal/attach transport were removed; anything else is refused with a
typed `400 unknown_capability` before the authorizer, before any adapter
dispatch, and before the audit log. The step-up gate and the
claim-based-approval seam existed only for destructive payloads and were
removed with them.

## Security model

- **Loopback by default, public refused.** `corrald` binds `127.0.0.1`
  by default; `--bind` also accepts private (RFC 1918), Tailscale/CGNAT
  (100.64/10), and IPv6 unique-local addresses (#65) — public IPs and
  0.0.0.0 are hard refusals. The WRITE plane is device-signed everywhere.
  The READ plane — `/healthz`, `/snapshot`, `/events`, and `/history` — is
  credential-free: on loopback that is process-local trust — unless the
  loopback daemon is fronted by Tailscale Serve, where the boundary is
  the tailnet again; on a tailnet bind the boundary is the tailnet
  itself (WireGuard device auth) — expose the read plane (by bind OR by
  Serve) only on tailnets whose every device may see fleet state. An
  RFC 1918 bind has NO comparable boundary (any LAN device reads fleet
  state): permitted for lab setups, but prefer tailnet or loopback.
- **Three credentials, never one** (D13): registration token (routing
  only, gates `POST /register`), per-device Ed25519 keypair (authenticates
  signed reads; host identity is X25519, published by `GET /host-key`),
  and per-capability grants (read-only default, promoted by the host via
  `POST /grants` with the admin token; host-administration reads use the
  same token on `GET /grants`). Expiry (90 days) + revocation are checked
  on every verify.
- **Default deny, no auto-approve.** A fresh device has zero grants, and
  only `read_tail` / `read_diff` can ever be granted.
- **Audit log**: append-only, SHA-256 hash-chained, `0600`; grows only on
  signed drive dispatches (executions + dispatch refusals) — never on GETs
  or auth failures.
- Key material is persisted `0600` under a `0700` dir; secrets are never
  logged; the release binary exposes no secret accessors.

### Trust boundaries

Four places where data crosses a trust line, and what guards each:

1. **herdr / git / gh → the read model.** Adapter output is untrusted text
   (agent panes contain whatever an agent printed). Redaction runs at the
   adapter boundary *before* facts enter the store — `sk-ant-*`, `ghp_*`,
   `AKIA*`, high-entropy strings, `.env`-shaped content.
2. **Device → daemon (signed reads).** Loopback is not authentication.
   Every `POST /drive` is Ed25519-signed over a canonical envelope,
   checked against a registered, unexpired, unrevoked key, then against
   that key's grants. Only the two read capabilities exist, so the drive
   plane is read-only by construction.
3. **Daemon → Apple (APNs egress).** This is the only path where fleet
   content leaves the machine. Payloads are re-redacted at build time —
   the adapter's redaction is not trusted to have been sufficient — and
   bounded to the APNs size limit.
4. **Lock screen → daemon.** Removed with the #354 cut: notification
   replies were approve replies, and the approve capability no longer
   exists. Notifications are display-only state-change alerts.

## Clients

- `crates/corrald-client` — shared client layer: typed read model,
  reconnecting SSE with resume, signed read drive with idempotent
  retries. No GUI.
- `clients/egui` (`corrald-ui`) — desktop fleet board (egui/wgpu), macOS +
  Linux. Device keys in the OS keychain; auto-register on localhost; read
  rows rendered for the canonical capability set; enabled/disabled state
  and reason derive from `agent.capabilities` plus the grant ledger.
  Settings hosts the admin-token audit log and grant editor.
- `ios/FleetNotifier` — SwiftUI iOS client: SSE read model, signed read
  drive, the single Recent-output surface (live segmented blocks), APNs
  registration. Action controls were retired with the #354 cut. See the
  README's Status section for what is and is not verified on hardware.

## Layout on main

```
src/main.rs          binary: arg parsing (non-public bind allowlist), auth plane,
                     planes supervisor, axum serve
src/lib.rs           library surface
src/adapters/        herdr (push), git_plane, gh_plane, Adapter trait
src/core/            canonical model, events (plane channel), store,
                     redaction
src/integrate/       plane-channel drain folding git/gh facts onto agents
src/drive/           frozen P3 contract: capabilities (ReadTail/ReadDiff
                     only since #354), envelope, signing,
                     authorizer/audit traits
src/auth/            host identity, device registry, authorizer,
                     hash-chained audit, HTTP routes
src/api/             router, /snapshot /events /history /issues /healthz,
                     POST /drive (signed reads), POST /device-token
src/history/         D23 event ring (rotating JSONL) + D33 daily digest
src/push/            APNs provider, payload build + redaction, transition
                     notifier
crates/corrald-client/  shared client layer + R1–R10 conformance suite
clients/egui/        corrald-ui desktop client (Board, Audit, Registry,
                     Settings; Issues tab read-only)
ios/FleetNotifier/   iOS client: SSE, signed reads, APNs
tests/               integration tests per module (incl. the #354
                     read-only-cut probe in tests/readonly_cut.rs)
```

Phase briefs (P1–P4) and the normative wire contract live in
`docs/corral/` — see the README docs index.
