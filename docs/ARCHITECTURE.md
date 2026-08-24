# Corral Architecture

`corrald` is a Rust daemon that collapses the herdr agent fleet into a
snapshot read model (served over HTTP + SSE — loopback by default,
tailnet/private interfaces allowlisted, never public) and exposes a
signed, capability-gated write plane (`POST /drive`). Design
authority: `docs/corral/DECISIONS.md` (D1–D14).

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

The canonical board model contains no provider-specific pricing, quota, or
transcript fields. Provider session stores are consulted only by the
on-demand transcript boundary (`GET /transcript`), whose store binding and
redaction live under `src/transcript/` and never feed the snapshot model.
The three transcript store roots are env-overridable
(`CORRAL_OPENCODE_DB`, `CORRAL_CLAUDE_DIR`, `CORRAL_CODEX_DIR`) so a
different install layout can be used without changing the core model.

**Implication for a "no-herdr" mode.** The adapter is the runtime coupling
point. A second-runtime / no-runtime mode would add an `Adapter`
implementation that derives agents from git worktrees + a mapping file (or
another runtime's socket) instead of herdr's. That is a documented limitation
today, not a bug.

## Read side: planes → integrator → store → HTTP/SSE

```
herdr socket ── herdr adapter (push: events.subscribe + bounded catalog refresh)
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
        GET /transcript (#63: on-demand session-transcript pages,
              newest first, redacted at the module boundary; the ONLY
              grant-gated GET — requires the `read_tail` grant via a
              signed envelope in the `x-corral-drive` header)
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

The gh plane also publishes the *repo-level* issue set it fetches into a
read-only `GET /issues` view (`src/api/issues.rs`), which the desktop board's
Issues tab renders — separate from the per-agent `closingIssuesReferences`
join in the snapshot. The write side has one fleet-level capability,
`start_worktree` (#113), routed by `src/api/drive.rs` to
`src/fleet/worktree.rs` (validate → plan → idempotent create → herdr
handoff) instead of the per-agent adapter dispatch. GitHub stays READ-ONLY:
no issue create/edit/close surface exists.

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

- **Transcript reader** (`src/transcript/`, `GET /transcript`) — binds an
  agent to its own tool's session store, reads one bounded page at a time,
  and redacts entries before they cross the module boundary. It is an
  on-demand, grant-gated read path; it does not compute provider usage or
  alter the fleet snapshot.
- **Fleet registry** (`src/fleet/`,
  `corrald fleet list|check|watch|add|remove|pause|resume|models|switch|reap|prune`)
  — parses, validates and atomically rewrites the `fleets.json`
  control-plane registry, and owns the CLI-only destructive ops half:
  auth-gated orchestrator switch, verified-agent reaping, and provably-dead
  worktree pruning. The daemon read path may consult the validated registry
  for primary-checkout attribution, but never mutates it; the registry
  subcommands dispatch before the tokio runtime is built and never talk to a
  running daemon.

### Workspace attribution

Repo/branch grouping is one shared read-side fact flow. The daemon seeds
explicit primary checkout roots from `CORRAL_REPO_ROOT` and, when present,
the fleet registry's `local` + `gh_repo` pairs. Registry roots have
precedence: if a registry `local` canonicalizes to `CORRAL_REPO_ROOT`, its
`gh_repo` basename wins over the configured directory basename. The git plane
probes those roots and the Herdr linked-worktree root; the integrator records
branch facts by canonical worktree path, and the Herdr adapter reads the same
facts while building a fresh agent record.

Path identity is raw-then-canonical, including symlinked `$HOME` and missing
path tails. A primary checkout must match a known root exactly. A linked
worktree uses the established `<worktrees_root>/<repo>/<label>` layout, with
the `<label>` treated only as a path component—not as branch identity.
Branches come from git HEAD facts; display names, pane labels, and terminal
titles never participate. A supervised git-plane restart clears the previous
generation's branch cache and the branch field on already-stored recognized
agents, then repopulates present paths from fresh probes. Repo identity and
the other workspace/GitHub fields survive that boundary; this prevents a
missed removal event from reviving a vanished worktree's old branch. Paths
that match neither source are not reconciled and remain
`workspace.repo: null`, therefore staying in the `(no repo)` orphan bucket.

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
- Herdr keeps the canonical agent-to-pane target and its reverse mapping under
  one state lock. A stable session moving panes evicts the old pane, and a
  disappeared or moved target leaves a stale-agent tombstone. Dispatch
  observes that tombstone as `stale_agent` (HTTP 409 before replay claim when
  possible; a typed refusal if the adapter loses the race). Desktop and iOS
  clients remove the stale row immediately and refresh their snapshot; the
  live SSE stream remains the authority for the replacement row.

## Capabilities

`prompt`, `interrupt`, `approve`, `read_tail` (bounded live tail served as
segmented blocks on `/drive`, plus the grant-gated paged `/transcript`
view that the client folds into the same Recent-output surface),
`kill`, `attach`, and the fleet-level `start_worktree` — the closed set
in `src/drive/mod.rs`. Anything else is refused with a typed error before
dispatch.

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
  writes; host identity is X25519, published by `GET /host-key`), and
  per-capability grants (read-only default, promoted by the host via
  `POST /grants` with the admin token; host-administration reads use the
  same token on `GET /grants`). Expiry (90 days) + revocation are checked
  on every verify.
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

Transcript session stores are opened **read-only and bounded**; they are
inputs, and corral never writes to another tool's state.

## Clients

- `crates/corrald-client` — shared client layer: typed read model,
  reconnecting SSE with resume, signed drive with idempotent retries,
  step-up flow, approval claims. No GUI.
- `clients/egui` (`corrald-ui`) — desktop fleet board (egui/wgpu), macOS +
  Linux. Device keys in the OS keychain; auto-register on localhost; drive
  controls rendered for the canonical capability set; enabled/disabled state
  and reason derive from `agent.capabilities` plus the grant ledger. Settings
  hosts the admin-token audit log and grant editor.
- `ios/FleetNotifier` — SwiftUI iOS client: SSE read model, signed drive
  (including Kill/Attach), the single Recent-output surface (live
  segmented blocks merged with paged older transcript pages), APNs registration,
  and canned lock-screen replies bound to `prompt_hash`. Disabled controls
  name a missing grant or an unadvertised capability. See the README's
  Status section for what is and is not verified on hardware.

## Layout on main

```
src/main.rs          binary: arg parsing (non-public bind allowlist), auth plane,
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
src/api/             router, /snapshot /events /history /healthz,
                     GET /transcript (read_tail-gated), POST /drive,
                     POST /device-token
src/transcript/      D35: per-store paged transcript readers (opencode
                     sqlite3-CLI, claude/codex backwards JSONL) +
                     agent→session binding by worktree; redaction inside
                     the module boundary
src/history/         D23 event ring (rotating JSONL) + D33 daily digest
src/fleet/           fleets.json registry: parse + validate + atomic CRUD + CLI ops
src/push/            APNs provider, payload build + redaction, transition
                     notifier
crates/corrald-client/  shared client layer + R1–R10 conformance suite
clients/egui/        corrald-ui desktop client (Board, Issues, Audit,
                     Registry, Settings)
ios/FleetNotifier/   iOS client: SSE, drive, APNs, lock-screen replies
tests/               integration tests per module
```

Phase briefs (P1–P4) and the normative wire contract live in
`docs/corral/` — see the README docs index.
