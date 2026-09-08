# Corral #442 — Herd view: two information architectures

**Status: PROTOTYPE — awaiting Guy approval.** Design-gate evidence only.
No production Swift/Rust/config/CI changed. Implementation is blocked
until Guy explicitly approves one variant. Recommendation below does not
clear the gate.

Open `index.html` on a phone for the gallery; `prototype-v1-pasture-panorama.html`
and `prototype-v2-ranch-map.html` are the interactive prototypes
(self-contained, no network). `sprite-sheet.html` shows the horse identity
axes and every state pose. `PROVENANCE.md` records art/fixture provenance.

## Product boundary (unchanged)

Corral stays two modes over one authoritative data model:

| Mode | Role |
|---|---|
| **Board** | existing dense, blocked-first operational monitor and authoritative list |
| **Herd** | optional full-screen playful visualization of the *same* live state |

Both variants demonstrate the shared contracts: one segmented **Board / Herd**
switch in the nav row; one **Filters** control (host + repository scope)
whose selection applies to both modes (`Scope · shared`); tapping a horse
opens the same read-only **Recent Output** sheet the Board row opens.

## Baseline read from the app

- `ios/FleetNotifier/UI/AppTheme.swift` — locked Catppuccin tables, mauve
  UI accent, state→token map (working=teal, blocked=red, done=green,
  idle=subtext0, unknown=surface2), chip mix ratios, `RepoHue` FNV-1a ring.
- `ios/FleetNotifier/UI/StateStyle.swift` — raw herdr labels + marks
  (`!`, `○`, `◦`, `✓`, `?`), attention ranks (blocked → working → idle/done
  → unknown).
- `ios/FleetNotifier/UI/FleetViews.swift` — `WorkingMotion` heartbeat
  (1.2 s, peak 42 %, 160 ms stagger; static dot under Reduce Motion),
  `RepoLabelChip`, `AgentRow` state chip, #427 top-left Filters control,
  status sections with repo subgroups, `RecentOutputSheet`.
- `docs/ARCHITECTURE.md` — read-only monitor; `done` renders idle-equivalent,
  never active; disconnected = no live feed, never fabricated.

Every token, mix, glyph, and timing in the prototypes is ported from those
files (see `scripts/fixtures.py`, `scripts/build.py`). Only the pasture
environment colours (sky/grass/fence) are new, and they are documented token
mixes.

## Locked brand grammar → prototype

| Rule | How it is honoured |
|---|---|
| Every agent is a horse | `horsesvg.py` is the only art source; harness/model text never selects art |
| Deterministic identity | `sha256(name)` → coat(8) × breed silhouette(3) × mane(3) × tack(3) × accessory(3); probe P10 |
| Fleet = herd, repo = paddock | both variants group horses inside labelled paddocks with the repo's `RepoHue` colour |
| Blocked at the front rail | a dedicated rail region at the top of the screen, `!` alert mark + `blocked` text on each horse; probe P08 |
| State = pose, never species | poses per state; Reduce Motion swaps poses for statics, never pauses; probe P13 |
| No kitchen vocabulary/assets | none used; PROVENANCE.md |

## The two architectures

### V1 — Pasture Panorama (`prototype-v1-pasture-panorama.html`)

Side-on illustrated ranch. Top: the **front rail** strip (a fence line;
blocked horses stand at it, largest sprites). Below: a horizontally paged
**paddock strip** — one repository per page, swipe ↔, page dots and a
`n/4` counter. Horses are large (148 px wide) and read as characters; the
grazing/trotting poses carry the storytelling.

Overflow strategy: paging by paddock; within a paddock the field wraps and
scrolls vertically. Labels never shrink (12 px mono nameplates in the
field, 11 px at the rail).

### V2 — Ranch Map (`prototype-v2-ranch-map.html`)

Elevated overview. Top: the **front rail** as a bounded, red-bordered card
(blocked horses only). Below: every paddock as a bounded card with a repo
hue band, per-state count chips (`!1 ○3 ◦1 ✓1 ?1`), a collapse chevron,
and a 4-up horse grid. A paddock shows one 4-up row by default; more
horses sit behind a **Show all N · M more** row — overflow by navigation,
not by shrinking. The page scrolls vertically; all four paddock boundaries
are visible from one screen.

Horses are smaller (78 px in the grid, 96 px at the rail) but keep every
identity axis; nameplates wrap to two lines at 10 px mono instead of
truncating.

## Measured comparison (390×844, Mocha, from the stage DOM)

Measured by DOM probe against the exact stage HTML the PNGs were captured
from (`measure` block in `logs/measure.log`).

| Metric | V1 Pasture Panorama | V2 Ranch Map |
|---|---|---|
| Horses visible on first screen — blocked-heavy (12 total) | **4** | **9** |
| Horses visible on first screen — dense fleet (18 total) | **5** | **13** |
| Blocked horses visible on first screen (2 total) | 2 | 2 |
| Paddock headers visible on first screen (4 total) | 1 | 3 |
| Gestures to see every paddock | 3 swipes | ~0.5 screen of scroll (1.42–1.51 screens total) |
| Horse sprite width | 112 px rail / 148 px field | 96 px rail / 78 px grid |
| Nameplate size | 11–12 px mono, single line | 10–11 px mono, wraps to 2 lines |
| Dense-fleet overflow | swipe + vertical field scroll | `Show all 5 · 1 more` row (one per paddock over 4) |
| Repo-scoped view (Filters → one repo) | one paddock fills the page | one card, same chrome |
| Interactive targets < 44 pt | 0 | 0 |

Readability: both variants keep every label at or above the 10 px floor and
every control at ≥ 44 pt (probe P03/P15). V1's horses are 1.9× larger and
individually more recognisable; V2 keeps all identity axes legible at 78 px
(checked visually on the dense-fleet PNG — coat, mane, tack and hat all
distinguish at that size).

Scalability: at 18 horses V2 shows 13 on the first screen with all paddock
boundaries and counts; V1 shows 5 and needs three swipes to establish that
the other paddocks exist. At the issue's floor (4 paddocks / 12 horses) V1
is comfortable; beyond ~6 paddocks V1's swipe count grows linearly while
V2 grows by a fraction of a screen per paddock and its count chips let you
skip collapsed paddocks.

## Recommendation: V2 Ranch Map (with V1 as the paddock zoom)

Recommend **V2** for the first Herd implementation:

1. Blocked-first scanning survives scale — the front-rail card and every
   paddock's `!` count are on the first screen regardless of fleet size.
2. Predictable navigation maps onto the existing Board mental model
   (sections → repo subgroups → rows becomes rail → paddock cards → horses),
   so the shared filter/selection state ports with no new concepts.
3. Bounded cards are the cheapest SwiftUI port (a `LazyVStack` of cards, a
   `LazyVGrid` per paddock) and give Dynamic Type somewhere to go (cards
   grow; the grid drops to 3-up at accessibility sizes).

Tradeoffs accepted: V2's horses are smaller and the pasture is less of a
"place"; V1 is the stronger story and the better demo. The recommended
path keeps that value by making V1's single-paddock page the **zoom** when
you tap a paddock header in V2 (both are already built from the same
screen generator, so the impl lane inherits both). If Guy prefers the
story over scale, V1 as shipped is complete and gate-verified; its cost is
the swipe count on large fleets.

## State truthfulness

| State | Pose (motion) | Reduce Motion static | Non-colour cue |
|---|---|---|---|
| working | trot loop (leg swing 0.62 s, body bob), heartbeat squares on the flag | planted stand, heartbeat → static teal dot (#371 lock) | flag text `working`, `○` count glyph |
| blocked | head high, near hoof pawing (1.1 s), flag pulses | hoof planted, no pulse | position (front rail) + `!` alert mark + `blocked` text |
| idle | grazing, slow neck sway (4 s) | grazing static | flag text `idle`, `◦` |
| done | settled level stand — never a work loop | same | `✓ done` flag; ranked with idle |
| unknown | uneasy stance, `?` badge on the sprite, desaturated | same | `? unknown` flag + `?` badge |
| disconnected | **no animation at all** (Reduce Motion forced), every horse last-known → `unknown`, saturation 30 % | same | `Disconnected` bar under the chrome + `Source disconnected` card (host, reason, last snapshot rev/age, `Open Board` / `Retry`); front rail says `unknown · cannot be confirmed`, never "no blocked" |

Nothing animates or counts as live during an outage (probe P09).

## Shared Board/Herd behaviour (demonstrated in both prototypes)

- **Switch**: segmented `Board | Herd` in the nav row; state (`data-mode`)
  flips and the same fixture renders as rows or horses (probe P04).
- **Filters**: one sheet, one model. Picking `atlas-vector` in Herd scopes
  Board to the same repo; `Filters · 1` and `Scope · All hosts · atlas-vector`
  read identically in both modes (probe P05).
- **Detail**: horse tap → Recent Output sheet for that agent; the Board row
  for the same agent opens the identical sheet (probe P06). The sheet
  header carries the #371 state chip, repo chip, host chip, and the #373
  block-per-run tail with role markers.

## Accessibility constraints (documented for the impl lane)

- **Tap targets**: every horse button, option row, chevron, segmented
  button and pill is ≥ 44 × 44 pt (probe P03 measures the rendered boxes).
- **Dynamic Type**: nameplates and flags are `caption2`-class mono text
  and must scale with `@ScaledMetric`; at accessibility sizes V2's grid
  drops from 4-up to 3-up, V1's field wraps to one horse per row, and the
  rail strip scrolls horizontally. Repo names use `lineLimit(1)` with
  truncation only at the header level; horse names wrap (V2) or truncate
  with the full name in the accessibility label (V1). Floors: 10 pt mono
  nameplate, 11 pt flag, 13 pt body.
- **Reduce Motion**: `prefers-reduced-motion` and the in-app toggle remove
  every animation (`animation: none`) and swap poses; nothing merely pauses
  mid-stride (probe P13). Maps to `ThemeStore.reduceMotion`.
- **VoiceOver**: each horse is one element — "`name`, `state`, `repo` on
  `host`. Opens recent output." Rail and paddocks are landmarks with labels.
- **Colour**: no state or repo meaning is colour-only — every state has a
  glyph + word; repos have a name chip beside the hue; the blocked cue is
  also positional (front rail).

## Reproduction

All commands run from `docs/design/evidence/issue-442/`. Python 3.9+
stdlib only (no venv needed; nothing installed). Renderer is the local
Playwright `chrome-headless-shell` (set `CHROME_HEADLESS_SHELL` to override).

```
bash scripts/run-gates.sh                              # all six steps below, raw exit codes -> logs/exit-codes.log
PYTHONDONTWRITEBYTECODE=1 python3 scripts/build.py     # all HTML + stage/  (logs/build.log)
PYTHONDONTWRITEBYTECODE=1 python3 scripts/render.py    # 21 PNGs, IHDR-checked 390x844 (logs/capture.log)
PYTHONDONTWRITEBYTECODE=1 python3 scripts/measure.py   # README comparison numbers (logs/measure.log)
PYTHONDONTWRITEBYTECODE=1 python3 scripts/check-dimensions.py  # independent 390x844 + count (logs/dimensions.log)
PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify.py    # DOM/interaction gate + manifest (logs/verify.log)
shasum -a 256 -c manifest.sha256                       # independent check (logs/manifest-check.log)
PYTHONDONTWRITEBYTECODE=1 python3 scripts/mutate.py    # ~5 min RED/GREEN proof the probes bite (logs/verify-mutation.log)
```

`logs/` is written by the gates and therefore excluded from the manifest
(every other file, including every script and `stage/`, is hashed).

Scripts: `fixtures.py` (fictional fleet + palette/mix ports, self-checking),
`horsesvg.py` (original horse art), `build.py` (one CSS, all screens),
`render.py`, `verify.py`, `mutate.py`. A visual change = edit the generator,
rerun the five commands; every SHA regenerates.

## Artifact inventory

- `prototype-v1-pasture-panorama.html`, `prototype-v2-ranch-map.html` — interactive
- `index.html` — phone gallery; `sprite-sheet.html` — identity × pose sheet
- `stage/*.html` + `stage/specs.json` — the 21 exact capture inputs
- 21 PNGs `*-390x844.png`: 12 required (2 variants × Mocha/Latte ×
  blocked-heavy/dense-fleet/disconnected) + detail-sheet, filter-sheet,
  reduce-motion (Mocha) and repo-scoped (Latte) per variant + Board mode
- `README.md`, `PROVENANCE.md`, `manifest.sha256`, `logs/`

## Not captured here

No iOS implementation, no simulator stills, no SwiftUI. The impl lane owns
those after variant approval.
