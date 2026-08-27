# #246 Agent sheet redesign — prototype variants (design gate)

Status: **PROTOTYPE ONLY — no SwiftUI changes made in this lane.** The design
gate was recorded on the issue: **DESIGN APPROVED (Guy, 2026-08-27):
Variant 2 — bottom toolbar** (issue comment
[5439585142](https://github.com/jirathip-dev/corral/issues/246#issuecomment-5439585142),
mockup source `~/design-output/corral/246-agent-sheet-controls/`). This bundle
presents both variants as 390x844 PNG mockups and records the locked decisions;
implementation waits for a later `APPROVED: <variant>` addendum
(serialization: #241 and #245, both merged/closed as of main @ 8e108d9).

## Baseline (current app)

`ios/FleetNotifier/UI/FleetViews.swift` → `AgentDetailContent`:

- Top `ScrollView` holds the state summary (**Controls** headline), then a
  full-width `borderedProminent` primary button (Interrupt / Answer / Attach /
  PR — one per state, issue #166), the overflow `Menu`, and a caption line when
  Kill is disabled by grants.
- Below it, `RecentOutputView` (minHeight 260, maxHeight ∞) and the composer.
- Guy's report (#219): big Interrupt/More pills + hairline + dead black space in
  the controls section; controls should be small unobtrusive side buttons; the
  chat/output should fill the full remaining sheet height.

## Variants (both in repo design tokens — no new design language)

Sources: `contracts/state-tokens.json` + the approved
`docs/design/corral-ux-prototype.html` system (accent `#2dd4bf`, panels
`#10151c/#161b22/#1c2128`, ink `#e6edf3`, muted `#8b949e`, working `#58a6ff`,
user-tint `#12263f`, etc.). HTML mockups are outline only — SwiftUI is the
visual truth.

### Variant 1 — side controls (`variant-1-side-controls-390x844.png`)

1. Full-width pill row + "Controls" section label removed → three small
   buttons (`■ Interrupt` · `± Diff` · `⋯ More`) right-aligned on the meta row.
2. Dead black gap gone: Recent output takes the full height down to the reply
   field.
3. Kill-permission note shrunk to one small secondary line with ⓘ.

### Variant 2 — bottom toolbar (`variant-2-bottom-toolbar-390x844.png`) — APPROVED

1. Pills moved out of the content area → thin toolbar bar (28 h) pinned just
   above the reply field (Interrupt · ± Diff · ⋯ More).
2. Recent output fills the whole mid area (no Controls label, no black gap).
3. Kill-permission note lives as tiny secondary text (ⓘ no kill grant, 9.5 px)
   in the toolbar; the full sentence stays in the ⋯ More overflow entry.
4. `± Diff` = one of the diff access points for #232 (→ `232-diff-page`).

## Evidence

- PNGs rendered from the wrapped stage copies below via the machine-cached
  chrome-headless-shell (2x render, downscaled to the 390x844 evidence
  standard); exact commands + SHA-256s in `capture.log`.
- `prototype-view-*` are the stage copies (source + wrap notes in
  `capture.log`; source SHA-256s: V1 `04567fb1…`, V2 `890f1674…` from the
  design lane's `~/design-output`).
- Both PNGs vision-inspected: full 390x844 frame, no scrollbars/caption
  clipping, no full-width pills, no dead band, chat fills the sheet.

## NOT captured here

- No SwiftUI implementation (later lane, after `APPROVED:` addendum).
- No live-simulator before/after stills (the impl lane owns that evidence, as
  in issue-245).
