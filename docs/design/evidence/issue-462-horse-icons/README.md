# Corral #462 — selectable horse app icons: design-gate prototype

**Status: PROTOTYPE — awaiting Guy icon-art approval.**
Nothing in this bundle changes the shipping app icon, the asset catalog, or any
app source. The shipping `AppIcon.appiconset` bytes are referenced byte-exact
and untouched. Issue #455 V1/A approval does **not** approve these icon
compositions; #463 (packaging) and #464 (Settings picker) remain blocked on a
NEW explicit Guy icon-art approval of one treatment + set.

## Decision framing

iOS 17 alternate app icons require pre-bundled opaque 1024x1024 masters with
square, unrounded corners (the system applies its own mask). This gate asks
one question: **which composition should the horse icon set use?**

- **Treatment A — large head-and-neck portrait** (recommended). The canonical
  neck+head group fills the frame; the eye, blaze, muzzle and mane read at
  every phone-icon size.
- **Treatment B — tightly framed full horse.** The whole canonical figure
  (including ground shadow) is visible, but at Home Screen sizes the canonical
  thin legs and hooves approach the legibility floor and the face — the
  identity carrier — is ~2.5x smaller than in A.

## The five choices (identical list in both treatments)

| Choice | Master | Meaning |
| --- | --- | --- |
| Original | `masters/treatment-{a,b}/original-1024.png` | The current unchanged app icon, **copied byte-exact** from `ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/AppIcon-512@2x.png` (sha256 `e2c754cf3dd7cbc8f10090597360eb56c56fb4672856d48407453dc8190e15e7`). Never redrawn. |
| Bay | `masters/treatment-{a,b}/bay-1024.png` | Canonical `COAT["bay"]` `#8a5a33/#6f4527`, mane `#2e2019` |
| Palomino | `masters/treatment-{a,b}/palomino-1024.png` | Canonical `COAT["palomino"]` `#c89a56/#a37a3c`, mane `#e8dcc0`, canonical blaze (`BLAZE_COATS`) |
| Black | `masters/treatment-{a,b}/black-1024.png` | Canonical `COAT["black"]` `#3b3b44/#2a2a31`, mane `#17161a` |
| Grey | `masters/treatment-{a,b}/grey-1024.png` | Canonical `COAT["grey"]` `#b9bcc6/#989ca9`, mane `#7d8089` |

No extra coats, editor, animation, download path, runtime generation, or
automatic mode/theme switching are proposed. Exactly these five, everywhere.

## Recommendation: Treatment A

1. **Small-size recognition.** The canonical eye is r=1.7 units. In A's window
   (side 64.43 units) that is ~54 px on the 1024 master (~6.4 px on a 120 px
   preview); in B's window (side 108.66) ~32 px (~3.8 px at 120 px). A keeps
   the face — the recognition anchor of this art — more than twice as large.
2. **Canonical geometry costs land outside A's frame.** The generator's legs
   are thin rounded rects (5 units wide at breed "light") and hooves are
   read as detached dots in B previews; the muzzle/eye/mane groups are the
   strong shapes. A composes exactly those; B spends 60% of its pixels on the
   barrel and fragile extremities.
3. **Coat differentiation survives better in A.** The distinguishing per-coat
   cues (palomino blaze + cream mane, grey tone, black tone) are all
   head-zone features; at 60 px, B previews for bay/black/grey converge on
   "brown-ish horse shape" while A keeps distinct silhouettes.
4. **The Original's own grammar supports it.** The byte-exact Original is a
   full-scene composition; A is the treatment that most closely matches its
   subject scale (single subject, head prominent) without redrawing it.

B remains fully exported and equally reviewable — this is Guy's choice, not a
foregone conclusion.

## Provenance (no redesign, generator untouched)

- Art source of record: `ios/tools/herd-art/horsesvg.py`
  (sha256 `a9c172de9aa42080daf987ca6919f827895c363903c16e1ec3fbdf865b6041ec`,
  matching `ios/tools/herd-art/provenance.json` "byte-identical to original
  V1"), imported **unmodified** by `tools/gen_icons.py`. No horse art is
  authored in this bundle; the renderer in `tools/svgmini.py` only interprets
  the exact SVG primitives the canonical generator emits (same parse contract
  as the shipping `ios/tools/herd-art/export.py`).
- Identity: static-stand pose (`state="stand"`, reduce-motion equivalent),
  breed `light`, mane `flowing`, no tack/accessory — one fixed identity per
  coat, mirrored with the generator's own `facing=-1` transform so proposals
  face left, matching the Original icon's orientation.
- A/B crops are **viewBox windows in canonical 132x100 coordinates**, derived
  from parsed geometry with fixed multipliers, identical for every coat
  (asserted at build time):
  - **A** = square window centered on the neck+head group bbox
    (5.49, 3.03)–(54.98, 52.59), side = bbox max-dim × 1.30 = **64.43**,
    center (30.24, 27.81). ≥7.4 units clear margin around every neck-group
    vertex; barrel intentionally exits bottom/right (portrait crop).
  - **B** = square window centered on the full-figure bbox incl. ground
    shadow (5.49, 3.03)–(108.00, 101.00), side = bbox max-dim × 1.06 =
    **108.66**, center (56.75, 52.01). ≥3.1 units margin on every side —
    nothing clipped.
- Backgrounds: opaque ranch tones, **luminance-opposed to the coat**
  (light coats on dark grounds, dark coats on light grounds), identical per
  coat across A and B so treatment is the only variable:

| Coat | Background | bg vs body | bg vs shade | bg vs mane | bg vs muzzle | bg vs hoof |
| --- | --- | --- | --- | --- | --- | --- |
| Bay | hay `#ecdcb4` | 4.31 | 6.05 | 11.57 | 1.90 | 9.13 |
| Palomino | night pasture `#1f2a38` | 5.68 | 3.74 | 10.67 | 1.89 | 2.55 |
| Black | sand `#d9c39a` | 6.45 | 8.29 | 10.48 | 1.50 | 7.22 |
| Grey | deep pine `#22302a` | 7.26 | 5.03 | 3.49 | 2.81 | 1.72 |

  WCAG contrast ratios vs the coat's fill/shade/muzzle/hoof inks. Lowest
  meaningful pair is grey mane vs deep pine (3.49) — tone-on-tone definition,
  acceptable because the coat body carries contrast (7.26). Pink muzzle on
  light grounds (black 1.50, bay 1.90) is a soft accent by canonical palette;
  the black nostril dot inside it preserves the read. First-draft backgrounds
  (`#5f7484` range blue, `#5d564e` barnwood) failed the same table
  (1.90/1.72) and were replaced before review.

## Small-size / crop / contrast analysis

- Previews: `previews/mask{180,120}-treatment-{a,b}-<coat>.png` (rounded-rect
  mask approximating the iOS silhouette — **preview-only**, masters stay
  square/unrounded) and `previews/plain060-…` (60 px spotlight size).
- Vision-inspected results (all 30 previews + all 10 masters + all 8 phone
  contexts): every coat recognizable at 120 px in both treatments; in B the
  canonical thin legs nearly vanish by 60 px (silhouette still reads); in A
  the face reads at every exported size.
- Crop safety: window bounds ⊇ anatomy bbox with positive margin in both
  treatments (numbers above); A's bottom/right crop of the barrel is the
  intended portrait composition, applied identically to all four coats.
- Labels: none baked into any master (asserted: no `text` primitives); names
  appear only as accessible text in the picker mockup.

## Home Screen + Settings contexts (renders, not screenshots)

`contexts/{home,picker}-{a,b}-{light,dark}-390x844.png` — 390x844 mock Home
Screens (five labeled choices on light/dark gradient wallpapers, dock, page
dots, ≥44 px tap equivalents in the gallery HTML) and Settings → Appearance →
App Icon picker mockups (name + subtitle per row, checkmark on Original,
Original-recovery copy). Sources: `html/stage/*.html`; rendered with
chrome-headless-shell at 2x, downscaled to exactly 390x844 (`file(1)` gate).
Icons in-context are the exported masters with CSS preview masks — no
packaged assets exist.

Known, accepted context observations (not defects):
- The **Original** on a dark wallpaper is low-contrast by nature — it is the
  unchanged shipping icon (black background), shown byte-exact as required.
- In B, the canonical thin legs dissolve at the smallest in-context sizes —
  this is the legibility cost that recommendation A weighs.

## Reproduction

```bash
cd <worktree>/docs/design/evidence/issue-462-horse-icons
python3 -B tools/gen_icons.py            # regenerates everything byte-for-byte
python3 -B tools/verify_bundle.py        # structural + determinism gate
```

Deterministic: stdlib + Pillow only, no network, no timestamps, fixed
supersampling (2x) and LANCZOS downscale. Re-running replaces outputs
byte-identically (proven in `verification.json` `determinism`).

`index.html` (bundle root) is the tappable review gallery (treatment A/B
toggle, five tappable choices with checkmark state, small-size strip,
light/dark context shots, dark preview-background toggle). It is a
**prototype**; it changes no real icon. Stage sources for the rendered
contexts live under `html/stage/`.

## Limitations

- Preview masks are 22.5% rounded rects, not Apple's exact squircle.
- In-context icons are CSS-masked masters, not packaged `AppIcon`-style
  assets (packaging is #463, blocked on approval).
- The renderer's flattening (24-segment Bézier, 2x supersampling) can differ
  sub-pixel from Core Graphics; masters here are proposal art, not shipping
  assets. #463 regenerates approved masters through the deterministic tool
  chain before packaging.
- Canonical geometry quirks (single-ear profile read, mane pinch at the ear
  base, thin legs) are inherited from the source generator and are visible in
  these proposals; any art change belongs to #448/#459 lanes, not this gate.
- Wallpapers, dock placeholders and all fixtures are illustrative mock data.

## Verification

See `verification.json` (counts, dimensions, opacity, contrast samples,
determinism, browser checks, reachability) and `SHA256SUMS` (every retained
artifact except itself). Logs with raw exit codes: `/tmp/corral-462-generate.log`,
`/tmp/corral-462-verify.log`, `/tmp/corral-462-browser.log`.
