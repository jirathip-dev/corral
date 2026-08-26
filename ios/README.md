# Corral — iOS app for the corral control plane (fleet board + APNs notifier).

The iOS client for corrald: a fleet dashboard that surfaces blocked agents
with their approval claims, answers them from the lock screen, and drives
agents through the signed drive plane (D10/D13) — all Swift, no third-party
SDKs (URLSession + Codable + CryptoKit).

The wire contract is `docs/corral/P4-conformance.md`; the read model mirrors
`src/core/model.rs` (schema v5). The daemon API is frozen — this app only
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
    Demo/DemoFleet.swift         Debug-only seeded fleet for local tests
    App/                          store, app model, SwiftUI entry, live-verify harness
    UI/                           fleet list, tappable agent detail/actions, claim cards, registration, settings
    FleetNotifierTests/            unit tests (canonical bytes, SSE, claims, controls, step-up, demo)
  check-release-demo.py           source and Release-binary demo boundary gate
  embed-release-source-digest.py  Release build-phase source-digest generator
  release_source_manifest.py      shared source manifest/digest implementation
  tools/anti-slop-swift/          pinned, vendored SwiftSyntax advisory linter
```

## Build

Requires Xcode 26+ and `xcodegen` (only if you change `project.yml`):

```sh
xcodegen generate
hermes-sim-task --shell 'xcodebuild -project FleetNotifier.xcodeproj -scheme FleetNotifier \
  -destination "id=$SIMULATOR_UDID" CODE_SIGNING_ALLOWED=NO test'
```

The Herdr wrapper owns a private simulator when an iOS runtime is installed.
It must be used for simulator-backed actions; the command does not install or
launch the app on a user device. A generic Release build is intentionally
separate from that wrapper so the release artifact can be inspected without a
simulator:

```sh
release_derived_data="$(mktemp -d)"
trap 'rm -rf "$release_derived_data"' EXIT
HERDR_XCODEBUILD_DIRECT=1 xcodebuild -project FleetNotifier.xcodeproj \
  -scheme FleetNotifier -configuration Release -sdk iphonesimulator \
  -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath "$release_derived_data" CODE_SIGNING_ALLOWED=NO build
python3 check-release-demo.py \
  --binary "$release_derived_data/Build/Products/Release-iphonesimulator/FleetNotifier.app/FleetNotifier"
```

Run `python3 check-release-demo.py --self-test` as a hermetic negative
check. The source proof masks line/block comments, evaluates only `true`,
`false`, `DEBUG`, and `!DEBUG` branches (including `#elseif`/`#else`), and
fails closed on unsupported conditions. The `--binary` proof requires the
actual executable's `lipo` architecture list, then validates every thin slice
with successful `file`/`otool` iOS or iOS Simulator platform checks and scans
each slice independently. Release must retain the real registration, SSE, and
drive client/error paths in every slice while containing no demo entrypoint,
menu label, or seeded fake-agent identifier.

The binary proof also requires the Release-only
`corral-release-source-sha256:<digest>` source-digest marker. The declared
Release build phase generates it from the complete app Swift source manifest
and embeds it in a dedicated Mach-O linker section; the checker compares that
marker in every architecture slice with the expected digest for this checkout.
This catches ordinary source drift and mismatched artifacts when the declared
generator phase runs. It is a build-workflow consistency check, not
cryptographic authenticity, code signing, or protection against a deliberate
builder reusing or forging the unkeyed marker. The self-test covers a
modified-source build produced by the documented generator and intentionally
does not claim resistance to manual stale-marker injection.

The XcodeGen spec declares the app source directory and both digest helper
scripts as explicit Release `inputFiles`; the generated project preserves
those declarations and quotes its `SRCROOT`/`DERIVED_FILE_DIR` paths. This
keeps the user-script sandbox inputs explicit and supports build paths with
spaces.

## Swift lint (advisory)

`tools/anti-slop-swift` is a checked-in snapshot of
[sawfwair/anti-slop-swift](https://github.com/sawfwair/anti-slop-swift)
`v0.1.7` at commit `0c63917fd59c30230c47561e3442ddd4e7cc6d6a`. The package's
MIT license and tests are included, and its `Package.resolved` pins
SwiftSyntax `600.0.1` at revision
`0687f71944021d616d34d922343dcef086855920`. This avoids a global install or a
floating tool dependency; `swift run` builds the committed package with the
available Swift 6+ toolchain (verified here with Swift 6.3.3/Xcode 26.6).

Run the same source-scoped command used by CI from the repository root:

```sh
swift run --package-path ios/tools/anti-slop-swift anti-slop \
  ios/FleetNotifier ios/FleetNotifierTests
```

The committed root `.anti-slop.json` initially disables
`no-any-dictionary-value` and `no-any-parameters`. The command intentionally
passes only `FleetNotifier` and `FleetNotifierTests`; generated `ios/build/`
is therefore excluded and is never linted. The iOS workflow is deliberately
`workflow_dispatch`-only because macOS minutes are a manual-cost gate; it does
not run automatically on PRs. After a branch push, the orchestrator can
dispatch the final SHA with:

```sh
gh workflow run ios.yml --ref g208/ios-anti-slop
```

The advisory step emits warning commands for findings, but GitHub renders at
most 10 warning annotations from one step. It also writes the complete
HTML-escaped linter output — including multiline diagnostics — to the job log
and the GitHub step summary; those are the durable source of truth for the
full baseline. The step uses `continue-on-error` while this baseline is being
retired.

The upstream README is retained byte-for-byte in the vendored snapshot for
provenance and re-sync checks, although SwiftPM does not need it to build or
run the pinned package. Its intentionally fake `apiKey` example is covered by
a path-and-exact-line allowlist in the repository `.gitleaks.toml`; the secret
scan regression keeps a changed same-line value detectable. If the upstream
snapshot changes that example, update only this narrow allowlist and its
regression fixture alongside the re-sync.

Measured on 2026-08-25 with Swift 6.3.3:

| Run | Violations | Files traversed | Rule types | Outcome |
|---|---:|---:|---:|---|
| Before the config | 104 | 24 | 5 | Baseline; exit 1 |
| With the committed config | 77 | 24 | 4 | Advisory; exit 1 |

The pre-fix baseline is dominated by `FleetNotifierTests.swift` (89 of 104
findings). The 27 disabled dictionary findings are five Keychain query/update
dictionaries in `DeviceKeyStore.swift`, five APNs `userInfo` boundary
dictionaries in `PushPayload.swift`, and 17 matching test fixtures/parsing
helpers in `FleetNotifierTests.swift`. They are intentionally left for a
typed Foundation-boundary follow-up; `no-any-parameters` had no baseline hits
but remains disabled initially as requested.

The enabled advisory findings are triaged as follows:

| Rule | Remaining hits | Disposition |
|---|---:|---|
| `no-force-unwrap` | 71 | Three production fallback/URL sites and 68 deterministic test fixtures/helpers. Replace dynamic cases with guarded/XCTUnwrap paths and document or remove only the verified static fixture invariants before making the gate blocking. |
| `no-shape-in-symbol-names` | 3 | Test method names use “shape” to describe daemon wire fixtures. Rename them in a focused cleanup; no runtime behavior is implicated. |
| `no-swallowed-errors` | 2 | `AppModel` deliberately keeps cached grants after refresh failure; `LocalNotifier` deliberately keeps the in-app claim path after notification authorization denial. Add explicit logging or user-facing recovery before enabling this rule as a blocking gate. |
| `no-force-try` | 1 | Test-only `DeviceMeta` JSON seeding uses a known `Codable` value. Make the helper throwing/XCTUnwrap-based in a focused test cleanup. |

No application source, assertion, or test behavior was changed for this
advisory adoption. The remaining findings are available in the manually
dispatched workflow's job log and complete step summary, with the rendered
annotation list limited by GitHub's per-step cap; they are documented above
rather than hidden by broad rule changes.

## Signing / distribution status

The project is configured for iOS **simulator** builds with no signing
(`CODE_SIGNING_ALLOWED=NO`); an installed runtime is required to execute or
thin those builds. A distribution archive requires the eventual
owner's Apple Developer account, signing assets, and a physical-device or
TestFlight verification pass. This repository does not claim that pass. The
Release build is intentionally a real-only product path: it has registration,
live SSE, and signed drive behavior, but no Demo mode or fake fleet.

### fastlane credentials

The TestFlight lane reads App Store Connect API credentials from
`fastlane/.env`, which is **gitignored and never committed**. Set it up once:

```sh
cp fastlane/.env.example fastlane/.env
# then fill in ASC_KEY_ID, ASC_ISSUER_ID, and ASC_KEY_PATH
```

`ASC_KEY_PATH` points at the `.p8` private key **outside this repo** — the
key itself is never copied in (`*.p8` is gitignored). `fastlane/.env` was
tracked in this repo until #26 untracked it; if you cloned before that, run
`git rm --cached fastlane/.env` locally.

From the repository root, invoke the local lane through the pinned root
bundle:

```sh
BUNDLE_FROZEN=true bundle install
bundle exec fastlane testflight
```

### Manual TestFlight CI lane

`.github/workflows/ios-testflight.yml` adds a committed CI surface for the
same lane. It is `workflow_dispatch` only and has **no** `pull_request`
trigger. The dispatch exposes one required `mode` input that defaults to
`validate`:

- `validate` — secret-free preflight. It installs the pinned Fastlane gem from
  the root `Gemfile` with Bundler, proves `spaceship` loads, parses the
  `Fastfile`, checks that the Xcode project bundle is clean, and confirms no
  credential-shaped file is tracked. It never contacts ASC, so a dispatch can
  be green before any repository secret exists.
- `upload` — deliberate human upload. It is rejected (not silently skipped)
  unless dispatched from `main`, and only then reads the ASC secrets, installs
  the certificate/profile in runner-private paths, verifies the certificate
  against ASC, and runs `bundle exec fastlane testflight`.

Repository secrets:

- `ASC_KEY_ID`, `ASC_ISSUER_ID`, `ASC_API_KEY` — required. The workflow writes
  `ASC_API_KEY` verbatim; it must be the **raw `.p8` PEM** (including
  `BEGIN/END PRIVATE KEY`), not base64. The key is written to the gitignored
  `fastlane/` path with mode `0600` and never written to `GITHUB_ENV` or the
  logs.
- `ASC_DISTRIBUTION_CERT_P12` — optional on a runner that already has the
  Distribution cert installed; otherwise base64-encode the `.p12` here.
- `ASC_DISTRIBUTION_CERT_PASSWORD` — required when that `.p12` is protected.
- `ASC_PROVISIONING_PROFILE` — optional; base64-encode the App Store
  `.mobileprovision` here if you want to install it before the lane fetches
  the current one through the ASC API key.

The workflow imports the optional `.p12` into a temporary signing keychain,
then fails before `bundle exec fastlane testflight` if no valid `Apple Distribution`
identity for the `ASC_TEAM_ID` team is visible. This makes the run fail before
Fastlane's `get_certificates` step could mint a new certificate on a hosted
runner; it also confirms the matching installed identity is registered in ASC
before the lane runs. The temporary keychain is passed explicitly to
`get_certificates`. The lane still fetches and installs the App Store
provisioning profile through the ASC API key. After upload, the entire
`ios/FleetNotifier.xcodeproj` bundle is checked for restoration; no signing
material is added to the repository.

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
- A Herdr target that disappears or migrates returns `stale_agent` as a typed
  409/refusal. The app removes the stale row, shows a refresh banner, and
  requests one fresh snapshot; SSE remains the source of truth afterward.
- A successful `read_tail` stores the daemon-redacted, bounded `result.lines`
  plus the segmented `result.blocks`, and the agent detail surface renders a
  single Recent-output pane: live tail (bottom, auto-loaded and auto-refreshed
  while open) with older history paged in above via the transcript cursor. An
  empty result is shown as "No output yet". The client applies the same
  200-line / 32 KiB bounds and a hard timeout (error + Retry, never a spinner).
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

## Tappable controls and accessibility (#110)

Every visible agent row is a full-width navigation target. Its detail surface
re-resolves the live fleet record before dispatch and exposes Recent output,
Prompt, Interrupt, and blocked approval controls. Recent output auto-loads the
live tail (no tap) and auto-refreshes while the detail view is open. The
approved transcript-chat prototype supersedes the earlier unbounded detail
scroll: Recent output owns one bounded nested transcript scroll, while the
composer remains its sibling below it; older transcript pages are loaded from
the top history affordance. The non-jamming `[info] Tail …` fleet banner is
gone, and the result stays in the detail view. Approvals echo the current
`approval_id` and `prompt_hash`, and a changed/deleted target is refused
locally before signed bytes leave the device.

The Idle / done section is collapsed by default. Its header is a full-width,
44-point disclosure target with visible `Collapsed` / `Expanded` text and the
same state exposed through VoiceOver. Rows and detail summaries show explicit
Working, Idle, Done, and Blocked text alongside their color cues. Disabled
controls retain a plain-language explanation naming the missing agent
capability or device grant.

## Row and detail actions (#166)

The fleet board is de-crammed: state + tool render as fixed-width badges and
the row title truncates before them. Every state chip shows a relative
time-in-state duration (`Needs you · 42m`). The daemon snapshot has no
state-change timestamp, so the store derives `stateEnteredAt` client-side:
seeded from the first-seen record's `ts` and re-stamped only when `state`
actually changes (never on title/reason churn), falling back to `agent.ts`
for callers without a store. Remaining limitation (documented in
`TimeInState`): an agent already mid-state at launch is seeded from a later
`ts`, so its initial duration may be shorter than the true in-state time.

Blocked rows render the pending question inline (≤2 lines) and surface a
borderless **Answer** affordance (also available as a leading swipe action)
that opens a focused, keyboard-up prompt field in a sheet reusing the shared
prompt drafts. The detail surface exposes ONE primary action per state —
blocked → Answer, working → Interrupt, done → Attach/PR — with the rest in a
"More" overflow menu. Kill lives in that overflow as a destructive control
guarded by a confirmation dialog; a read-only device sees a plain-language
reason for why it is disabled.

A pinned filter-chip row (`All · Needs you · repo₁…repoₙ`) plus a
`.searchable` field over repo/branch/title/issue mirror the egui search. When
zero agents are blocked the whole "Needs you" section is hidden — no
`Needs you (0)` header and no empty-state row.

The iOS test target includes coverage for the disclosure transition and the actual
`NavigationStack` path reconciliation when an agent is deleted, explicit
lifecycle labels, Recent-output block rendering + 4-state machine, and grant
explanations. The type-checked deterministic URLProtocol-backed action tests
cover Prompt, Interrupt, direct approval, notification approval, duplicate
claim replies, and cancellation of multiple live drives at the demo boundary.
Held-boundary tests also cover concurrent cold-start notification snapshot
replies and stale-agent refreshes crossing a demo boundary, plus cancellation
during biometrics before either `/step-up` or `/drive` is sent.

The URLProtocol harness waits on held gates asynchronously outside
URLSession's loader thread, so concurrent requests can all start.

The connection-failure suite probes `URLProtocol.startLoading()` separately
from the FleetStore callback whose timeout it awaits. A failed hosted run
therefore reports whether the loader never dispatched the mock or the stream
error/reconnect never landed in the store. The 5-second diagnostic bound is
deliberately unchanged: its purpose is to identify which side stalled, not to
turn runner scheduling delay into a longer wall-clock wait.

`CorraldClient.stream` is a nonisolated async operation, while FleetStore's
connection state remains `@MainActor` UI state. The three async connection tests
are nonisolated and await the real URLProtocol and FleetStore callbacks rather
than polling; the final state transition is still required to land on MainActor
within the unchanged 5-second contract. A MainActor backlog beyond that bound
is a real missed UI-state deadline, not a reason to lengthen the test.

Registration and APNs identity transitions are lifecycle-owned: reset and the
Debug-only demo boundary cannot resurrect a late `/register` response, live
SSE, metadata write, or retired `/device-token` upload, and concurrent
registration is refused.
An APNs callback received outside live mode retains only the latest token in
memory and retries it exactly once under the restored live identity; reset
clears that retained token and the APNs bridge state. Fleet cursor persistence
is injected into `FleetStore` (production defaults to `UserDefaults.standard`),
so reset clears only the configured store.
Exiting Debug demo is also model-owned: it validates the persisted live identity,
clears demo rows and cursors so a fresh snapshot is required, and falls back
to setup without dispatch when that identity is missing or inconsistent.
Simulator execution is covered by the reproducible #205 design-gate bundle;
this repository still makes no physical-device or TestFlight claim. See
`docs/design/evidence/issue-205/conformance.md` for the exact capture record.

## Demo mode (Debug only)

Debug builds retain the Settings/registration → "Demo fleet" harness: eight
seeded agents cover every `WaitingOnKind` (ApproveTool/Menu/AnswerQuestion/
Crash), with choices, workspace/PR/CI columns, and locally answered drives.
`-demoMode`, the Demo mode/Exit demo controls, the fake fleet, and the local
demo-drive methods are all compiled only under `#if DEBUG`. Release ignores
`-demoMode` and presents only the real registration, SSE, and signed-drive
path. The harness is for local Debug/simulator development and deterministic
tests; it is not an App Review or TestFlight product path.

`DemoSeedTests` and the lifecycle/action tests continue to exercise the
Debug-only fixture. No physical-device or TestFlight result is claimed here.

### Transcript-chat evidence route (#205)

The checked-in Debug build has one opt-in, deterministic detail route for the
approved transcript-chat capture. `-corralDemoDetail` selects the featured
transcript agent and its after-state; adding `-corralDemoBefore` selects the
legacy monotone-output presentation used only for the before frame. The route
is state-driven, seeds the composer with a non-empty draft, and is compiled
only under `#if DEBUG`; production and Release builds cannot enter it.

The dark transcript surface intentionally forces SwiftUI's `.dark` color
scheme so the prototype's charcoal tokens remain coherent even when the
containing app follows the system appearance. User-role blue is centralized
as `RecentOutputPalette.userBlue`, and Model/Effort/Worktree chips expose
their field names to VoiceOver.

From the repository root, regenerate the bundle from committed source through
the real renderer and the Herdr-owned simulator:

```sh
CHROME_BIN='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' \
  scripts/design-gate-evidence.sh --issue 205 --surface ios \
  --prototype docs/design/corral-ux-transcript-chat-prototype.html \
  --ios-mode demo --ios-launch-arg -corralDemoDetail \
  --ios-before-launch-arg -corralDemoDetail \
  --ios-before-launch-arg -corralDemoBefore \
  --output-root docs/design/evidence --force
```

The gate creates its own temporary Chrome profile, uses loopback-only DevTools
for shutdown, and removes its private staging directory. It never reads or
modifies the user's Chrome profile, and iOS simulator installation/launch is
owned by `hermes-sim-task`. The resulting `prototype.png`,
`ios-before-detail.png`, `live-after.png`, `comparison.png`, `capture.log`,
and `conformance.md` are all published together with per-file hashes and an
issue-205 implementation identity; no copied `/tmp` PNG is an input.

## Live verification (historical Debug evidence)

Against a real corrald on `127.0.0.1:8474` (herdr socket with live agents),
the Debug-only harness (`-liveVerify`,
`ios/FleetNotifier/App/LiveVerifyRunner.swift`) was used for the historical
simulator run below. It is not part of Release and is not a physical-device or
TestFlight result:

```
key storage: insecureFallback public key es1GjVYl0srTbD/…
registered key_id=dev_5b6e0e… grants=[] expiry_ts=1794642094   # R1 read-only default
snapshot schema_version=5 rev=57 agents=25                     # R2 read path
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
own R8/R9 conformance suite (W1's). The local JSON-RPC listener tests in the
repository are hermetic protocol mocks, not evidence of a live Herdr
migration or stale-agent event.

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
| R11 stale target recovery | typed `stale_agent`, remove row, refresh snapshot | API/adapter unit tests; no live migration proof |
