# Issue #455 — approved immersive-Herd prototype package (V1/A) — docs/design/evidence

Status: **DESIGN GATE CLEARED FOR V1/A (owner receipt 2026-09-09); variant B is
NOT approved.** This directory is the promoted, reproducible record of the
owner-approved #455 design prototype. It packages existing approved design --
it is not new design, not native implementation, and it does not re-open the
gate. Tracker #455 stays open; implementation children #456--#460 remain their
own routed work. No production code is touched by this docs-only package.

## Owner decision and approval receipt

Exact receipt: issue comment
`https://github.com/jirathip-dev/corral/issues/455#issuecomment-5601951267`
(comment id 5601951267, author `jirathip-k`, posted 2026-09-09T12:38:14Z to
https://github.com/jirathip-dev/corral/issues/455). The design-gate paragraph,
quoted verbatim:

> Design gate #455 is cleared for V1/A, NOT variant B. Full-screen ranch;
> floating top scope+Settings; bottom paddock nav; saved/immediate Board/Herd
> in Settings, no top switch, Open Board recovery retained. Shipping remains
> native procedural Swift with approved ranch/horse geometry.
> Approved artifact: variant-a.html SHA-256
> `ca0a1a0991aa66364598eaacdac4d328de6a4ac1e57e03707aa708851791bf61`. Gallery:
> https://jirathips-macbook-air.tail8c3301.ts.net:8444/corral/455-immersive-herd/variant-a.html?preview=herd
> Owner says motion is not visible to him in prototype. Approval is NOT
> acceptance of imperceptible motion: #459/#460 require clearly perceptible
> gentle movement in real-time at phone scale, plus Reduce
> Motion/power/lifecycle guards. Browser frame deltas alone cannot satisfy
> native/device ACs. Preserve A layout while improving motion.

Two earlier comments are part of the package's context and are preserved in
`references/issue-455.json`: the 2026-09-09 "Filed child map — all UNROUTED"
comment (5600142429) and the "DESIGN DISPATCH — owner approved prototype only"
comment (5600394076). The routing comment above explicitly supersedes the
earlier FILE ONLY / UNROUTED status for this issue.

Approved-artifact identity is recomputed in this package:
`git`-independent `sha256(variant-a.html) =
ca0a1a0991aa66364598eaacdac4d328de6a4ac1e57e03707aa708851791bf61` (see
`package-verification/reconcile.json` and `package-verification/regen-proof.json`).
`variant-b.html` (`fa31421c…`) is retained **only** as the historical
comparison artifact; nothing in this package approves it.

## History reconciliation — B was recommended, A was selected

The designer run's README (`README-designer-original.md`, retained
byte-identical) recommended **B** ("Surface archetype: Monitor" — B places
readable host health and all five counts immediately beneath scope; A trades
the full status cluster for a compact summary and keeps more open sky). The
owner selected **A** instead. This package reconciles the record without
rewriting it:

- the designer recommendation and rationale stay intact and dated 2026-09-09;
- the owner decision (A) and its receipt are the governing record;
- B remains in the package as an explicitly historical comparison, labeled
  NOT approved -- it is useful only as the A-vs-B hierarchy/density reference;
- no text in this package claims A was always the recommendation.

## What this package is / is not

- **Is:** the promoted record of the approved prototype: gallery, both
  variants (A approved; B historical), the full labeled 2026-09-09 browser
  evidence set, source/provenance snapshots, the designer README and brief,
  the reproducer toolchain (10/17 files byte-identical; 7 adapted for
  portability/labels — see `tools/PORTABILITY-NOTES.md`), a recomputed
  package manifest, and a promotion-time verification layer
  (`package-verification/`).
- **Is not:** new design work; a native implementation; a motion-perceptibility
  acceptance; a release/TestFlight or merge artifact; a replacement for the
  physical-device evidence that #456--#460 must produce.

## Acceptance-criteria matrix (issue #455's eight canonical ACs)

| # | Canonical AC (from issue #455) | Status in this package | Evidence | Still open |
|---|---|---|---|---|
| 1 | Phone-tappable comparison gallery with both treatments, recommendation, labeled screenshots and motion clips; variants differ in density/hierarchy, not palette alone | Satisfied as package; recommendation element **superseded by the owner's A selection** | `index.html` gallery (functional A/B Day/Night links, stills, both real-time clips), `evidence/comparison-contact.png`, `evidence/a-*/b-*` stills, `day-realtime.mp4` + `night-realtime.mp4`; `evidence/verification.json` "gallery loaded" check; promotion-time served-byte identity (`package-verification/reachability-*.json`) | Final visual adjudication for native ships with the implementation children |
| 2 | Background reaches screen edges; status bar/notch/home indicator, top HUD and bottom controls readable and tappable; no separate opaque Board header above the scene | Prototype-satisfied (browser) | A/B Day/Night stills; `evidence/verification.json` geometry gates on every measured case (no horizontal overflow, HUD top ≥54px, bottom margin ≥24px, no clipped labels, targets ≥44px) | Actual iOS safe areas, system chrome, native renderer |
| 3 | Scope, host health, blocked attention and Settings discoverable; offline/key-mismatch/active filters never hidden | Prototype-satisfied (browser) | `evidence/a-offline|connecting|key-mismatch|empty|dense-390x844.png` + the same set for B; truthful-state checks (unknown=12, outputs disabled, retry does not fabricate data) in `evidence/verification.json` | Live data/network behavior (none claimed) |
| 4 | Settings → saved mode change → dismiss shows chosen mode; return/relaunch restores it; Open Board recovery reachable; preference semantics explicit | Prototype-satisfied (browser localStorage simulation) | `evidence/verification.json` mode round-trips + cold-reload checks for A and B; `evidence/workflow-verification.json` (56 shared-scope/keyboard checks); defaults table below | Real UserDefaults migration, app relaunch (#458); native spec |
| 5 | All owner-selected direction and proposed defaults reconciled in one final approved specification; no silent change to locked art or Board theme | **Partial by design** — owner receipt reconciles the direction; defaults table below records every proposed item with its disposition; nothing is silently resolved | Receipt above; defaults table below; locked-art provenance (`references/provenance.json`, `evidence/layer-verification.json` pixel equality, horse-viewBox/vector checks in `tools/package.py`) | "Final approved specification" for native is owned by the implementation children (#456--#460), not by this package |
| 6 | Verify prototype reachability, safe areas at small/large widths, tap targets ≥44pt, readable long labels, contrast, Reduce Motion/static alternative | Prototype-verified (browser) + re-measured at promotion | `evidence/a|b-small-360x740.png`, `a|b-large-430x932.png`, 145% type + rail-end PNGs, `b-reduced-motion-390x844.png`; measured minimum contrast **4.788:1** behind actual text (`evidence/verification.json`); promotion re-measure: 12/12 canonical + 6/6 package-local targets HTTP 200 and byte-identical (`package-verification/`) | Physical phone; iOS Dynamic Type/VoiceOver certification; native Reduce Motion |
| 7 | Package reproducible evidence and provenance under `docs/design/evidence/issue-<N>/` plus phone gallery link; no production code changes | **This package** | This directory; generator byte-identity 35/35 (`package-verification/regen-proof.json`); `tools/package.py --verify` exit 0; recomputed `manifest.sha256`; docs-only commit | Repo-local gallery serving is documented below; the canonical phone gallery remains the existing Tailscale URL |
| 8 | Stop at PROTOTYPE — awaiting Guy visual approval | Superseded for the design gate by the 2026-09-09 routing comment (gate cleared for V1/A); this package still stops at packaging — no implementation, no tracker writes | Receipt above; this docs-only change | Native acceptance and physical-device evidence remain with #456--#460 |

"Prototype-satisfied" means: satisfied **in the labeled browser prototype
evidence only**, not in native code or on a device. Every child reference is
design coverage, not child completion.

## Proposed defaults and interaction semantics — disposition

Sources: issue #455 "Proposed details to validate visually", the designer
README's "Proposed edge defaults — awaiting approval", and the owner receipt.

| Item | Disposition |
|---|---|
| Saved Board/Herd mode controlled from Settings, applies immediately, no top Board/Herd switch, Open Board recovery retained | **Owner-approved** (receipt, verbatim above) |
| No saved preference → Board; unknown stored value → Board (migration default) | Issue-preserved contract ("Preserve Board as the no-preference migration default") + prototype-confirmed; carried as prototype semantics; final native spec owned by #458 |
| Open Board recovery is a temporary override and does not rewrite the saved preference | Owner direction ("Open Board recovery retained") + issue contract; prototype-verified (recovery with `saved=Herd`) |
| Foregrounding alone does not reverse recovery; cold reload restores the saved preference; explicit Settings re-selection wins immediately | Prototype-verified in browser (reload/restore/focus checks); not separately quoted in the receipt -- carried as prototype semantics for the native spec |
| Board and general Settings keep the established app theme; ranch Day/Night treatment applies only in the ranch context | Issue contract + prototype; unchanged locked art/theme |
| Day/Night/Auto environment independent of the Catppuccin app preference; Auto is a deterministic demo Day (no location/solar computation) | Preserved issue contract; Auto labeled as prototype simulation |
| Namespace `corral455.prototype.savedMode`; only mode+theme persisted, never hosts/agents/output; environment and filters session-only | Prototype implementation detail (browser only); not a shipping contract |
| Reduce Motion resolves to phase zero / standing idle specimen; background/obscured states stop work | Prototype-verified (emulated system reduce + lifecycle checks); native owned by #459/#460 |

Items in this table that the receipt does not quote explicitly are **not**
claimed as separately owner-approved; they remain prototype semantics pending
the native specifications.

## Motion — what is and is not accepted

The owner explicitly could **not** see motion in the prototype, and the
receipt states that approval is NOT acceptance of imperceptible motion.
Therefore this package makes **no perceptibility claim**: the retained clips
(`day-realtime.mp4`, `night-realtime.mp4`) and the measured frame deltas
(`evidence/motion-verification.json`, `*-motion-difference-8x.png`) document
what the browser prototype does; they do not satisfy #459/#460, which require
clearly perceptible gentle movement in real time at phone scale plus Reduce
Motion/power/lifecycle guards. Browser frame deltas alone cannot satisfy
native/device ACs. Preserve the A layout while improving motion.

## Native / device boundary (not proven here)

Everything in this package is labeled browser PROTOTYPE evidence. No physical
iPhone, no SwiftUI/Canvas render, no native build, no CI, no CPU/frame-rate/
thermal/battery result, no VoiceOver or Dynamic Type certification, no
UserDefaults migration, and no SSE/subscription invariants are proven. The
child-map comment requires physical iPhone screenshots/video before
visual/motion claims pass; prototype evidence does not prove production
behavior.

## Contents and provenance

| Path | Provenance |
|---|---|
| `index.html`, `variant-a.html`, `variant-b.html`, `fixtures.json`, `assets/`, `evidence/`, `references/` (except the two added files below) | byte-identical copies of the 2026-09-09 designer bundle; verified against `references/source-manifest.sha256` by `package-verification/reconcile.py` (one whitespace-only normalization: `evidence/run-log.txt`, see below) |
| `README-designer-original.md` | the designer bundle's `README.md` (label only; byte-identical) |
| `references/designer-brief-455.md` | the designer bundle's `.brief.md` (relocated; byte-identical) |
| `references/source-manifest.sha256` | the designer bundle's `manifest.sha256` (relocated; byte-identical) -- its 146 entries cover the source bundle except itself |
| `tools/` | copied reproducer toolchain; 10/17 byte-identical, 7 adapted for portability/labels -- `tools/PORTABILITY-NOTES.md`, machine-checked |
| `manifest.sha256` | **recomputed promotion-time** package manifest (excludes itself) |
| `package-verification/` | promotion-time verification layer (new) |

Excluded from the source bundle: `designer-run.log` (designer session runtime
log, sha256 `2aac34fae51322f0e8fdba626d62febb4c017fd4aa92cc289499a2a7d16c6d02`,
234,075 bytes) -- a runtime log, not a dependency of any claim; it remains in
the read-only source bundle. This is the only exclusion.

One whitespace-only packaging adaptation: `evidence/run-log.txt` ends with a
single trailing newline here (the source ends with a blank line at EOF, which
`git diff --check` reports for an added file). Content is otherwise
byte-identical -- source sha256
`59a170a7159230c41e4b71a6b234d8ec6a1aa8d1f1d027d98c0fb8344a914cc2`, package
sha256 `92e67a605b9ca335afd9976246e347c50c1dba6fc44a153d3dfe6a46f87b7e43` --
and `package-verification/reconcile.py` machine-checks the normalized content
hash plus the single-trailing-newline property.

The source bundle at
`/Users/jirathip/design-output/corral/455-immersive-herd/` (symlink-resolved)
was **read-only** for this promotion: recursive tree digest unchanged across
the work (see `package-verification/README.md` "Source read-only proof").

## Reproduce

Requirements (already present on this host; no installs): Python 3 with
Pillow and websocket-client, the cached Playwright
`chromium_headless_shell-1234` headless shell, ffmpeg/ffprobe. The existing
gallery server on `127.0.0.1:8777` is used only for canonical-base checks.

Run from a **scratch copy** so the committed evidence stays byte-stable:

```sh
rsync -a <this dir>/ /tmp/c455/ && cd /tmp/c455
python3 -B tools/build.py                     # regenerate A/B/fixtures/asset SVGs
python3 -B tools/package.py --verify          # validate + manifest match
python3 -B package-verification/reconcile.py  # approved-hash + source-manifest ledger
```

Minimal portable localhost serving path (no shared services touched):

```sh
python3 -m http.server 8963 --bind 127.0.0.1 --directory /tmp/c455
CORRAL455_BASE=http://127.0.0.1:8963 python3 -B tools/probe-visibility.py
```

Existing canonical gallery (Mac-side check only, not a physical-phone test):
`https://jirathips-macbook-air.tail8c3301.ts.net:8444/corral/455-immersive-herd/index.html`
-- measured HTTP 200 with byte-identical content at promotion
(`package-verification/reachability-canonical.json`). No Tailscale wrapper,
Serve or shared networking configuration was changed.

Heavy browser steps (`verify-browser.py`, `verify-workflows.py`,
`verify-art-geometry.py`, `verify-layers.py`, `record-motion.py`,
`run-all.py`) must be serialized on this host under the shared heavy gate:
`~/.local/bin/flock /tmp/corral-heavy-gate.lock <command>` (fresh ≥8 GiB free
floor checked before the run). See `package-verification/README.md` for the
promotion-time attempt and its outcome.

## Package integrity

- `python3 -B tools/package.py --verify` -- fail-closed: evidence invariants
  (`verification.json` PASS, 329 checks + 42 labeled captures, minimum
  contrast 4.788:1), layer pixel equality day+night, art geometry, motion
  clips, fixture set, pinned-source provenance snapshots, secret/absolute-path
  hygiene of the variants, and the manifest byte-match.
- `python3 -B package-verification/reconcile.py` -- fail-closed: approved
  artifact hash equals the receipt constant; every source-manifest entry is
  byte-identical under its original or relocated path (or documented as
  excluded); the adapted tools are exactly the documented seven; the package
  manifest matches the file set on disk. Measured totals (entries, bytes,
  per-extension counts) are recorded in `package-verification/reconcile.json`.
- Promotion-time re-verification (2026-09-12): the full browser battery was
  re-run on a scratch copy of this package under the house heavy gate -- every
  step exit 0, reproducing the retained results (329 checks, 42 labeled
  captures, minimum contrast 4.788:1, 56 workflow checks, Day+Night layer
  pixel equality, real-time clips without time compression). Fresh reports and
  PROTOTYPE-labeled captures: `package-verification/fresh/`; the fresh
  captures differ from the retained set only in the capture label bar.

## Honest limits

- The evidence screenshots/clips are 2026-09-09 browser prototype captures
  (byte-preserved); promotion-time re-verification (2026-09-12) and scratch
  re-runs are records of this host and this commit -- see
  `package-verification/`.
- Scene motion is subtle by the owner's own report; no smoothness, battery or
  perceptibility claim is made anywhere in this package.
- The 145% text check is a CSS equivalent, not iOS accessibility-size
  certification. The 30 fps MP4 containers hold timestamped browser captures;
  effective capture cadence is lower and wall-clock movie bytes are not
  deterministic across reruns.
- Browser SVG/CSS is a disposable design proof; shipping remains native
  procedural SwiftUI Canvas/Path with distinct exportable layers (no
  flattened raster, video backdrop, WebView or new engine).
