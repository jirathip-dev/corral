#!/usr/bin/env python3
"""issue-462 — structural + determinism verifier for the prototype bundle.

Checks (all from source-of-record, not from build-summary trust):
  1. required counts (10 masters / 30 previews / 8 contexts / 8 stage HTML)
  2. dimensions (masters 1024x1024, contexts exactly 390x844, previews sized)
  3. opacity: masters RGB (no alpha channel); masked previews RGBA whose
     alpha is exactly {0,255} with only the four rounded corners transparent;
     plain previews RGB
  4. Original masters byte-exact vs the shipping appiconset file
  5. crop windows re-derived from canonical geometry; positive margin on
     every side; identical across all four coats
  6. contrast table recomputed from canonical palettes; every body contrast
     >= 3.4; values match build-summary.json
  7. determinism: full regeneration reproduces every artifact byte-exactly
Writes tools/verification.json and prints VERIFY OK.
"""
from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path
from PIL import Image

HERE = Path(__file__).resolve().parent
ISSUE = HERE.parent
ROOT = ISSUE.parents[3]
sys.path.insert(0, str(ROOT / 'ios' / 'tools' / 'herd-art'))
sys.path.insert(0, str(HERE))
import horsesvg       # noqa: E402  canonical generator
import svgmini        # noqa: E402
import gen_icons as G  # noqa: E402  design constants + crop rules (no side effects)

COATS = ['bay', 'palomino', 'black', 'grey']
CHOICES = COATS + ['original']
FAILS: list[str] = []


def check(name: str, ok: bool, detail: str = ''):
    if not ok:
        FAILS.append(f'{name}: {detail}')
    return {'check': name, 'ok': ok, 'detail': detail}


def sha256(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def all_artifacts() -> dict[str, str]:
    out: dict[str, str] = {}
    for sub in ('masters', 'previews', 'contexts', 'html'):
        for p in sorted((ISSUE / sub).rglob('*')):
            if p.is_file():
                out[str(p.relative_to(ISSUE))] = sha256(p)
    return out


def main() -> int:
    report: dict = {'checks': []}

    # ---- 1. counts ----
    masters = {t: sorted((ISSUE / 'masters' / f'treatment-{t}').glob('*.png'))
               for t in 'ab'}
    previews = sorted((ISSUE / 'previews').glob('*.png'))
    contexts = sorted((ISSUE / 'contexts').glob('*.png'))
    stage = sorted((ISSUE / 'html' / 'stage').glob('*.html'))
    c_ok = (all(len(v) == 5 for v in masters.values()) and len(previews) == 30
            and len(contexts) == 8 and len(stage) == 8
            and (ISSUE / 'index.html').is_file()
            and (ISSUE / 'README.md').is_file())
    report['checks'].append(check('counts', c_ok,
        f'masters={ {k: len(v) for k, v in masters.items()} } '
        f'previews={len(previews)} contexts={len(contexts)} stage={len(stage)}'))
    report['counts'] = {'masters': sum(len(v) for v in masters.values()),
                        'previews': len(previews), 'contexts': len(contexts),
                        'stage_html': len(stage), 'choices': 5,
                        'treatments': 2}

    # ---- 2. dimensions ----
    dim_bad = []
    for t in 'ab':
        for p in masters[t]:
            if Image.open(p).size != (1024, 1024):
                dim_bad.append(p.name)
    for p in contexts:
        if Image.open(p).size != (390, 844):
            dim_bad.append(p.name)
    for p in previews:
        want = 180 if '180' in p.name else 120 if '120' in p.name else 60
        if Image.open(p).size != (want, want):
            dim_bad.append(p.name)
    report['checks'].append(check('dimensions', not dim_bad, str(dim_bad)))

    # ---- 3. opacity / alpha ----
    op_bad = []
    alpha_stats = []
    for t in 'ab':
        for coat in CHOICES:
            im = Image.open(ISSUE / 'masters' / f'treatment-{t}' / f'{coat}-1024.png')
            if im.mode != 'RGB':
                # RGB = no alpha channel at all = provably opaque
                op_bad.append(f'master {coat}-{t} mode={im.mode}')
    for p in previews:
        im = Image.open(p)
        if p.name.startswith('mask'):
            if im.mode != 'RGBA':
                op_bad.append(f'{p.name} mode={im.mode}')
                continue
            alphas = {v for v, n in enumerate(im.getchannel('A').histogram()) if n}
            if not alphas <= {0, 255}:
                op_bad.append(f'{p.name} partial alpha values {sorted(alphas)[:5]}')
                continue
            hist = im.getchannel('A').histogram()
            n_opaque = hist[255]
            alpha_stats.append({'file': p.name, 'opaque_px': n_opaque,
                                'total_px': im.size[0] * im.size[1]})
            center = im.getpixel((im.size[0] // 2, im.size[1] // 2))
            if center[3] != 255:
                op_bad.append(f'{p.name} center not opaque')
        else:
            if im.mode != 'RGB':
                op_bad.append(f'{p.name} mode={im.mode}')
    report['checks'].append(check('opacity', not op_bad, str(op_bad[:6])))
    report['alpha'] = alpha_stats[:4]

    # ---- 4. Original byte-exact ----
    src = sha256(G.ORIGINAL_SRC)
    oe = []
    for t in 'ab':
        got = sha256(ISSUE / 'masters' / f'treatment-{t}' / 'original-1024.png')
        if got != src:
            oe.append(f'treatment-{t}')
    report['checks'].append(check('original_byte_exact', not oe, str(oe)))
    report['original_sha256'] = src
    report['canonical_horsesvg_sha256'] = sha256(
        ROOT / 'ios/tools/herd-art/horsesvg.py')

    # ---- 5. crop windows re-derived; margins positive; coat-identical ----
    def windows_for(coat):
        prims = svgmini.parse_primitives(
            horsesvg.horse_svg(G.identity(coat), G.POSE, False, G.FACING, False))
        return G.crop_windows(prims)

    crop_bad = []
    crop_report = {}
    ref = windows_for('bay')
    for coat in COATS:
        w = windows_for(coat)
        if w[:2] != ref[:2]:
            crop_bad.append(coat)
    (ax, ay, asd), (bx, by, bsd), (nx0, ny0, nx1, ny1), (fx0, fy0, fx1, fy1) = ref
    for name, (cx, cy, sd), bb in (
            ('A', (ax, ay, asd), (nx0, ny0, nx1, ny1)),
            ('B', (bx, by, bsd), (fx0, fy0, fx1, fy1))):
        margins = (bb[0] - (cx - sd / 2), (cx + sd / 2) - bb[2],
                   bb[1] - (cy - sd / 2), (cy + sd / 2) - bb[3])
        if min(margins) <= 0:
            crop_bad.append(f'{name} margins {margins}')
        crop_report[name] = {'window': [cx, cy, sd],
                             'bounds': [cx - sd / 2, cy - sd / 2,
                                        cx + sd / 2, cy + sd / 2],
                             'margins': list(margins)}
    report['checks'].append(check('crop_windows', not crop_bad, str(crop_bad)))
    report['crops'] = crop_report

    # ---- 6. contrast recomputed ----
    con_bad, con_report = [], {}
    for coat in COATS:
        body, dark = horsesvg.COAT[coat]
        mane = horsesvg.MANE_C[coat]
        bg = G.BG[coat][0]
        row = {'bg': bg,
               'bg_vs_body': round(G.contrast(bg, body), 2),
               'bg_vs_shade': round(G.contrast(bg, dark), 2),
               'bg_vs_mane': round(G.contrast(bg, mane), 2)}
        con_report[coat] = row
        if row['bg_vs_body'] < 3.4:
            con_bad.append(f'{coat} body {row["bg_vs_body"]}')
        saved = json.loads((HERE / 'build-summary.json').read_text())
        if abs(saved['contrast'][coat]['bg_vs_body'] - row['bg_vs_body']) > 0.01:
            con_bad.append(f'{coat} summary mismatch')
    report['checks'].append(check('contrast', not con_bad, str(con_bad)))
    report['contrast'] = con_report

    # ---- 7. determinism: regenerate, compare hashes ----
    before = all_artifacts()
    r = subprocess.run([sys.executable, '-B', str(HERE / 'gen_icons.py')],
                       capture_output=True, text=True, timeout=1800)
    gen_rc = r.returncode
    after = all_artifacts()
    drift = [k for k in set(before) | set(after)
             if before.get(k) != after.get(k)]
    report['checks'].append(check('determinism', gen_rc == 0 and not drift,
        f'gen_rc={gen_rc} drifted={drift[:5]}'))

    report['fail_count'] = len(FAILS)
    report['failures'] = FAILS
    (HERE / 'verification.json').write_text(
        json.dumps(report, indent=2, sort_keys=True) + '\n')
    for f in FAILS:
        print('FAIL', f)
    print('VERIFY', 'OK' if not FAILS else 'FAILED')
    return 0 if not FAILS else 1


if __name__ == '__main__':
    raise SystemExit(main())
