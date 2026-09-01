# #316 V3 Context split — implementation evidence

Real compiled captures of the approved V3 design (issue comment 5472195052;
pinned archive 6e0d9a4ee60629b64609869baed711e328dba49f). Both artifacts are
synthetic demo data only — no daemon data, real agent ids, repo names,
hostnames, or user content.

## iOS — `v3-context-split-ios-390x844.png` (390x844)

- Source: compiled Debug `FleetNotifier.app` from THIS branch (simulator
  iPhone 16, UDID 59DDC0C5-891E-4EC0-91AF-4F50DF68D793), launched with the
  DEBUG-only demo detail route `-corralDemoDetail`
  (agent `herdr:demo-output` from `DemoFleet.seed()`).
- Captured with `xcrun simctl io <udid> screenshot`, resized
  `sips -z 844 390`.
- Shows the full V3 stacked order: **Session status** (structured values:
  `Working · live`, session/model/worktree chips), **Conversation** (user
  bubble, assistant code block, expanded tool diff with literal +/-/@@ and
  gutter numbers), collapsible **Harness activity · 2 outside conversation**,
  and the pinned composer (Reply to agent… / Send) at the bottom.

## egui — `v3-context-split-egui-1320x860.png` (1320x860)

- Source: compiled egui WASM build of THIS branch (`wasm-pack build
  --target web`), served locally, rendered by the machine-cached
  chrome-headless-shell (same engine as prior design-gate prototype renders),
  2x render downscaled to exactly 1320x860.
- Demo mode is driven by the bundled `clients/egui/assets/demo-fixture.json`
  (`recent_output_blocks` = canonical golden-shape blocks). The detail pane
  shows the V3 wide composition: **Conversation** column (tool disclosure
  with syntax-colored diff, line-numbered gutters), **Session utilities**
  column (Session status grid: State/Session/Tool/Worktree), collapsible
  **Harness activity · 3 outside conversation**, pinned composer below.

## Known capture constraint (egui artifact)

In the egui capture the assistant/tool text renders perfectly, but ONE user
bubble paints with degraded glyph rasterization (vertical letter fragments)
in this headless capture. This is a capture-path artifact, not a production
defect: the egui painted-galley geometry tests (`v3_session_status_and_composer_are_structurally_outside_the_conversation`,
`v3_narrow_detail_width_stacks_the_same_regions_deterministically`)
locate the full rendered galley words ("Conversation", "Harness activity",
"Reply to agent…", per-block text) as horizontal painted rects in the
output of the exact same `right_pane` code path, byte-identical across
`--disable-gpu`, swiftshader, and 2x capture attempts. A native
window-server capture (the `CORRAL_UI_SCREENSHOT` path used by #201/#207)
was not usable here because the only reachable daemon serves THIS host's
real agents/repositories — committing that frame would leak real data,
which the privacy contract forbids.

## SHA-256

```
9efbb625e9497dc7bab4b4492982bed4d7396c0c  v3-context-split-egui-1320x860.png
b6888d99b1c16a7f59cff89b84524f442f73183e  v3-context-split-ios-390x844.png
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
- Deviation: egui user-bubble glyph degradation in the headless capture
  (documented above; production code proven by executed painted-rect tests).
- iOS capture shows the conversation scrolled mid-history (demo seed window),
  so the user bubble sits above the fold; Load earlier, diff disclosure,
  Harness activity, and composer remain in frame — scroll position is
  user state, not layout.
