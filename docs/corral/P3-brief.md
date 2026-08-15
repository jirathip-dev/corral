# Corral — P3 brief: the drive plane (full, security-critical)

Branch: feat/corral-p3. P2 shipped the three data planes (PR #6, merged).
DECISIONS.md (hermes-brain/plans/corral/) is the authoritative design.
P3 is the WRITE side of corrald — everything from D3/D8/D10/D13 that turns
the snapshot/SSE read model into a safe remote control surface. P4 (UI)
builds on this; P3 must be complete and hardened first (Guy decision
2026-08-15: "keep P3 full before UI" — no P3-lite).

## Goal

`corrald` gains authenticated, claim-verified WRITE endpoints (drive plane)
on loopback, backed by the canonical agent model's `capabilities` and
`waiting_on` structures. A client can prompt, interrupt, approve, read tail,
kill, and attach — with every write idempotent, revision-ordered, and safe
against the "approve the wrong question" race.

## Scope (workstreams)

### W1 — Drive endpoints (POST)
- `POST /drive` with a typed command envelope:
  `{request_id, capability, target: {agent_id}, payload, rev (client's last)}`
- Capabilities honored from the canonical model (D7): `prompt`, `interrupt`,
  `approve`, `read_tail`, `kill`, `attach` — server REFUSES unknown or
  un-granted capabilities with a typed error.
- Idempotent: same `request_id` → same response, no double-send (D3).
  Response carries the new monotonic `rev`.
- NO send-keys-by-coordinates in the public API (D8 — banned).
- `read_tail` returns bounded tail (200 lines / 32KB, D5) — never prefetch.

### W2 — Claim-based approvals (D8, load-bearing)
- Adapter emits approval request: `{approval_id, prompt_hash, choices[]}`.
- Client replies `{approval_id, prompt_hash, choice}`; host REFUSES when the
  current prompt's hash doesn't match (kills the race + wrong-question
  attack). Hash the EXACT prompt text; no trimming.
- Blocked state (`waiting_on`) drives the UI: approve-tool vs answer-question
  vs menu vs crash are distinct kinds — never collapsed.

### W3 — Device-keypair signatures (D10)
- Host identity = X25519 public key (NOT hostname). `GET /host-key`.
- Per-device client keypairs (Secure Enclave/Keychain on iOS). Drive
  commands signed by the device key. localhost bearer token is NOT
  sufficient for writes.
- 3 credentials, never 1 (D13): host↔client registration token (routing
  only) + per-device keypair (writes) + per-capability grants (read-only
  default; promote on host). Expiry + revocation list.
- Biometric step-up for destructive patterns (rm -rf, push --force, curl|sh,
  ~/.aws, ~/.ssh, .env). Signed append-only audit log on host.
- Default deny. No auto-approve in v1.

### W4 — Hardening & hygiene (D9/D13)
- Secret redaction at the adapter BEFORE bytes leave the machine (sk-ant-,
  ghp_, AKIA, high-entropy, .env-shaped).
- Loopback bind only. No GUI. No new polling in the herdr adapter (P1 rule,
  grep-able).
- `unknown` status is first-class and rendered honestly.

## Non-negotiable
- Same quality bar as P1/P2: cargo build --release clean, clippy -D
  warnings clean, cargo test green, zero polling in the herdr adapter.
- Read-only GitHub from the daemon (D-083 discipline) — the drive plane
  drives herdr agents, NOT GitHub mutations.
- Schema: additive-only, versioned strictly (P1 rule).

## Acceptance criteria (verdict gate)
1. Signed drive command with a valid device key executes; unsigned/tampered
   is refused with a typed error; replay of a request_id is idempotent.
2. Approval with a stale prompt_hash is REFUSED; matching hash executes;
   demonstrated with a live herdr blocked agent (race simulation).
3. Read-only default: a device with only read grants cannot drive.
4. Interrupt/prompt/kill round-trips verified live against a herdr pane.
5. cargo build --release + clippy -D warnings + cargo test all green;
   audit log grows only on writes.
