# #270 egui Issues tab — per-repo browser (side repo picker + inline body) (design gate)

Status: **PROTOTYPE ONLY — no Rust (egui/daemon) source changes made in this lane.**
This bundle presents three 1320x860 board-frame PNGs of the reworked egui
Issues tab and records the design decisions so Guy can pick one on the issue.
Implementation is serialized: it waits for a recorded `APPROVED: <variant>`
on #270 AND for #269 (Fleets-tab removal lane, same crate `clients/egui`) to
land — see #270's serialization note.

## Scope recap (from #270)

The Issues tab renders ALL repos' issues at once (75+ grouped) and issue
bodies require the web/GitHub. Direction (Guy 2026-08-28): per selected
repo with the repo picker on the side, and expand each issue row to read
its body inline, egui board. Read-only; no new write endpoints; no change
to iOS or daemon write paths.

## Baseline (current app)

- `clients/egui/src/ui/issues.rs` — `show()` renders: heading `Issues (N)` +
  muted sublabel `all issues grouped by repository`; `toolbar()` = selectable
  labels `all/open/closed` + `TextEdit` hint `search title or #number` +
  `↻ refresh`; then a `ScrollArea` of `CollapsingHeader`s — one per
  `display_repo` (`repo  (shown)`) — with rows built by `issue_row()` as
  `row_label = format!("#{}  {}  {}", number, title, state)` (monospace
  `selectable_label`). Selecting a row indents an inline detail (state badge,
  label badges, issue URL, and the `start worktree` action / not-startable
  note). **No issue body anywhere** — title/labels/state/url only.
- `clients/egui/src/model.rs` `GhIssueRef` (mirrors
  `crates/corrald-client/src/model.rs` + `src/core/events.rs`): `repo,
  number, state, title, labels, url`. **No `body` field** — the snapshot
  legitimately lacks bodies (the gh poller's `closingIssuesReferences` join
  carries no body either).
- `src/api/issues.rs` `GET /issues` — last-known `repo -> [GhIssueRef]` map
  from `IssuesCache` (written ONLY by the gh plane event handler; the GET is
  a deliberate non-auth read surface; never mutates GitHub). Repo keys =
  fleet identity names + live `workspace.repo` category union — the same
  category source as today's grouping.
- `src/adapters/gh_plane.rs` — one aliased GraphQL query per poll (WS2),
  read-only (D-083), no mutations; currently fetches issue list metadata but
  not bodies on demand.
- `clients/egui/src/app.rs` — `TAB_LABELS`: Board / Issues / Fleets /
  Settings; `tab_strip()` underscores the active tab with a 2px ACCENT line.
  **The mockups render the post-#269 set: Board / Issues / Settings (Fleets
  gone), per #270's direction.**
- `clients/egui/src/main.rs` — native window `with_inner_size([1320.0,
  860.0])`: the evidence standard here is the real board frame, 1320x860.

## Variants (all in repo design tokens — no new design language)

Design system: `contracts/state-tokens.json` + the approved
`docs/design/corral-ux-prototype.html` (bg `#0d1117`, panels
`#10151c/#161b22/#1c2128`, line `#30363d`, ink `#e6edf3`, muted `#8b949e`,
accent `#2dd4bf`) + `clients/egui/src/theme.rs` (selection `#1f3a3d` /
stroke `#148f84` — the mockups' selected controls use exactly these, not new
hexes). Issue-state vocabulary, carried from the #267 evidence bundle: OPEN =
teal accent (the system's positive accent), CLOSED = muted gray with closed
rows dimmed (same treatment as idle/done agent rows). Label pills use GitHub
label colors (data — `gh label list` output — not a language).

### Variant 1 — side rail repo picker · inline-detail list (the direction)
(`variant-v1-side-rail-1320x860.png`, `prototype-v1-side-rail.html`)

1. Strip under the tabs: left rail `repositories` — one item per `display_repo`
   category (the same keys `GET /issues` returns as today's grouping;
   `corral (7)`, `sendmeter (17)`, `project-hearthwild (28)`, `dotfiles (1)` —
   open counts at fetch time) with the selected repo highlighted via the
   theme selection treatment; rail footer notes the daemon snapshot source.
2. Main pane: `Issues (7)` + sublabel `corral · open · per-repo browser`;
   toolbar `open | closed | all` + search + refresh retained **per repo**
   (open filter active). Rows stay board-parity `#N  title  STATE`
   monospace (search/label pills do NOT crowd row titles; labels live in the
   detail only, per #267).
3. Selecting a row expands the detail **inline** under it (selected-row
   backing, `▴` collapse): state pill + label pills, mono meta
   (`corral · #270 · opened 2026-08-28`), the issue body (title/labels/state
   + body), the read-only note, and today's actions (`start worktree`,
   capability-gated) unchanged. The #270 expansion shows the real body.

### Variant 2 — repo combo picker · closed-bucket proof
(`variant-v2-combo-picker-1320x860.png`, `prototype-v2-combo-picker.html`)

1. Same tab strip; no rail. The repo picker is a **combo** in the toolbar
   (`corral ▾ · 7 open · 5 closed`) — one-row pick, list space wider than V1.
2. Subnote makes the per-repo containment explicit: "repo picked from the
   dropdown — filter + search stay inside the picked repo (today: all repos'
   issues in one long list)" (prose illustrative; the claim is the point).
3. The evidence shows the **Closed** bucket on `corral` with a `grant`
   search: `#257`, `#256`, `#250` closed rows (dimmed titles, CLOSED), `#250`
   expanded inline — CLOSED pill + enhancement pill + body + the existing
   `closed issue — not startable (the daemon refuses too)` action note. Proves
   closed rendering AND that filter + search survive per-repo in the combo
   layout.

### Variant 3 — expanded-row close-up
(`variant-v3-expanded-closeup-1320x860.png`, `prototype-v3-expanded-closeup.html`)

The V1 expansion at close-up scale (single pane, no rail): full title/body
for `#270` rendered verbatim from the fetched GitHub body (Why / Scope /
DESIGN GATE / Acceptance criteria / Serialization), mono meta, OPEN +
enhancement pills, read-only note, `start worktree`, `▴ collapse`. This is
the artifact that shows what a user actually reads when the row expands.

## Evidence

| Label | File | Dimensions |
|---|---|---|
| V1 · side repo rail | `variant-v1-side-rail-1320x860.png` | 1320x860 |
| V2 · repo combo picker | `variant-v2-combo-picker-1320x860.png` | 1320x860 |
| V3 · expanded-issue close-up | `variant-v3-expanded-closeup-1320x860.png` | 1320x860 |

- All three PNGs are exactly 1320x860 (the native egui window size —
  `main.rs with_inner_size`), rendered from the in-repo stage copies via the
  machine-cached chrome-headless-shell (2x capture, downscale), and every
  frame vision-inspected (full frame, no clipping, no mid-word wraps —
  `project-hearthwild` fits its rail item, the read-only note stays one
  line). Exact commands + SHA-256s in `capture.log`.
- Mock data is REAL data: issue titles/numbers/states/labels fetched from
  `jirathip-dev/corral`, `sendmeter/sendmeter`, `jirathip-k/project-hearthwild`,
  `jirathip-k/dotfiles` at 2026-08-28 (open counts: 7 / 17 / 28 / 1); the
  `#270` and `#250` bodies are verbatim GitHub bodies (`gh issue view`).
  Illustrative: rendered wrapping/spacing and the one-line subnote prose —
  flagged in the frames by the ⓘ read-only note where relevant.

## Body-bearing read path (OUTLINE — decision for the impl lane, no code here)

`GhIssueRef` carries no `body` (snapshot + `/issues` today), so expansion needs
a read-only body fetch. Proposed (matching repo conventions, **no mutation**):

- Daemon: extend `IssuesCache` with a `GET /issues/<repo>/<number>` (or
  `.../body`) handler that fetches the single issue via the gh plane's
  existing read-only GraphQL transport (`gh_plane.rs`, D-083), caches the
  result, and serves it — same non-auth read surface / loopback-only caveat
  as `/issues`; never a GitHub mutation.
- egui client: on row expansion, request the body for the selected issue only
  (not all issues, no polling), cache it in `Fleet`, render "loading…" /
  failure states; existing `start worktree` action path unchanged.

## NOT captured here

- No Rust/egui implementation (impl is serialized: #269 + recorded Guy
  approval).
- No live-app screenshots (as in issue-245/#267, the impl lane owns shipped
  surface evidence; the #267 close-up style is the mockup-language only).
- No comment `APPROVED:` markers; approval is Guy's, recorded on the issue.
