#!/usr/bin/env python3
"""#530 actual-app metric on the unoccluded WORKING draft horse.

`demo-atlas-worker` (multi-host demo seed, host A live) is a DUN draft horse in
the working pose, drawn in the paddock. Its fetlock tufts are painted in the
dun mane colour and must wrap the near legs (body colour); the hoof band is the
shared hoof colour. The pre-fix renderer rotated each tuft by the gait swing
alone, so the tuft sat off the leg's distal end by up to 12 art points — a
visible background gap between the tuft band and its leg. The fixed renderer
puts the tuft back on the leg (gap 0, tuft touching leg/hoof paint).

Per frame this reports, for every tuft blob under the horse's barrel:
  * whether it touches leg or hoof paint (within 2 px),
  * the minimum distance from the tuft's pixels to that paint.

Scale: 3 px per art point (iPhone 16, @3x), confirmed by the leg spacing.
"""
import json
import math
import sys
from pathlib import Path

import numpy as np
from PIL import Image

BODY = np.array([0xB0, 0x8D, 0x5E])   # dun coat (near legs)
MANE = np.array([0x41, 0x32, 0x1F])   # dun mane == dun draft fetlock tuft
HOOF = np.array([0x3A, 0x33, 0x2E])
PX_PER_POINT = 2.995


def mask(img, colour, tol=8):
    return (np.abs(img - colour).max(axis=2) <= tol)


def blobs(m, min_pixels=40):
    out, seen = [], np.zeros_like(m, dtype=bool)
    ys, xs = np.nonzero(m)
    for y, x in zip(ys, xs):
        if seen[y, x]:
            continue
        stack, seen[y, x] = [(y, x)], True
        pts = []
        while stack:
            cy, cx = stack.pop()
            pts.append((cy, cx))
            for ny, nx in ((cy-1, cx), (cy+1, cx), (cy, cx-1), (cy, cx+1)):
                if 0 <= ny < m.shape[0] and 0 <= nx < m.shape[1] and m[ny, nx] and not seen[ny, nx]:
                    seen[ny, nx] = True
                    stack.append((ny, nx))
        if len(pts) >= min_pixels:
            out.append(np.array(pts))          # (y, x) pairs
    return sorted(out, key=len, reverse=True)


def main():
    report = {}
    for name in sys.argv[1:]:
        img = np.asarray(Image.open(name).convert('RGB')).astype(int)
        body = mask(img, BODY)
        ys, xs = np.nonzero(body)
        box = (int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max()))
        x1, y1, x2, y2 = box
        mane_full = mask(img, MANE)
        mane_region = np.zeros_like(mane_full)
        mane_region[y1 + (y2 - y1) // 2:y2 + 40, max(0, x1 - 20):x2 + 20] = True
        tufts = blobs(mane_full & mane_region)
        leg_like = np.zeros_like(body)
        leg_like[y1 + (y2 - y1) * 3 // 5:y2 + 40, max(0, x1 - 20):x2 + 20] = True
        contacts = mask(img, HOOF) | body
        entries = []
        for blob in tufts:
            by, bx = blob[:, 0], blob[:, 1]
            best, best_pixel = None, None
            for y, x in zip(*np.nonzero(contacts & leg_like)):
                d = np.sqrt((by - y) ** 2 + (bx - x) ** 2).min()
                if best is None or d < best:
                    best, best_pixel = d, (x, y)
            if best is not None and best < 120:
                entries.append({'tuft_pixels': int(len(blob)),
                                'tuft_bbox': [int(bx.min()), int(by.min()), int(bx.max()), int(by.max())],
                                'gap_px': round(float(best), 1),
                                'gap_points': round(float(best) / PX_PER_POINT, 2),
                                'touches_leg_paint_within_2px': bool(best <= 2.0),
                                'nearest_paint_px': [int(best_pixel[0]), int(best_pixel[1])]})
        report[Path(name).name] = {'horse_box': list(box), 'tuft_blobs': len(tufts), 'tuft_measurements': entries}
    print(json.dumps(report, indent=2))
    out = Path(sys.argv[1]).parent / 'actual-app-working-draft-tuft.json'
    out.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
