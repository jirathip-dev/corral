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
cargo deny check
cargo audit
cargo clippy --all-targets -- -D warnings
cargo build --release
cargo test --workspace
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

## Supply-chain gates and baseline

[`deny.toml`](../deny.toml) is the workspace supply-chain policy. It checks
all workspace features and target-specific dependency branches, allows the
permissive MIT-compatible licenses used by the current graph, rejects unknown
registries and git sources, and blocks legacy `failure`, `rust-crypto`, and
`yaml-rust` crates. The advisory ignore list is intentionally empty. A new
RustSec vulnerability, unapproved license, unapproved source, or banned crate
therefore fails the gate; duplicate dependency versions are warn-level until
an upstream release makes them removable.

CI installs `cargo-deny` 0.20.2 and `cargo-audit` 0.22.2 after selecting the
Rust version from the pinned `rust-toolchain.toml`. Dependabot is enabled in
`.github/dependabot.yml` for both Cargo dependencies and GitHub Actions, with
weekly update checks.

Measured local baseline on 2026-08-25, using Cargo/rustc 1.97.1,
`cargo-deny` 0.20.2, and `cargo-audit` 0.22.2:

| Run | Result |
|---|---|
| Initial `cargo deny check` before `deny.toml` | Exit 5: cargo-deny fell back to its default policy and rejected the graph's licenses. |
| Initial `cargo audit` before remediation | Exit 1: one `RUSTSEC-2026-0258` finding for `h2` 0.4.15, used by `hyper`/`reqwest`; RustSec requires `h2` >=0.4.16. |
| Remediation | `cargo update -p h2 --precise 0.4.16`; the checked-in lockfile now contains `h2` 0.4.16. |
| Final `cargo deny check` | Exit 0: advisories, bans, licenses, and sources all passed; only existing duplicate-version warnings remain. |
| Final `cargo audit` | Exit 0: no known vulnerabilities in the 554-dependency lockfile scan. |

The initial audit database contained 1,226 RustSec advisories. Neither CI
gate has an advisory ignore, so future findings cannot be silently carried
forward as part of this baseline.

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
