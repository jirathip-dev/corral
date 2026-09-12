# tools/ portability + label ledger (promoted #455 package)

The scripts in this directory are copies of the designer-run bundle's tools.
**10 of the 17 files are byte-identical to the source**; 7 were adapted for
in-package portability or capture labels only. This ledger is machine-checked:
`python3 -B package-verification/reconcile.py` verifies that the set of files
whose bytes differ from the retained source manifest
(`references/source-manifest.sha256`) is exactly the seven listed below, and
fails closed on any other drift.

| file | change | source sha256 | package sha256 |
|---|---|---|---|
| browser.py | served base URL is now `CORRAL455_BASE` (default: the canonical existing gallery server `http://127.0.0.1:8777/corral/455-immersive-herd`) so any local mount of this package can be driven | `cb6b74beba0d3a52a9c5e3ab68618c2d02ef2eafc5b231a79c2a9a665267150b` | `ae1afdcad2c187468352eb2e3ed1479c2d07e9e7f01a7a8070fb0ffcd2092489` |
| check-reachability.py | bases (`CORRAL455_BASES`) and served path prefix (`CORRAL455_PREFIX`) are env-overridable; defaults keep the canonical two bases | `7682e74f19826874e1aa8ee5f566368ecee92be3595f474964033554cd7212e2` | `1de4d12c10802a7d543b6225f5d56edbf14c90730ce584fcf2fddd577c26fcba` |
| gather.py | refuses to run in-package unless `CORRAL455_GATHER_OK=1`: it is a source-run-only collector that would overwrite the retained `references/` snapshots from external checkouts | `6306a74e9ce3c86ca508ac8a734e9c3a87d39695d2f885c32f125da1abe702b4` | `2f57617d9a24c360ad6293cf0dd9b51b0f611e168fbc67041fe6373997462076` |
| package.py | `validate()` maps the accepted #442 reference entries of `references/provenance.json` to their in-package snapshots under `references/` (the source run resolved them from the design-output checkout); the source-run working-tree scope audit was removed (it compared the design-output repo around the run; its result is retained verbatim in `evidence/scope-audit.json`) | `c97e8aad0f4b0311e0f73245597727ed663fb7e620135562f04228a4aca9da46` | `f0578adc142e4bf67352193f9cec46696f9e75a0881ae2cd43be53e54d529399` |
| record-motion.py | capture label now `... / PROTOTYPE BROWSER PROOF` (was `... / BROWSER PROOF`) | `88878120d0cd7450004dee2ca27941f5744bc11ab2d2ec384c485d33f7285cbe` | `f17ee5ee65346ee46822c9694250d6c4ef8e846d125bcf9eeb4bfb1270b31407` |
| verify-browser.py | capture label now `... \| PROTOTYPE \| HTML PROOF` (was `... \| HTML PROOF`) | `f04e5831b5633d073a9f8e42e630aa56506d7da13fefc56bb28f6f052d846c3a` | `7736fa8dbd55f2d8e279d74f95a8109101bb6434f09d1e6751b2b977874e8c68` |
| verify-workflows.py | capture label now `... \| PROTOTYPE SYNTHETIC PROOF` (was `... \| SYNTHETIC HTML PROOF`) | `c6b1be914b3f77a4e4ee22b844d759425f68fa22c2aa1749d3a784c12d81525e` | `ce1d202617289341ce5bf23512296ea5f688d20519d3dd62b67587c07d446d11` |

Byte-identical to source (all 10): `app.css`, `app.js`, `build.py`,
`contact-sheets.py`, `inspect-motion.py`, `probe-visibility.py`, `run-all.py`,
`template.html`, `verify-art-geometry.py`, `verify-layers.py`.

## Consequences to keep in mind

- **Generated-HTML claim.** Only `build.py` regenerates the HTML artifacts, and
  it is byte-identical to the source. It was re-run in a scratch copy during
  promotion: all 35 generated files (including `variant-a.html`) regenerated
  byte-identically. See `package-verification/README.md` and
  `package-verification/regen-proof.json`. No adapted script claims to
  reproduce the approved artifact outside that proof.
- **Labels.** The PNGs shipped in `evidence/` were captured on 2026-09-09 by
  the original scripts and keep their original labels and bytes. Captures made
  by the promoted copies are labeled `PROTOTYPE` because the browser prototype
  is design proof, not native/device evidence.
- **Heavy steps.** `verify-browser.py`, `verify-workflows.py`,
  `verify-art-geometry.py`, `verify-layers.py`, `record-motion.py` and
  `run-all.py` drive a headless browser. On this host they must be serialized
  under the shared heavy gate (`~/.local/bin/flock /tmp/corral-heavy-gate.lock
  <command>`), and run from a scratch copy so the committed evidence set stays
  byte-stable (see the top-level README "Reproduce").
- **No new dependencies.** Every script uses only what the designer run
  already required: Python 3 with Pillow and websocket-client, the cached
  Playwright `chromium_headless_shell-1234` headless shell, ffmpeg/ffprobe.
