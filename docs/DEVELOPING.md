# Corral Development Guide

Everything needed to work on `corrald` and `crates/corrald-client`.
All quality-gate commands below were run and verified on main
(2026-08-16).

## Workspace layout

A cargo workspace at the root: `members = ["crates/corrald-client"]`,
`default-members = [".", "crates/corrald-client"]` — so root-level
build/clippy/test cover **both** crates. Additive-only: new crates go
under `crates/`; `corrald` itself is never restructured.

```
src/main.rs              binary entrypoint: --socket/--port/--bind parsing
                         (refuses non-loopback binds), auth-plane init,
                         planes supervisor, axum serve
src/lib.rs               library surface: adapters, api, approve, auth,
                         core, drive, integrate
src/adapters/            herdr.rs (push, zero polling), git_plane.rs,
                         gh_plane.rs, mod.rs (Adapter trait)
src/core/                model.rs (canonical Agent, schema v3),
                         events.rs (Plane trait + channel),
                         store.rs (revisioned store, coalescing, resume),
                         redact.rs (secret redaction at the boundary)
src/integrate/           plane-channel drain; folds git/gh facts onto
                         agent records
src/drive/               the FROZEN P3 contract: Capability, DriveEnvelope,
                         SignedDrive, canonical_envelope_bytes,
                         DriveAuthorizer + AuditLog traits
src/approve/             claim-based approvals (approval_id, prompt_hash,
                         choice validation)
src/auth/                mod.rs (AuthPlane: registry, authorizer, step-up,
                         audit, tokens), host_identity.rs, registry.rs,
                         authorizer.rs, step_up.rs, audit.rs, http.rs
src/api/                 mod.rs (router: /healthz /snapshot /events),
                         drive.rs (POST /drive handler)
crates/corrald-client/   shared client layer: model, drive, keypair,
                         stepup, approval, sse, client; tests/conformance.rs
tests/                   auth.rs, drive.rs, http.rs, integration.rs,
                         store.rs, model.rs, redact.rs, git_plane.rs,
                         gh_plane.rs
docs/corral/             P1–P4 briefs (history) + P4-conformance.md
                         (normative wire contract)
```

## Quality gates (run all four before merging)

```sh
cargo build --release
cargo clippy --all-targets -- -D warnings
cargo test
cargo test -p corrald-client
```

Verified results on main:

| Gate | Result |
|---|---|
| `cargo build --release` | `Finished \`release\` profile [optimized]` (2m44s cold) |
| `cargo clippy --all-targets -- -D warnings` | clean, no warnings |
| `cargo test` | all green — 95 lib + 24 + 15 + 8 + 2 + 7 + 11 + 11 + 3 tests |
| `cargo test -p corrald-client` | 12 unit + 4 wire-format pins green; live suite `#[ignore]`d |

The R1–R10 conformance scenarios (register, read path + SSE resume,
signed drive executes, tamper refused, read-only denied, replay
idempotent, stale-hash refused, matching approve, step-up, audit growth)
run against a **real spawned corrald** with a fake herdr unix server —
fully self-contained, needs no live fleet:

```sh
cargo test -p corrald-client -- --ignored
```

Verified: 12/12 pass. This is the W1 acceptance bar shared by both P4
clients.

## Conventions

- **Zero polling in the herdr adapter.** Push from `events.subscribe`,
  converge on events, never a sleep-loop calling `herdr agent list`.
  The gh plane is poll-by-design (one GraphQL round-trip per poll, SWR);
  the git plane is fsevents push with a 10s sweep safety net. When in
  doubt, grep: `rg "sleep|interval" src/adapters/`.
- **Additive-only versioned schema.** New fields/variants extend the
  model (`SCHEMA_VERSION` bumps additively); existing shapes never
  change. The drive contract in `src/drive/mod.rs` is frozen — add
  capabilities, never mutate.
- **Same quality bar for every phase.** No phase skips the four gates
  above; the conformance suite grows with each wire change.
- **Signatures over canonical bytes.** `canonical_envelope_bytes` is the
  fixed-order struct serialization; never reorder struct fields in the
  drive contract, or signatures break.
- **Default deny, secrets never logged.** New endpoints default to
  unauthenticated-read or admin-token-gated; key material stays `0600`
  under `0700` dirs.
- **Typed errors everywhere.** Unknown capability / bad grant /
  no-waiting-approval etc. are typed refusals with stable HTTP mappings
  (see `docs/corral/P4-conformance.md`), never 500s.

## How to add a capability

1. **Contract** (`src/drive/mod.rs`): add the variant to `Capability`
   plus its `Display`/`FromStr` arms. Additive only — never change an
   existing variant. Add a typed `DrivePayload` variant and its
   `DrivePayload::parse` arm if the capability carries a payload.
2. **Grants**: nothing to do — `POST /grants` parses capability strings
   through `Capability::FromStr`, so the new name is grantable by
   default. Unknown strings fail loudly, so no typo can silently no-op.
3. **Dispatch** (`src/adapters/mod.rs`): extend `DriveCommand` and the
   herdr adapter's `drive()` match. Resolve the canonical `agent_id` to
   the transport target; return typed `DriveError`s.
4. **Approve seam** (`src/approve/mod.rs`): only if the capability
   answers a waiting prompt — wire it through `check_approval_claim`.
5. **Client** (`crates/corrald-client`): mirror the wire type and add a
   scenario to `tests/conformance.rs`.
6. **Gate**: run all four quality gates, then the live conformance suite
   against a scratch daemon (recipe in `tests/common/mod.rs` —
   `spawn_live_daemon`).

## Testing a scratch daemon by hand

The conformance suite spawns its own daemon; for manual poking use a
throwaway config dir and any free port (the live daemon on 8574 must
stay untouched):

```sh
CORRAL_CONFIG_DIR=/tmp/corral-dev CORRAL_REPO_ROOT=/tmp/corral-dev \
  CORRAL_WORKTREES_ROOT=/tmp/corral-dev \
  ./target/release/corrald --port 8599 --socket ~/.config/herdr/herdr.sock
```

Expected (verified): `GET /healthz` → `ok`; config dir minted with
`admin-token`, `host-key`, `registration-token`; with a throwaway repo
root the git plane logs a benign `worktree scan failed` warning (there
is no `.git` to scan).
