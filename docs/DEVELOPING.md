# Corral Development Guide

Everything needed to work on `corrald` and `crates/corrald-client`.
All quality-gate commands below were run and verified on main
(2026-08-16).

## Workspace layout

A **non-virtual** cargo workspace at the root:
`members = ["crates/corrald-client", "clients/egui"]`,
`default-members = [".", "crates/corrald-client", "clients/egui"]` — so
root-level build/clippy/test cover **all three** crates. That
`default-members` line is load-bearing: at a non-virtual workspace root
cargo would otherwise operate on the root package only and silently skip
the other two. Additive-only: new crates go under `crates/`; `corrald`
itself is never restructured.

```
src/main.rs              binary entrypoint: --socket/--port/--bind parsing
                         (allowlist: loopback/RFC 1918/Tailscale CGNAT/
                         IPv6 ULA — public and 0.0.0.0 refused), auth-plane
                         init, planes supervisor, axum serve
src/lib.rs               library surface: adapters, api, approve, auth,
                         core, drive, integrate
src/adapters/            herdr.rs (event push + trusted catalog refresh),
                         git_plane.rs, gh_plane.rs, mod.rs (Adapter trait)
src/core/                model.rs (canonical Agent, schema v5),
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
src/api/                 mod.rs (router: /healthz /snapshot /events
                         /history /transcript), drive.rs (POST /drive handler)
src/transcript/          bounded, redacted session-transcript reads and
                         agent-to-session binding
src/history/             mod.rs, ring.rs (D23 persistent event ring),
                         digest.rs (D33 `corrald digest`)
crates/corrald-client/   shared client layer: model, drive, keypair,
                         stepup, approval, sse, client; tests/conformance.rs
tests/                   auth.rs, drive.rs, http.rs, integration.rs,
                         store.rs, model.rs, redact.rs, git_plane.rs,
                         gh_plane.rs
docs/corral/             P1–P4 briefs (history) + P4-conformance.md
                         (normative wire contract)
```

## Icon assets and packaging

The approved icon outputs are checked in under `assets/icon/` and the iOS
AppIcon catalog:

- `corral-master.png` is the cropped square reference.
- `corral-icon-1024.png` is the opaque repository/reference output.
- `corral-icon-256.png` is the opaque egui/Linux desktop output.
- `corral-icon-macos.png` is the 1024px full-bleed opaque macOS output.
  macOS applies the squircle mask itself, and `macos_fullbleed()` keeps the
  glyph inside the standard ~80% safe region.
- `social-preview.png` is the 1280×640 repository preview asset. Committing
  it does not change GitHub's social-preview setting; that remains a manual
  repository-settings step.
- `ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/AppIcon-512@2x.png`
  is the opaque 1024px iOS AppIcon selected by the Xcode project.

When the approved source PNG is available, regenerate the outputs with:

```sh
mise exec -- python tools/icon/from-user-png.py <approved-source.png>
mise exec -- python tools/icon/check-assets.py
mise exec -- python tools/icon/check-assets.py --self-test
```

The generator pins the approved SFNS wordmark font by SHA-256
(`2bfd40dc72e6759e248f82a52a40d551338979fffc9b5c070e685b4b7ad19e66`) and
fails before writing outputs if `/System/Library/Fonts/SFNS.ttf` is absent or
different. A machine with the exact approved font at another path may set
`CORRAL_ICON_FONT`; the fixed fingerprint is still enforced and there is no
silent Pillow fallback. The checked-in social preview is the approved output
and should not be regenerated without an approved visual-equivalent change.
The macOS output can also be derived directly from the checked-in canonical
`corral-icon-1024.png` with `macos_fullbleed()`; the approved source image is
not checked in.

`check-assets.py` is read-only: it checks the pinned SHA-256 manifest for the
approved assets and the complete active icon-integration sources, PNG
dimensions/modes/alpha, social wordmark/caption structure, the parsed iOS
asset-catalog resource phase, Python/shell syntax, and the release embedding
gate. The egui binary embeds
`corral-icon-256.png` at compile time; after a release build, prove that
embedding with:

```sh
cargo build --release -p corrald-ui
mise exec -- python tools/icon/check-assets.py --require-build
```

The negative self-tests mutate temporary fixtures, including macOS full
opacity, corner opacity, safe padding, and centering, social copy, the
AppIcon catalog's actual Resources phase, an immediate-return generator, and
both detached and commented-out egui icon applications. Full-source hashes
plus compile/build-derived checks make those mutations fail even when a token
or comment is left behind. Pillow checks use the repository's documented
`getdata()` pixel API rather than a newer-only helper.
`scripts/test-icon-packaging.sh` uses only temporary destinations and failing
converter/copy/rename stubs to verify macOS, Linux, and Other-platform staging,
special-character desktop-entry `Exec` escaping, cleanup, and single- and
double-failure rollback. If both restoration renames fail, the installer
reports the exact retained rollback directory instead of deleting the only
recoverable old payload; it never targets `/Applications` or a user config
directory. Linux `Exec` values are encoded in the two required passes: command
quoting is followed by desktop-entry general-string escaping, so the on-disk
forms use doubled backslashes for quotes, `$`, and backticks, four for
a literal backslash, and `%%` for a literal percent. Newline- or
carriage-return-containing requested prefixes, and executable paths containing
`=`, are rejected before destination directories are created.
Install prefixes and the macOS app parent are physically canonicalized before
guards; root-resolving paths, `..` traversal, and symlinked `bin`/`share`
payload parents are refused. A symlinked prefix is accepted only by resolving
it to its safe canonical target, and every stage is created beside that
resolved target so final renames stay on one filesystem. Linux and Other
preflight the device of every target parent against the staging prefix before
creating payload directories and recheck before commit; a device mismatch is
an error rather than a cross-filesystem fallback. Rollback diagnostics
distinguish a payload-restore failure, which retains the old payload, from a
cleanup-only failure, which reports that the payload was restored. For the
cleanup-only case, an existing empty or partial rollback directory gets an
inspection path, a missing root gets no path or recoverability claim, and an
existing but uninspectable root is reported as indeterminate with its path.
If cleanup fails after a fresh install, the diagnostic instead identifies an
empty rollback directory for inspection rather than claiming a rollback copy
exists. After a replacement, cleanup diagnostics inspect the expected rollback paths after the failed
removal: macOS reports a remaining previous path only when it exists, while
Linux and Other distinguish all, some, or none of the expected backup paths.
An existing empty directory gets an inspection path; a missing rollback root
gets no path or recoverability claim; and an existing but uninspectable root
is reported as indeterminate without being called empty or retained. They
never infer recoverability from pre-cleanup backup state. The multi-file Linux
commit remains rollback-based; the installer does not claim one atomic
operation for the whole payload.

`scripts/setup-corrald.sh` delegates desktop installation to
`scripts/install-corral-ui.sh`. The installer builds and validates the entire
macOS bundle—including a round-tripped `Corral.icns`, `CFBundleIconFile`, and
the executable—or the complete Linux binary/icon/`.desktop` payload in a
sibling staging directory first. It commits only after validation; existing
installations are moved into same-filesystem rollback storage and restored if
a final rename fails. Linux installs the 256px PNG as the `corral` desktop
icon.

## Quality gates (run all of these before merging)

```sh
cargo fmt --check
cargo deny --locked --workspace check
cargo audit --deny warnings
cargo clippy --all-targets -- -D warnings
cargo build --release
cargo test --workspace
# The client report gate reuses profiles from the preceding combined run.
cargo llvm-cov clean --locked --workspace
cargo llvm-cov \
  --locked \
  --package corrald \
  --package corrald-client \
  --all-targets \
  --no-fail-fast \
  --quiet \
  --fail-under-lines 85 \
  --fail-under-functions 82
cargo llvm-cov report \
  --locked \
  --package corrald-client \
  --fail-under-lines 40 \
  --fail-under-functions 35
cargo test -p corrald-client                       # client unit + wire pins
cargo test -p corrald-ui --test live -- --ignored  # egui live tests
```

These are the same gates hosted CI runs (`.github/workflows/rust.yml`), so
a local pass is a good predictor of a green run — with two caveats worth
knowing before you push:

- **`cargo fmt --check` is a blocking gate** as of #40 — a formatting
  failure fails the build, and it runs first, so an unformatted PR never
  reaches clippy or the tests. Run it before you push. The tree was
  reformatted in one isolated commit to get here; do not reformat as a
  side effect of an unrelated PR.
- **CI runs on Linux; most development here happens on macOS.** That
  difference is not cosmetic — it has already caught a test that passed on
  macOS only because `/var` is a symlink to `/private/var` while `/tmp` on
  Linux is real. If a test touches canonicalized paths, assume the two
  platforms disagree until CI says otherwise.

Historical verified results on main:

| Gate | Result |
|---|---|
| `cargo build --release` | `Finished \`release\` profile [optimized]` (2m44s cold) |
| `cargo clippy --all-targets -- -D warnings` | clean, no warnings |
| `cargo test` | all green — 95 lib + 24 + 15 + 8 + 2 + 7 + 11 + 11 + 3 tests |
| `cargo test -p corrald-client` | 12 unit + 4 wire-format pins green; live suite `#[ignore]`d at that baseline |

The R1–R10 conformance scenarios (register, read path + SSE resume,
signed drive executes, tamper refused, read-only denied, replay
idempotent, stale-hash refused, matching approve, step-up, and audit growth)
run against a **real spawned corrald** with a fake herdr unix server —
fully self-contained, needs no live fleet:

```sh
cargo test -p corrald-client -- --ignored
```

R11 (GitHub PR binding) additionally requires read-only GitHub access and a
suitable open PR on a tracked repository. Verified locally on 2026-08-25:
13/13 ignored conformance tests pass. This is the W1 acceptance bar shared by both P4
clients.

## Rust coverage gate and baseline

The blocking Rust coverage gate runs in the existing `rust` job after the
ordinary workspace tests. It uses `cargo-llvm-cov` **0.8.7** with
`llvm-tools-preview` from the pinned Rust **1.97.1** toolchain. Keeping the
gate in the existing job reuses its toolchain, Linux dependencies, and cache;
the instrumented build is required for LLVM coverage, while the client floor
and report generation reuse the resulting profiles without compiling or
rerunning tests. The explicit `cargo-llvm-cov@0.8.7` CI pin is not
Dependabot-managed; update it manually only with a deliberate toolchain and
fresh-baseline check.

The measured scope is the pure-Rust `corrald` daemon package and
`corrald-client` package, with `--all-targets` and the normal non-ignored test
set. `clients/egui` (`corrald-ui`) is excluded deliberately: egui/wgpu and its
X11/Wayland/OpenGL dependencies are GUI/platform code, and including it would
make this core gate depend on platform-specific rendering paths rather than
measure the daemon/client contract. The egui crate remains covered by the
ordinary workspace clippy, build, and test gates.

Install the same local coverage tool and component, then reproduce the
blocking CI command from the repository root:

```sh
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --locked --version 0.8.7
cargo llvm-cov clean --locked --workspace
cargo llvm-cov \
  --locked \
  --package corrald \
  --package corrald-client \
  --all-targets \
  --no-fail-fast \
  --quiet \
  --fail-under-lines 85 \
  --fail-under-functions 82
cargo llvm-cov report \
  --locked \
  --package corrald-client \
  --fail-under-lines 40 \
  --fail-under-functions 35
```

The client command is report-only: it reuses the profiles from the combined
run and does not rerun client tests. To generate the same local files that CI
uploads after those gates (the files stay under ignored `target/` and must not
be committed):

```sh
mkdir -p target
stage="$(mktemp -d target/.rust-core-coverage.XXXXXX)"
cargo llvm-cov report \
  --locked \
  --package corrald \
  --package corrald-client \
  > "$stage/rust-core-summary.txt"
cargo llvm-cov report \
  --locked \
  --package corrald \
  --package corrald-client \
  --json \
  --summary-only \
  --output-path "$stage/rust-core-summary.json"
cargo llvm-cov report \
  --locked \
  --package corrald \
  --package corrald-client \
  --lcov \
  --output-path "$stage/rust-core.lcov"
cargo llvm-cov report \
  --locked \
  --package corrald-client \
  > "$stage/rust-client-summary.txt"
cargo llvm-cov report \
  --locked \
  --package corrald-client \
  --json \
  --summary-only \
  --output-path "$stage/rust-client-summary.json"
cargo llvm-cov report \
  --locked \
  --package corrald-client \
  --lcov \
  --output-path "$stage/rust-client.lcov"
for report in \
  rust-core-summary.txt \
  rust-core-summary.json \
  rust-core.lcov \
  rust-client-summary.txt \
  rust-client-summary.json \
  rust-client.lcov
do
  test -s "$stage/$report"
done
for summary in rust-core-summary.json rust-client-summary.json
do
  grep -Eq '"files":[[:space:]]*\[[[:space:]]*\{' "$stage/$summary"
  grep -Eq '"lines":\{"count":[1-9][0-9]*,"covered":[1-9][0-9]*' "$stage/$summary"
done
grep -q '^SF:' "$stage/rust-core.lcov"
grep -q '^SF:' "$stage/rust-client.lcov"
rm -rf -- target/coverage
mv "$stage" target/coverage
```

The uploaded artifact is named **`rust-core-coverage`** and contains the
human-readable `rust-core-summary.txt`, machine-readable
`rust-core-summary.json` (summary-only LLVM export), line-level
`rust-core.lcov`, human-readable `rust-client-summary.txt`, machine-readable
`rust-client-summary.json` (summary-only LLVM export), and line-level
`rust-client.lcov`. CI stages every report in a temporary directory and
checks that every expected file is non-empty, that both JSON reports contain
real source files with positive covered lines, and that both LCOV reports
contain line records before publishing `target/coverage`. The report step runs
only when the coverage step actually ran and was not cancelled; upload runs
only after that validated report step succeeds. Therefore a threshold failure
with valid profiles still uploads diagnostics, while a pre-profile failure or
cancellation publishes nothing. `cargo-llvm-cov` 0.8.7 does not expose a
report path-remapping option, so LCOV `SF:` paths retain the runner checkout
prefix; the LCOV files are diagnostic line-level reports rather than
path-independent comparison data.

Measured on **macOS** on **2026-08-25**, with Cargo/rustc **1.97.1**,
`cargo-llvm-cov` **0.8.7**, the command's exact package/target/test scope
above, and live `#[ignore]` tests left disabled. The table records the lower
result observed across repeated macOS runs so the floors are not based on a
lucky test-process schedule; the first baseline run was 20,708/23,814 lines
(86.957252%).

| Scope | Lines | Functions |
|---|---:|---:|
| `corrald` + `corrald-client` | 20,701/23,814 = **86.927858%** | 1,832/2,149 = **85.248953%** |
| `corrald` | 20,264/22,805 = **88.857707%** | 1,783/2,024 = **88.092885%** |
| `corrald-client` | 440/1,009 = **43.607532%** | 49/125 = **39.200000%** |

The blocking floors for the combined core scope are **85% lines** and **82%
functions**. They are evidence-based and intentionally below the measured
baseline to leave deterministic headroom for Linux/platform differences and
normal additions, while still rejecting a material regression. Repeated local
runs observed 20,701–20,710 covered lines; the lower result above is the one
used for the floor decision.

All measurements above are macOS-only; neither floor is presented as Ubuntu
portable yet. Revisit both floors after hosted Ubuntu evidence is available.

The separate blocking `corrald-client` floors are **40% lines** and **35%
functions**. Against the measured client baseline, that leaves **3.607532
percentage points** of line headroom and **4.2 percentage points** of function
headroom. These client floors are deliberately provisional pending the first
hosted Ubuntu measurement; they are not a claim that macOS and Ubuntu produce
identical coverage. They remain positive so deleting all client tests cannot
pass: an isolated no-client-test run produced a client report with 0 executed
lines/functions and the report gate exited 1 at these floors.

## Supply-chain gates and baseline

[`deny.toml`](../deny.toml) is the workspace supply-chain policy. The blocking
command is `cargo deny --locked --workspace check`: `--workspace` makes all three
members (`corrald`, `corrald-client`, and `corrald-ui`) graph roots. Its
`all-features` graph setting evaluates feature branches for every target, and
`licenses.include-dev = true` includes workspace dev-dependencies in the
license check. The checked-in `Cargo.lock` contains 554 package records; `cargo audit` scans that lockfile
directly, while cargo-deny evaluates the complete workspace graph rooted at
the three members.

The license policy allows the project's MIT/Apache-compatible dependency
licenses plus the reviewed permissive BSL-1.0 and Zlib licenses. MPL-2.0 is
accepted only for the transitive `option-ext` utility, because its copyleft is
file-level and does not change this project's MIT/Apache terms. The bundled
`epaint_default_fonts` asset crate is the only exception for OFL-1.1 and
Ubuntu-font-1.0. CC0-1.0 is allowed as a public-domain dedication, not as an
OSI-approved software license. Unknown registries and git sources fail, the
legacy `failure`, `rust-crypto`, and `yaml-rust` crates are banned, and
registry/git wildcard requirements fail. The two current wildcard path
dependencies are workspace dev-dependencies. `allow-wildcard-paths` applies
cargo-deny's dev-dependency exemption to these non-registry path edges even
though the workspace crates are currently publishable.

CI selects the Rust version from the pinned `rust-toolchain.toml`, then uses
the pinned `taiki-e/install-action` release to download checksum-verified
`cargo-deny` 0.20.2 and `cargo-audit` 0.22.2 binaries with source-build
fallback disabled. Dependabot groups Rust and GitHub Actions updates and
prefixes their commits in `.github/dependabot.yml`. The Rust workflow keeps
its push and pull-request triggers and also runs every Monday at 03:17 UTC so
new advisories are checked even when the dependency graph is unchanged.

`cargo-deny` explicitly denies all unmaintained and unsound advisory scopes,
denies yanked crates, and has no advisory ignore list. CI runs
`cargo audit --deny warnings`; cargo-audit reads `Cargo.lock` directly and
does not take Cargo's `--locked` flag. cargo-deny 0.20.2 accepts `--locked` as
a global option, and the documented blocking invocation rejects any
Cargo.toml/Cargo.lock drift.

Measured local baseline on 2026-08-25, using Cargo/rustc 1.97.1,
`cargo-deny` 0.20.2, and `cargo-audit` 0.22.2:

| Run | Result |
|---|---|
| Initial root `cargo deny check` before `deny.toml` | Exit 5: cargo-deny fell back to its default policy and rejected the graph's licenses; this was not a full-workspace check. |
| Initial `cargo audit` before remediation | Exit 1: one `RUSTSEC-2026-0258` finding for `h2` 0.4.15, used by `hyper`/`reqwest`; RustSec requires `h2` >=0.4.16. |
| Remediation | `cargo update -p h2 --precise 0.4.16`; the checked-in lockfile now contains `h2` 0.4.16. |
| Final `cargo deny --locked --workspace check` | Exit 0: all workspace roots and their full feature/target graph passed advisories, bans, licenses, and sources; 40 existing duplicate-version warnings remain. |
| Final `cargo audit --deny warnings` | Exit 0: no known vulnerabilities, yanked crates, or other advisory warnings in the 554-package lockfile scan. |

The initial audit database contained 1,226 RustSec advisories. Neither CI
gate has an advisory ignore, and yanked/unsound findings are deny-level, so
future findings cannot be silently carried forward as part of this baseline.

## Conventions

- **Event push with a bounded trusted-catalog refresh in the herdr adapter.**
  `events.subscribe` remains the primary path, but the session also runs a
  serialized `agent.list` reconciliation every `CATALOG_REFRESH_INTERVAL`
  (2s). This bounded cross-check keeps state, membership, and session ids
  live when the socket stays open but silently stops delivering events.
  Refreshes are serialized with event handling, failures are logged and
  retried on the next tick, and unchanged catalogs are no-ops that do not
  publish a new snapshot rev. The reconciliation also evicts stored herdr
  sessions that are absent from that fresh catalog, leaving the same
  refreshable stale tombstone as pane retirement. A session-less catalog view
  is debounced: one omitted `agent_session` keeps a live pane's explicit id,
  and demotion requires two consecutive corroborating refreshes. The gh plane is
  poll-by-design (one GraphQL round-trip per poll, SWR); the git plane is
  fsevents push with one immutable watcher per commondir and a 10s sweep
  safety net.
- **Additive-only versioned schema.** New fields/variants extend the
  model (`SCHEMA_VERSION` bumps additively); existing shapes never
  change. The drive contract in `src/drive/mod.rs` is frozen — add
  capabilities, never mutate.
- **Provider-neutral canonical model.** The `Agent` and snapshot shapes carry
  fleet state only; they do not represent provider pricing, quota, or spend.
  Store-specific transcript parsing belongs in the bounded, redacted
  `src/transcript/` boundary and must not feed the board model.
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

## State token contract

The shared state→color/label vocabulary for both clients lives in
`contracts/state-tokens.json` (one entry per `AgentState` carrying `state`,
`rank`, `label`, `dark`, `light`, `mark`). The egui board keeps native
`Color32` consts in `clients/egui/src/theme.rs` and the iOS notifier keeps a
`StateStyle` in `ios/FleetNotifier/UI/StateStyle.swift`, but neither may
diverge from the contract: a drift test per client reads the JSON and
asserts the hexes/labels/ranks/marks stay in sync. Update the contract first,
then re-point each client and its drift test together. Color is never the
only state channel — every chip renders a mark plus a label.

## How to add a capability

1. **Contract** (`src/drive/mod.rs`): add the variant to `Capability`
   plus its `Display`/`FromStr` arms. Additive only — never change an
   existing variant. Add a typed `DrivePayload` variant and its
   `DrivePayload::parse` arm if the capability carries a payload.
2. **Grants**: the daemon needs nothing extra — `POST /grants` parses
   capability strings through `Capability::FromStr`, so the new name is
   grantable by default. Unknown strings fail loudly, so no typo can
   silently no-op. The desktop grant editor does need its closed-set
   mirror updated in `clients/egui/src/protocol.rs`
   (`GRANT_CAPABILITIES`).
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

Fleet-level capabilities (e.g. `start_worktree`, #113) do NOT dispatch
through the per-agent adapter: `src/api/drive.rs` routes them to
`dispatch_worktree` before the agent/tombstone/replay-claim path, so a
worktree start is idempotent on its own `request_id` and audited once. The
`target` is the fleet/repo name, not an `agent_id`. The client still names
the capability in `POST /grants` exactly like an agent capability.

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
