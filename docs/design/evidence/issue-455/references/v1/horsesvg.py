#!/usr/bin/env python3
"""issue-442 — original horse sprite generator (vector, deterministic).

ALL horse art in this lane is authored HERE, from scratch, as original flat
vector work. No borrowed mascots, kitchen assets, or provider logos. The
natural coat/accessory colors are FIXED real-world horse colors (deliberately
NOT flavor tokens) so identity is stable across Mocha/Latte; the environment
(sky, grass, fences, chips) is what theming flips. Provenance: 100% authored
by this generator; see PROVENANCE.md.

Identity axes (deterministic from sha256(agent_name), fixtures.py):
coat x breed silhouette x mane x tack x accessory.
State changes POSE only — never species or identity bits.

Geometry notes (v2 rebuild): side-profile silhouette, facing right, ground
at y=96 in a 132x100 viewBox. Level topline (withers 36 / back dip 41 /
croup 37.5), rounded hindquarter, neck+sloped elongated head as its own
group pivoting at (90, 46) so idle grazing rotates the neck down (~80 deg)
and blocked raises it (~-16 deg) without moving the barrel.
"""
from __future__ import annotations

# Fixed natural-art palette (identity axis; palette-independent by design)
INK = "#2f2a26"            # outline / deep shade on any coat
HOOF = "#3a332e"
MUC = "#c9958b"            # muzzle / inner ear
EYE = "#1d1a17"
BLAZE = "#efe7dc"
TACK_LEATHER = "#7a4f2c"
TACK_LEATHER_D = "#5d3a1f"
BANDANA = "#c2543f"
HAT_STRAW = "#d8b46a"
HAT_BAND = "#8a5a33"
QUESTION_BG = "#efe7dc"

BREED = {  # leg width, leg length, belly y (deeper = stockier)
    "light": {"leg_w": 5.0, "leg_h": 34.0, "belly": 66.0},
    "stock": {"leg_w": 6.2, "leg_h": 31.0, "belly": 69.0},
    "draft": {"leg_w": 7.4, "leg_h": 28.0, "belly": 72.0},
}

COAT = {"bay": ("#8a5a33", "#6f4527"), "chestnut": ("#a05c2c", "#7d4520"),
        "black": ("#3b3b44", "#2a2a31"), "grey": ("#b9bcc6", "#989ca9"),
        "palomino": ("#c89a56", "#a37a3c"), "dun": ("#b08d5e", "#8e6f45"),
        "roan": ("#96685a", "#77524a"), "buckskin": ("#bd8f4e", "#96703a")}
MANE_C = {"bay": "#2e2019", "chestnut": "#5d3317", "black": "#17161a",
          "grey": "#7d8089", "palomino": "#e8dcc0", "dun": "#41321f",
          "roan": "#4a342c", "buckskin": "#241a10"}
BLAZE_COATS = {"palomino", "dun", "chestnut", "buckskin"}


def _leg(x, y, w, h, angle, coat, cls):
    """One leg: outer translate to the hip, inner rotate driven by the CSS
    custom property --a (so CSS animation can swing it without losing the
    translate). transform-box:fill-box + origin top-center = the hip."""
    return (f'<g transform="translate({x:.1f} {y:.1f})">'
            f'<g class="leg {cls}" style="--a:{angle:.0f}deg">'
            f'<rect x="{-w/2:.1f}" y="0" width="{w:.1f}" height="{h:.1f}" '
            f'rx="{w/2:.1f}" fill="{coat}"/>'
            f'<rect x="{-w/2:.1f}" y="{h-5:.1f}" width="{w:.1f}" '
            f'height="5" rx="2.5" fill="{HOOF}"/></g></g>')


def horse_svg(identity: dict, state: str, reduce_motion: bool = False,
              facing: int = 1, desaturated: bool = False) -> str:
    """Inline SVG string of one horse. viewBox 0 0 132 100, ground y=96.

    facing: 1 = faces right, -1 = faces left (mirror).
    Poses by state; `reduce_motion` swaps every motion-bearing pose for its
    static equivalent (working -> settled stand; blocked keeps head high but
    plants the hoof; pulse is the CALLER's CSS class, removed under RM).
    """
    coat = identity["coat"]
    breed = identity["breed"]
    mane = identity["mane"]
    tack = identity["tack"]
    acc = identity["accessory"]
    body, dark = COAT[coat]
    mane_c = MANE_C[coat]
    b = BREED[breed]
    belly = b["belly"]
    leg_w, leg_h = b["leg_w"], b["leg_h"]
    leg_top = belly - 6
    flip = (f' transform="scale({facing} 1) '
            f'translate({-132 if facing < 0 else 0} 0)"')
    desat_attr = ' filter="url(#desat)"' if desaturated else ""
    rm_attr = " horse-rm" if reduce_motion else ""

    # ---- pose parameters (state language; never identity) ----
    neck_angle = 0.0        # neck+head group rotation about (90,46)
    body_tilt, bob = 0.0, 0.0
    tail = "mid"
    legs = (0.0, 0.0, 0.0, 0.0)   # near-front, far-front, near-hind, far-hind
    if state == "working" and not reduce_motion:
        body_tilt, bob = -4, -1.5
        legs = (28, -26, -26, 24)                  # trot, extended
        neck_angle = -8
        tail = "up"
    elif state == "idle":
        neck_angle = 80                            # grazing, muzzle to grass
        tail = "mid"
    elif state == "blocked":
        neck_angle = -16                           # head high at the rail
        legs = (-26, 0, 0, 0) if not reduce_motion else (0, 0, 0, 0)
        tail = "up"
    elif state == "done":
        neck_angle = 12                            # relaxed, settling
        tail = "mid"
    elif state == "unknown":
        legs = (6, -6, -6, 6)                      # uneasy stance
        neck_angle = 4
        tail = "mid"
    else:  # reduce-motion static stand (working RM + fallback)
        neck_angle = -6
        tail = "mid"

    g_open = (f'<g class="horse horse-{state}{rm_attr}"'
              f' data-state="{state}"'
              f' data-coat="{coat}" data-breed="{breed}" data-mane="{mane}"'
              f' data-tack="{tack}" data-accessory="{acc}"'
              f'{desat_attr}>')

    parts = [g_open]
    parts.append(f'<ellipse cx="64" cy="97" rx="44" ry="4" fill="{INK}" '
                 f'opacity="0.14"/>')
    parts.append(f'<g{flip}>')
    inner = bob or body_tilt
    if inner:
        parts.append(f'<g class="body-g" transform="translate(0 {bob}) '
                     f'rotate({body_tilt} 66 52)">')

    # far-side legs (darker)
    parts.append(_leg(78, leg_top, leg_w * .88, leg_h, legs[1], dark, 'leg-ff'))
    parts.append(_leg(51, leg_top, leg_w * .88, leg_h, legs[3], dark, 'leg-fh'))

    # tail from the dock (top of hindquarter)
    if tail == "up":
        parts.append(f'<path d="M 40 41 Q 30 35 26 21" stroke="{mane_c}" '
                     f'stroke-width="6.5" fill="none" '
                     f'stroke-linecap="round"/>')
    else:
        parts.append(f'<path d="M 39 43 Q 28 53 27 69" stroke="{mane_c}" '
                     f'stroke-width="6.5" fill="none" '
                     f'stroke-linecap="round"/>')

    # neck + head group (drawn first so the barrel overlaps its base).
    # Grazing geometry is authored in final position — no group rotation.
    neck_rot = ("" if neck_angle >= 60 else
                f' transform="rotate({neck_angle:.0f} 90 46)"')
    parts.append(f'<g class="neck"{neck_rot}>')
    if neck_angle >= 60:
        # dedicated grazing geometry: smooth S-curved neck sweeping well
        # forward, muzzle at grass height ahead of the front legs.
        parts.append(f'<path d="M 74 40 Q 86 42 92 52 Q 97 62 100 70 '
                     f'Q 102 76 108 80 Q 115 84 122 83 Q 127 82 127 79 '
                     f'Q 127 75 121 75 Q 112 75 106 68 Q 104 58 98 49 '
                     f'Q 91 40 78 36 Z" fill="{body}"/>')
        parts.append(f'<path d="M 103 73 Q 107 77 113 79 Q 107 82 102 78 '
                     f'Z" fill="{dark}" opacity="0.3"/>')
        parts.append(f'<path d="M 120 75 Q 127 76 127 79 Q 127 82 122 83 '
                     f'Q 117 84 113 81 Q 115 76 120 75 Z" fill="{MUC}"/>')
        parts.append(f'<circle cx="122.5" cy="79" r="1" fill="{INK}"/>')
        parts.append(f'<path d="M 102 70 L 100 64 L 106 67 Z" '
                     f'fill="{body}"/>')
        parts.append(f'<circle cx="104" cy="69" r="1.6" fill="{EYE}"/>')
        if coat in BLAZE_COATS:
            parts.append(f'<path d="M 110 77 Q 114 79 119 80 L 118 82 '
                         f'Q 113 81 109 79 Z" fill="{BLAZE}"/>')
        if acc == "hat":
            parts.append(f'<ellipse cx="105" cy="62" rx="11.5" ry="3.8" '
                         f'fill="{HAT_STRAW}"/>')
            parts.append(f'<path d="M 99.5 62 a 5.5 5.5 0 0 1 11 0 z" '
                         f'fill="{HAT_STRAW}" stroke="{HAT_BAND}" '
                         f'stroke-width="1.3"/>')
        if mane == "flowing":
            parts.append(f'<path d="M 80 36 Q 92 42 97 54 Q 100 63 98 70 '
                         f'Q 91 64 88 54 Q 85 46 76 40 Z" fill="{mane_c}"/>')
        elif mane == "braided":
            for bx, by in [(89, 44), (94, 52), (98, 61)]:
                parts.append(f'<circle cx="{bx}" cy="{by}" r="3.4" '
                             f'fill="{mane_c}"/>')
        else:  # cropped
            parts.append(f'<path d="M 76 36 Q 90 44 96 55 L 92 57 '
                         f'Q 86 47 74 41 Z" fill="{mane_c}"/>')
        if acc == "bandana":
            parts.append(f'<path d="M 76 36 L 90 42 L 83 48 Z" '
                         f'fill="{BANDANA}"/>')
        parts.append('</g>')
    else:
        parts.append(f'<path d="M 80 40 C 92 30 102 22 110 15 L 118 20 '
                     f'Q 124 23 127 30 Q 129 34 127 36 Q 124 39 120 38 '
                     f'Q 112 36 106 31 Q 100 40 96 50 L 84 52 Z" '
                     f'fill="{body}"/>')
        # cheek shade
        parts.append(f'<path d="M 104 28 Q 110 32 116 34 Q 108 38 101 33 '
                     f'Z" fill="{dark}" opacity="0.3"/>')
        # muzzle
        parts.append(f'<path d="M 122 27 Q 128 31 127 36 Q 124 39 120 38 '
                     f'Q 120 32 122 27 Z" fill="{MUC}"/>')
        parts.append(f'<circle cx="124.5" cy="32.5" r="1.1" fill="{INK}"/>')
        # ear
        parts.append(f'<path d="M 109 14 L 111 5 L 116 11 Z" '
                     f'fill="{body}"/>')
        parts.append(f'<path d="M 111 11 L 112 8 L 114 10 Z" '
                     f'fill="{MUC}"/>')
        # eye
        parts.append(f'<circle cx="116" cy="22.5" r="1.7" fill="{EYE}"/>')
        # blaze (light coats only)
        if coat in BLAZE_COATS:
            parts.append(f'<path d="M 119 21 Q 123 25 125 30 L 123 31 '
                         f'Q 120 26 117 22 Z" fill="{BLAZE}"/>')
        # hat accessory
        if acc == "hat":
            parts.append(f'<ellipse cx="115" cy="11" rx="12.5" ry="4" '
                         f'fill="{HAT_STRAW}"/>')
            parts.append(f'<path d="M 109 11 a 6 6 0 0 1 12 0 z" '
                         f'fill="{HAT_STRAW}" stroke="{HAT_BAND}" '
                         f'stroke-width="1.4"/>')
        # mane (identity axis) along the crest, behind the crest line
        if mane == "flowing":
            parts.append(f'<path d="M 111 13 Q 101 16 95 24 Q 90 31 84 35 '
                         f'L 78 36 Q 87 28 93 19 Q 100 10 111 13 Z" '
                         f'fill="{mane_c}"/>')
            parts.append(f'<path d="M 111 13 Q 116 15 118 20 Q 113 22 110 18'
                         f' Z" fill="{mane_c}"/>')
        elif mane == "braided":
            for i, (bx, by) in enumerate([(91, 29), (98, 23), (105, 17)]):
                parts.append(f'<circle cx="{bx}" cy="{by}" r="3.4" '
                             f'fill="{mane_c}"/>')
        else:  # cropped
            parts.append(f'<path d="M 82 37 Q 95 26 108 15 L 111 18 '
                         f'Q 98 28 86 39 Z" fill="{mane_c}"/>')
        if acc == "bandana":
            parts.append(f'<path d="M 84 38 L 97 40 L 90 49 Z" '
                         f'fill="{BANDANA}"/>')
        parts.append('</g>')  # neck

    # barrel: level topline, dip mid-back, rounded hindquarter
    parts.append(f'<path d="M 40 40 Q 47 35.5 60 38.5 Q 72 41 80 36 '
                 f'Q 87 33 90 42 Q 92 48 92 54 Q 92 {belly-6:.0f} '
                 f'88 {belly-2:.0f} Q 66 {belly+2:.0f} 44 {belly-4:.0f} '
                 f'Q 31 {belly-9:.0f} 30 54 Q 30 46 34 42 Q 37 39.5 40 40 '
                 f'Z" fill="{body}"/>')
    # hindquarter + shoulder shading
    parts.append(f'<path d="M 34 42 Q 30 46 30 54 Q 31 {belly-9:.0f} '
                 f'44 {belly-4:.0f} Q 40 {belly-5:.0f} 38 46 Q 37 42 38 40 '
                 f'Z" fill="{dark}" opacity="0.32"/>')
    parts.append(f'<path d="M 80 36 Q 87 33 90 42 Q 91 47 91 52 '
                 f'Q 85 50 82 44 Q 80 40 80 36 Z" fill="{dark}" '
                 f'opacity="0.22"/>')

    # tack on the back
    if tack == "saddle":
        parts.append(f'<path d="M 50 37 Q 60 33 70 36 L 71 {belly-22:.0f} '
                     f'Q 61 {belly-17:.0f} 51 {belly-21:.0f} Z" '
                     f'fill="{TACK_LEATHER}"/>')
        parts.append(f'<path d="M 60 {belly-19:.0f} Q 60 {belly-6:.0f} '
                     f'64 {belly-3:.0f}" stroke="{TACK_LEATHER_D}" '
                     f'stroke-width="1.8" fill="none"/>')
    elif tack == "pad":
        parts.append(f'<rect x="52" y="{belly-30:.0f}" width="20" height="9" '
                     f'rx="3" fill="{BANDANA}"/>')

    # near-side legs
    parts.append(_leg(84, leg_top, leg_w, leg_h, legs[0], body, 'leg-nf'))
    parts.append(_leg(43, leg_top, leg_w, leg_h, legs[2], body, 'leg-nh'))
    if breed == "draft":   # feathered hooves
        parts.append(f'<rect x="{84-leg_w/2-1:.1f}" y="{leg_top+leg_h-8:.1f}"'
                     f' width="{leg_w+2:.1f}" height="6" rx="3" '
                     f'fill="{mane_c}"/>')
        parts.append(f'<rect x="{43-leg_w/2-1:.1f}" y="{leg_top+leg_h-8:.1f}"'
                     f' width="{leg_w+2:.1f}" height="6" rx="3" '
                     f'fill="{mane_c}"/>')

    if inner:
        parts.append('</g>')
    parts.append('</g>')

    # unknown mark (outside the flip so glyphs never mirror)
    if state == "unknown":
        parts.append(f'<g class="mark-q"><circle cx="66" cy="9" r="8.5" '
                     f'fill="{QUESTION_BG}" stroke="{INK}" '
                     f'stroke-width="1.2"/>'
                     f'<text x="66" y="14" text-anchor="middle" '
                     f'font-size="12.5" font-weight="700" fill="{INK}" '
                     f'font-family="system-ui">?</text></g>')
    parts.append('</g>')
    return (f'<svg class="horse-svg" viewBox="0 0 132 100" '
            f'xmlns="http://www.w3.org/2000/svg" role="img" '
            f'aria-hidden="true">' + "".join(parts) + "</svg>")


DEFS = (  # shared filter defs (desaturation for unknown / disconnected)
    '<svg width="0" height="0" style="position:absolute" aria-hidden="true">'
    '<defs><filter id="desat"><feColorMatrix type="saturate" '
    'values="0.25"/></filter></defs></svg>'
)
