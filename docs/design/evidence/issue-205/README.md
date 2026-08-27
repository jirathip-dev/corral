# Issue #205 evidence

This bundle records the approved transcript-chat comparison and two iOS
simulator frames for issue #205. The before frame is the prior monotone
concatenated output; the after frame is the semantic transcript surface with
a user bubble plus agent/tool segmentation, an expanded diff,
model/effort/worktree badges, a rounded output border, and an enabled dark-ink
Send button.

## Approved source

- `docs/design/corral-ux-transcript-chat-prototype.html` is present in this
  branch and matches the supplied primary-checkout source byte-for-byte.
- Source SHA-256:
  `ca6d149e64b773ee53e2c3e5c62b8d9c592bb033152912cd0f9ebb9ffe2733a4`.
- The implementation identity is issue-specific (`#205`) and covers every
  changed iOS/egui implementation and test input, release wiring/docs, the
  capture scripts, this prototype, and the selected renderer lock records.
  Generated evidence is excluded from its own identity.
- The recorded implementation content digest is
  `sha256:2d279aa962d4008cc7200041ebf28893167c803f42101db507f7c12ad06a2502`;
  the per-file manifest is in `conformance.md`.

## Artifacts

| Artifact | Dimensions | SHA-256 |
| --- | --- | --- |
| `prototype.png` | 900x900 | `88d2a47f5cf09da5e8c6cb75420559bdd102b77862635469b79e6748c20a7345` |
| `ios-before-detail.png` | 1206x2622 | `deb0a161715efa5999346737ccc6896aacb721507f6468aa5b1f87b762400adb` |
| `live-after.png` | 1206x2622 | `ad9535c545a1e41b0388f25c84dea272d579c771a45af6ab334a75b02f93cb60` |
| `comparison.png` | 2400x960 | `69d473601cd49d03b4e75573f73dad6b67e1873f6b97083408d77da26a2db7cf` |
| `capture.log` | n/a | `648c404a1ec78fdfdb3d43a6a35c29bb4e6669c13522e508c86601e88c390e0b` |
| `conformance.md` | n/a | `15c7dfcb68313d1fe5c369437f35332e35e3404ecf636a5a63b2a528ca2f68ad` |

`capture.log` is the complete Herdr build/install/launch/screenshot record,
not a copied image provenance note. Both PNG frames were generated from the
Debug source route through `hermes-sim-task`; the fixture is intentionally not
live-daemon evidence.

## Reproduction

The capture used the bounded known-good renderer explicitly and no supplied
PNG fixture:

```text
CHROME_BIN='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' \
  scripts/design-gate-evidence.sh --issue 205 --surface ios \
  --prototype docs/design/corral-ux-transcript-chat-prototype.html \
  --ios-mode demo --ios-launch-arg -corralDemoDetail \
  --ios-before-launch-arg -corralDemoDetail \
  --ios-before-launch-arg -corralDemoBefore \
  --ios-delay-seconds 4 --output-root docs/design/evidence --force \
  --provenance-note 'approved transcript prototype is tracked byte-for-byte; before/after frames are captured from the permanent DEBUG routes through hermes-sim-task; explicit Google Chrome renderer uses a stage-local private profile and loopback DevTools; no supplied PNG fixture or user Chrome session is used'
```

The renderer creates a stage-local private Chrome profile, binds DevTools to
loopback, sends only `Browser.close`, and removes the owned profile/stage
after each capture. The iOS simulator and its derived data are likewise
owned by the Herdr task; no user simulator is deleted or modified. The route
arguments are implemented permanently under `#if DEBUG`: `-corralDemoDetail`
selects the featured after-state and adding `-corralDemoBefore` selects the
legacy before-state. The composer draft and transcript metadata are seeded by
the same per-agent demo model that supplies the semantic blocks, so the
capture does not depend on an untracked navigation hook or copied image.

## Gates

- `hermes-sim-task` focused iOS tests:
  `RecentOutputModelTests` — 31 passed, 0 failed.
- `hermes-sim-task` full iOS tests — 195 passed, 0 failed.
- `cargo fmt --all --check` — passed.
- `cargo test -p corrald-ui recent_ --lib` — 12 passed, 0 failed.
- `cargo clippy -p corrald-ui --all-targets -- -D warnings` — passed.
- `cargo clippy --all-targets -- -D warnings` — passed.
- `cargo test --workspace` — 794 passed, 0 failed, 20 ignored.
- `python3 ios/check-release-demo.py` and `--self-test` — passed.
- `git diff --check` — passed.
- Real design-gate capture with explicit Google Chrome/private profile —
  passed; `conformance.md` records the per-file implementation and artifact
  hashes.

## Documentation audit

- `docs/design/corral-ux-transcript-chat-prototype.html`: checked and retained
  byte-for-byte as the approved #205 target.
- `docs/design/corral-ux-prototype-spec.md`: checked; no change because the
  shared dark tokens and master/detail guidance remain authoritative.
- `docs/design/corral-ux-prototype.html`: checked; no change because #205 has
  its separate approved transcript-chat target.
- `docs/DEVELOPING.md`: checked; no change because workspace and gate
  instructions are unchanged.
- `docs/corral/P4-conformance.md`: checked; no change because the wire
  contract is unchanged.
- `ios/README.md`: updated with the permanent DEBUG route, forced-dark policy,
  accessibility token guidance, and reproducible capture command.
- `README.md`: checked; no change because this is a scoped iOS/egui surface
  change, not a product overview or setup change.
