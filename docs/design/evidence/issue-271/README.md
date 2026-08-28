# #271 Transcript rendering restyle — prototype variants (design gate)

Status: **PROTOTYPE ONLY — no Rust/Swift/daemon source changes made in this lane.**
This bundle presents three variants of the restyled Recent-output / chat
transcript on BOTH surfaces (egui board frame + iOS 390x844 phone frame) plus
per-surface tofu before/after evidence rows, and records the design decisions
so Guy can pick one on the issue. Implementation is serialized: it waits for a
recorded `DESIGN APPROVED … Variant N` on #271 (Guy's rule; the orchestrator
owns PR/merge).

## Scope recap (from #271)

1. **Block rendering, not flat text** — group per speaker/tool; no per-block
   "assistant" labels (label on speaker change only); compact repeated tool
   runs ("preparing terminal…" once, icons for tool lines); diff hunks kept
   but styled.
2. **Tofu fix** — unsupported glyphs (Nerd-Font/PUA) must never render as □:
   per-surface strategy below. Supported emoji stay.
3. **Syntax highlighting** — code/diff blocks highlighted (Python/shell/diff
   minimum) on both surfaces, client-side render.

## Baseline (current app, as read in this worktree)

- egui board: `clients/egui/src/ui/board.rs` — `recent_output_surface()` renders
  every block with a **per-block role label**: `RichText::new("assistant")`
  (around L1953), `RichText::new("tool")` (L1936), `RichText::new("user")` —
  the label repeats on every block, and body text is flat 12pt lines
  (`recent_message_lines`) with `recent_code_line` for code-shaped content.
  No font bundling exists under `clients/egui` (`assets/` holds only the demo
  fixture; egui's default Hack/Ubuntu fonts have no Nerd-Font/PUA coverage) —
  PUA icon glyphs from agent output render as tofu/blank.
- iOS: `ios/FleetNotifier/UI/RecentOutputModel.swift` — `RecentOutputRow` keeps
  `TranscriptBlock` boundaries; `codeLines(for:)` highlights **only tool
  blocks** that pass `isCodeOrDiff` (diff +/-/@@ + single-line
  keyword/string/comment scan); agent/user blocks stay plain text. The row
  view in `ios/FleetNotifier/UI/FleetViews.swift` renders each block on its
  own (kind treatments per block).
- daemon: `src/adapters/herdr.rs` `bounded_redacted_tail()` (L2906) — bound +
  redact + `scrub_tui_furniture` (`src/core/blocks.rs` L239, the #253
  box-drawing/block-element furniture scrub). **No PUA/Nerd-Font glyph
  handling exists today**; `src/api/drive.rs` serves the same scrub result as
  `{lines}` (egui) and `{blocks}` (iOS), so a glyph pass placed there is
  shared by both clients automatically.

## Design language (no new design language)

`contracts/state-tokens.json` + the approved
`docs/design/corral-ux-prototype.html` system: bg `#0d1117`, panels
`#10151c/#161b22/#1c2128`, line `#30363d`, ink `#e6edf3`, muted `#8b949e`,
accent `#2dd4bf`, blocked `#f85149`, done/amber `#d29922`, working/blue
`#58a6ff`, unknown `#6e7681`, user tint `#12263f`. **No new hexes.**

Syntax-highlight palette (client-side render, token palette only):

| kind | token |
|---|---|
| keywords / `@@` hunk | working `#58a6ff` (hunk uses done `#d29922`) |
| strings / punctuation | done `#d29922` |
| comments / shell output | muted `#8b949e` |
| diff addition `+` | accent `#2dd4bf` (soft teal row tint) |
| diff deletion `-` | blocked `#f85149` (soft red row tint) |
| success line / function names | accent `#2dd4bf` |

## Variants (2-3 distinct stances, not pixel variants)

All variants use the **same illustrative transcript excerpt** (terminal run,
apply_patch + diff hunk, continuation message, user message, final reply) so
the three treatments are directly comparable. Mock content is illustrative
synthetic copy (marked as such); the diff hunk mirrors real `board.rs` code
shape. PUA codepoints in the tofu evidence are representative Nerd-Font
icons.

### Variant 1 — Speaker-run cards (`prototype-v1-cards-board.html` / `-phone.html`)

- One **run card** per speaker run (contiguous same-speaker blocks); the run
  header carries the role chip (`ASSISTANT`/`USER`) + model/effort/worktree
  meta + timestamp **once, at speaker change only**.
- Tool runs inside a run are **icon-chip rows** (terminal/edit/check SVG
  icons at this mock; implementation may use a bundled font subset or vector
  glyphs): "preparing terminal… · cargo test" with a run count `×4 · exit 0`
  and a collapse chevron — 1 icon row replaces N raw text lines. Diff hunks
  are **kept but styled**: inset code card with a numbered gutter, teal `+`
  rows, red `-` rows, amber `@@` hunk, muted headers.
- After a tool run the same speaker continues inside the SAME card (no second
  label). User message gets a tinted card. Emoji pass-through line (✅ ⚠️ 🚀)
  shown at the bottom of the run.

### Variant 2 — Timeline rail (`prototype-v2-rail-board.html` / `-phone.html`)

- Chronological **rail** with a per-speaker marker dot (accent = assistant,
  blue = user, muted = tool) and timestamp per block; the speaker name chip
  appears **only when the speaker changes** (continuation blocks show the
  time only).
- Tool calls are **inline chips on the rail** (icon + summary + count `×4` /
  `+2 −1`); code/diff blocks are full-width cards with the numbered gutter +
  token colours.
- A `● live · stick-to-bottom` tick anchors the bottom (the transcript's live
  edge).

### Variant 3 — Dense reduced (`prototype-v3-dense-board.html` / `-phone.html`)

- **Zero card chrome**: sticky run header (agent · model · worktree · age +
  collapse) at the top; blocks separated by hairlines; a tiny role tag
  (`assistant`/`user`) inline **only at speaker boundaries**.
- Tool lines are single-line icon rows (`preparing terminal…` shown **once**,
  counts `×4`); code/diff boxed with the gutter and highlighted; the densest
  of the three (most content per px) — the "as close to raw text as the
  requirements allow" stance.

## Tofu strategy (per surface — from the issue's options)

- **Shared floor (both clients): daemon-side scrub/transliterate in the #253
  path** — `bounded_redacted_tail` after the existing furniture scrub:
  PUA/unsupported glyphs → bounded ASCII tags (`[ok]/[warn]/[env]`-style, or
  stripped ≤ 8 chars/token); **non-PUA emoji + VS16 pass through, never
  stripped**. Because the scrub sits at `bounded_redacted_tail`, board and
  phone see the identical text (no client-specific hacks — #271's shared-path
  criterion).
- **egui board additionally bundles a small covering-font subset** (curated
  Nerd-Font range appended to egui's `FontDefinitions`), so a curated icon
  allowlist still renders as real glyphs on the board; every other PUA char
  is still scrubbed (the scrub is the guarantee; the font is the
  enhancement).
- **iOS: shared scrub only** (no per-surface font; the platform fonts get the
  ASCII tags, emoji stay color).
- Before/after evidence rows + the bounded codepoint→tag mapping table live
  in `tofu-before-after-{phone,board}.png`. The "before" rows render the raw
  PUA glyphs — they show as the notdef box/tofu the client paints today; the
  "after" rows show the scrubbed text with emoji preserved.

## Evidence files

| File | What it shows |
|---|---|
| `variant-1-cards-390x844.png` | iOS phone frame, Variant 1 (run cards) |
| `variant-1-cards-board.png` | egui board frame (1040x720 mock window), Variant 1 |
| `variant-2-rail-390x844.png` | iOS phone frame, Variant 2 (timeline rail) |
| `variant-2-rail-board.png` | egui board frame, Variant 2 |
| `variant-3-dense-390x844.png` | iOS phone frame, Variant 3 (dense reduced) |
| `variant-3-dense-board.png` | egui board frame, Variant 3 |
| `tofu-before-after-390x844.png` | iOS phone frame: glyph before/after + mapping |
| `tofu-before-after-board.png` | egui board frame: glyph before/after + mapping |

Phone frames are exactly **390x844** (the gate). Board frames use a
**1040x720 mock window** — the real egui window is resizable; the phone
surface is the pinned evidence size (board has no fixed gate; #206's board
captures used different window sizes).

All 8 PNGs rendered from the in-repo stage copies via the machine-cached
chrome-headless-shell (2x render → downscale; phones downscaled to exactly
390x844) and vision-inspected (full frame, no scroll chrome, no mid-word
chip cuts, no tofu squares in the variant frames, clean pane endings).
Exact commands + SHA-256s in `capture.log`.

## NOT captured here

- No implementation (egui / iOS / daemon unchanged — impl is serialized after
  recorded approval).
- No live-simulator screenshots (the impl lane owns that evidence, as in
  issue-245/246).
- No final decision on the exact ASCII tag strings or the bundled-font library
  — those are details for the impl lane (the mock's `[ok]/[warn]/[env]`
  tags are illustrative); the strategy (shared daemon scrub as floor +
  optional egui font enhancement) is the lock.
- Design mockup is outline-only (SwiftUI/egui are the visual truth in code);
  this bundle is disposable proof-of-direction, per the standing design gate.
