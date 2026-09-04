# Issue #376 — remove the desktop client: iOS-only product + static demo (doc-truth + removal evidence)

Branch g376-remove-egui, exact base b66657340096393206ae9a90150a8d8fa909f61d
(integration after #373 merged). Rust/CI/docs deletion lane: daemon and iOS
code untouched (verified: `git diff HEAD -- ios/ src/` empty at fix head).

## What the lane removed (inventory)

| Surface | Action |
|---|---|
| `clients/egui/` (source, tests, conformance, evidence screenshots, `web/`, `assets/`) | deleted (26 files, incl. the two demo board PNGs, now moved verbatim to `docs/demo/`) |
| Workspace member | `Cargo.toml` members/default-members = `.` + `crates/corrald-client` only; `corrald-ui` package and its 204 transitive packages pruned from `Cargo.lock` (584 → 356 records; added: none; changed: only `wnaf` 0.14.0 → 0.14.1, the upstream yank bump) |
| CI/workflows | `.github/workflows/web-pages.yml` ("Web demo Pages" wasm publish) deleted; `rust.yml` egui/wgpu apt deps dropped (keyring-only) + comments updated; `release.yml` corrald-ui build/bundle/notes removed (daemon-only macOS release) |
| Desktop install/update lifecycle (egui-only scripts) | `scripts/install-corral-ui.sh`, `scripts/test-icon-packaging.sh`, `scripts/test-design-gate-egui-integration.sh`, `tools/icon/check-desktop-entry.py` deleted; `scripts/install-corral.sh`, `scripts/setup-corrald.sh`, `scripts/update-corral.sh`, `scripts/test-daemon-launchd-env.sh` trimmed to daemon-only |
| Supply chain | `deny.toml` dropped the two now-unmatched license exceptions (`option-ext` MPL-2.0, `epaint_default_fonts` OFL/Ubuntu-font) — cargo-deny fails hard on unmatched exceptions; `cargo deny --locked --workspace check` now passes with wnaf 0.14.1 |
| Docs | README rewritten (short, plain, setup-first per the issue's 09-03 SCOPE ADD: daemon setup + TestFlight iOS connect; iOS-only product) and `docs/QUICKSTART.md`, `docs/OPERATIONS.md`, `docs/ARCHITECTURE.md`, `docs/DEVELOPING.md`, `docs/ios-showcase.md` made free of removed-surface claims; desktop UI section removed from QUICKSTART (sections renumbered) |
| Static demo | `docs/demo/index.html` + the two existing board PNGs (390x844 + 1280x800, byte-identical to the #354 L3 evidence captures) — synthetic-only, no wasm toolchain |
| Evidence tooling forced by the deletion | `scripts/design-gate-content-identity.py` re-scoped from the deleted #205/#206 egui file lists + eframe/wgpu `Cargo.lock` fingerprint to the current iOS product scope; `scripts/design-gate-evidence.sh` identity call + conformance scope prose updated; `scripts/test-design-gate-evidence.sh` lockfile-fingerprint battery replaced with a scope-absence battery; `tools/icon/check-assets.py` dropped deleted-source pins/checks/self-tests (and refreshed the `project.pbxproj` pin that was already stale at base — see notes) |
| Test fix forced by the deletion | `tests/http.rs::no_fleets_json_reference_anywhere_in_src` walked the now-missing `clients/` dir and panicked (`read_dir` NotFound); probe now scans only existing dirs |

## Doc-truth gate (RED/GREEN)

`doc-truth-gate.sh` scans the six live docs (README + the five docs/ guides)
for removed-surface tokens (desktop client names, renderer stack, web-demo
build/publish vocabulary). Historical archives (`docs/design/evidence/*`,
`docs/corral/*`, `docs/design/*` prototypes) are NOT scanned: they are dated
records of the removed surface. One technical supply-chain mention is
deliberately allowed and disclosed: DEVELOPING calls the remaining
`rustls-platform-verifier` dependency "wasm32-only" (its web-target triple);
that describes a dependency still in the lock, not a Corral web product.

RED at base b6665734 (the gate matches every removed-surface claim):

```
$ bash docs/design/evidence/issue-376/doc-truth-gate.sh   # on base copies
README.md:42:- **`corrald-ui`** — the desktop board (egui, also compiles to a read-only WASM demo).
...
doc-truth-gate: FAIL - a rewritten doc reintroduces removed desktop/WASM surface (matches above).
RED-exit=1
```

GREEN at fix head:

```
$ bash docs/design/evidence/issue-376/doc-truth-gate.sh
doc-truth-gate: PASS - README + docs/ carry no desktop-board, corrald-ui, WASM/web-demo, or renderer-stack references as current behavior.
green-exit=0
```

## Removal probes (fail if the client returns)

```
$ test -d clients/egui; echo $?        # → 1 (absent)
$ git ls-files clients/egui | wc -l    # → 0
$ git ls-files | grep -cE 'corrald-ui|install-corral-ui'   # → 0
$ grep -c corrald-ui Cargo.toml .github/workflows/*.yml    # → 0 in every file
$ cargo metadata --no-deps --format-version 1 | grep -c 'clients/egui'   # → 0 (not a member)
```

Gate logs (real runs, serialized under /tmp/corral-heavy-gate.lock):
`/tmp/376-cargo-build.log`, `/tmp/376-cargo-test2.log`,
`/tmp/376-cargo-gates.log` (clippy/deny/audit), `/tmp/376-ios-gates2.log`
(iOS battery).

## Rust gates (real, serialized under /tmp/corral-heavy-gate.lock)

| Gate | Result |
|---|---|
| `cargo fmt --check` | exit 0 |
| `cargo build --release` | exit 0 — `Finished release profile [optimized] in 2m 40s` |
| `cargo test --workspace` | exit 0 — all suites green (full per-suite counts in the lane report) |
| `cargo clippy --all-targets -- -D warnings` | exit 0 |
| `cargo deny --locked --workspace check` | exit 0 — advisories ok, bans ok, licenses ok, sources ok (wnaf 0.14.1) |
| `cargo audit --deny warnings` | exit 0 — 356 crate dependencies scanned |

## Static demo render

`docs/demo/index.html` served over `python3 -m http.server`-style loopback
(SimpleHTTPRequestHandler on 127.0.0.1): `/index.html` → 200 text/html
(2,151 bytes), both PNGs → 200 image/png (82,918 / 89,900 bytes), and the
HTML references both images. `scripts/check-demo-privacy.py` over the three
files (tesseract OCR available): 0 forbidden matches, exit 0. The PNG pixels
were additionally reviewed by vision: fully synthetic demo data
(demo-alpha/bravo/charlie/delta, `p0x:role` pane ids) — no private
identifiers. Note: the frozen #354-era captures contain fictional branch
labels (`g354-l3-egui-cut`, `g304-wasm-mobile`) inside the rendered pixels;
they are synthetic fixture content, not doc claims.

## Notes / loud disclosures

- **iOS untouched**: `git diff HEAD -- ios/` is empty on this branch. iOS
  verification ran against the committed `FleetNotifier.xcodeproj`.
- **Pre-existing xcodegen drift (not this lane)**: on this host,
  `xcodegen generate --spec project.yml` (xcodegen 2.45.4) rewrites
  `ios/FleetNotifier.xcodeproj/project.pbxproj` (66 lines, PBXGroup order +
  resource IDs) vs the file committed at base b6665734. ios/ was restored
  byte-identical after the verification runs; the drift predates #376
  (ios/ untouched here) and belongs to the iOS lane toolchain.
- **Stale icon pin refreshed**: `tools/icon/check-assets.py` pinned the iOS
  `project.pbxproj` SHA from before the #371-#373 xcodegen additions, so the
  icon gate was already failing at base. #376 refreshed the pin to the
  base-identical current hash (4e51ae80…) so the checker passes; the pin
  refresh is the only non-removal hunk in that file.
- **Residual mentions kept on purpose**: dormant capture-tool code paths and
  comments in `scripts/design-gate-evidence.sh` / `scripts/test-design-gate-evidence.sh`
  (desktop-surface mode exercised only by their hermetic mock tests),
  `tools/icon/from-user-png.py` (still regenerates the retained icon
  outputs), `scripts/check-demo-privacy.py` (generic privacy scanner, kept
  for the static demo + future fixtures), `deny.toml` comment, and iOS
  source comments/fixtures (`StateStyle.swift` "egui mirror" comments from
  #371/#372, test branch-name fixtures) — the iOS files are outside this
  lane's fence (read-only). The doc-truth gate above is the canonical
  current-behavior surface.
- gh-pages root: the last deployed wasm demo on the `gh-pages` branch root
  is not removed by this lane (no GitHub mutation beyond the branch push);
  with web-pages.yml gone nothing refreshes it — a one-time gh-pages root
  cleanup/republish is an orchestrator/host follow-up if the root demo URL
  must stop serving stale content.
