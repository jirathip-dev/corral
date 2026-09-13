#!/usr/bin/env python3
"""#530 audit driver: classify hoof-to-leg correspondence for EVERY variant,
pose and gait phase, in the same terms the native XCTest asserts.

The mirror below reproduces `HerdArt.drawing`'s leg/tuft transforms exactly:
each leg bar AND its hoof band are one rigid rounded rect rotated about the hip
pivot `(x, belly-6)`; the draft fetlock tuft is a second rounded rect placed at
the fetlock. The metric per member ink is the offset from its leg's own ink, so
the classification matches the native check's.
"""
import json
import math
import sys
from pathlib import Path

BREEDS = {'light': dict(w=5.0, h=34.0, belly=66.0), 'stock': dict(w=6.2, h=31.0, belly=69.0),
          'draft': dict(w=7.4, h=28.0, belly=72.0)}
LEG_X = {0: 84.0, 1: 78.0, 2: 43.0, 3: 51.0}
POSES = {'stand': [0, 0, 0, 0], 'working': [28, -26, -26, 24], 'blocked': [-26, 0, 0, 0],
         'done': [0, 0, 0, 0], 'unknown': [6, -6, -6, 6], 'graze': [0, 0, 0, 0],
         'alertStatic': [0, 0, 0, 0]}
STEPPING_POSES = {'working', 'stand'}
PHASES = [0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875]
GAITS = [(0.0, 0.0)] + [(p, s) for s in (22.0, 8.0) for p in PHASES]


def member_offsets(breed, pose, swing, phase):
    b = BREEDS[breed]
    rest = POSES[pose]
    step = swing * math.sin(2 * math.pi * phase) if pose in STEPPING_POSES else 0.0
    legs = [rest[0] + step, rest[1] - step, rest[2] - step, rest[3] + step]
    rows = {}
    for index, x in LEG_X.items():
        rad = math.radians(legs[index])
        hip = (x, b['belly'] - 6)
        leg_centre = (x - (b['h'] / 2) * math.sin(rad), hip[1] + (b['h'] / 2) * math.cos(rad))
        hoof = (x - (b['h'] - 2.5) * math.sin(rad), hip[1] + (b['h'] - 2.5) * math.cos(rad))
        entry = {'leg_angle': legs[index],
                 'hoof_offset': round(math.hypot(hoof[0] - leg_centre[0], hoof[1] - leg_centre[1]), 3),
                 'hoof_expected': round(b['h'] / 2 - 2.5, 3)}
        if breed == 'draft' and index in (0, 2):
            tstep = step if index == 0 else -step
            trad = math.radians(tstep)
            tuft = (x - (b['h'] - 5) * math.sin(trad), hip[1] + (b['h'] - 5) * math.cos(trad))
            d = math.hypot(tuft[0] - hoof[0], tuft[1] - hoof[1])
            entry.update({'tuft_offset_from_hoof': round(d, 3),
                          'tuft_expected': 2.5,
                          'tuft_aligned': abs(d - 2.5) <= 0.01})
        rows[index] = entry
    return rows


def main():
    table, findings = [], {}
    for breed in BREEDS:
        for pose in POSES:
            aligned_everywhere = True
            worst = 0.0
            worst_tuft = None
            hoof_all = True
            for swing, phase in GAITS:
                rows = member_offsets(breed, pose, swing, phase)
                for index, entry in rows.items():
                    if abs(entry['hoof_offset'] - entry['hoof_expected']) > 0.01:
                        hoof_all = False
                    if 'tuft_aligned' in entry and not entry['tuft_aligned']:
                        aligned_everywhere = False
                        if entry['tuft_offset_from_hoof'] > worst:
                            worst = entry['tuft_offset_from_hoof']
                            worst_tuft = index
            status = 'n/a (no fetlock tuft: breeds light/stock)' if breed != 'draft' else (
                'ALIGNED' if aligned_everywhere else 'MISALIGNED')
            if breed == 'draft' and any('tuft_aligned' in member_offsets(breed, pose, 22.0, 0.25)[i]
                                        for i in (0, 2)):
                pass
            entry = {'hoof_bands': 'ALIGNED (rigid with the leg bar in every sample)' if hoof_all else 'MISALIGNED',
                     'fetlock_tufts': status}
            if status == 'MISALIGNED':
                entry['worst_tuft_offset_pt'] = round(worst, 3)
                entry['tuft_expected_pt'] = 2.5
                entry['worst_leg'] = worst_tuft
                entry['rest_angles'] = POSES[pose]
            table.append(f"{breed:7s} {pose:12s} {entry['fetlock_tufts']:11s} "
                         + (f"worst {entry.get('worst_tuft_offset_pt')} pt (expected 2.5) on leg {entry.get('worst_leg')}"
                            if status == 'MISALIGNED' else ''))
    print('\n'.join(table))
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('.')
    # The text rendering is stripped of trailing whitespace so the committed
    # table never trips `git diff --check`; the JSON keeps the raw field
    # values it was generated from.
    (out / 'audit-gait-hoof.txt').write_text('\n'.join(line.rstrip() for line in table) + '\n')
    (out / 'audit-gait-hoof.json').write_text(json.dumps(
        {'breeds': {k: v for k, v in BREEDS.items()}, 'poses': POSES, 'gaits': GAITS,
         'leg_x': LEG_X, 'table': table}, indent=2) + '\n')


if __name__ == '__main__':
    main()
