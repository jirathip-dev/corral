# Corral — P4 brief: the client stack (egui desktop ∥ SwiftUI iOS ∥ shared client layer)

Branch: feat/corral-p4 (from main). P3 shipped the drive plane (PR #11,
merged). DECISIONS.md (hermes-brain/plans/corral/) is the authoritative
design — D14 (2026-08-15, Guy) fixes the stack: Rust egui desktop + SwiftUI
iOS ONLY, no SwiftUI on macOS, Windows is a free bonus target. This brief
ships the D14 v1.1 desktop surface + the iOS-first MVP ("Fleet Notifier").
P4 has NO merges of backend behavior: corrald on main is the contract.

## Goal

Two clients speak corrald's JSON/SSE read model and its SIGNED drive plane
directly. The desktop client is a dark-dashboard board (v1.1 surface); the
iOS client is the MVP "Fleet Notifier" (push-when-blocked, lock-screen
answer, per D5/D12). Both authenticate as registered devices (D10/D13:
device-keypair-signed writes, read-only default, claim-based approvals with
prompt_hash, biometric step-up for destructive patterns — all already
implemented server-side in P3).

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
  read_tail (bounded, on tap), approve (choice buttons from the claim),
  kill/attach (typed). Step-up prompt for destructive payloads.
- Device registration UX on first run (paste registration token / auto-
  register on localhost), keypair in local storage (OS keychain where
  available; documented fallback 0600 file with a warning).
- Speaks corrald directly on loopback (default) or Tailscale host.

**Dev build note — macOS keychain prompts (Guy 2026-08-16):** the dev
binary is unsigned, and `keyring` reads its device key from the macOS
Keychain. A fresh `cargo build` changes the (unsigned) identity, so macOS
re-prompts "corrald-ui wants to access key…" on EVERY launch of an
existing device. Fix after every rebuild in this worktree:

```
codesign -s - --force target/release/corrald-ui
```

Ad-hoc signing gives a stable CDHash, so the next launch prompts once and
"Always Allow" sticks. First-run/new devices don't prompt (keychain adds
are silent) — prompts appear when READING an item created by an older
binary identity. If a session hits the prompts again, the binary was
rebuilt without re-signing.

### W3 — iOS SwiftUI "Fleet Notifier" (MVP, iOS-first, per D12/TestFlight)
- Fleet list + blocked agents surfaced; push when blocked/done (APNs —
  P4 scope: the LOCAL notification path on the phone + a documented hook
  for a relay/push later; no relay in v1).
- Lock-screen / notification answer: canned replies (Approve/Deny/Continue)
  bound to prompt_hash — the D8 claim flow end to end.
- Keypair in Secure Enclave/Keychain (D10); read-only default; step-up via
  Face ID for destructive patterns.
- Tails: 200 lines on tap, never prefetch (D5). Backgrounded = no
  connection (D5).

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
   prompt/interrupt/read_tail/approve round-trip against real herdr agents
   through the signed drive plane (approval with correct prompt_hash
   executes; stale is refused; step-up required for destructive payloads).
2. iOS client (simulator + device where available): fleet renders, blocked
   agent surfaces the claim with choices, canned answer executes the
   approve with the correct prompt_hash, read-only default enforced.
3. W1 conformance suite green: register → read → sign → drive → step-up →
   approve against a real corrald, from BOTH client implementations.
4. Desktop: dark-dashboard theme pass done (not default egui flat).
5. Green gates per client; zero regressions on corrald.

## Risks

- App Store 4.2 (minimal functionality — demo mode with seeded data planned)
  and 3.1.1 (no IAP; hosted relay sells on web only) per D12.
- TestFlight logistics; no Apple Developer account in-repo (needs Guy).
- Secure Enclave/Keychain vs simulator constraints.
- egui theme pass effort (immediate-mode styling is manual).
- Daemon discovery: loopback default; Tailscale hostname config needed for
  remote. Relay is OUT of v1 (D1/D13: Tailscale direct is the default).

## Open questions for Guy (from DECISIONS.md + P4 scoping)

1. D14 licensing split: apps proprietary (GPLv3-family) — confirm before
   repo setup/TestFlight.
2. Open questions 1-4 in DECISIONS.md (Python-vs-Rust hermesd is moot —
   corrald is Rust; AGPL-vs-MIT for daemon half; MCP server distribution
   play; hosted relay in/out) — none block P4 code, but 2 + 3 affect repo
   layout and marketing.
3. P4 demo mode (seeded data) for App Review 4.2 — confirm scope.
4. iOS push: local notifications only for v1, or should P4 also stub the
   relay-side APNs hook? (Relay itself is out.)

## Sequencing

W1 (shared contract + conformance suite) first — W2 and W3 build against
it. All three can start after the W1 contract commit. Reviewers per the
corral gauntlet (opencode-only, adversarial, separate panes). No P4 merge
touches corrald behavior.
