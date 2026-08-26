# Corral — P4 brief: the client stack (egui desktop ∥ SwiftUI iOS ∥ shared client layer)

Branch: feat/corral-p4 (from main). P3 shipped the drive plane (PR #11,
merged). `docs/corral/DECISIONS.md` is the authoritative design record — D14
(2026-08-15) fixes the stack: Rust egui desktop + SwiftUI
iOS ONLY, no SwiftUI on macOS, Windows is a free bonus target. This brief
ships the D14 v1.1 desktop surface + the iOS-first MVP ("Fleet Notifier").
P4 has no redesign of backend behavior: corrald on main is the contract,
with #140 as the approved additive herdr kill/attach adapter fill.

## Goal

Two clients speak corrald's JSON/SSE read model and its SIGNED drive plane
directly. The desktop client is a dark-dashboard board (v1.1 surface); the
iOS client is the MVP "Fleet Notifier" (push-when-blocked, lock-screen
answer, per D5/D12). Both authenticate as registered devices (D10/D13:
device-keypair-signed writes, read-only default, claim-based approvals with
prompt_hash, biometric step-up for destructive patterns — all already
implemented server-side in P3; #140 closes the remaining herdr kill/attach
adapter executions on that drive plane).

## Scope (workstreams)

### W1 — Shared client layer (foundation, both clients)
- A corrald protocol crate/lib shared by both clients: snapshot/SSE client
  (rev cursor, delta resume via Last-Event-ID), typed JSON models mirroring
  src/core/model.rs, and a SIGNED-DRIVE client: device keypair generation +
  registration (POST /register), envelope signing over
  canonical_envelope_bytes, typed error mapping (400/401/404/409/422/403),
  idempotent retries by request_id, step-up flow (POST /step-up +
  X-Step-Up-Token), approval claims (approval_id/prompt_hash echo, menu
  choices). NO shared UI code.
- Desktop: a Rust crate (corrald-client or standalone repo module); iOS: the
  same protocol implemented in Swift (models + a DriveClient) — the PROTOCOL
  is the shared contract, the code is not (Swift has no Linux target; D14).
  W1 defines the wire-level contract tests both clients must pass: a
  conformance suite against a real corrald (register → read → sign → drive
  → step-up → approve).

### W2 — Rust egui desktop client (macOS + Linux, v1.1 surface)
- eframe/wgpu, one codebase, dark-dashboard theme pass (egui::Visuals).
- Board: agent fleet (state/reason/waiting_on kinds distinct), worktree
  topology (repo/branch/dirty/ahead-behind), PR/CI columns, audit log view.
- Drive controls rendered from `capabilities`: prompt, interrupt,
  read_tail (bounded; egui Cards performs one automatic fetch only for its
  currently visible, attention-resolved selected card when the capability
  and device grant allow it; later pages remain explicit Load earlier
  requests, with no background/pane-wide prefetch), approve (choice buttons from the claim),
  kill (`pane.close`), attach (`terminal_ref`). Step-up prompt for
  destructive payloads.
- Device registration UX on first run (paste registration token / auto-
  register on localhost), keypair in local storage (OS keychain where
  available; documented fallback 0600 file with a warning).
- Speaks corrald directly on loopback (default) or Tailscale host.

**Dev build note — macOS keychain prompts (Guy 2026-08-16):** the dev
binary is unsigned, and `keyring` reads its device key from the macOS
Keychain. A fresh `cargo build` changes the (unsigned) identity, so macOS
re-prompts "corrald-ui wants to access key…" on EVERY launch of an
existing device.

PERMANENT FIX (preferred — one-time, survives any rebuild): give the
keychain item an any-app ACL so no binary identity ever prompts again.
After a first successful run (item exists), or to seed it fresh:

```
security add-generic-password -s corrald-ui \
  -a corral-device:<host-fingerprint> -w "$(openssl rand -base64 32)" -A
```

(`-A` = accessible to all applications; `-U` to update in place. Get the
fingerprint from the daemon: `curl http://127.0.0.1:<port>/host-key` →
SHA-256 of the `public_key` field, first 16 hex chars — same as
`keys.rs::host_fingerprint`. A fresh seed rotates the device identity;
re-registration is automatic against the localhost daemon.)

If you do NOT use the -A fix, re-sign after every rebuild (stable CDHash
makes the next launch prompt once, "Always Allow" sticks):

```
codesign -s - --force target/release/corrald-ui
```

First-run/new devices don't prompt (keychain adds are silent) — prompts
appear when READING an item created by an older binary identity. If a
session hits the prompts again, the binary was rebuilt without re-signing
or a scratch UI run churned a new restricted-ACL item.

### W3 — iOS SwiftUI "Fleet Notifier" (MVP, iOS-first)
- Fleet list + blocked agents surfaced; push when blocked/done (APNs —
  P4 scope: the LOCAL notification path on the phone + a documented hook
  for a relay/push later; no relay in v1).
- Lock-screen / notification answer: canned replies (Approve/Deny/Continue)
  bound to prompt_hash — the D8 claim flow end to end.
- Keypair in Secure Enclave/Keychain (D10); read-only default; step-up via
  Face ID for destructive patterns.
- Tails: the egui Cards surface may hydrate one visible selected card once,
  only when `read_tail` is advertised and granted; older pages remain on-tap
  and no other cards are prefetched (D5). The existing iOS detail-view
  auto-load/refresh behavior is unchanged by #207 and is not a fleet
  prefetch. Backgrounded = no connection (D5).
- Debug/simulator development retains a seeded deterministic demo fixture;
  Release/distribution builds expose only real registration, live SSE, and
  signed drives. The fixture is not an App Review or TestFlight product path.

## Non-negotiable

- corrald on main is the contract: no backend changes for P4 unless an
  additive, reviewed PR (same gauntlet) is opened and merged first.
- Same quality bar as P1-P3: build/clippy/test green per client; the W1
  conformance suite green against a real corrald.
- Device-keypair-signed writes ONLY (D10): a localhost bearer token is
  never used for writes. Read-only default (D13).
- No auto-approve (D13). Biometric step-up for destructive patterns.

## Acceptance criteria (verdict gate)

1. Desktop client: full board renders a live fleet from a real corrald;
   prompt/interrupt/read_tail/approve/kill/attach round-trip against real
   herdr agents through the signed drive plane (approval with correct
   prompt_hash executes; kill retires the row; attach returns a terminal
   handle; stale is refused; step-up required for destructive payloads).
2. iOS Debug client (simulator where available): fleet renders, blocked agent
   surfaces the claim with choices, canned answer executes the approve with
   the correct prompt_hash, read-only default enforced. Physical-device and
   TestFlight verification remain separate future gates.
3. W1 conformance suite green: register → read → sign → drive → step-up →
   approve against a real corrald, from BOTH client implementations.
4. Desktop: dark-dashboard theme pass done (not default egui flat).
5. Green gates per client; zero regressions on corrald.

## Risks

- App Review 4.2 evidence must use the real product path; the seeded fixture
  is Debug-only and is not shipped in Release.
- Distribution and physical-device verification require the eventual Apple
  Developer/TestFlight setup; no such result is claimed by this brief.
- Secure Enclave/Keychain vs simulator constraints.
- egui theme pass effort (immediate-mode styling is manual).
- Daemon discovery: loopback default; Tailscale hostname config needed for
  remote. Relay is OUT of v1 (D1/D13: Tailscale direct is the default).

## Open questions for Guy (from DECISIONS.md + P4 scoping)

1. D14 licensing split: apps proprietary (GPLv3-family) — confirm before
   repo setup/TestFlight.
2. Open questions 1-4 in DECISIONS.md (the separate Python-vs-Rust daemon is
   moot —
   corrald is Rust; AGPL-vs-MIT for daemon half; MCP server distribution
   play; hosted relay in/out) — none block P4 code, but 2 + 3 affect repo
   layout and marketing.
3. iOS push: local notifications only for v1, or should P4 also stub the
   relay-side APNs hook? (Relay itself is out.)

## Sequencing

W1 (shared contract + conformance suite) first — W2 and W3 build against
it. All three can start after the W1 contract commit. Reviewers per the
corral gauntlet (opencode-only, adversarial, separate panes). No P4 merge
touches corrald behavior apart from the approved #140 adapter fill.
