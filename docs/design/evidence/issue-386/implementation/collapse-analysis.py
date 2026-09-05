#!/usr/bin/env python3
"""#386 evidence analysis: measure the thick status-bar bands (theme
surface1 fill) in the raw simulator frames and report their extents in pt.

The status bar is the ONLY full-width surface1 surface on the board (the
chrome strips are mantle, rows ride base, repo captions are hue-tinted
repoBand strips), so a row scan for the active flavor's surface1 token
isolates the bars. Reports per-phase bar count + heights, proving: the
collapsed blocked section renders its bar alone, expanded bars match the
collapsed bar's thickness, and an all-collapsed board still renders every
bar (empty sections keep their header).

Usage: python3 collapse-analysis.py phase-1-board-mocha.png ... [--scale 3]
"""
import argparse
import sys

from PIL import Image

# Catppuccin surface1 per flavor (Mocha dark / Latte light), sRGB.
SURFACE1 = {
    "mocha": (69, 71, 90),
    "latte": (188, 192, 204),
}


def is_surface1(rgb, target, tol=7):
    return all(abs(rgb[i] - target[i]) <= tol for i in range(3))


def bar_bands(path, target, scale, tol=7, min_width_frac=0.65):
    im = Image.open(path).convert("RGB")
    w, h = im.size
    rows = []
    for y in range(h):
        # Sample the row every 4 px.
        hits = 0
        samples = 0
        for x in range(0, w, 4):
            samples += 1
            if is_surface1(im.getpixel((x, y)), target, tol):
                hits += 1
        rows.append(hits / samples >= min_width_frac)
    bands = []
    in_band = False
    start = 0
    for y, on in enumerate(rows):
        if on and not in_band:
            start = y
            in_band = True
        elif not on and in_band:
            if y - start >= 6:  # >= 2 pt at @3x
                bands.append((start / scale, y / scale))
            in_band = False
    if in_band and h - start >= 6:
        bands.append((start / scale, h / scale))
    return bands


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("images", nargs="+")
    ap.add_argument("--scale", type=float, default=3.0)
    args = ap.parse_args()
    for path in args.images:
        name = path.split("/")[-1]
        flavor = "latte" if "latte" in name else "mocha"
        bands = bar_bands(path, SURFACE1[flavor], args.scale)
        print(f"{name} ({flavor}): {len(bands)} surface1 bar band(s)")
        for a, b in bands:
            print(f"  bar {a:6.1f}-{b:6.1f}pt  height={b - a:4.1f}pt")


if __name__ == "__main__":
    main()
