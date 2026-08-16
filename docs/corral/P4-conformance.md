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

## Drive wire shapes (normative)

```
SignedDrive   { key_id: String, signature: String, envelope: DriveEnvelope }
DriveEnvelope { request_id: String, capability: "prompt"|"interrupt"|"approve"|"read_tail"|"kill"|"attach",
                target: String (agent_id), payload: Value, rev: Option<u64> }
```
- `signature` = device Ed25519 signature over the canonical envelope bytes
  (the exact JSON serialization of `DriveEnvelope` — deterministic field
  order; serde_json::to_vec on the struct).
- Payloads (tagged):
  - prompt: `{"kind":"prompt","text":String}`
  - read_tail: `{"kind":"read_tail","lines":Option<u32>}` (clamped 1..=200)
  - approve: `{"kind":"approve","approval_id":String,"prompt_hash":String,"choice":String}`
- Responses: success → 200 `DriveResponse {request_id, ok, error?, rev, result?}`;
  typed refusals ride the body (`ok:false` + `error`). Client errors:
  - 400 `bad_request` / `unknown_capability` / `missing_signature`
  - 401 `bad_signature` / `step_up_failed`
  - 404 `unknown_key` / `unknown_agent`
  - 403 `expired` / `revoked` / `not_granted` / `step_up_required`
  - 409 `in_flight` / `no_waiting_approval` / `stale_approval` / `hash_mismatch`
  - 422 `payload` / `choice_not_in_menu` / `cannot_approve_kind`
- Idempotency: same `request_id` → same stored response, never double-send.
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

## Open questions resolved by default (noted on #12/#13)

- Rust crate layout: in-repo cargo workspace (`crates/`) — W1 decides;
  additive only, corrald binary untouched.
- iOS key storage: Secure Enclave/Keychain on device, Keychain-shim on
  simulator.
- Desktop key storage: macOS Keychain where available, 0600 file fallback
  with startup warning.
- No backend changes for P4. Any needed daemon change = additive PR through
  the same gauntlet.
