# Enrollment v1 — host-approved read-tail pairing and live revocation

Status: **implementation of the frozen #485 contract.** The frozen
contract text is `.proposal-485-owner-resolved.md`
(sha256 `bfa5e392ba04bc86d700876aeb8dcf4de83f9e221c8a9df613ed2d822799aaa4`,
independently reviewed by `review485-owner-r1`); owner decisions O1/O2 are
pinned: the owner channel is genuinely local-only, and a revoked device is
restored only through fresh explicit owner approval, `read_tail` only.
This document describes exactly what the daemon ships; it is not a
security certification.

## Channels

| Channel | Transport | Who | Operations |
|---------|-----------|-----|------------|
| Owner | unix socket `<config-dir>/owner.sock`, mode `0600` inside the `0700` config dir, accept-time peer-credential check (`LOCAL_PEERCRED` on macOS, `SO_PEERCRED` on Linux) | host owner only | `mint`, `pending`, `approve`, `revoke` |
| Device | the ordinary network listener (existing daemon port) | any client holding a code | `POST /enroll/redeem`, `POST /enroll/status` |

The owner operations have **no HTTP route** — the network listener `404`s
them even with a valid admin token (#354: per-device grants are never
mutated over HTTP). The admin token is not accepted on the owner socket:
the OS credential is the owner proof.

## Owner protocol (unix socket, v1)

One JSON request object per connection, terminated by `\n` or half-close
(≤ 8 KiB); one JSON response object follows.

```sh
printf '%s' '{"op":"mint","endpoint":"https://host.tailnet.ts.net"}' | nc -U "$HOME/.config/corral/owner.sock"
printf '%s' '{"op":"pending"}' | nc -U "$HOME/.config/corral/owner.sock"
printf '%s' '{"op":"approve","enrollment_id":"enr_..."}' | nc -U "$HOME/.config/corral/owner.sock"
printf '%s' '{"op":"revoke","key_id":"dev_..."}' | nc -U "$HOME/.config/corral/owner.sock"
```

- `mint` → `{"enrollment_id":"enr_<32 hex>","expires_ts":<u64>,"scope":["read_tail"],"payload":"<QR text>"}`.
  `endpoint` (the host's public `https://…` URL) is required and embedded
  in the payload; the single deadline is 300 s for the whole
  mint → redeem → approve flow.
- `pending` → `{"sessions":[{"enrollment_id","key_id"|null,"name"|null,"state":"pending"|"redeemed","expires_ts"}]}` —
  `key_id` appears once a device redeems; approve only the `key_id` your
  device shows (`name` is device-supplied and unverified). Approved
  sessions are terminal and not listed.
- `approve` → `{"ok":true,"key_id","grants":["read_tail"],"expiry_ts":<u64>}`.
  Creates the record for a fresh key or restores a revoked one — grants
  are `read_tail` **exactly**, never `read_diff`, never control. A retry
  after a lost response returns `enroll_already_approved` **with** the
  record fields (self-heal).
- `revoke` → `{"ok":true,"key_id","revoked":true}` (idempotent). Revoke
  cannot set grants, cannot clear `revoked`, cannot touch any other key.

Errors are `{"error":"<human>","code":"<machine>"}` with the codes below.
Requests carrying unexpected fields (e.g. a `grants` parameter) are
refused as `malformed_request` — the channel cannot express grant edits.

## Device protocol (HTTP, v1)

Both routes require `"v":1` (unknown versions are rejected) and the code
is the only credential.

```sh
curl -s -X POST http://127.0.0.1:8474/enroll/redeem \
  -H 'Content-Type: application/json' \
  -d '{"v":1,"code":"<b64>","public_key":"<b64 32B>","name":"iPhone"}'
curl -s -X POST http://127.0.0.1:8474/enroll/status \
  -H 'Content-Type: application/json' \
  -d '{"v":1,"code":"<b64>"}'
```

- `redeem` → `200 {"state":"pending","key_id":"dev_…","expires_ts":<u64>}` —
  a pending request, **never authority**. A key that already exists and is
  live is refused `409 already_registered`; a key that exists **revoked**
  enters the pending restore path (approval restores it, redeem never
  does).
- `status` → `200 {"state":"pending","expires_ts":…}` or
  `200 {"state":"approved","key_id","grants",…,"expiry_ts","revoked"}`.
  **Never 409**: a consumed-but-unapproved code still reports `pending`,
  so a lost redeem response cannot brick polling. If the daemon restarted
  mid-pairing (sessions are in-memory), `status` 404s/410s; the device
  should then probe its signed `POST /grants-read` once — “registered and
  `read_tail` granted” means pairing succeeded.

## Precedence and error codes

Order is pinned: **unknown code → 404 `enroll_unknown_code`; past
`expires_ts` → 410 `enroll_expired`; then session state.** (`now >=
expires_ts` matches the authorizer's expiry edge.)

| HTTP | code | when |
|------|------|------|
| 400 | `malformed_request` | bad JSON / missing fields / wrong or missing `v` / unexpected fields |
| 400 | `bad_public_key` | not 32-byte base64, or non-canonical/weak Ed25519 |
| 400 | `bad_name` | label fails the display-name rules |
| 404 | `enroll_unknown_code` | code never minted on this host (or the session is gone) |
| 409 | `enroll_redeemed` | code already consumed (replay / second device) |
| 409 | `already_registered` | redeem presents a live registered key |
| 410 | `enroll_expired` | redeem/status past the deadline |
| — | `enroll_pending_cap` | mint refused: 8 sessions already await action (fail-safe) |

Owner-socket errors: `enroll_unknown_session` (404-equivalent),
`enroll_not_redeemed`, `enroll_already_approved` (+ record fields),
`enroll_expired`, `unknown_key`, `registry_persist_failed`,
`owner_credential_mismatch`, `malformed_request`.

## QR payload v1

Strict canonical JSON text, fixed field order, no extra fields, no
whitespace; encoded verbatim into the QR:

```
{"v":1,"host_key":"<b64 X25519 host key>","endpoint":"https://<host>.<tailnet>.ts.net","code":"<b64 32B>","expires_ts":<unix s>,"scope":"read_tail"}
```

Contains no admin token, no registration token, no private key, no
long-lived secret. Devices must reject unknown `v`, missing/extra fields,
a non-`https` endpoint, an already-past `expires_ts`, and a `host_key`
mismatch against the live `GET /host-key`. **Honest pinning:** the
`host_key` comparison catches payload tampering and host-key rotation —
it does **not** stop a wholesale forged QR (attacker endpoint + attacker
key are mutually consistent); the remaining bindings are the human
hostname confirmation and TLS.

## Security properties (frozen contract, implemented)

- **Default deny unchanged:** `POST /register` still produces empty
  grants; only the explicit owner approve grants anything.
- **Codes:** 256-bit, single-use, consumed under the session lock (exactly
  one winner among racing redeems, even for different keys), constant-time
  compared, never logged, never persisted.
- **Disk first:** approve/restore and revoke commit to `registry.json`
  (temp+fsync+rename, 0600) **before** the live state changes; a persist
  failure reports `registry_persist_failed`, changes nothing in memory,
  and leaves the session retryable.
- **Revocation:** immediate (next signed read is refused), idempotent,
  per-device, restart-surviving.
- **Restore (O2):** only via a fresh mint → redeem → explicit approve;
  the restored record gets `read_tail` exactly with a fresh 90-day
  registration TTL; nothing restores automatically (redeem, status,
  grants-read and restarts all leave `revoked` untouched).
- **"Signed-drive success"** means the signed `read_tail` drive the
  record authorizes — never commit/push/dispatch/control.

## Tests

The full frozen C1–C7 matrix (replay, expiry, status-never-409, two-key
redeem race, fresh-owner restore, immediate revoke + restart persistence,
persist failure, HTTP route absence, peer-credential denial, default-deny
registration, read_tail-only witness) lives in `tests/enrollment.rs`;
unit-level precedence/cap/session rules in `src/auth/enrollment.rs`.

```sh
cargo test --test enrollment
cargo test --lib auth::enrollment
```
