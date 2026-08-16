# Fleet Notifier — Corral P4 W3 (iOS, SwiftUI)

The iOS client for corrald: a fleet dashboard that surfaces blocked agents
with their approval claims, answers them from the lock screen, and drives
agents through the signed drive plane (D10/D13) — all Swift, no third-party
SDKs (URLSession + Codable + CryptoKit).

The wire contract is `docs/corral/P4-conformance.md`; the read model mirrors
`src/core/model.rs` (schema v3). The daemon API is frozen — this app only
speaks it.

## Layout

```
ios/
  project.yml                    xcodegen source of truth
  FleetNotifier.xcodeproj        generated project (committed; regenerate with `xcodegen generate`)
  FleetNotifier/
    Models/Models.swift          read model + wire response types (snake_case CodingKeys)
    Wire/CanonicalJSON.swift     serde_json-parity encoder (canonical envelope bytes)
    Wire/DriveClient.swift       register / drive / step-up / typed errors
    Wire/DestructivePatterns.swift  client mirror of the daemon's step-up gate
    Network/SSEParser.swift      incremental SSE parser
    Network/CorraldClient.swift  snapshot + /events with Last-Event-ID + backoff
    Keychain/DeviceKeyStore.swift  Ed25519 key storage (Keychain + documented fallback)
    Security/Biometrics.swift    Face ID gate (injectable for tests)
    Notifications/LocalNotifier.swift  lock-screen canned answers bound to prompt_hash
    Demo/DemoFleet.swift         seeded fleet (App Review 4.2)
    App/                          store, app model, SwiftUI entry, live-verify harness
    UI/                           fleet list, claim cards, registration, settings
  FleetNotifierTests/            37 unit tests (canonical bytes, SSE, claims, step-up, demo)
```

## Build

Requires Xcode 26+ and `xcodegen` (only if you change `project.yml`):

```sh
xcodegen generate
xcodebuild -project FleetNotifier.xcodeproj -scheme FleetNotifier \
  -sdk iphonesimulator -destination 'platform=iOS Simulator,name=iPhone 17 Pro Max' \
  CODE_SIGNING_ALLOWED=NO build test
```

## Signing / TestFlight (documented blocker)

The project builds cleanly for the iOS **simulator** with no signing
(`CODE_SIGNING_ALLOWED=NO`). Shipping to TestFlight needs an Apple Developer
account and a development/distribution team selected in `CODE_SIGN_STYLE =
Automatic` (set `DEVELOPMENT_TEAM` in project.yml or Xcode). **No account is
configured in this repo (D12: App Store via TestFlight first) — that is the
only blocker between this build and TestFlight.**

## Key storage

- Device keypair: Ed25519 via CryptoKit (`Curve25519.Signing`). The Secure
  Enclave hosts only EC P-256 keys, so the raw private key lives in the
  Keychain (`kSecAttrAccessibleWhenUnlockedThisDeviceOnly`, which on modern
  iPhones is itself SE-protected). Registration metadata in UserDefaults.
- **Simulator note:** unsigned/ad-hoc simulator builds can get
  `errSecMissingEntitlement (-34018)` from the simulator keychain daemon.
  On that path the app uses a documented in-app plaintext store and shows a
  persistent warning banner (Settings → "Key storage"). On a signed device
  build the Keychain path is used. Verified live: the harness logs the
  storage location (`key storage: insecureFallback` on this simulator).
- `DeviceKeyStore.wipe()` (Settings → Reset) removes key + metadata.

## Local notifications + lock-screen answers

- When an agent transitions to `blocked` with a `waiting_on` (or its
  `prompt_hash` changes while blocked), a local `UNUserNotification` fires
  with the claim (`agent_id`, `kind`, `approval_id`, `prompt_hash`,
  `choices`). Idempotent per `prompt_hash`.
- Actions: Approve / Deny / Continue, resolved to a `choice` that the
  claim will accept (`choice ∈ choices[]` for Menu/ApproveTool; free-form
  for AnswerQuestion; Crash is never approvable). The drive echoes the
  snapshot's `approval_id` + `prompt_hash` byte-for-byte; a stale or
  wrong-hash reply is refused with the typed banner (`stale_approval`,
  `hash_mismatch`, `choice_not_in_menu`).
- **APNs hook (out of v1, per D12):** the relay does not exist. The seam is
  `LocalNotifier.ClaimPayload` — a future relay would register the device
  token and push exactly that claim dict; the action-execution path
  (`AppModel.handleCannedAction`) already runs on the delegate and works
  from a cold launch.

## Step-up (Face ID)

`DriveClient.drive` mirrors the daemon's destructive-pattern table
(`DestructivePatterns`, ported verbatim from `src/auth/step_up.rs`) and runs
Face ID **before** sending a destructive payload, then mints a single-use
token via `POST /step-up` (signed `StepUpRequest`, freshness `|now-ts|<60s`
enforced host-side) and retries with `X-Step-Up-Token`. A server-side
`step_up_required` refusal (mirror mismatch/expired token) triggers the same
flow reactively — same `request_id`, so an attempt that actually dispatched
replays instead of double-sending.

## Read-only default (D13)

Registration returns empty grants. Drive buttons render only when the grant
AND the agent's `capabilities` both allow them; any refusal surfaces the
daemon's typed error banner (`not_granted`, `expired`, `revoked`, …).

## Demo mode

Settings/registration screen → "Demo fleet": seven seeded agents covering
every `WaitingOnKind` (ApproveTool/Menu/AnswerQuestion/Crash) with choices,
workspace/PR/CI columns, and locally-answered demo drives. App Review 4.2
(minimal functionality) is met without a daemon.

## Live verification (evidence)

Against a real corrald on `127.0.0.1:8474` (herdr socket with live agents),
the dev-only harness (`-liveVerify`, `ios/FleetNotifier/App/LiveVerifyRunner.swift`)
ran the full flow from inside the app on the iOS simulator:

```
key storage: insecureFallback public key es1GjVYl0srTbD/…
registered key_id=dev_5b6e0e… grants=[] expiry_ts=1794642094   # R1 read-only default
snapshot schema_version=3 rev=57 agents=25                     # R2 read path
/grants set_grants → HTTP 200
drive read_tail request_id=b2e333f9-… target=herdr:ff72a82e…   # R3 signed drive
DRIVE OK ok=true rev=57
tampered envelope → HTTP 401 bad_signature                      # R4 tampered refused
step-up mint ok token_prefix=JzSUvvzx ttl_secs=300              # R9 mint half
```

Daemon audit log (hash-chained, `valid: true`) grew by exactly two entries
— both `read_tail` drives, `outcome: executed`; the 401 tamper and the
step-up mint were never logged (R10):

```
{"capability":"read_tail","key_id":"dev_4171…","outcome":"executed",
 "request_id":"6f1fdb6c-…","target":"herdr:pane:w27:p1","seq":0,…}
{"capability":"read_tail","key_id":"dev_5b6e…","outcome":"executed",
 "request_id":"b2e333f9-…","target":"herdr:ff72a82e…","seq":1,
 "prev":"f7b35e8e…",…}
```

The `DRIVE OK` is the strongest wire-fidelity proof: the daemon re-derived
the canonical envelope bytes from the parsed request and verified the Swift
client's Ed25519 signature over them — byte-for-byte equality with
`serde_json::to_vec` holds. The 401 on the tampered envelope is the negative
case (R4). `read_tail` dispatched against a live herdr agent. A live
approve round-trip needs a blocked agent, which no herdr session is
currently — the claim flow is covered by the unit tests
(`ClaimTests`, `DeltaApplyTests`, canonical-bytes vectors) and the daemon's
own R8/R9 conformance suite (W1's).

## Conformance mapping (R1–R10, from the Swift client)

| Scenario | Swift path | Evidence |
|---|---|---|
| R1 register, empty grants | `DriveClient.register` | live log |
| R2 snapshot/SSE resume | `CorraldClient` + `SSEParser`, `Last-Event-ID` | live log; SSETests |
| R3 signed drive executes | `CanonicalJSON.envelopeBytes` + `DeviceSigner.sign` | live `DRIVE OK` + audit |
| R4 tampered refused | signed body with mutated payload | live 401 `bad_signature` |
| R5 read-only denied | buttons gated on grants; `not_granted` banner | ReadOnlyTests |
| R6 replay idempotent | stable `request_id` per command | daemon contract (W1 suite) |
| R7 stale hash refused | claim echoed byte-for-byte | daemon contract (W1 suite) |
| R8 matching approve executes | `driveApprove` + canned actions | daemon contract (W1 suite) |
| R9 step-up | Face ID → mint → `X-Step-Up-Token` retry | live mint; destructive-mirror tests |
| R10 audit grows only on writes | never on auth failures | live audit (2 entries) |
