# #316 V3 Context split — implementation evidence (R3)

Real compiled captures of the approved V3 design (issue comment 5472195052;
pinned archive 6e0d9a4ee60629b64609869baed711e328dba49f). Both artifacts are
synthetic demo data only — no daemon data, real agent ids, repo names,
hostnames, provider/harness names, or user content.

R3 regeneration note: both PNGs were regenerated on this branch from real
compiled surfaces using only the bundled synthetic fixture. The egui capture
now uses the demo-only deterministic capture seam (`CORRAL_UI_DEMO_SEED=1`)
added for R3, which seeds the exact bundled fixture into the NATIVE compiled
`corrald-ui` and reuses the existing validated screenshot pipeline
(`CORRAL_UI_SCREENSHOT`) — no live daemon, no real host data, readable
native-wgpu glyphs. The previous WASM/headless-WebGL route (which produced
the one-character-per-line user-bubble corruption) is no longer used for
evidence.

## iOS — `v3-context-split-ios-390x844.png` (390x844)

- Source: compiled Debug `FleetNotifier.app` from THIS branch (simulator
  iPhone 16, UDID 59DDC0C5-891E-4EC0-91AF-4F50DF68D793), launched with the
  DEBUG-only demo detail route `-corralDemoDetail`
  (agent `demo-session:recent-output` from `DemoFleet.seed()`).
- Captured with `xcrun simctl io <udid> screenshot`, resized
  `sips -z 844 390`.
- Shows the full V3 stacked order: **Session status** (structured values:
  `Working · live`, session `demo-session:recent-output`, tool `tool-one`;
  Worktree omitted because the demo session has no worktree — Omit
  unavailable values rather than inventing), **Conversation** (user bubble
  "Please verify the diff too.", assistant code block, expanded tool diff
  with literal +/-/@@ and gutter numbers), collapsible **Harness activity ·
  2 outside conversation**, and the pinned composer (Reply to agent… / Send)
  at the bottom.

## egui — `v3-context-split-egui-1320x860.png` (1320x860)

- Source: compiled native `corrald-ui` (release) from THIS branch, window
  1320x860 logical, with `CORRAL_UI_DEMO_SEED=1` seeding the bundled
  `clients/egui/assets/demo-fixture.json` (canonical golden-shape blocks),
  captured through the existing `CORRAL_UI_SCREENSHOT` viewport pipeline,
  then downscaled 2x → exactly 1320x860.
- Demo values are neutral synthetic labels: repos `demo-alpha` /
  `demo-bravo` / `demo-charlie` / `demo-delta`, tool pills `tool-one` /
  `tool-two`, session `demo:p01:impl`.
- Detail pane shows the V3 wide composition: **Conversation** column
  (numbered tool disclosure: `$ python3 verify_demo.py --sample`,
  `git diff -- src/board_view.rs`, `@@ -1,2 +1,3 @@`, `-old label`,
  `+speaker rail`, `+compact tool run`, `run status: ok`; user bubble
  "ship the canonical transcript stream"; assistant "the fictional snapshot
  is consistent ✓"), **Session utilities** column (Session status grid:
  State `Working`, Session `demo:p01:impl`, Tool `tool-one`; Model/Effort/
  Worktree omitted — the fixture carries no metadata line), collapsible
  **Harness activity · 2 outside conversation**, pinned composer
  (Reply to agent… / Send) below. No setup overlay; stick-to-bottom: on.

## Visible-text audit (R3 semantic audit)

Every identifier class listed below was checked in BOTH final PNGs with a
deterministic Vision-framework OCR pass over the committed 1320x860 / 390x844
artifacts; the class list below is exhaustive of what the evidence contract
forbids:

- Provider / harness names (`claude`, `opencode`, `codex`, …) — ABSENT.
- Repo names (previous synthetic `atlas-board`, `orbit-console`,
  `pixel-garden`, `route-lab`, `crystal-garden`, `ledger-lantern`,
  `signal-grove`, `paper-orchard`) — ABSENT; neutral `demo-*` labels only.
- Branch / worktree names and worktree paths — ABSENT; the demo sessions
  carry no worktree, so Session-status Worktree is omitted rather than
  synthesized.
- Hostnames / user paths — ABSENT (only neutral `/demo/…` strings appear in
  fixture sources, none in the captured frames).
- Real agent / session identifiers — ABSENT; session chips render
  `demo-session:recent-output` (iOS) and `demo:p01:impl` (egui).
- Credentials / tokens — ABSENT (fixture/seed carry none).

OCR text (both artifacts, `Vision` `VNRecognizeTextRequest`, accurate, en-US)
is recorded in `capture.log`. The privacy scanner
(`scripts/check-demo-privacy.py`) was NOT weakened — its approved allowlists
were re-targeted to the new neutral fixture values and its self-test still
rejects every identity mutation and live title.

## SHA-256

```
8734db6fca21eb159d864560b95abfc546a4c1f5c15a9bcf31ed61a3e7b1448a  v3-context-split-egui-1320x860.png
fec586acc2db831e8df051485487e79b2e699de723b82db959be93afba31cab9  v3-context-split-ios-390x844.png
```

## Self-review vs pinned V3 references

Compared against the local verified read-only copies of the approved design
archive (`variant-3-context-split-egui-1320x860.png`,
`variant-3-context-split-ios-390x844.png`, same pinned directory as the
brief's approved files):

- Region ORDER matches the locked V3 composition on both platforms
  (egui: Conversation + utility column with Session status over Harness
  activity, composer below; iOS: Session status → Conversation → Harness
  activity → composer).
- Semantic labels match the locked naming exactly: `Session status`,
  `Conversation`, `Harness activity · N outside conversation`, `Diagnostic` /
  `Unknown activity` identities (harness rows), `You said…` / `Assistant` /
  `Tool` accessibility roles (tested, not all visible at transitions).
- No new hues, no bubbles/gradients/shadow slop beyond the existing #205
  user-tint inset bubble; tokens and 4/8/12 rhythm reused.
- No known deviation: R3 fixed the egui user-bubble glyph degradation at the
  source (the RTL infinite-height allocation collapsed the wrap width; the
  block now allocates a finite desired size with a left-to-right rail +
  content layout) and the wide conversation scroll now fills the pane.
  Both committed frames show readable user/assistant/expanded-diff content.
- iOS capture shows the conversation scrolled mid-history (demo seed window),
  so the user bubble sits above the fold; Load earlier, diff disclosure,
  Harness activity, and composer remain in frame — scroll position is
  user state, not layout.
