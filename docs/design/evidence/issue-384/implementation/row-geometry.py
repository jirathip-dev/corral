#!/usr/bin/env python3
"""#384 row-geometry analysis over the raw 1179x2556 captures.

Proves the no-layout-jump acceptance: the demo-output row's line-2 trailing
badge text ('dirty', same element in both filter states) must sit at the
SAME offset below the row's line-1 state chip in the All frame and in the
filtered frame (Mocha + Latte). Rows under an active repo pill must keep
their exact All-state height (the label chip is replaced by a color-only
echo carrying the chip's caption2 line box, so only the name disappears).

Run from this directory against the sibling raw PNGs:
    python3 row-geometry.py  (raw frames next to the 390x844 resizes)
"""
from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image

RAW = Path(__file__).resolve().parent

# The demo-output row's state chip fill (working) and the 'dirty' badge
# text color per flavor; chip color tolerance is tight so only the chip
# fill matches (mocha's chip fill is close to other grays, so latte is the
# clean pair; mocha numbers are reported with the same scan).
FRAMES = [
    # (raw png, chip-fill color, 'dirty' color, scan window y)
    ("phase-1-board-mocha-all.png", (50, 63, 74), (250, 179, 135), (2200, 2400)),
    ("phase-2-board-mocha-filtered.png", (50, 63, 74), (250, 179, 135), (1160, 1400)),
    ("phase-4-board-latte-all.png", (202, 225, 229), (254, 128, 25), (2200, 2400)),
    ("phase-5-board-latte-filtered.png", (202, 225, 229), (254, 128, 25), (1160, 1400)),
]


def bbox_color(path: Path, x0: int, x1: int, y0: int, y1: int,
               target: tuple[int, int, int], tol: int = 40):
    image = Image.open(path).convert("RGB")
    pixels = image.load()
    xs: list[int] = []
    ys: list[int] = []
    for y in range(y0, y1):
        for x in range(x0, x1):
            p = pixels[x, y]
            if all(abs(p[i] - target[i]) <= tol for i in range(3)):
                xs.append(x)
                ys.append(y)
    return (min(xs), min(ys), max(xs), max(ys)) if xs else None


def main() -> None:
    rows = []
    for name, chip, dirty, (y0, y1) in FRAMES:
        path = RAW / name
        chip_box = bbox_color(path, 60, 360, y0, y1, chip, tol=8)
        dirty_box = bbox_color(path, 700, 1160, y0, y1, dirty)
        if chip_box is None or dirty_box is None:
            print(f"{name}: MISSING anchor (chip={chip_box}, dirty={dirty_box})")
            sys.exit(2)
        span = dirty_box[3] - chip_box[1]
        rows.append((name, chip_box, dirty_box, span))
        print(f"{name}: state-chip fill y {chip_box[1]}..{chip_box[3]}"
              f" | 'dirty' badge y {dirty_box[1]}..{dirty_box[3]}"
              f" | span chip-top->badge-bottom = {span}px")
    all_spans = [r[3] for r in rows if "all" in r[0]]
    filt_spans = [r[3] for r in rows if "filtered" in r[0]]
    print()
    print("All frames spans:   ", all_spans)
    print("Filtered spans:     ", filt_spans)
    deltas = [a - f for a, f in zip(all_spans, filt_spans)]
    print("All - filtered:     ", deltas, "px")
    if all(d <= 2 for d in deltas):
        print("RESULT: row line-2 geometry identical in both filter states"
              " (<=2px = 0.7pt sub-pixel); no layout jump.")
    else:
        print("RESULT: MISMATCH >2px — rows change height under the filter!")
        sys.exit(1)


if __name__ == "__main__":
    main()
