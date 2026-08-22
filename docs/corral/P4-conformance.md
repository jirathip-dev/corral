# Corral P4 — W1 wire-level conformance contract

The daemon API on main is frozen (P3 contract). Every P4 client (Rust egui
desktop, SwiftUI iOS) must pass this conformance suite against a REAL
corrald. This document is the contract W1 implements as tests and W2/W3
build against — the wire shapes below are normative, taken verbatim from
src/drive/mod.rs and src/api/* on main.

## Endpoints

| Endpoint | Method | Auth | Purpose |
|---|---|---|---|
| `/healthz` | GET | none | liveness |
| `/snapshot` | GET | none | full state `{schema_version, rev, generated_at, agents}` |
| `/events` | GET | none | SSE; resumes from `Last-Event-ID` (snapshot when cursor stale, deltas `{rev, upd, del}` otherwise) |
| `/host-key` | GET | none | host identity `{algorithm: "X25519", public_key}` |
| `/register` | POST | registration token in body | `{token, public_key}` → `{key_id, grants, expiry_ts}`; read-only default |
| `/step-up` | POST | device signature | `{key_id, signature, request}` → `{token, expires_ts}`; single-use, 5 min, freshness `\|now-ts\| < 60s` |
| `/grants` | POST | admin Bearer token | `{action: set_grants\|revoke, key_id, grants[]}` |
| `/audit` | GET | admin Bearer token | `{entries[], valid}` — hash-chained log, grows only on writes |
| `/drive` | POST | device signature | signed command envelope |
| `/issues` | GET | none (read-only) | `{repos: {repo: [GhIssueRef…]}}` — repo-level issue view |

## Drive wire shapes (normative)

```
SignedDrive   { key_id: String, signature: String, envelope: DriveEnvelope }
DriveEnvelope { request_id: String, capability: "prompt"|"interrupt"|"approve"|"read_tail"|"kill"|"attach"|"start_worktree",
                target: String (agent_id), payload: Value, rev: Option<u64> }
```
- `signature` = device Ed25519 signature over the canonical envelope bytes
  (the exact JSON serialization of `DriveEnvelope` — deterministic field
  order; serde_json::to_vec on the struct).
- Payloads (tagged):
  - prompt: `{"kind":"prompt","text":String}`
  - read_tail: `{"kind":"read_tail","lines":Option<u32>}` (clamped 1..=200)
  - approve: `{"kind":"approve","approval_id":String,"prompt_hash":String,"choice":String}`
  - start_worktree (fleet-level; `target` is the fleet/repo name, not an agent id):
    - issue-linked: `{"kind":"start_worktree","mode":"issue","repo":String,"number":u64,"issue_url":String}`
    - issue-free: `{"kind":"start_worktree","mode":"free","repo":String,"name":String}`
- `start_worktree` result (`result` on `ok:true`):
  - started: `{"state":"started","branch":String,"path":String,"handoff":"launched"|"deferred"}`
  - already-started (idempotent replay): `{"state":"already_started","branch":String,"path":String}`
  - typed `error_kind`s: `unknown_fleet`, `issue_not_found`, `issue_closed`,
    `already_started`, `invalid_name`, `git_failure`, `launch_failure`
- Responses: success → 200 `DriveResponse {request_id, ok, error?, error_kind?, rev, result?}`;
  typed refusals ride the body (`ok:false` + human `error` and stable
  `error_kind`). Client errors:
  - 400 `bad_request` / `unknown_capability` / `missing_signature`
  - 401 `bad_signature` / `step_up_failed`
  - 404 `unknown_key` / `unknown_agent`
  - 403 `expired` / `revoked` / `not_granted` / `step_up_required`
  - 409 `in_flight` / `no_waiting_approval` / `stale_approval` / `hash_mismatch` /
    `stale_agent`
  - 422 `payload` / `choice_not_in_menu` / `cannot_approve_kind`
- Idempotency: same `request_id` → same stored response, never double-send.
- `stale_agent` means the daemon knew the target but its current Herdr session
  disappeared or moved. It is a conflict, not an unknown target: the daemon
  does not claim a replay or append an audit entry for the pre-dispatch 409.
  Clients must remove/disable the stale row and fetch a fresh snapshot; the
  live SSE stream remains authoritative. A narrow adapter race may return the
  same typed `error_kind` in a 200 refusal body after the dispatch claim, with
  the same client recovery behavior.
- Herdr `agent_not_found` replies are awaited for `read_tail`, `prompt`, and
  `approve`. The adapter captures the canonical mapping generation and exact
  wire target used by the RPC; it retires the mapping, tombstones the agent,
  and removes the canonical store row under a Store-side predicate that
  confirms no newer live mapping exists at removal time. Same-generation
  status/integration updates do not block cleanup. A late reply from an older
  pane/target generation is classified stale for that request but cannot
  retire the newer mapping or its row. Mapping generations are allocated from
  a monotonic adapter-lifetime counter while only live mappings retain map
  entries; event-derived read/modify/write upserts use the same generation
  predicate at Store commit, so an in-flight event cannot resurrect a retired
  row. Tombstones are bounded by TTL and capacity.
- Step-up: destructive payloads require `X-Step-Up-Token` header (minted via
  `/step-up`); failures are never audited.

## Approval claims (D8, load-bearing)

- Claim identity: `approval_id = "<agent_id>:<prompt_hash>"`.
- `prompt_hash` = `sha256:` + hex of the SHA-256 of the EXACT untrimmed,
  redacted prompt string in the snapshot's `waiting_on.prompt`. Clients MUST
  hash the snapshot string byte-for-byte — never raw pane text.
- Refusal precedence: no waiting approval → 409 `no_waiting_approval`; id
  mismatch → 409 `stale_approval`; hash mismatch → 409 `hash_mismatch` (the
  wrong-question race kill — distinct from stale); menu choice not in
  `choices[]` → 422 `choice_not_in_menu`; Crash kind → 422
  `cannot_approve_kind`.
- Both approval store reads classify a missing row at return time. If the
  adapter has acquired a stale tombstone during the lookup, the missing row
  is a 409 `stale_agent` rather than a misleading 404 `unknown_agent`.

## Conformance scenarios (both clients must pass, against a real corrald)

R1. **Register** — POST /register with the registration token + fresh device
     keypair → `key_id` with EMPTY grants (read-only default).
R2. **Read path** — GET /snapshot returns schema 3, monotonic rev, agents;
     GET /events with `Last-Event-ID: <rev>` resumes (snapshot or deltas) and
     delivers live deltas.
R3. **Signed drive executes** — grant `prompt` (admin /grants), sign an
     envelope over canonical bytes, POST /drive → 200 `ok:true`, response rev
     ≥ request rev.
R4. **Tampered refused** — same envelope, payload mutated after signing →
     401 `bad_signature`; zero dispatch.
R5. **Read-only denied** — fresh device, no grants → 403 `not_granted`; zero
     audit growth.
R6. **Replay idempotent** — same request_id twice → byte-identical responses,
     exactly one dispatch.
R7. **Stale hash refused** — approve with current `approval_id` + WRONG
     `prompt_hash` → 409 `hash_mismatch`, zero dispatch, zero audit.
R8. **Matching approve executes** — correct `approval_id` + `prompt_hash` +
     choice ∈ choices → 200, dispatch exactly once, audit +1.
R9. **Step-up** — `rm -rf ...` payload without token → 403 `step_up_required`
     (audit 0); mint via /step-up, retry with header → 200, audit +1; token
     replay → 401 `step_up_failed`.
R10. **Audit grows only on writes** — GETs, auth failures, step-up failures
     never grow the log; each executed/refused-at-dispatch drive does.
R11. **Stale target recovery** — a target that disappears or migrates before
     dispatch returns `stale_agent`; no pre-dispatch replay/audit is created,
     and both clients remove the row and refresh their snapshot. The checked-in
     evidence is the API regression suite plus the Herdr adapter's hermetic
     JSON-RPC error/migration tests. A local `UnixListener` mock is not a live
     Herdr socket and must not be reported as live migration proof.

     A bounded read-only probe on 2026-08-20 sent one `agent.list` request to
     the configured Herdr Unix socket and received a valid response containing
     11 agents. It proves socket reachability and current-list decoding only;
     it does not prove a live drive, migration, disappearance, or stale-event
     recovery.

## Open questions resolved by default (noted on #12/#13)

- Rust crate layout: in-repo cargo workspace (`crates/`) — W1 decides;
  additive only, corrald binary untouched.
- iOS key storage: Secure Enclave/Keychain on device, Keychain-shim on
  simulator.
- Desktop key storage: macOS Keychain where available, 0600 file fallback
  with startup warning.
- No backend changes for P4. Any needed daemon change = additive PR through
  the same gauntlet.
