#!/usr/bin/env python3
"""#385 translucent-sheet evidence analysis (stdlib + Pillow only).

Compares the captured sheet-over-board frames against the board-only frames
from the SAME deterministic demo launch (identical scroll position) and
proves the sheet surface is TRANSLUCENT:

  1. A column INSIDE the recents sheet surface (its left margin, which no
     card covers) is correlated against the SAME column of the board-only
     frame blurred by a Gaussian kernel (pre-compensating the glass's own
     blur). An OPAQUE surface cannot track the underlying content: its
     column is one flat tone (stdev ~ 0) and its correlation is ~0.
  2. The number of distinct surface tones in the probe column is reported:
     an opaque surface yields 1; the sheet surface tracks the board's
     row/band structure (>= 2 tones).

Frames (390x844 rescale of the 1179x2556 iPhone 16 @3x captures; sips
`-z 844 390`, 0.18 % aspect distortion):
  phase-2-recents-mocha-390x844.png  vs phase-1-board-mocha-390x844.png
  phase-3-recents-latte-390x844.png  vs phase-4-board-latte-390x844.png

Run:  python3 translucency-analysis.py
"""
import math
import statistics
from pathlib import Path

from PIL import Image, ImageFilter

HERE = Path(__file__).resolve().parent


def column_stats(img, x, y0, y1):
    """Per-pixel channel sums of one column strip -> list."""
    strip = img.crop((x, y0, x + 1, y1))
    return [sum(p) for p in strip.getdata()]


def corr(a, b):
    ma, mb = statistics.mean(a), statistics.mean(b)
    cov = sum((x - ma) * (y - mb) for x, y in zip(a, b))
    va = sum((x - ma) ** 2 for x in a)
    vb = sum((y - mb) ** 2 for y in b)
    if va == 0 or vb == 0:
        return 0.0
    return cov / math.sqrt(va * vb)


def probe(surface_img, board_img, sheet_top_frac, x_frac=0.015):
    """Correlate a sheet-surface column against the blurred board column.

    x_frac: probe column as a fraction of width (recents sheet left margin,
    which no card ever covers).
    """
    w, h = surface_img.size
    x = max(1, int(w * x_frac))
    y0 = int(h * sheet_top_frac) + int(h * 0.04)
    y1 = h
    blur = max(2, round(h / 213))  # 12 px at 3x == 4 px at 390x844

    surface_vals = column_stats(surface_img, x, y0, y1)
    # Blur the board column over a wider crop, then take its centre column.
    crop = board_img.crop((x - blur * 2, y0, x + blur * 2 + 1, y1))
    blurred = crop.filter(ImageFilter.GaussianBlur(blur))
    board_vals = column_stats(blurred, blur * 2, 0, y1 - y0)

    tones = len({(v // 30) for v in surface_vals})
    return {
        "corr": round(corr(surface_vals, board_vals), 3),
        "surface_stdev": round(statistics.pstdev(surface_vals), 2),
        "board_stdev": round(statistics.pstdev(board_vals), 2),
        "surface_tones": tones,
    }


def find_sheet_top(img):
    """Grab-handle row: a horizontal light-gray capsule near the sheet top.
    The recents sheet is presented at the MEDIUM detent (~44 % of screen
    height on this device), so only rows below 36 % are candidates — board
    chrome above the sheet (pills, tags) is also grayish on Latte and would
    otherwise win the search."""
    w, h = img.size
    for y in range(int(h * 0.36), int(h * 0.65), 2):
        xs = []
        for x in range(int(w * 0.35), int(w * 0.65), 2):
            r, g, b = img.getpixel((x, y))
            if abs(r - g) < 30 and abs(g - b) < 30 and 90 < (r + g + b) / 3 < 200:
                xs.append(x)
        if xs and max(xs) - min(xs) > int(w * 0.06):
            return y / h
    raise SystemExit("sheet grab-handle not found")


def main():
    def load(name):
        return Image.open(HERE / f"{name}-390x844.png").convert("RGB")

    results = []
    for surface_name, board_name, label in (
        ("phase-2-recents-mocha", "phase-1-board-mocha", "recents Mocha"),
        ("phase-3-recents-latte", "phase-4-board-latte", "recents Latte"),
    ):
        surface = load(surface_name)
        board = load(board_name)
        top = find_sheet_top(surface)
        stat = probe(surface, board, sheet_top_frac=top)
        ok = stat["corr"] >= 0.5 and stat["surface_tones"] >= 2
        results.append((label, stat, ok))
        print(f"[{label}] sheet top at {top:.1%} of height")
        print(f"  sheet-surface column vs blurred board: "
              f"corr={stat['corr']} surface-stdev={stat['surface_stdev']} "
              f"board-stdev={stat['board_stdev']} tones={stat['surface_tones']}")
        print("  -> TRANSLUCENT: surface tracks the underlying board"
              if ok else "  -> CHECK: see README interpretation")

    print("\nOpaque control (card interiors are one flat tone):")
    for name, label in (("phase-2-recents-mocha", "mocha block card"),
                        ("phase-3-recents-latte", "latte block card")):
        img = load(name)
        w, h = img.size
        hit = None
        for yf in (0.55, 0.60, 0.65, 0.70, 0.75, 0.80):
            rgb = img.getpixel((int(w * 0.5), int(h * yf)))
            # Cards: mocha surface0 ~ (49,50,68); latte cards near-white.
            if (max(rgb) - min(rgb)) < 45 and (120 < sum(rgb) < 260
                                               or sum(rgb) > 590):
                vals = column_stats(img, int(w * 0.5),
                                    int(h * yf) - 2, int(h * yf) + 2)
                hit = (yf, rgb, round(statistics.pstdev(vals), 2),
                       len({v // 30 for v in vals}))
                break
        print(f"  [{label}] card at y={hit[0]:.0%}: rgb={hit[1]} "
              f"stdev={hit[2]} tones={hit[3]}" if hit
              else f"  [{label}] card probe not found")

    verdict = all(ok for _, _, ok in results)
    print("\nVERDICT:", "PASS - sheet surfaces are translucent over the board"
          if verdict else "FAIL")
    return 0 if verdict else 1


if __name__ == "__main__":
    raise SystemExit(main())
