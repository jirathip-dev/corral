#!/usr/bin/env python3
"""Check #548 rendered-frame measurements and unchanged scenery pixels."""
import json
from pathlib import Path
from PIL import Image, ImageChops

OUT = Path('/tmp/g548-evidence')


def main():
    base = json.loads((OUT / 'base/capture.json').read_text())
    head = json.loads((OUT / 'head/capture.json').read_text())
    native = json.loads((OUT / 'native/capture.json').read_text())
    report = {'comparison': {}, 'native': {}}
    for light in ('day', 'night'):
        keys = [f'{light}-{state}-large' for state in ('empty', 'blocked', 'last-known')]
        before = [base['measurements'][key]['firstRowInWindow'] for key in keys]
        after = [head['measurements'][key]['firstRowInWindow'] for key in keys]
        assert len(set(before)) > 1, 'base control must demonstrate the rail jump'
        assert len(set(after)) == 1, 'candidate must keep the first row fixed'
        assert after[0] < before[0], 'the first row must start higher than base'
        # The fence's unobscured left edge: outside every padded horse/card.
        # This is a scenery crop, not a claim that foreground cards never cover it.
        box = (0, 328, 10, 388)
        old = Image.open(base['captures'][keys[0]]['path']).convert('RGB').crop(box)
        new = Image.open(head['captures'][keys[0]]['path']).convert('RGB').crop(box)
        difference = ImageChops.difference(old, new)
        changed = sum(pixel != (0, 0, 0) for pixel in difference.get_flattened_data())
        assert changed == 0, f'{light}: lower fence scenery changed'
        report['comparison'][light] = dict(base_first_row_points=before,
                                           head_first_row_points=after,
                                           upward_delta_points=before[0]-after[0],
                                           fence_crop=list(box), fence_changed_pixels=changed)
    for light in ('day', 'night'):
        for size in ('large', 'accessibility3'):
            keys = [f'{light}-{state}-{size}' for state in ('empty', 'blocked', 'last-known')]
            rows = [native['measurements'][key]['firstRowInWindow'] for key in keys]
            heights = [native['measurements'][key]['frames']['rail-zone'][3] for key in keys]
            card = native['measurements'][keys[-1]]['frames']['rail-horse'][3]
            assert len(set(rows)) == len(set(heights)) == 1
            assert abs(heights[-1] - card) < 0.01, 'reservation must be the measured longest rail card'
            report['native'][f'{light}-{size}'] = dict(first_row_points=rows,
                                                     reserved_height_points=heights,
                                                     last_known_card_height_points=card)
    assert 'day-blocked-accessibility3-scrolled' in native['captures']
    (OUT / 'verification.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(report, indent=2))
    print('PASS: identical first-row origins; measured card reservation; higher than base; unchanged lower fence edge')


if __name__ == '__main__':
    main()
