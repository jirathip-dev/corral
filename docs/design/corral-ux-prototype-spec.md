# Corral UX — Prototype Conformance Spec (authoritative for the fleet)

> Source of truth: `docs/design/corral-ux-prototype.html` (design-gate, commit ab9194f).
> This spec is the strict, codeable checklist the egui board (`corrald-ui`) AND iOS
> (`FleetNotifier`) MUST match. **Prototype wins for look/interaction; ACs win for
> behavior/tests.** Verify against the rendered prototype (`/tmp/prototype-egui-crop.png`
> captured 2026-08-24) — not from prose alone.

## 1. Theme — exact tokens

| Token | Hex | Use |
|---|---|---|
| `--bg` | `#0d1117` | window / list background |
| `--panel` | `#10151c` | frame, left pane |
| `--panel2` | `#161b22` | card / row selected / recent-output block |
| `--panel3` | `#1c2128` | tool tag, question box |
| `--line` | `#30363d` | borders, dividers, chips |
| `--ink` | `#e6edf3` | primary text |
| `--muted` | `#8b949e` | secondary text, idle label, timestamps |
| `--accent` | `#2dd4bf` | teal: active tab underline, active "Cards", "Load earlier", "Answer inline" |

### State palette (non-negotiable)

| State | Color | Example |
|---|---|---|
| needs-you / blocked | `#f85149` (red) | card left-border 3px, dot fill, state text |
| done / review | `#d29922` (amber) | dot fill-faded, state text |
| working | `#58a6ff` (blue) | dot ring, state text, active filter chip bg `rgba(88,166,255,.12)` |
| idle | `#8b949e` (gray) | dot empty, state text |
| unknown | `#6e7681` | fallback |
| user message | `#12263f` (user-tint) | recent-output user block bg `rgba(18,38,63, ...)` |

## 2. egui master/detail layout

Two-pane, `42% / 58%` grid, both panes full-height, rounded 12px, border `#2a2f37`.

### Left pane (master) — the agent list

- **Top**: search field (`Search repo / branch / issue…`), then chips `Needs you` (red-tinted `rgba(248,81,73,.16)`, text `#f85149`) and `All` (active = `--working` bg, dark text `#08131f`).
- **Column header**: `Agent` | `State · time` (9px, muted, uppercase-ish letterspacing).
- **Rows**: colored **left border 3px** by state (`#f85149` / `#d29922` / `#58a6ff` / transparent), row bg tinted by state (`rgba(248,81,73,.06)` blocked, `rgba(210,153,34,.05)` done, `rgba(88,166,255,.05)` working). Each row:
  - state **dot** (11px): `.fill` blocked (solid red), `.f-done` amber, `.ring` working (blue outline, transparent fill), `.empty` idle (gray).
  - title (13px, weight 600, ellipsis), tool tag (`claude`/`codex`, mono 9px pill on `--panel3`).
  - right cell: `state` (colored, bold 10px) `·` `time-in-state` (muted 10px).
- **Row selected**: `--panel2` bg.
- **Bottom**: `Idle / done (N) — expandable` collapsed row (chevron ▸, muted).

### Right pane (detail)

- **Tabs**: `Board` `Issues` `Audit` (11px, weight 600, muted; active = `--ink` + 2px teal underline).
- **Action buttons**: `Cards` (active: teal border `--accent` + `--ink`), `Table`, `Interrupt`, `Kill` (red `#f85149`).
- **`Recent output` header** (12px, bold).
- **Recent output**: `● live` (teal, 10px) + `stick-to-bottom` (muted); blocks:
  - tool block: mono, `--muted`, collapsible `▸` toggle (`--working`).
  - agent block: regular text, `--ink`.
  - user block: bg `--user-tint`, right-aligned `margin-left:24px`, who label `#6ea8ff`.
  - divider `─────  229 earlier lines  ·  Load earlier ─────` (`Load earlier` teal).

## 3. Zero-state rule (both clients)

When there are 0 blocked / needs-you agents, HIDE the entire "Needs you" section (header + empty row). The list shows only Running and Idle. No `Needs you (0)` header, no "No blocked agents" row.

## 4. Non-negotiable conformance checks

- [ ] Exact theme hex values above (no near-miss grays/blues; accent MUST be `#2dd4bf`).
- [ ] State **color maps to state** (red=needs-you, amber=done, blue=working, gray=idle) — dot, border, text all consistent.
- [ ] egui is master/detail (42/58), NOT a flat repo-grouped card grid.
- [ ] Chips `Needs you` + `All` render with correct active treatment.
- [ ] Right pane has Board/Issues/Audit tabs + Cards/Table/Interrupt/Kill + Recent output, in that treatment.
- [ ] Zero-state hides "Needs you" at count 0.
- [ ] No glassmorphism, no feature-tile grid, no extraneous gradients (slop-audit constraint).

## 5. Pixel-diff procedure (post-#200, per Guy)

Once #200 lands and the board populates with real agents:

1. Render this spec's prototype (`/tmp/prototype-egui-crop.png`) and screenshot the live board.
2. Produce a per-element divergence list: color drift, spacing/density, missing labels, wrong state colors, missing chips/tabs.
3. File each divergence back as a conformance issue referencing this spec.

## Revision

2026-08-24 — extracted from rendered prototype; authoritative tokens + layout + zero-state.
