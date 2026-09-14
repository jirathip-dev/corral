#!/usr/bin/env python3
"""#530 actual-app measurement: where is the chestnut draft horse's fetlock
tuft relative to its leg, before and after the fix?

Both screenshots are the same fresh simulator, the same launch arguments and
the same demo fleet (`-demoMode -corral458HerdScenario`); the only build
difference is the #530 tuft line. `demo-garden-agent` is a chestnut DRAFT
horse in the blocked pose (leg 0 rest angle -26 deg, leg 2 rest angle 0), so
leg 2's tuft is the mapping anchor (identical in both arms, angle 0) and
leg 0's tuft is the measured one.

Art -> pixel mapping is derived from the two tuft blobs of the base frame
(art x = 43 and 84 at art y = 89), so no device metadata is assumed.
"""
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

MANE = np.array([0x5D, 0x33, 0x17])   # chestnut mane == draft fetlock tuft
LEG_REGION = (150, 560, 980, 1120)    # x1, x2, y1, y2 -- the rail horse legs


def blobs(img, colour, region, tol=12, min_pixels=6):
    x1, x2, y1, y2 = region
    sub = img[y1:y2, x1:x2]
    m = (np.abs(sub - colour).max(axis=2) <= tol)
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
            p = np.array(pts)
            out.append({'pixels': len(pts),
                        'centroid': [round(float(p[:, 1].mean()) + x1, 1), round(float(p[:, 0].mean()) + y1, 1)],
                        'bbox': [int(p[:, 1].min()) + x1, int(p[:, 0].min()) + y1,
                                 int(p[:, 1].max()) + x1, int(p[:, 0].max()) + y1]})
    return sorted(out, key=lambda b: -b['pixels'])


def main():
    before, after, out = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
    a = np.asarray(Image.open(before).convert('RGB')).astype(int)
    b = np.asarray(Image.open(after).convert('RGB')).astype(int)
    anchor_leg2 = blobs(a, MANE, (150, 290, 1040, 1110))   # art x=43, y=89, angle 0 in both arms
    tuft_before = blobs(a, MANE, (290, 430, 1040, 1110))   # art x=84
    tuft_after = blobs(b, MANE, (290, 430, 1040, 1110))
    report = {'before': str(before), 'after': str(after), 'pixels': list(a.shape)}
    if not anchor_leg2 or not tuft_before or not tuft_after:
        report['error'] = 'an expected tuft blob is missing'
        print(json.dumps(report, indent=2))
        return
    ax, ay = anchor_leg2[0]['centroid']
    bx, by = tuft_before[0]['centroid']
    # 41 art points separate leg 2 (x=43) and leg 0 (x=84) at the same art y.
    scale = (bx - ax) / 41.0
    origin_x = ax - 43 * scale
    origin_y = ay  # both centres sit at art y = 89
    report['anchor_leg2_centroid'] = [ax, ay]
    report['tuft_before_centroid'] = [bx, by]
    report['tuft_after_centroid'] = tuft_after[0]['centroid']
    report['tuft_after_blobs'] = tuft_after
    report['px_per_point'] = round(scale, 4)
    height, belly = 28.0, 72.0   # draft breed
    hip_y = belly - 6
    import math
    for label, angle in (('dislocated_pre_fix_angle_0', 0.0), ('aligned_leg_angle_-26', -26.0)):
        rad = math.radians(angle)
        art_x = 84.0 - (height - 5) * math.sin(rad)
        art_y = hip_y + (height - 5) * math.cos(rad)
        report[label] = {'art': [round(art_x, 2), round(art_y, 2)],
                         'px': [round(origin_x + art_x * scale, 1), round(origin_y + (art_y - 89) * scale, 1)]}

    def dist(p, q):
        return round(math.hypot(p[0]-q[0], p[1]-q[1]) / scale, 2)
    pre = report['dislocated_pre_fix_angle_0']['px']
    aligned = report['aligned_leg_angle_-26']['px']
    report['tuft_before_vs_pre_fix_px'] = round(math.hypot(bx-pre[0], by-pre[1]), 1)
    report['tuft_before_vs_aligned_pt'] = dist([bx, by], aligned)
    cx, cy = report['tuft_after_centroid']
    report['tuft_after_vs_aligned_px'] = round(math.hypot(cx-aligned[0], cy-aligned[1]), 1)
    report['tuft_after_vs_pre_fix_pt'] = dist([cx, cy], pre)
    report['tuft_moved_pt'] = dist(tuft_before[0]['centroid'], report['tuft_after_centroid'])
    print(json.dumps(report, indent=2))
    (out / 'actual-app-tuft-measurement.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
