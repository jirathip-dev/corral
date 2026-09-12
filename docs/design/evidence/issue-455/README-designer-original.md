# Corral #455 — immersive Herd prototype

PROTOTYPE — awaiting Guy approval. Local conductor-verification package; not an implementation handoff or a cleared visual gate.

## Open

- `index.html` — phone comparison gallery, functional A/B links, stills and playable real-time clips.
- `variant-a.html?preview=herd&env=day` — compact single-row scope/status/Settings HUD.
- `variant-b.html?preview=herd&env=day` — two-level scope and explicit health/count cluster.
- Existing served gallery: https://jirathips-macbook-air.tail8c3301.ts.net:8444/corral/455-immersive-herd/index.html

The existing localhost and Tailscale HTTPS endpoints returned HTTP 200 with exact local-byte identity for the gallery, both variants, the recommended Day PNG and both videos. See `evidence/reachability.json`. This is a check from the Mac, not a physical-phone connection test. No Tailscale wrapper, installation, Serve or shared networking configuration was changed.

Requested local root: `/Users/jirathip/design-output/corral/455-immersive-herd/`. The host resolves the design-output symlink to `/Users/jirathip/Projects/design-output/`; these are the same physical artifact, not two copies.

## Recommendation: B

Surface archetype: **Monitor**. B places readable host health and all five counts immediately beneath scope. Scope and Settings remain on the first floating row; Day/Night/Auto is secondary. It does not require tapping a compressed summary to discover idle, done or unknown.

A retains the same ranch composition and controls, but trades the full status cluster for a compact summary (`blocked`, `working`, `total`; tap for all five states and host health). The saved space remains open sky rather than moving the rail horses into the sky. A is better for atmosphere; B is better for monitoring and outage recognition. This is a hierarchy/density comparison, not a palette swap.

Both maintain the accepted panorama. The top Board/Herd switch is absent. Open Board stays reachable at the bottom and in the outage explanation.

### Visual system

- Ranch Day: warm translucent backing `rgba(246,245,225,.94)`, ink `#23362f`, secondary `#3e5147`.
- Ranch Night: dark backing `rgba(29,43,51,.95)`, ink `#edf1e8`, secondary `#c2cec9`.
- Board and general Settings use the four exact Catppuccin source palettes; mauve remains the app-selection accent. Ranch lighting does not rewrite app flavor.
- System-font hierarchy: scope 14/13, status 12, paddock 15, horse name/state 11, host 10 CSS px at the reference size. Text scales to 145% in the accessibility-equivalent proof; the rail becomes one full-width label per scroll page rather than showing a clipped half-label.
- Controls have at least 44×44 CSS-pixel targets. Insets reserve 54px top / 24px bottom minimum plus device safe-area environment values. Status bar, island and home indicator are explicitly simulated browser chrome.
- Glass is requested by the owner, not a generic decoration. Opaque/Reduce Transparency fallback is present; native Liquid Glass availability and fallback behavior remain unverified.

## Locked artwork and provenance

Read-only production source: commit `86d1287a9e5c611ec6bef677c95544c53e71b982` via `git show`, not the stale checkout. Snapshots of HerdView, RanchEnvironment, HerdModel, HerdArt, FleetViews, AppTheme and AppModel are under `references/source/`. Issue #455 including its dispatch comment and #456–460 are retained under `references/issue-*.json`.

- Environment: accepted #442 R2 `art.py:world`, with its original barn, hills, trees, paint and palette. Extend only background edge coverage and add motion wrappers/seeded guard-band stars. The new full-screen viewBox is `0 -110 390 844`, uniformly scaled/cropped, never aspect-stretched.
- Horses: **original flat V1** `horsesvg.py`, not the R2 sculpted replacement horses. Original SHA-derived coats/breeds/manes/tack/accessories and barrel/leg construction are retained. The idle neck/head uses the corrected coordinates from pinned `HorseDraft.grazing()`. This is a faithful browser adaptation, not native-renderer pixel parity.
- No gait redesign, horse roaming experiment or repository activity-ordering work. Working/blocked poses use the original planted static options; the only motion in this slice is the environment. Idle has corrected grazing; Reduce Motion uses an intentional standing specimen.
- Six separately exported environment planes per lighting mode: sky, hills, ground, barn/trees, rear fences, foreground. Individual motion groups and horse SVGs are also exported under `assets/`.
- `evidence/layer-verification.json` proves actual-browser, same-origin pixel equality between the intact resting Day/Night worlds and their six reassembled plane exports.
- The accepted R2 browser illustration contains an embedded procedural paint texture in the ground SVG. It is retained for visual fidelity, **not a proposed native asset path**. There is no flattened scene raster or video background in the interactive prototype. Native shipping stays procedural SwiftUI Canvas/Path with distinct layers; no WebView, SVG decoder, new renderer dependency or video asset is proposed.

Old #442 README recommendation/status text is historical. This dispatch's accepted combination—V1 panorama + R2 environment + original V1 horses + corrected grazing—is authoritative for this slice.

## Fictional data and interactions

`fixtures.json` is one deterministic, entirely fictional fixture set: Meadow / Orchard hosts, four invented repositories and twelve named horses. Base counts are 2 blocked, 4 working, 3 idle, 2 done, 1 unknown. Dense expands the same fixtures deterministically to 30, retaining composite host/name IDs; the board, rail, paddock and filter counts all derive from that one set. There are no real host endpoints, users, secrets, telemetry or terminal output in the interactive artifact.

Every variant supports:

1. Day / Night / Auto. Auto is explicitly a deterministic demo Day (or `auto=night` query), not location permission or solar computation.
2. Shared host/repository filters, per-scope clear, full reset, selected checkmarks and an explicitly scrollable sheet. Selection applies immediately; no redundant Apply action.
3. Settings Board/Herd save-and-apply simulation; four independent Catppuccin app flavors; Reduce Motion.
4. Global front rail across the current scope, paged repository paddocks, horizontal rail overflow and vertical field overflow.
5. Horse or Board-row → **SYNTHETIC PLACEHOLDER** Recent Output. No host is contacted and no fabricated terminal response appears.
6. Normal, dense, offline, connecting, key-mismatch and empty review fixtures via the bottom Prototype button. Offline/connecting/key-mismatch turn retained agents into unknown, explicitly preserve last-known blocked membership, disable output and pause the scene. Retry explicitly reports that nothing was fetched; it does not manufacture reconnection.
7. Low Power, serious-thermal, background and opaque-material review controls. These are labeled simulations, not operating-system integration.

### Proposed edge defaults — awaiting approval

- No saved mode → Board.
- Unknown stored value → Board.
- Settings selection saves and applies immediately, including an explicit re-selection after recovery.
- Open Board is a temporary recovery override; it does not rewrite saved preference.
- Foregrounding alone does not reverse recovery.
- Cold reload restores saved preference.

The namespace is `corral455.prototype.savedMode`; only the mode string and a separate prototype theme string are persisted, not hosts/agents/output. A and B share these prototype preferences. Gallery `preview=herd` opens a temporary first-navigation preview; a reload deliberately ignores it and tests actual saved-mode restoration. Environment and filters are session-only in this browser simulation; persistence of native environment settings is outside this proof.

## Motion contract and evidence

One requestAnimationFrame owner samples a shared deterministic phase, capped to about 30 paint updates per second. Fleet projection is not recomputed in the animation loop. Only the visible environment's cached node groups are transformed.

- Shared wind: 9.6-second sine cycle; canopy rotation up to approximately 4.4°, with small stable per-tree phase variation. Trunks and horses never transform.
- Grass bands: shared wind with small stable phase offsets, up to approximately 4.9° skew about each band's ground origin. These are grouped geometry updates, not per-leaf timers.
- Clouds: bounded, slow displacement (`50 × sin(t/65)` reference pixels; later clouds slightly slower), with no wrapping seam or randomly regenerated cloud identity.
- Star field and Milky Way: coherent slow position change (`70 × sin(t/140)`, `16 × sin(t/180)`), fixed moon, guard-band duplicates of the same seeded edge stars. No shooting stars, flashing, weather, particles or audio. This browser proof emphasizes position rather than reproducing the native twinkle exactly.
- Reduce Motion resolves to phase zero and standing idle horses. Sheet/Board/background/source-loss/empty/Low Power/thermal states cancel the pending animation callback. Resume retains elapsed phase and avoids a time catch-up jump.

`day-realtime.mp4` and `night-realtime.mp4` are actual 390×844 browser captures with wall-clock timestamps. No time compression, interpolation or synthesized frames. Their 30 fps video container holds the timestamped browser captures; effective capture cadence is lower and is reported in `motion-verification.json`. This is **not** a smoothness or battery-safety claim.

The rendered phase gate observes changed canopy/sky bounding geometry and changed viewport pixels. Actual start/end frame differences are also retained. Static side-by-side visual review found the ambience subtle; final phone-scale perceptibility should be judged from the playable real-time clips by Guy, not inferred from a difference image or a two-frame model review. Physical-device motion approval is still open.

## Evidence / acceptance matrix

| Scope | Browser evidence in this package | Still-open native/approval boundary |
|---|---|---|
| #455 two hierarchy variants and recommendation | `index.html`; `comparison-contact.png`; A/B Day/Night | Guy selects A/B; final specification not approved |
| #456 immersive shell, safe controls, rail and bottom paddocks | A/B Day/Night, small 360×740, large 430×932, 145% type and rail-end PNGs; geometry and real pointer-navigation gates | Actual iOS safe areas, system chrome, VoiceOver, scroll/parallax physics, physical iPhone proof |
| #457 ranch-context controls and shared sheet | A/B filter Day/Night + scrolled-end PNGs; reset/host/repo click gates; all four app flavors; rendered contrast | Native material fidelity, OS availability, Reduce Transparency/high-contrast integration, no styling leakage in actual app |
| #458 saved mode and temporary recovery | A/B Settings + scrolled end; immediate Board/Herd selection, no/unknown preference, cold reload and background-return gates | Real UserDefaults migration, setup/demo preservation, SSE/subscription invariants, actual app relaunch |
| #459 wind, canopy, grass and shared lifecycle | real-time Day clip; rendered changed geometry/pixels; system Reduced Motion emulation and browser background/lifecycle tests | Actual Canvas sampling/cadence, invalidation boundaries, native Low Power/thermal, CPU and battery measurements |
| #460 slow sky using the same clock | real-time Night clip, stable star/galaxy transforms, cloud/sky group exports | Native continuous coverage/parallax transitions, named build/device CPU/frame evidence, phone visual approval |
| Truthful edges and output | A/B offline/connecting/key-mismatch/empty/dense PNGs; output placeholder PNGs; disabled-output + non-fake-retry gates | No live data/network/security-route behavior is proved here |
| Reproducible package and provenance | `tools/`, SHA manifest, exact pinned source, browser verification and plane reassembly | No copy to the Corral product repository; no archive promotion/implementation approval |
| #448 gait / #449 ordering | intentionally untouched | Both remain separate and UNROUTED |

All child references above are **design coverage, not child completion**. #456–460 and #448/#449 remain UNROUTED. Fleet remains paused; no process controlling it was spawned or resumed.

## Verification results and honest limits

The authoritative detailed result is `evidence/verification.json` (PASS), not a source-string assertion. The primary suite passes 329 checks and captures 42 labeled cases; `evidence/workflow-verification.json` adds 56 shared-scope/keyboard checks and two Board PNGs. `evidence/art-geometry.json` records the final painted-horse bounds. Final visual adjudication and the ten-point slop audit are in `evidence/visual-review.md`. It records real loaded pages, browser version, exceptions/console errors, DOM geometry, actual-pointer interactions, preference reloads, system Reduce Motion emulation, actual hidden/frozen lifecycle, and rendered-pixel contrast. `evidence/layer-verification.json` independently verifies layer composition.

- No console/runtime errors in the measured cases.
- No document horizontal overflow or in-container label clipping in the measured phone/large-text cases; all measured interactive targets meet 44×44 CSS px.
- Minimum sampled contrast behind the actual text is **4.788:1**. Sampling temporarily masks glyph paint, keeps the real backing/blur, and tests the actual text bounds—not unrelated rounded-border corners.
- Scroll folds are not text truncation. Scrolled-end filter, Settings and large-text rail proofs explicitly show the last row, complete proposed-default paragraph and complete second rail label.
- Representative B-Day fixed-phase PNG captures are byte-identical in the same renderer. Full generated HTML/SVG source determinism is checked separately. Cross-platform font/raster parity and deterministic wall-clock movie bytes are not claimed.
- Background restore in headless Chromium requires focus emulation after actual frozen/hidden state; the harness verifies the document's visibility before treating it as resumed. This is a browser-harness detail, not an iOS lifecycle implementation.
- Only a 145% CSS text equivalent is provided—not iOS accessibility-size/VoiceOver certification. No physical iPhone, SwiftUI/Canvas render, native build, CI, CPU, frame-rate or thermal/battery result was produced.
- The Board is a functional synthetic recovery/list context, not a full port of every native Board row feature. Recent Output is deliberately a placeholder.

## Reproduce

Requirements already present on this host: Python 3 with Pillow and websocket-client, the cached Playwright Chromium headless shell (`chromium_headless_shell-1234`), FFmpeg/ffprobe, and the existing read-only gallery server on localhost:8777. No package install or shared browser daemon is needed. The owned Chrome profile lives temporarily under this package's `tools/.runtime/` and is removed by the closeout tool. Do not run simultaneous captures if matching cadence matters.

From `/Users/jirathip/design-output/corral/455-immersive-herd/`, `python3 -B tools/run-all.py` runs the full tested chain and seals it. The individual steps are:

```sh
python3 -B tools/build.py
python3 -B tools/verify-layers.py
python3 -B tools/verify-browser.py
python3 -B tools/verify-workflows.py
python3 -B tools/verify-art-geometry.py
python3 -B tools/record-motion.py
python3 -B tools/contact-sheets.py
python3 -B tools/inspect-motion.py
python3 -B tools/check-reachability.py
python3 -B tools/package.py --repro --seal
python3 -B tools/package.py --verify
```

`tools/gather.py` records read-only provenance and was run once at entry; do not rerun it to overwrite the baseline during closeout. The reproduction sequence consumes the retained source snapshots, not mutable product working-tree files. Movie timings are wall-clock evidence and naturally vary on rerun. Manifest excludes only itself and lists every other final file, including the brief, reference snapshots, scripts, reports and proofs; no runtime cache is silently excluded.

## Scope / delivery boundary

This run writes only inside the named prototype bundle. Other dirty/untracked design-output projects and the product checkout are preserved. No product changes, repository commit, push, PR, issue write, implementation route, profile/config/credential change, fleet resume or release was performed. This is the local PROTOTYPE checkpoint for conductor verification; archive promotion is not included in this run.

Next decision: Guy reviews B versus A and the real-time ambience, then explicitly approves the selected variant and proposed edge semantics. This package itself does not clear the gate.
