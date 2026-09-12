# Fresh browser re-verification (2026-09-12) — scratch copy of this package

The full browser battery was re-run during promotion on a **scratch copy** of
this package served locally (`python3 -m http.server 8963`, bound to
127.0.0.1), under the house heavy gate
(`~/.local/bin/flock /tmp/corral-heavy-gate.lock`). Run window:
2026-09-12T08:35:22Z–08:36:53Z. Free-disk floor checked fresh before the run:
44 GiB (≥8 GiB required); host load average ~140 at start. The sandbox had no
network side effects and no shared service was reconfigured.

Every step exited 0 (`../logs/suite-steps.log`):

| step | raw exit | seconds |
|---|---|---|
| smoke (`tools/probe-visibility.py`) | 0 | 2 |
| `tools/verify-browser.py` | 0 | 56 |
| `tools/verify-workflows.py` | 0 | 9 |
| `tools/verify-art-geometry.py` | 0 | 0 |
| `tools/verify-layers.py` | 0 | 2 |
| `tools/record-motion.py` | 0 | 26 |
| `tools/contact-sheets.py` | 0 | 0 |
| `tools/inspect-motion.py` | 0 | 0 |

Fresh results are numerically identical to the retained 2026-09-09 designer
evidence:

- `evidence/verification.json`: **PASS**, 329 checks, 42 labeled captures,
  118 contrast samples, minimum contrast **4.788:1** (same as retained).
- `evidence/workflow-verification.json`: **PASS**, 56 shared-scope/keyboard
  checks — check names and pass values identical to the retained report.
- `evidence/layer-verification.json`: pixel equality for both Day and Night
  plane reassembly (same as retained).
- `evidence/art-geometry.json`: painted-horse bounds adjudication PASS.
- `evidence/motion-verification.json`: two real-time clips, no time
  compression (Day 98 frames / 12.09 s wall; Night 77 frames / 12.07 s wall).

## Fresh vs retained captures (label-only difference)

All 42 fresh screenshot **paths and pixel sizes** match the retained set, and
every check name + pass value matches. Screenshot hashes differ **only** in
the capture label bar: the promoted copies label captures `... | PROTOTYPE |
...` (see `../tools/PORTABILITY-NOTES.md`). Pixel comparison of representative
stills (a-day, a-night, b-day, gallery) shows the difference confined to the
label bar rows (y≈3–12); **below the label bar the frames are pixel-identical**
to the 2026-09-09 captures.

Files here are the scratch root's `evidence/` outputs (JSON paths inside them
are scratch-relative). They are PROTOTYPE browser proof and never a
replacement for native/device evidence; the retained, original-labeled
evidence set remains untouched in `../../evidence/`.
