# #267 iOS issue browser — prototype variants (design gate)

Status: **PROTOTYPE ONLY — no SwiftUI/egui source changes made in this lane.**
This bundle presents three 390x844 PNG variants of the read-only iOS issue
browser (list + detail) and records the design decisions so Guy can pick one
on the issue. Implementation is serialized: it waits for a recorded
`APPROVED: <variant>` on #267 AND for #232 (same iOS surfaces / FleetViews
area) to land — see #267's serialization note.

## Scope recap (from #267)

Read-only issue browser on iOS: **list (open/closed filter) + detail
(title/body/labels/state)**, lazy-paged like `AgentTranscriptView`; same
grant-gating as read_tail/read_diff (read-only, default-empty); **no GitHub
mutations from iOS**. Board parity = the egui Issues tab
(`clients/egui/src/ui/issues.rs` + `GET /issues`), which this bundle mirrors
in iPhone idiom.

## Baseline (current app)

`ios/FleetNotifier/UI/FleetViews.swift` — there is **no** issue browser. The
phone has agent rows, issue chips, search, transcript, and (pending #232) the
worktree diff. Entry points today: a single trailing toolbar `Menu`
(`slider.horizontal.3` with Settings/Demo-mode items), a `.plain` List with a
pinned chrome section (connection line → filter chips → pull-to-refresh hint),
lowercase pinned section headers (`needs you (2)`, repo groups), and
`AgentRow` = state dot + state badge + title + trailing chips. The issue
browser must be a minimal delta against this idiom, and the egui Issues tab
(`#N  title  STATE` rows, open/closed filter, repo grouping, inline detail
with badges/labels/state) is the board-parity target.

## Variants (all in repo design tokens — no new design language)

Design system: `contracts/state-tokens.json` + the approved
`docs/design/corral-ux-prototype.html` (accent `#2dd4bf`, panels
`#10151c/#161b22/#1c2128`, ink `#e6edf3`, muted `#8b949e`). Issue-state
vocabulary (new, minimal): OPEN = teal accent (`#2dd4bf` — the system's
positive accent, same as "● live" / active chips), CLOSED = muted gray
(`#8b949e`; closed rows dim at 0.65 like idle/done agent rows). Label pills
reuse GitHub label colors (data, not a language). Copy is device-neutral
("read-only · no mutations from this device" — THIS-DEVICE style, no
possessive/slang wording).

### Entry points (fleet screen)
- **Toolbar button** (`prototype-fleet-issues-button.html` →
  `variant-fleet-issues-button-390x844.png`): an "Issues" teal button (icon +
  label) next to the existing slider menu — one-tap, discoverable. Used by
  V1 and V3.
- **Toolbar menu item** (`prototype-v2-fleet-entry-menu.html` →
  `variant-2-fleet-entry-menu-390x844.png`): "Issues" as the middle item of
  the existing slider menu (with a read-only caption); zero new chrome.
  Used by V2.

### Variant 1 — repo groups · chip filter · push detail
(`variant-1-repos-list-390x844.png`, `variant-1-repos-detail-390x844.png`)
Board parity on iOS, closest analog of the egui Issues tab.
1. Filter = open/closed **chips** in the pinned chrome (same chip treatment
   as the fleet screen; "open" active in the evidence).
2. List = **repo-grouped pinned sections** with counts (`corral (5)`,
   `sendmeter (2)`, …) — identical structure to the fleet screen's repo
   groups; rows are `#N + title + OPEN/CLOSED pill` (the egui `#N title
   STATE` row), titles ellipsize.
3. Detail = **push** (‹ back, "Issue #232" nav title): OPEN + label pills,
   title, repo/opened meta, body, comments with the transcript-style
   `────  N earlier comments · Load earlier ────` divider (lazy paging),
   "older comments load on scroll · 20 per page" note, and a small read-only
   grant hint.

### Variant 2 — flat list · segmented filter · sheet detail
(`variant-2-flat-list-390x844.png`, `variant-2-flat-detail-390x844.png`)
Fewest taps to content; standard iOS idioms; densest.
1. Filter = **segmented control** (Open | Closed); the evidence shows the
   Closed bucket (closed rows dimmed, CLOSED pills gray) so both states are
   proven; open rows look like the V1 rows (OPEN pill teal, full opacity).
2. List = **flat**, newest-first, one repo line per row (no grouping), count
   subline (`closed issues (5) · newest first · read-only`).
3. Detail = **sheet** (grabber, rounded top, dimmed list behind) with the
   same content stack as V1 incl. lazy-paging divider and paging note.

### Variant 3 — flat list · chip filter · inline-expanding detail
(`variant-3-inline-list-390x844.png`)
The egui master/detail idea on one screen: selecting a row expands the full
detail **inline** under it (row gets selected `--panel2` backing; panel shows
OPEN/label pills, repo meta, body, `Load earlier` divider, one comment,
`▴ collapse`). Entry = toolbar button (same as V1). One screen, no push or
sheet; detail is immediately adjacent to the list rows.

## Evidence

- 7 PNGs rendered from the in-repo stage copies via the machine-cached
  chrome-headless-shell (2x render, downscaled to the 390x844 evidence
  standard); every PNG vision-inspected (full frame, no clipping, no scroll
  chrome, no mid-word chip cut). Exact commands + SHA-256s in `capture.log`.
- Mock data: issue lists use **real repo data** (fetched from
  `jirathip-dev/corral`, `-sendmeter`, `-plush-meadow` at prototype time);
  the #232 body is real; comment text and durations are illustrative mock
  copy (label it as such at review time).

## NOT captured here

- No SwiftUI implementation, no egui changes (impl is serialized: #232 +
  recorded Guy approval).
- No live-simulator screenshots (the impl lane owns that evidence, as in
  issue-245).
- No new capability name was invented in code: the read-only grant would
  follow the read_tail/read_diff pattern (e.g. a `read_issues` capability,
  default-empty) — to be settled between the prototype and the impl lane.
