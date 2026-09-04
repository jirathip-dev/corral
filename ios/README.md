# Corral — iOS app for the corral control plane (read-only fleet monitor).

> **#354 L2 state (2026-09-02):** the client is READ-ONLY. The board groups
> agents into raw herdr STATUS sections — Blocked → Working → Idle → Unknown
> (a Done section renders only when herdr reports done; repo is row
> metadata, never a grouping key), tapping a row opens the live recent-output
> tail, and Settings holds connection + notification pairing only. Issues
> browsing, Terminal, Diff, every action control (answer/prompt/interrupt/
> kill/attach/start-worktree), and the device/grant admin UI were REMOVED.
> Sections of this README that still describe those pre-cut surfaces are
> historical.

The iOS client for corrald: a read-only fleet dashboard grouped into raw
herdr status sections (Blocked → Working → Idle → Unknown; Done only when
herdr reports it) where every row shows what an agent is doing (state,
repo, branch, time-in-state, pane), streams the bounded recent-output tail,
and notifies on state changes
(start / blocked / done) — all Swift, no third-party SDKs (URLSession +
Codable + CryptoKit). Every read is signed with the device key (D10/D13).

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
    Wire/DriveClient.swift       register / read_tail drive / device-token / typed errors
    Network/SSEParser.swift      incremental SSE parser
    Network/CorraldClient.swift  snapshot + /events with Last-Event-ID + backoff
    Keychain/DeviceKeyStore.swift  Ed25519 key storage (Keychain + documented fallback)
    Notifications/LocalNotifier.swift  state-change notifications + deep link
    Demo/DemoFleet.swift         Debug-only seeded read-only fleet for local tests
    App/                          store, app model, SwiftUI entry
    UI/                           read-only board, recents sheet, registration, settings
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

## Local notifications (state changes, #354 L2)

- When an agent's SSE delta moves it INTO `working` (episode start), INTO
  `blocked`, or OUT of an active state into `idle` (episode end, the v2
  "done" — fires ONCE per episode, deduped until the agent starts again), a
  `UNUserNotification` fires with content title `agent · repo`, body
  `state · branch`. No badges, no catch-up on foreground.
- Tapping a notification deep-links to the agent's row with its recents
  sheet open (`LocalNotifier.onOpenAgent` → `AppModel.openRecents`).
- The one control is Settings → Notifications (global on/off). There are no
  per-agent controls and no notification actions.
- The DEBUG local bridge embeds the same `PushPayload` userInfo shape an
  APNs push carries (`type`/`agent_id`/`ts` + `aps.alert`), so one handler
  serves both paths. Real APNs delivery requires the daemon-side
  provisioning checkpoint (APNs `.p8` + `CORRAL_APNS_*` env) — see the
  #354 queue mandate; simulator/DEBUG verification uses the local bridge.

## Read-only board (home)

Status sections in the locked attention order: Blocked → Working → Idle →
Unknown, each headed by the raw status name + count; a Done section renders
only when herdr reports done (wire-done ranks with idle; herdr 0.8.2
finished panes fall back to idle). Blocked agents lead the board visually —
their section is first; repo is ROW METADATA only (a small label on each
row), never a grouping key, and an agent appears in exactly one section.
Each row: agent name, repo, state (raw herdr token: working / idle /
blocked / unknown), time-in-state, branch, and a small pane reference
(debug aid). Rows inside a status sort by recency (ts desc, then agent id
for determinism), and there is no search
and no repo filter chip. Live SSE + pull-to-refresh keep the board fresh;
when the daemon is unreachable the board keeps the last-known fleet under a
"daemon offline" banner.

Tapping a row opens the recents bottom sheet: LIVE TAIL ONLY (the daemon's
bounded ≤200-line read_tail, auto-scrolled, refreshed while open; loading /
empty / error+Retry states included). Rows render as ONE continuous
chronological rail of raw output (#361): no divider-only rows, no
role-grouped cards, and no role labels — role appears only as a transition
marker (Assistant=circle, You=diamond, Tool=square) at semantic role
changes. There is no load-earlier paging, no Conversation/Harness
partition, and no composer.

Settings = connection pairing (host/key/grants display + reset) and
notification pairing (global on/off). The Devices & Grants admin surface is
gone; grants are provisioned by the host out-of-band on the registry.

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

Debug builds retain the Settings/registration → "Demo fleet" harness: a
seeded READ-ONLY board with every raw status section populated (blocked /
working / idle / unknown agents across fictional repos — incl. an orphan
row with repo = nil — plus attachment pane refs) with the featured agent's
live-tail fixture behind its recents sheet.
`-demoMode`, the Demo mode/Exit demo controls, the fake fleet, and the
local demo read_tail responder are all compiled only under `#if DEBUG`.
Release ignores `-demoMode` and presents only the real registration, SSE,
and signed read path. The harness is for local Debug/simulator development
and deterministic tests; it is not an App Review or TestFlight product
path, and it contains no approval/action surfaces.

`DemoSeedTests` and the fixture tests continue to exercise the Debug-only
board. No physical-device or TestFlight result is claimed here.

### Recent-output evidence route

The checked-in Debug build has one opt-in, deterministic recents route for
the read-only capture: `-corralDemoDetail` seeds the demo fleet and opens
the featured agent's recents bottom sheet (simctl cannot inject the tap).
The route is state-driven and compiled only under `#if DEBUG`; production
and Release builds cannot enter it.

The #372 recorded-evidence route `-corralDemoThemeEvidence` drives the
Mocha board → Settings Appearance section → live Latte flip → Latte board →
Latte recents rail sequence (marker files in Documents/ux-evidence), so the
host screenshot script captures both themes deterministically.

### Theming (#372)

The whole app renders through the active Catppuccin flavor — Latte, Frappé,
Macchiato, or Mocha (default Mocha) — chosen in Settings → Appearance (the
ONLY picker; placement lock). Every surface consumes theme tokens from
`ThemeStore` (see `FleetNotifier/UI/AppTheme.swift`): base/mantle/crust and
surface/overlay/text/subtext hierarchy, per-flavor state colors (working =
teal, blocked = red, done = green, idle = subtext0, unknown = surface2),
the mauve UI accent (teal stays reserved for the working state), the
deterministic fnv1a32 repo-hue ring, and per-flavor ANSI slot remaps for
the recents tail. The recents sheet no longer forces SwiftUI's `.dark`
color scheme — it follows the active flavor (its output panel recesses to
`base` on Latte — the accepted light-panel rule — and to `mantle` on the
dark flavors). Reduce Motion is respected app-wide: the recents auto-scroll
lands without animation when the system Reduce Motion setting is on, and
`ThemeStore.reduceMotion` is the plumbing the #371 working-motion chip
consumes.

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
