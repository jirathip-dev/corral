# Corral #442 V1 R2 — native composition and behavior handoff

**V1 R2 DIRECTION GATE — awaiting Guy approval**

V1 Pasture Panorama is the accepted information architecture. The current three R2 decision PNGs are frozen at **premium cozy-game quality**, not an AAA/painterly-production bar. This document supplies source-export contracts and proposed native implementation values; it does not approve R2, start implementation, or authorize another polish round.

SwiftUI with SpriteKit or Canvas will recreate the approved composition and behavior. Browser HTML/CSS/Python effects are reusable only where exported as PNG/SVG layers; all other procedural effects must be separately reimplemented in Swift.

## 1. Authority, provenance, and proof boundary

Read this alongside `SCOPE-CAP.md`, the supplied lane briefs (uncommitted), protected parent `README.md` / `PROVENANCE.md`, and R2 `build.py`, `control-input.html`, `fixtures.py`, `art.py`, and `README.md`. The scope cap supersedes further visual iteration and the original V2 recommendation. Working head supplied for this handoff: `8c7119701effe9d423d9e58bc993c93b0b54d79f`; accepted-control baseline remains `ecd3938a72cdfce256128c7d437dcd589141baf5`.

Evidence labels used below:

- **Retained contract:** accepted V1 semantics or explicit R2/scope-cap requirement.
- **Source-defined:** exact numbers read from the current generator/CSS; not a claim of native measurement.
- **Measured export data:** final `layers/geometry.json` and `layers/index.json`, produced and verified by the export lane. These are the authoritative pixel coordinates, bounds, transforms, layer inventory, and hashes. This document does not invent viewport rectangles or certify files that were still being generated when it was written.
- **Native proposal:** exact conservative behavior or adaptation values needed to make a later implementation brief unambiguous, but not implemented, measured, or approved by the static R2 images. Approval of still images must not silently approve these proposals.

Exactly three top-level decision images remain: `v1-r2-day-390x844.png` (390 × 844), `v1-r2-night-390x844.png` (390 × 844), and `v1-r2-idle-anatomy-study.png` (600 × 760). SVG layers and the source lineup are supporting assets, not new decisions. Do not rerun the old 21-frame matrix or generate variants.

The R2 stages are static. `build.py` suppresses leg/body/neck/heartbeat/flag CSS animation, and the stage control affordances do not gain new handlers. Their proof is composition, fixed fixture, pose artwork, and environment comparison—not roaming, real-time parallax, solar switching, native material rendering, haptics, audio, or native navigation. The parent evidence documents previous browser interaction probes for Board/Herd, shared filters, detail, state truthfulness, and Reduce Motion. Those are historical browser evidence, not re-executed here and not proof that R2/native behaviors work.

Art is original deterministic procedural illustration, including **analytic-depth painted pixels embedded in SVG**. It is not pure all-vector path art. Embedded raster surfaces remain editable through their retained generator, not by manipulating individual SVG paths. SVG gradients, filters, blend operations, and embedded images do not automatically import as native SwiftUI/SpriteKit effects. Preserve exported pixels or implement and validate equivalents in Swift. No Comfy Cloud, local ComfyUI, generation service, paid API, external art dependency, or borrowed mascots.

## 2. Native ownership and immutable product structure

**Retained contract:** Board is the authoritative dense blocked-first monitor; Herd is an optional visualization of exactly the same live model. Keep side-on V1, horizontal repository paddocks, physical blocked front rail, native Board/Herd switch, shared Filters, raw names and raw states, and the same native agent-detail/Recent Output sheet. Do not introduce V2 cards or a new filter, selection, output-fetching, or command model.

**Native proposal:** SwiftUI owns navigation, toolbar/HUD, scope/counts, accessible horse buttons, stable selection, filters, and sheets. SpriteKit or Canvas owns noninteractive world/sprite painting only. Use one read-only scene adapter from the existing authoritative snapshot. A scene node never independently polls, invents activity, changes an agent, or runs a command. Reuse existing `AppTheme.swift`, `StateStyle.swift`, `ThemeStore`, Board, filter components and `RecentOutputSheet`; extend their established tokens rather than styling the app to match HTML.

Native world and SwiftUI overlay must use the same measured transform. Decorative nodes cannot intercept touches. Stable agent identity keys must come from the model; art identity still derives from the exact raw agent name as specified below. Multiple presentations of a selection must resolve to one model object.

## 3. Export and coordinate contract

All final assets belong under `docs/design/evidence/issue-442/r2-v1-premium/` and its manifest. Resolve asset paths relative to that directory.

### Required separately addressable exports

For each of `day` and `night`:

- `layers/{day,night}/00-sky.svg`: sky; Night includes stars/Milky Way.
- `layers/{day,night}/10-hills.svg`: distant hills.
- `layers/{day,night}/20-ground.svg`: ground plane.
- `layers/{day,night}/30-barn-trees.svg`: barn and trees.
- `layers/{day,night}/40-rear-fences.svg`: rear fences.
- `layers/{day,night}/50-foreground.svg`: foreground grass.

Also `layers/front-rail.svg`; separately addressable identity sprite/pose exports for all identities used in Day/Night with standing, shift, graze and selected reduce-motion; separate grounding/contact-shadow and moon-rim exports; and square-safe identity selections plus editable lineup. Resolve exact sprite, grounding, rim, and lineup filenames through `layers/index.json`, not guessed filenames in implementation code.

**Source-defined local dimensions:** world master `viewBox="0 0 390 640"`; horse master `viewBox="0 0 148 112"`; physical front rail master `viewBox="0 0 390 44"`. The world currently uses `preserveAspectRatio="none"` and stretches into the pasture DOM rectangle; that rectangle is **not** the entire 390 × 844 viewport. Horse and rail placement must use their measured parent transforms, not a guessed world-to-screen offset. The final index records the actual exported dimensions/viewBoxes; do not replace these with screenshot dimensions.

### Required metadata for every layer

`layers/index.json` must inventory every expected export and supply, directly or by explicit linked records:

1. Exact relative path, semantic layer/identity/pose/environment, file format, width/height, SVG viewBox, and aspect-ratio policy.
2. Exact draw order/z-index **and parent stacking context**; local paint ordering for grounding, silhouette, rim and front rail; visibility by Day/Night/state/Reduce Motion.
3. Anchor/origin and painted bounds in named local coordinate units; facing/mirror pivot and transform; no ambiguous center-versus-hoof anchor.
4. Composition parent, measured source rectangle, transform into the relevant parent/world/viewport, and matching `layers/geometry.json` element key. Include offscreen paddock positions and scroll offsets, not only the first visible horses.
5. Parallax ratio, axis/coupling, and whether it is measured static behavior (zero) or a proposed native coefficient from section 8. A proposal is not a measured animation.
6. SHA-256 of the actual exported bytes; source generator/master and function or stable source identifier; deterministic parameters/seed/identity/state/pose; **exact executable reproduction command and working directory** for that export.
7. Square exports: safe bounds, original horse bounds, exact placement transform, and lineage to the identity source.

`layers/geometry.json` must name the reference capture viewport, output/device scale, scroll positions, pasture rectangle, rail rectangle, paddock/header/field rectangles, each horse button/sprite/nameplate/state-label rectangle, and source-to-composition transforms. Anchor and geometry records, not rounded screenshots or prose, are the authoritative measured pixel data. If a record is absent, mark the handoff incomplete rather than inventing a coordinate. This document prescribes the metadata content, not an unverified JSON key schema.

Preserve environment filename order within its world group. Composite separate grounding behind its horse and moon-rim over the matching silhouette; front-rail artwork occludes rail horses but never their labels. In current CSS the world is below scene content; rail horse art uses z-index 1, physical rail 2, horse label glass 2 within its isolated button context, text 3. HUD/sheets remain outside the world stack. The final export index must represent these parent contexts, not flatten equal z values blindly.

Do not composite both a full sprite containing its shadow/rim and the separated shadow/rim again. Index records must distinguish composite convenience exports from mutually composable parts. Supply isolation-compatible definitions/unique SVG IDs so multiple identical horses do not share conflicting clip/filter references.

### Reproduction and verification

Existing source reproduction from the R2 directory is `PYTHONDONTWRITEBYTECODE=1 python3 build.py`; capture is `PYTHONDONTWRITEBYTECODE=1 python3 capture.py`. Export command: `python3 -B export-layers.py` from this directory. Full gate command: `python3 -B run-gates.py`. The layer index records this exact export command for each asset. Run export regeneration into a disposable directory and compare bytes to frozen canonical exports; do not overwrite frozen decision images to establish equality.

The parent gate must reject missing/extra layer paths, wrong dimensions/viewBoxes, absent metadata, stale SHA-256, identity/pose omissions, mismatched layer composition, and nondeterministic rerenders. Compute expected names/counts from the explicit required environment set plus fixture identity/pose cross-product and square/grounding/rim/lineup inventory; log expected and observed totals independently. All referenced sources, index, geometry and exports must be manifest-covered. Independent manifest verification: `shasum -a 256 -c manifest.sha256`. Package checks also require exact three decision PNGs, protected-control equality and canonical/mirror raw path and byte equality. This handoff has not run or certified those parent gates.

## 4. Square-safe source handoff — not an app-icon change

**Required source export:** `layers/square/{identity}.svg`, each 256 × 256 with `viewBox="0 0 256 256"`, 24 px inset safe area. Place the original 148 × 112 horse using exactly `translate(24,49.2972972973) scale(208/148)` (encode the numerical scale as needed while retaining that exact expression in metadata). The local square origin is its top-left; safe-area bounds are x/y 24 through 232. Preserve the horse's local origin and record the transformed silhouette/accessory bounds in the index. Do not claim that the transform alone proves all painted bounds are inside the safe area: verify the actual SVG bounds, including mane, tail, hat/tack and filters.

Preserve editable per-identity procedural masters and an editable SVG selection/lineup with references or embedded independent copies of the same identity exports. Record each lineup cell's bounds/anchor and source identity in the index. Identity selection is a source browsing aid, not an approved icon candidate or an additional decision PNG. Keep coat, mane, breed, tack and accessory readable in the square. Do not replace, edit, generate an asset catalog for, or wire the current app icon. Future icon work requires a **separate issue, design gate, and Guy approval**.

## 5. HUD tokens, typography, spacing, labels and hit zones

### Source-defined reference values (CSS pixels, not native device measurements)

| Surface | Exact current values |
|---|---|
| Reference phone | 390 × 844; no browser outer frame |
| Chrome | padding top 52, horizontal 12, bottom 8; row minimum height 44; row gap 8 |
| Heading / controls | app title 22, weight 700; toolbar/segment 15, weight 600; gear 20 in 44 × 44 |
| Segments | minimum 64 × 44 each; horizontal padding 12; outside corner 12, selected corner 9 |
| Scope | 13; minimum height 20; top margin 6; gap 6; shared marker 11 mono, padding 2 × 6, corner 6 |
| Summary | minimum height 44; horizontal padding 6, vertical 8; gap 3; wrapping; count text 10 mono, padding 3 × 4, corner 7 |
| Pasture / front rail | pasture top padding 108; rail minimum height 208; rail padding top 30, horizontal 12, bottom 9; horse gap 10 |
| Rail horse / field horse | rail button width 164; field button width 160; sprite 148 × 112 in both |
| Horse button | padding top 0, horizontal 2, bottom 50; minimum 44 × 44; isolated paint context |
| Nameplate | 11 mono; line height 14; maximum width 156; centered at left 50%; bottom 19; no individual background/border |
| Raw-state label | 11 mono, weight 700; line height 16; centered at left 50%; bottom 1; inline cue gap 4 |
| Combined label glass | left/right inset 4; bottom -2; height 41; 1 px border; corner 9; text drawn above |
| Rail label | top 0, left 12; 11 system, weight 600; padding 4 × 8; corner 6; alert mark 16 × 16 with 13 mono glyph |
| Physical rail | parent-relative bottom 42, left 0, width 100%, height 44; no pointer events |
| Paddock | width 100% of page; padding top 14, horizontal 12, bottom 12 |
| Paddock header | minimum height 38, horizontal padding 7; gap 5; border 1; corner 9; title 15 weight 700, count 10 mono |
| Field | row gap 8, column gap 6; padding top 6, bottom 8; centered wrapping; vertical overflow scroll |
| Pagination | dot 8 × 8; active dot 20 × 8 corner 4; gap 6; top padding 8, bottom 10; hint 11, bottom padding 12 |

These are CSS declarations after R2 overrides, not inferred bounding boxes. Font metrics, flex sizing and border-box rules affect final dimensions; consult `layers/geometry.json` for actual rendered rectangles. The 38 px repository header is a label, not permission to make an interactive native header smaller than 44 pt. The evidence stamp is not shipping chrome.

**Source-defined R2 glass/art-reference colors:** chrome gradient 125° `#283143ee` → `#161e30f5`, bottom border `#ffffff24`; segment fill `#ffffff0c`, border `#ffffff25`, selected fill `#ffffff22`, selected text `#edf0fa`, inset highlight `#ffffff30`; summary `#202a38`; header `#202d3acc` / border `#ffffff24`; label glass 125° `#273240ec` → `#172630f5`, border `#c5cdd42e`, shadow offset (0,2), blur 5, `#19242924`; name ink `#f0ede5`, state ink `#e3e8ef`, blocked label ink `#f6b0b5`; rail-label background `#2b3445e8`, border `#c8bfb32d`, ink `#ffd1cc`. These are measured-source visual references; native materials require their own validation, not a claim of identical HTML blur.

**Retained operational tokens:** accent mauve. State → token: working teal, blocked red, idle subtext0, done green, unknown surface2 (unknown readable ink uses subtext1). Current Mocha values: accent `#cba6f7`; working `#94e2d5`; blocked `#f38ba8`; idle `#a6adc8`; done `#a6e3a1`; unknown `#585b70`; text `#cdd6f4`; subtext1 `#bac2de`; base `#1e1e2e`; mantle `#181825`; surface0 `#313244`; surface1 `#45475a`. Use native palette tables for other Catppuccin flavors, not these Mocha colors hardcoded globally. State chips use token mix 17% fill / 34% border over base; repo chips 15% / 38% over base; repo band 9% over mantle. Reuse existing half-even sRGB mix and `RepoHue` FNV-1a ring/collision ordering from `fixtures.py` and native theme, not a new palette algorithm.

**Native proposals:** use source pixel values as baseline point sizes at the 390-wide reference, not as a screenshot stretched to each device. Use actual system top/bottom safe-area insets; 52 px chrome padding is capture scaffolding, not a universal notch inset. Place controls below the native top safe area with 8 pt content inset; keep bottom navigation/hint 12 pt above bottom safe area. Decorative world may bleed under safe areas; controls and raw text may not. Use system font and SF Mono equivalents, Dynamic Type scaling; keep minimum 44 × 44 pt targets. Nameplates/state words stay native accessible text, not baked sprite pixels.

At accessibility text sizes use one horse per field row, vertically scroll the field, horizontally scroll rail overflow, wrap names instead of shrinking, and grow the label glass to fit both raw name and raw-state line with 4 pt top/bottom padding and 2 pt line gap. Baseline uses the source geometry above. Repository headers may truncate with full accessible name. Always expose the full raw agent name to VoiceOver, even if baseline V1 truncates visually. State and name text never move with cosmetic roaming. One horse accessibility element: “name, state, repo on host. Opens recent output.” Decorative layers are accessibility-hidden; rail and paddocks are named landmarks. Reduce Transparency proposal: replace glass gradients/material with opaque `#202937` behind labels and opaque existing native theme surface for chrome; verify contrast natively rather than claiming it from source hex values.

## 6. Identity and state mapping

**Retained identity algorithm:** SHA-256 of the exact raw name encoded UTF-8, no normalization, provider, harness, model, state, theme, host or environment input. Preserve array ordering:

- Coat = digest byte 0 modulo 8: bay, chestnut, black, grey, palomino, dun, roan, buckskin.
- Mane = (byte 1 shifted right 2) modulo 3: flowing, braided, cropped.
- Breed = (byte 2 shifted right 4) modulo 3: light, stock, draft.
- Tack = byte 3 modulo 3: saddle, pad, none.
- Accessory = byte 4 modulo 3: bandana, hat, none.

Use the exported R2 appearance, not old flat `COAT_HEX` fills to repaint dimensional surfaces. State changes pose/cues, never identity. Day/Night uses the same identity, fixture, composition and raw labels; first paddock remains `atlas-vector`. Fixtures are fictional, never fallback live data. Study identity is `willow-bend` (roan, draft, cropped, pad, none); its fourth specimen is deliberately regenerated standing, not frozen grazing. The study's legacy “red pad” wording is descriptive, not authority to recolor the current exported tack.

| Raw state | Required mark / word | R2 static source pose | Retained behavioral direction | Proposed native layer/behavior mapping |
|---|---|---|---|---|
| working | `○ working` | stand with state-specific art | restrained purposeful step, never theatrical | matching standing identity; a single restrained step cycle only after native rigging is approved; selected reduce-motion planted stand |
| blocked | `! blocked` | stand with blocked cue | alert at physical front rail | blocked standing identity plus static pennant; one small alert ear/head adjustment; Reduce Motion planted alert |
| idle | `◦ idle` | graze in Day/Night; standing/shift/graze/standing in study | calm standing, weight shift, occasional grazing | cycle matching standing → shift → standing → graze → standing; selected reduce-motion export is calm standing |
| done | `✓ done` | stand | settled, never working/active | settled standing; zero repeating activity or roam; Reduce Motion same |
| unknown | `? unknown` | stand with uncertainty treatment | cautious stillness and explicit uncertainty | still matching identity plus question cue; zero repeated activity; Reduce Motion same |
| disconnected | not a new raw agent state | not rendered by these R2 Day/Night images | explicit outage, last-known → unknown, no live activity | all affected horses static unknown, last-known context labeled; suppress every motion and heartbeat |

The raw working mark is `○`; the source working flag/summary uses the existing heartbeat treatment rather than always printing that glyph. Retain raw word and non-color cue; do not misrepresent the static R2 image as proving a literal circle is present everywhere. Native proposal uses the raw mark in accessible/state text while reusing the native working indicator.

Historical control timings are **not** R2 animation proof: CSS working leg alternation 0.62 s per leg direction, blocked paw 1.1 s, idle neck sway 4 s, blocked label pulse 1.4 s, and #371 heartbeat 1.2 s with peak at 42% and 160 ms square stagger. R2 deliberately disables these effects and renders heartbeat squares at opacity 0.85. Do not copy the old exaggerated 26° trot or 1.08× blocked label pulse into the frozen R2 direction.

## 7. Physical blocked front rail and exact motion proposals

**Retained/source-defined:** blocked horses are grouped at the physical front rail, not solely red chips or a V2 alert card. Rail geometry is parent-relative as in section 5 and measured in `layers/geometry.json`. Front rail covers the lower horse artwork without covering the raw name/state glass. Overflow is horizontal; never compress labels or drop blocked agents. Each blocked horse has a restrained red pennant: source local pole from `(140,79)` to `(140,105)`, stroke `#bda980`, width 1.3; pennant path `M140 79H148L145 83L148 87H140Z`, fill `#ae5b61`. These are **sprite-local source coordinates**, not viewport coordinates. No lantern is present in this source. Native default proposal: preserve pennant only, lantern disabled, no glow/pulse; a future lantern substitution needs approval rather than being added as “already approved.”

### Exact conservative native motion values — all proposals unless explicitly inherited

| Behavior | Native proposal |
|---|---|
| Idle schedule | deterministic 30 s loop: standing 0–12 s; shift 12–16 s; standing 16–24 s; graze 24–28 s; standing 28–30 s. Blend poses over 400 ms inside each new interval. No identity change. |
| Idle details | one 300 ms ear twitch at 8 s, one 800 ms tail sweep at 20 s, maximum 3° local rotation; no label movement. These require native part pivots, not invented exported rigging. |
| Working signature | one restrained 1.6 s step cycle; maximum 1 pt vertical body excursion, 6° limb rotation; no theatrical trot. Until a native pose rig is validated, use planted standing plus existing working indicator, not simulated leg animation from a flat composite. |
| Blocked signature | one 400 ms alert ear/head adjustment every 8 s, maximum 2°; zero translation, zero pawing, zero label/pennant pulse. Use static alert until articulated pivots are validated. |
| Done / unknown | zero repeating pose animation, zero roaming; done remains idle-equivalent for activity. |
| State transition | update raw word/mark/count immediately with snapshot; pose crossfade 200 ms only if motion allowed. Enter blocked rail immediately without an animated cross-paddock journey. |
| Existing working HUD indicator | reuse native #371 behavior: 1.2 s period, 42% peak, 160 ms stagger, 4 × 4 pt squares with 3 pt gap if that native component requires these source values; Reduce Motion uses existing 7 pt static dot. No new duplicate indicator loop. |
| Selection feedback | existing native selected treatment; source outline reference 2 pt mauve. No bounce/scale. |
| Hidden/background | stop all cosmetic clocks immediately; no catch-up animation on resume. State subscriptions remain the existing app's responsibility. |

Use monotonic elapsed time for cosmetic scheduling, never derive operational state from animation phase. Proposed deterministic phase offset for idle is first identity digest byte modulo 30 seconds; geometry/identity is unchanged. Clear selection/touch tracking pauses roaming immediately. No motion is required to understand state.

**Reduce Motion retained contract + native proposal:** effective Reduce Motion is system preference OR app reduce-motion preference. Select explicit exported static alternatives immediately; do not pause a mid-stride frame or crossfade while Reduce Motion is enabled. Idle → selected calm standing; working → planted standing and static dot; blocked → planted alert with static pennant; done → settled stand; unknown → still uncertain. Set pose loops, ear/tail movement, parallax, roaming, ambient drift and pulse to zero. If only composite static exports exist, keep them static rather than claiming an articulation rig was exported. Disconnected forces these same zero-motion rules regardless of preferences.

### Bounded roaming proposal

Stable tappable zones are the measured horse button/slot rectangles in `layers/geometry.json`, not dynamic sprite bounds. Only idle/working non-rail horses may cosmetically translate. Their allowed local delta is x in `[-4,+4]` pt, y = 0; further intersect that range with the parent paddock bounds and the unchanged slot safe inset of 4 pt around the painted sprite bounds. If the intersection is empty or neighboring slots would overlap, translation is exactly zero. Never move into another repository, through a fence, over another label, or outside the stable hit zone. Blocked/done/unknown/disconnected translation is zero.

Proposed roam trajectory: `deltaX = A * sin(2π * elapsed / 24 s + phase)`, with A no greater than 4 pt after geometric clamping; phase derives from identity, not random runtime values. It is disabled by default until native hit-zone verification; the numeric recipe is a bounded optional implementation target. Pause on pointer-down, selection, detail/filter sheet, scrolling/dragging, VoiceOver focus, Reduce Motion, and source outage. Return to the slot anchor over 200 ms only if motion is allowed; otherwise snap. Horse labels and their accessible/tap targets remain stationary throughout.

## 8. Parallax and gesture coupling

**Retained direction:** horizontal swipe/scroll drives layers; foreground fastest, midground slower, distant landscape slowest. No gyroscope requirement, autonomous cinematic camera, or newly introduced navigation model. Static R2 has **zero implemented parallax**; the following exact coefficients are native proposals, not measured proof.

Let `s` be horizontal repository scroll offset in logical points, positive toward later pages. Place layer decorations at their indexed base transform plus `x = -ratio * s`, clamped to available artwork coverage; vertical parallax is zero. If a layer is already inside the translating pager, apply only the correction needed to produce that absolute transform—never double-scroll. All horse hit targets/labels share the same paging transform as their sprites.

| Layer/group | Proposed ratio |
|---|---:|
| Sky, stars, Milky Way | 0.05 |
| Distant hills | 0.12 |
| Ground plane | 0.40 |
| Barn/trees | 0.22 |
| Rear fences | 0.40 |
| Repository horse slots, grounding, rims, labels and interactive paddock content | 1.00 |
| Decorative foreground grass | 0.65 |
| Global blocked front rail, blocked horse slots and HUD | 0.00 to repository paging |

The global blocked rail is deliberately an exception to decorative foreground speed: it must stay visible and tappable while browsing repositories. Its **own** horizontal overflow scroll moves its horses/labels/physical rail together at 1.00 and does not drive background parallax. World foreground grass supplies the faster depth cue. Keep the index explicit about this distinction; do not apply 0.65 to the global alert region.

Use native horizontal paging/inertia directly, with zero additional camera lag and no synthetic page-duration constant. Vertical field scroll moves that field's horse slots/labels at 1.00 with zero world/sky coupling; do not drag the global rail vertically with it. Pager dots reflect the authoritative current page; filters removing that page choose the first remaining repository without changing model selection arbitrarily.

A 390-wide master is not automatically a seamless panorama: clamp translations to covered pixels; if coverage is insufficient, use zero differential movement for that layer until native clipping/tiling is validated. Do not stretch silhouettes or synthesize new scenery to hide seams. Ambient drift default is **off (0 pt/s)**. Optional future background-only drift proposal: maximum ±1 pt horizontally, 60 s sinusoidal period, no camera/horse/label movement; off under Reduce Motion, outage, sheets or background. Numeric ratios are implementation proposals requiring live swipe evidence.

## 9. Day / Night / Auto and lighting

**Retained:** environment selection is independent of Catppuccin theme. Changing Day/Night cannot alter operational tokens, state mapping, horse identity, filters, selection or detail state. Only clear Day and Night are exported; weather and seasons are deferred. Native proposal stores environment enum `day | night | auto` separately, default `auto`; expose these through existing native Settings with minimum 44 pt option rows, not a second fantasy HUD. The evidence Day/Night stamp is not a working selector.

**Source-defined lighting:** Day sky gradient `#388fc4` → `#e5e8c9`; Night `#121a32` → `#667991`. Day ground gradient `#bcc17b` → `#8e9c58` → `#485e3b`; Night `#63746a` → `#415d55` → `#273f3e`. Keep indexed exported sky/stars/Milky Way/haze/grain surfaces rather than a generic replacement gradient. Night horse CSS applies brightness 0.86 and saturation 0.82 to the sprite composition; moon rim uses `#d3dfeb`, widths 1 and 1.05, opacities 0.68 and 0.8. Night rail source drop-shadow is `(0,-1)` with blur 0 and `#c4d6e0a0`. Night cast-shadow transform is sprite-local `translate(135px,0) scale(-1,1)`, origin `(0,0)`. Exact shadow shape/contact anchor comes from indexed layers, not a generic oval.

When shadows/rims are split, preserve the source composition/filter order. Index whether light treatment is baked or must be applied; never apply Night darkening or mirrored shadow twice. Native rendering of CSS brightness/saturation, SVG filters or raster color spaces requires separate pixel/visual validation. Stable identity does not mean identical screen RGB under different lighting.

**Retained solar/fallback rule:** with existing device-location permission and valid solar calculation use local sunrise/sunset. Never require or automatically request location permission for Herd. If permission or valid calculation is absent, Day is device-local `[07:00,19:00)`, Night otherwise. Explicit Day/Night always overrides Auto. Reevaluate on local day/timezone changes.

**Native exact proposals for unspecified solar edges:** at sunrise use Day, at sunset use Night; accept an already available authorized cached location no older than 24 h and horizontal accuracy 10 km or better. Do not start continuous tracking; do not upload/store a location solely for scenery. Invalid, unavailable, denied/restricted, expired, polar no-rise/no-set or calculation-error results use the fixed fallback. Recompute on foreground entry, permission change, significant-time/timezone change, local midnight and the next computed transition; no per-frame or minute polling. If a permitted location is absent, fall back rather than presenting a prompt. Manual selection cancels the pending solar transition. Environment change crossfade proposal 600 ms only when visible and motion allowed; Reduce Motion/outage transition is immediate. None of permission handling, solar timing, scheduling or transitions is implemented by the HTML.

## 10. Ambience, haptics, outage and shared navigation

**Retained:** silent by default; ambience/haptics opt-in only. Native proposals: `ranchAmbienceEnabled=false`, `herdHapticsEnabled=false`; no autoplay, state alarms, horse voices or background audio. No audio asset is supplied. If a separate approved source later exists, foreground ambience maximum gain 0.10, fade in/out 1 s, obey silent mode/interruptions and stop on app background or source outage. Until then the option must not pretend an audio feature is available. Optional haptic is one existing native light selection feedback on deliberate horse selection, minimum interval 500 ms, never on snapshot updates, blocked counts, retries, page motion or animation. Disabled means zero haptic calls. Device behavior requires physical-device proof.

**Retained shared invariants:**

- One host/repository filter selection scopes Board and Herd, rail, paddocks, counts and sheets consistently. The same active-filter count/scope text appears in either mode. No independent scene filter.
- Board/Herd switching preserves model selection and filters; selecting a horse opens the same read-only Recent Output sheet as the corresponding Board row, keyed by the same agent identity, including existing state/repo/host chips and block-per-run tail with role markers.
- Blocked agents remain discoverable at the rail; count each model agent once. Rail is a presentation of scoped blocked agents, not a second source. Paddock count semantics follow existing shared model; do not count rail copies as additional agents.
- State words are exactly `working`, `blocked`, `idle`, `done`, `unknown`; no renaming into game jobs or species. Board stays blocked-first, then working, idle/done, unknown; done never counts active.
- A snapshot update changes both modes atomically. Animation cannot defer raw status/count updates. Native proposal: if a selected agent disappears, dismiss unavailable detail and clear selection using existing native lifecycle semantics; never retarget the sheet to the next horse.
- On disconnection retain labeled last-known context as unknown, remove live indicators, and show explicit `Disconnected` / `Source disconnected`, host/reason and last snapshot revision/age with `Open Board` and `Retry`. Rail must say `unknown · cannot be confirmed`, never assert “no blocked” from missing data. No animated fixture substitution. The original disconnected control disables horse interaction; retain that conservative behavior until a separately approved cached-detail policy exists.
- Empty filtered scope is a truthful empty state with access to Filters/Board, not sample horses; native proposal copy `No agents in this scope`, no empty-state animation. Recovery only adopts authoritative snapshots, never interpolates operational states.

## 11. Later native acceptance and gate stop

A later implementation issue must validate actual native source/model wiring, not only match PNGs. Require: deterministic identity/state tests; current shared filter/selection/detail regressions; blocked rail placement/overflow and non-color cues; disconnected/empty/recovery behavior; Dynamic Type, minimum targets, VoiceOver, safe areas, Reduce Motion/Transparency; exact layer inventory/geometry import and absence of duplicate grounding/rim; native simulator composition against the frozen reference at the reference size; gesture/roam bounds and stable touch targets; solar/manual/fallback boundary tests with a controllable clock; foreground/background suppression. Physical-device checks are required for native scrolling feel, accessibility, material legibility, opt-in haptics/audio and location permission behavior. No such native verification is claimed here.

**Open behavior decisions resolved only as proposals:** native material/safe-area adaptation, environment default/settings placement, timing and articulation, bounded roaming enablement, exact parallax ratios/coverage strategy, optional drift, solar location freshness/accuracy and polar fallback, transition fades, selection-disappearance/empty-state details, and optional audio/haptic implementation. Conservative defaults keep unproven motion/ambience off or static; the current pennant is retained and lantern absent. These require the later native implementation gate, not another R2 PNG polish round.

Parent delivery owns export generation and metadata validation, frozen-image/reproducibility checks, protected-control proof, manifest, mirror, docs-only commit/push, one issue comment beginning `V1 R2 DIRECTION GATE — awaiting Guy approval`, exact readback and uncommitted `.report.md`. This document neither certifies those external actions nor performs them. No production Swift/Rust, product wiring, app-icon change, PR, merge, integration/main, TestFlight, deploy or release is authorized. Stop at the human direction gate.
