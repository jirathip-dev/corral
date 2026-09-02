# Corral P4 — wire-level conformance contract (read-only since #354)

> **#354 (read-only cut):** the daemon's mutating drive surface is GONE.
> Capabilities `prompt`, `interrupt`, `approve`, `kill`, `attach`,
> `start_worktree`, `read_issues` are refused with a typed
> `400 unknown_capability` BEFORE the authorizer, before dispatch, and
> before the audit log. The `/step-up` endpoint, the approval-claim
> machinery, the terminal/attach transport (tmux), and the fleet/worktree
> module were removed with them. The read-only contract below is what
> remains; every P4 client (Rust egui desktop, SwiftUI iOS) must pass the
> conformance suite against a REAL corrald.

## Endpoints

| Endpoint | Method | Auth | Purpose |
|---|---|---|---|
| `/healthz` | GET | none | liveness |
| `/snapshot` | GET | none | full state `{schema_version, rev, generated_at, agents}` |
| `/events` | GET | none | SSE; resumes from `Last-Event-ID` (snapshot when cursor stale, deltas `{rev, upd, del}` otherwise) |
| `/host-key` | GET | none | host identity `{algorithm: "X25519", public_key}` |
| `/register` | POST | registration token in body | `{token, public_key}` → `{key_id, grants, expiry_ts}`; read-only default |
| `/grants` | GET | admin Bearer token | `{ok, devices[]}` — registered device key ids, grants, revoked, expiry/created ts; no public keys or push tokens (#137) |
| `/grants` | POST | admin Bearer token | `{action: set_grants\|revoke, key_id, grants[]}` — the grant set is the closed read set |
| `/audit` | GET | admin Bearer token | `{entries[], valid}` — hash-chained log, grows only on drive reads |
| `/drive` | POST | device signature | signed read envelope (`read_tail`, `read_diff`) |

Removed in #354: `POST /step-up` (biometric token mint).

## Drive wire shapes (normative)

```
SignedDrive   { key_id: String, signature: String, envelope: DriveEnvelope }
DriveEnvelope { request_id: String, capability: "read_tail"|"read_diff",
                target: String (agent_id), payload: Value, rev: Option<u64> }
```
- `signature` = device Ed25519 signature over the canonical envelope bytes
  (the exact JSON serialization of `DriveEnvelope` — deterministic field
  order; serde_json::to_vec on the struct).
- Payloads (tagged):
  - read_tail: `{"kind":"read_tail","lines":Option<u32>}` (clamped 1..=200)
  - read_diff: `{"kind":"read_diff"}` — worktree files, diffstat, paged diff
- Any other capability string (including every legacy mutating name) is a
  400 `unknown_capability` refusal before the authorizer — the capability
  set is closed at parse time, so a stale client cannot get past the gate.
- Responses: success → 200 `DriveResponse {request_id, ok, error?, error_kind?, rev, result?}`;
  typed refusals ride the body (`ok:false` + human `error` and stable
  `error_kind`). Client errors:
  - 400 `bad_request` / `unknown_capability` / `missing_signature`
  - 401 `bad_signature`
  - 404 `unknown_key` / `unknown_agent`
  - 403 `expired` / `revoked` / `not_granted`
  - 409 `in_flight` / `stale_agent`
  - 422 `payload`
- Idempotency: same `request_id` → same stored response, never double-send.
- `stale_agent` means the daemon knew the target but its current Herdr session
  disappeared or moved. It is a conflict, not an unknown target: the daemon
  does not claim a replay or append an audit entry for the pre-dispatch 409.
  Clients must remove/disable the stale row and fetch a fresh snapshot; the
  live SSE stream remains authoritative. A narrow adapter race may return the
  same typed `error_kind` in a 200 refusal body after the dispatch claim, with
  the same client recovery behavior.
- Herdr `agent_not_found` and `pane_not_found` replies are awaited for the
  read arms. The adapter captures the canonical mapping generation and
  exact wire target/pane used by the RPC; it retires the mapping, tombstones
  the agent, and removes the canonical store row under a Store-side predicate
  that confirms no newer live mapping exists at removal time. Same-generation
  status/integration updates do not block cleanup. A late reply from an older
  pane/target generation is classified stale for that request but cannot
  retire the newer mapping or its row. Mapping generations are allocated from
  a monotonic adapter-lifetime counter while only live mappings retain map
  entries; event-derived read/modify/write upserts use the same generation
  predicate at Store commit, so an in-flight event cannot resurrect a retired
  row. Tombstones are bounded by TTL and capacity.

## Conformance scenarios (both clients must pass, against a real corrald)

R1. **Register** — POST /register with the registration token + fresh device
     keypair → `key_id` with EMPTY grants (read-only default).
R2. **Read path** — GET /snapshot returns schema 3, monotonic rev, agents;
     GET /events with `Last-Event-ID: <rev>` resumes (snapshot or deltas) and
     delivers live deltas.
R3. **Signed read executes** — grant `read_tail` (admin /grants), sign an
     envelope over canonical bytes, POST /drive → 200 `ok:true`, response rev
     ≥ request rev.
R4. **Tampered refused** — same envelope, payload mutated after signing →
     401 `bad_signature`; zero dispatch.
R5. **Read-only denied** — fresh device, no grants → 403 `not_granted`; zero
     audit growth.
R6. **Replay idempotent** — same request_id twice → byte-identical responses,
     exactly one dispatch.
R7. **Removed capability refused** — a correctly signed `prompt` envelope
     (or any other mutating name) → 400 `unknown_capability` BEFORE the
     authorizer: an unregistered key gets the same 400, zero audit growth.
R8. **Audit grows only on reads** — GETs and auth failures never grow the
     log; each executed/refused-at-dispatch drive read does.
R9. **Stale target recovery** — a target that disappears or migrates before
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

Removed with #354 (superseded scenarios): R7-old stale-hash approve,
R8-old matching approve, R9-old step-up, R11 kill retirement, R12 attach
handle — the capabilities they exercised no longer exist.

## Open questions resolved by default (noted on #12/#13)

- Rust crate layout: in-repo cargo workspace (`crates/`) — W1 decides;
  additive only, corrald binary untouched.
- iOS key storage: Secure Enclave/Keychain on device, Keychain-shim on
  simulator.
- Desktop key storage: macOS Keychain where available, 0600 file fallback
  with startup warning.
- No backend changes for P4 beyond the approved #140 herdr kill/attach adapter
  fill. Any further daemon change = additive PR through the same gauntlet.
