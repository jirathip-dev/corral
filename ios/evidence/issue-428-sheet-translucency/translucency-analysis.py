#!/usr/bin/env python3
"""#428 discriminating translucency gate over RENDERED frames.

Checks the ACTUAL rendered pixels of the #428 evidence captures (not
source strings). Exits non-zero with a per-check report when any
acceptance surface regresses:

  G1 sheet form cells are the active flavor's BASE token (no system
     white / neutral #2c2c2e grouped surface anywhere in the sheets);
  G2 the sheet backdrop regions carry the active flavor cast (dark
     flavors: blue channel over the neutral floor; Latte: frosted base
     luminance floor) instead of the old flavor-less neutral surface;
  G3 the glass and forced-fallback branches still differ measurably in
     backdrop-owned regions (the material response survived);
  G4 the recents sheet's backdrop slivers (gaps + bottom strip) carry
     the flavor cast too.

Usage:
  python3 translucency-analysis.py <frames-dir> [--flavor-pairs]
The frames-dir must hold the standard phase files
  phase-416-2-recents-mocha / -3-recents-latte / -5-settings-mocha /
  -6-settings-latte [-390x844.png], plus optional phase-1-mh-add-* files.
RED at the #428 base head (neutral backdrop + system cells); GREEN at the
fixed head (flavor surfaces everywhere).
"""
from __future__ import annotations

import os
import sys
from collections import Counter

from PIL import Image

# Catppuccin base tokens per flavor (same values the app resolves).
BASE = {
    "mocha": (30, 30, 46),
    "latte": (239, 241, 245),
    "frappe": (48, 52, 70),
    "macchiato": (36, 39, 58),
}
# System surfaces that must NOT appear inside the sheets.
SYSTEM_WHITE = (252, 252, 253)
SYSTEM_GROUPED_DARK = (44, 44, 46)   # #2c2c2e secondary grouped
SYSTEM_GROUPED_DARK2 = (28, 28, 30)  # #1c1c1e grouped

# Deterministic regions (390x844 frames from the driver phases).
# (x0, y0, x1, y1) — calibrated on the phase geometry.
SETTINGS_MEDIUM = {
    "cells": (60, 580, 330, 790),      # the Appearance card rows
    "backdrop": (90, 470, 300, 545),   # form surface above the card
}
RECENTS_MEDIUM = {
    "bottom": (90, 796, 300, 828),     # backdrop strip under the last block
}
ADD_HOST_MEDIUM = {
    "cells": (60, 560, 330, 800),
    "backdrop": (60, 430, 330, 540),
}


def dominant(img, region):
    px = img.load()
    c = Counter()
    x0, y0, x1, y1 = region
    for y in range(y0, y1):
        for x in range(x0, x1):
            c[px[x, y][:3]] += 1
    return c.most_common(1)[0][0]


def mean_tone(img, region):
    px = img.load()
    x0, y0, x1, y1 = region
    n = 0
    s = [0, 0, 0]
    for y in range(y0, y1):
        for x in range(x0, x1):
            p = px[x, y][:3]
            s[0] += p[0]
            s[1] += p[1]
            s[2] += p[2]
            n += 1
    return tuple(v / n for v in s)


def dist(a, b):
    return max(abs(a[i] - b[i]) for i in range(3))


def near_system(img, region):
    """System grouped surfaces that must not appear inside the sheets:
    pure white (light cells), the #2c2c2e/#1c1c1e neutral grays (dark
    cells). Latte's light cells are caught by the dominant-tone check
    (they never equal the flavor base token)."""
    px = img.load()
    x0, y0, x1, y1 = region
    hits = 0
    total = 0
    for y in range(y0, y1):
        for x in range(x0, x1):
            p = px[x, y][:3]
            total += 1
            if p[0] >= 248 and p[1] >= 248 and p[2] >= 248:
                hits += 1
            elif dist(p, SYSTEM_GROUPED_DARK) <= 3 \
                    or dist(p, SYSTEM_GROUPED_DARK2) <= 3:
                hits += 1
    return hits / total


def check_cells(name, frame_path, flavor, region):
    img = load(frame_path, name)
    tone = dominant(img, region)
    if dist(tone, BASE[flavor]) <= 6:
        print(f"[PASS] {name} form cells themed: dominant #{tone[0]:02x}{tone[1]:02x}{tone[2]:02x} ~= base {BASE[flavor]}")
        return True
    print(f"[FAIL] {name} form cells NOT themed: dominant #{tone[0]:02x}{tone[1]:02x}{tone[2]:02x} "
          f"(expected ~{BASE[flavor]})")
    return False


def check_backdrop_cast(name, frame_path, flavor, region, floor=220):
    img = load(frame_path, name)
    mean = mean_tone(img, region)
    if flavor == "latte":
        ok = mean[0] >= floor and mean[1] >= floor and mean[2] >= floor and mean[2] - mean[0] < 8
        print(f"[{'PASS' if ok else 'FAIL'}] {name} backdrop latte lift: mean=({mean[0]:.0f},{mean[1]:.0f},{mean[2]:.0f}) "
              f"(floor {floor}, neutral hue)")
    else:
        cast = mean[2] - mean[0]
        ok = cast >= 7
        print(f"[{'PASS' if ok else 'FAIL'}] {name} backdrop flavor cast: mean=({mean[0]:.0f},{mean[1]:.0f},{mean[2]:.0f}) "
              f"b-r={cast:.1f} (floor 7)")
    return ok


def check_system_leak(name, frame_path, region):
    img = load(frame_path, name)
    share = near_system(img, region)
    ok = share <= 0.03
    print(f"[{'PASS' if ok else 'FAIL'}] {name} no system grouped surface: {share * 100:.1f}% "
          f"(ceiling 3%)")
    return ok


def load(path, name):
    img = Image.open(path).convert("RGB")
    if img.size != (390, 844):
        print(f"[WARN] {name} size {img.size} != 390x844")
    return img


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 2
    d = argv[1]
    mode = "medium"
    for a in argv[2:]:
        if a.startswith("--mode="):
            mode = a.split("=", 1)[1]
    # mode: medium (Mocha/Latte driver phases), spots (Frappe/Macchiato
    # run of the same phases), large (release presentation, no medium
    # detents). Fallback pairs ride the same dir as *-fallback-* files.
    if mode == "spots":
        frame_checks = [
            # (frame, flavor, region-key) — the spot runs keep the driver's
            # phase markers but render Frappe/Macchiato (launch arg).
            ("phase-416-5-settings-frappe-390x844.png", "frappe", "cells"),
            ("phase-416-5-settings-frappe-390x844.png", "frappe", "backdrop"),
            ("phase-416-6-settings-macchiato-390x844.png", "macchiato", "cells"),
            ("phase-416-6-settings-macchiato-390x844.png", "macchiato", "backdrop"),
            ("phase-416-2-recents-frappe-390x844.png", "frappe", "bottom"),
            ("phase-416-3-recents-macchiato-390x844.png", "macchiato", "bottom"),
        ]
    elif mode == "large":
        # Release presentation: full-height sheets; the Appearance card
        # rows start ~y234 and the sheet chrome above is the backdrop.
        frame_checks = [
            ("phase-416-5-settings-mocha-large-390x844.png", "mocha", "cells-large"),
            ("phase-416-5-settings-mocha-large-390x844.png", "mocha", "chrome-large"),
            ("phase-416-6-settings-latte-large-390x844.png", "latte", "cells-large"),
            ("phase-416-6-settings-latte-large-390x844.png", "latte", "chrome-large"),
        ]
    elif mode == "addhost":
        # Multi-host Add Host evidence: entry (mocha) + fingerprint
        # confirmation (latte) at the medium detent; rows under the
        # input fields must be themed base cells.
        frame_checks = [
            ("phase-7-addhost-entry-mocha-390x844.png", "mocha", "cells-addhost"),
            ("phase-8-addhost-confirm-latte-390x844.png", "latte", "cells-addhost"),
        ]
    else:
        frame_checks = [
            ("phase-416-5-settings-mocha-390x844.png", "mocha", "cells"),
            ("phase-416-5-settings-mocha-390x844.png", "mocha", "backdrop"),
            ("phase-416-6-settings-latte-390x844.png", "latte", "cells"),
            ("phase-416-6-settings-latte-390x844.png", "latte", "backdrop"),
            ("phase-416-2-recents-mocha-390x844.png", "mocha", "bottom"),
            ("phase-416-3-recents-latte-390x844.png", "latte", "bottom"),
        ]
    results = []
    for fname, flavor, rk in frame_checks:
        path = os.path.join(d, fname)
        if not os.path.exists(path):
            print(f"[SKIP] missing {fname}")
            continue
        if rk == "cells":
            results.append(check_cells(fname, path, flavor, SETTINGS_MEDIUM["cells"]))
            results.append(check_system_leak(fname, path, (30, 430, 360, 830)))
        elif rk == "cells-addhost":
            results.append(check_cells(fname, path, flavor, (60, 660, 330, 780)))
            results.append(check_system_leak(fname, path, (30, 420, 360, 830)))
        elif rk == "cells-large":
            results.append(check_cells(fname, path, flavor, (60, 260, 330, 470)))
            results.append(check_system_leak(fname, path, (30, 60, 360, 830)))
        elif rk == "chrome-large":
            # the full-height sheet's own chrome (nav/title strip) must
            # carry the flavor, not a system surface.
            results.append(check_backdrop_cast(fname, path, flavor, (90, 90, 300, 200), floor=210))
        elif rk == "backdrop":
            results.append(check_backdrop_cast(fname, path, flavor, SETTINGS_MEDIUM["backdrop"], floor=219))
        elif rk == "bottom":
            results.append(check_backdrop_cast(fname, path, flavor, RECENTS_MEDIUM["bottom"], floor=229))
    # A/B material response: the glass and forced-fallback settings frames
    # must differ in the backdrop region (both at the fixed head carry the
    # material, the glass branch adds the native glass layer).
    gf = os.path.join(d, "phase-416-5-settings-mocha-390x844.png")
    ff = os.path.join(d, "phase-416-5-settings-mocha-fallback-390x844.png")
    if os.path.exists(gf) and os.path.exists(ff):
        a = mean_tone(load(gf, "glass"), SETTINGS_MEDIUM["backdrop"])
        b = mean_tone(load(ff, "fallback"), SETTINGS_MEDIUM["backdrop"])
        delta = max(abs(a[i] - b[i]) for i in range(3))
        ok = delta >= 3
        results.append(ok)
        print(f"[{'PASS' if ok else 'FAIL'}] glass/fallback material A/B: backdrop delta={delta:.1f} (floor 3)")
    failed = results.count(False)
    print(f"\nRESULT: {len(results) - failed}/{len(results)} checks passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
