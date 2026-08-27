# Issue #249 evidence — board identity recovery after rebuild/reinstall

This bundle records the implementer-lane verification of #249 (g249/identity-recovery,
branch `g249/identity-recovery`): the egui board detects that its device key no longer
matches the registered key_id after a rebuild/reinstall, then either auto-recovers
(registration token + host-admin token) or surfaces the one-tap "Re-register + grant"
prompt — and the signed drive plane works immediately after, with zero manual keychain
surgery.

## What is captured

- `e2e.log` — the end-to-end test run (client's REAL wire layer against the REAL
  corrald router, loopback, in-process): register → grant → signed read_tail →
  wipe key material (simulated reinstall) → pre-fix 401 bad_signature reproduced →
  recovery re-register + grant restore → signed read_tail executes again.
  Full gate output for the same test:
  `cargo test -p corrald-ui --test identity_recovery -- --nocapture`
- `app-wiring.log` — the SAME journey driven through `CorralApp` itself (the
  startup hook detects the mismatch and auto-recovers; the recovered identity is
  registered and granted with no user action):
  `cargo test -p corrald-ui --lib identity_auto_recovery_completes_through_the_app_wiring`
- `live-recovery.log` — the pre-existing LIVE conformance suite
  (`clients/egui/tests/live.rs`, ignored by default) run against a REAL
  throwaway `corrald` binary (scratch `CORRAL_CONFIG_DIR`, port 9411, reading
  the live herdr socket for the agent catalog — read-only): real agents, real
  read_tail executions, real re-register rotation, and the typed
  `bad_signature` refusal for the stale key. Commands:
  `CORRALD_URL=http://127.0.0.1:9411 CORRAL_CONFIG_DIR=<scratch> CORRAL_UI_CONFIG_DIR=<scratch> CORRAL_UI_DISABLE_KEYRING=1 cargo test -p corrald-ui --test live -- --ignored --nocapture --test-threads=1`
- `binary-smoke.log` — the REAL `corrald` debug binary with a throwaway
  `CORRAL_CONFIG_DIR` on loopback port 9410: boot, GET /host-key, POST /register,
  host-admin grant via `scripts/corrald-grant.sh`, re-register + re-grant
  (the recovery steps against the real daemon process).
- `README.md` (this file) — acceptance mapping + how to reproduce.

## Acceptance mapping

| Acceptance criterion | Evidence |
|---|---|
| Board detects key != registered config after rebuild/reinstall | startup detection in `CorralApp::apply_fingerprint` → `check_identity_recovery`; first bad_signature refusal in `on_drive`; unit tests `identity_detection_ignores_consistent_keys_but_flags_mismatch`, `identity_detection_kicks_auto_recovery_when_the_token_exists`; e2e step 3 |
| Auto-re-registers via registration token OR surfaces a one-tap 'Re-register + grant' prompt (read_tail,prompt,interrupt,approve,kill,attach) | `try_start_recovery` + banner `identity_recovery_banner` (Unit test `identity_banner_renders_one_tap_recovery_and_resolves_state`); auto path verified in e2e steps 4-5 |
| No manual terminal-side reset (keychain delete) required | recovery is entirely in-app: re-register current key + restore grants (e2e steps 4-5); no keychain deletion anywhere |
| Signed drive plane works immediately after recovery (read_tail ok, no bad_signature) | e2e step 5 asserts `DriveOutcome::Ok` with the recovered key + lines served; pre-fix step 3 asserts the 401 bad_signature that recovery eliminates |

## Reproduce

```sh
# 1. wire-level end-to-end (in-process router + real client wire layer)
cargo test -p corrald-ui --test identity_recovery -- --nocapture

# 2. binary smoke against the real daemon process (throwaway config dir)
CORRAL_CONFIG_DIR=$(mktemp -d) target/debug/corrald --bind 127.0.0.1 --port 9410 &
```

## Security notes

- No unsigned fallback: every drive still carries the Ed25519 signature.
- The registration token is routing-only; re-registering a fresh key never grants
  anything. Grant restore goes through the host-admin token (`POST /grants`) — the
  exact existing mechanism `scripts/corrald-grant.sh` uses.
- `RECOVERY_GRANT_CAPS` excludes `start_worktree` (a binding operation); the
  previously recorded grant set is restored when the registration records one.

## Not captured / boundary

- A live GUI screenshot of the banner: native-frame capture needs the
  Accessibility gate on the responsible app (see `issue-209/README.md` — the
  known 2026-08-26 probe race). The banner render is covered by the egui
  render unit test instead.
- read_tail tail-serve through the production herdr adapter needs a live herdr
  agent; the e2e uses the same `Adapter::read_tail` seam the daemon suite uses
  (`tests/drive.rs`) — the #249 change is entirely client-side, the daemon
  auth/handler code is untouched.
