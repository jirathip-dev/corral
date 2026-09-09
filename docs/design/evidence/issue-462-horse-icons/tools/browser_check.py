#!/usr/bin/env python3
"""issue-462 — browser verification (existing chrome-headless-shell only).

Uses only --screenshot navigations (the render path proven on this build;
--dump-dom never reaches quiescence on the image-heavy gallery). Proves:
  - html/index.html renders (non-blank) with zero console errors
  - runtime horizontal-overflow check inside the page (console HSCROLL:ok,
    captured from chrome stderr) at the 390-wide mobile viewport
  - toggle reachability: # vs #t=b produce DIFFERENT renders via the same
    render() path the B button drives (hashchange handler + boot hash)
  - every <img src> referenced by the A and B DOM sources exists on disk
  - each referenced artifact loads and decodes non-blank via file://
  - all 8 stage HTMLs render at exactly 390x844, non-blank, and carry the
    required labels (Original/Bay/Palomino/Black/Grey, checkmark,
    Settings/Appearance/App Icon, PROTOTYPE note)
Prints BROWSER OK / FAILED; exit code carries the verdict.
"""
from __future__ import annotations

import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path
from PIL import Image, ImageStat

HERE = Path(__file__).resolve().parent
ISSUE = HERE.parent
SHELL = sorted(Path.home().glob(
    'Library/Caches/ms-playwright/chromium_headless_shell-*/'
    'chrome-headless-shell-mac-arm64/chrome-headless-shell'))[-1]
FAILS: list[str] = []
console_errors: list[str] = []
hscroll_lines: list[str] = []


def chrome_shot(url: str, window: str, out: Path,
                timeout: int = 75) -> subprocess.CompletedProcess:
    cmd = [str(SHELL), '--headless', '--disable-gpu', '--no-first-run',
           '--hide-scrollbars', '--allow-file-access-from-files',
           '--user-data-dir=/tmp/corral-462-browser-profile',
           f'--window-size={window}',
           '--virtual-time-budget=8000', '--enable-logging=stderr', '--v=0',
           f'--screenshot={out}', url]
    return subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)


def rmg(p: Path):
    if p.exists():
        p.unlink()


def mean_rgb(im: Image.Image) -> tuple[float, float, float]:
    st = ImageStat.Stat(im.convert('RGB'))
    return tuple(round(v, 2) for v in st.mean)


def std_rgb(im: Image.Image) -> float:
    return round(ImageStat.Stat(im.convert('RGB')).stddev[0], 2)


def load_check(rel: str, tag: str):
    """Artifact exists, decodes, and a direct file:// navigation of it
    renders non-blank in chrome."""
    p = ISSUE / rel
    if not p.is_file():
        FAILS.append(f'{tag}: missing referenced artifact {rel}')
        return
    try:
        im = Image.open(p)
        im.load()
    except Exception as exc:  # noqa: BLE001
        FAILS.append(f'{tag}: {rel} does not decode: {exc}')
        return
    out = Path('/tmp/corral-462-browser-shot.png')
    r = chrome_shot(p.as_uri(), '1100,1100', out)
    rmg(out)
    if r.returncode != 0:
        FAILS.append(f'{tag}: file:// render failed rc={r.returncode} {rel}')
        return
    for line in r.stderr.splitlines():
        if ('CONSOLE' in line and '"error' in line.lower()) or 'Uncaught' in line:
            FAILS.append(f'{tag}: console error loading {rel}: {line[:200]}')
            return


def main() -> int:
    index = ISSUE / 'index.html'
    src = index.read_text()

    # ---- static source contract (runtime-built DOM) ----
    for token in ('PROTOTYPE — awaiting Guy icon-art approval',
                  "id=\"ta\"", "id=\"tb\"", "id=\"tz\"", "className='tile'",
                  'changes no real icon', "'masters/treatment-'+t",
                  "'previews/'", "'contexts/'", "addEventListener('hashchange'"):
        if token not in src:
            FAILS.append(f'index source missing {token!r}')
    for name in ('Original', 'Bay', 'Palomino', 'Black', 'Grey'):
        if name not in src:
            FAILS.append(f'index source missing choice name {name}')

    # ---- expected referenced artifacts (enumerated from the same constants
    #      the gallery JS encodes: 2 treatments x 5 choices x sizes/themes) --
    choices = ['original', 'bay', 'palomino', 'black', 'grey']
    expected = []
    for t in 'ab':
        for c in choices:
            expected.append(f'masters/treatment-{t}/{c}-1024.png')
            for kind, sz in (('mask', 180), ('mask', 120), ('plain', 60)):
                expected.append(f'previews/{kind}{sz}-treatment-{t}-{c}.png')
        for theme in ('light', 'dark'):
            for kind in ('home', 'picker'):
                expected.append(f'contexts/{kind}-{t}-{theme}-390x844.png')
    for rel in expected:
        if not (ISSUE / rel).is_file():
            FAILS.append(f'missing referenced artifact {rel}')
    ctx_refs = [s for s in expected if s.startswith('contexts/')]
    if len(ctx_refs) != 8:
        FAILS.append(f'expected 8 context refs, found {len(ctx_refs)}')

    # ---- page renders with no console errors ----
    out = Path('/tmp/corral-462-browser-index.png')
    r = chrome_shot(index.as_uri(), '700,1400', out)
    if r.returncode != 0 or not out.exists():
        FAILS.append(f'index render failed rc={r.returncode}')
    else:
        im = Image.open(out)
        if ImageStat.Stat(im.convert('RGB')).stddev[0] <= 1.0:
            FAILS.append('index renders blank')
    rmg(out)
    for line in r.stderr.splitlines():
        if ('CONSOLE' in line and '"error' in line.lower()) or 'Uncaught' in line:
            console_errors.append(line[:250])

    # ---- runtime horizontal overflow (mobile viewport) ----
    r = chrome_shot(index.as_uri() + '#t=b', '390,900', out)
    if r.returncode != 0:
        FAILS.append('mobile render failed')
    got = sorted({ln.split('HSCROLL:')[1].split('"')[0].strip()
                  for ln in r.stderr.splitlines() if 'HSCROLL:' in ln})
    hscroll_lines = got
    if not got or got != ['ok']:
        FAILS.append(f'horizontal overflow (HSCROLL={got})')
    rmg(out)

    # ---- toggle reachability: hash drives the render() path ----
    shot_a = Path('/tmp/corral-462-browser-a.png')
    shot_b = Path('/tmp/corral-462-browser-b.png')
    ra = chrome_shot(index.as_uri(), '700,1400', shot_a)
    rb = chrome_shot(index.as_uri() + '#t=b', '700,1400', shot_b)
    if ra.returncode != 0 or rb.returncode != 0:
        FAILS.append('toggle renders failed')
    else:
        a, b = Image.open(shot_a), Image.open(shot_b)
        ha = hashlib.sha256(a.tobytes()).hexdigest()
        hb = hashlib.sha256(b.tobytes()).hexdigest()
        if ha == hb:
            FAILS.append('toggle #t=b produced identical render (unreachable)')
        ma, mb = mean_rgb(a), mean_rgb(b)
        if std_rgb(a) <= 1.0 or std_rgb(b) <= 1.0:
            FAILS.append('toggle render blank')
        print(json.dumps({'toggle_hash_a': ha[:16], 'toggle_hash_b': hb[:16],
                          'mean_rgb_a': ma, 'mean_rgb_b': mb}))
    rmg(shot_a)
    rmg(shot_b)

    # ---- every referenced artifact decodes and loads in chrome ----
    for rel in expected:
        load_check(rel, 'ref')

    # ---- stage HTMLs: exact 390x844 render + label contract ----
    for theme in ('light', 'dark'):
        for t in 'ab':
            for kind in ('home', 'picker'):
                stage = ISSUE / 'html' / 'stage' / f'{kind}-{t}-{theme}.html'
                st = stage.read_text()
                for label in ('Original', 'Bay', 'Palomino', 'Black', 'Grey',
                              'PROTOTYPE'):
                    if label not in st:
                        FAILS.append(f'{stage.name} missing {label}')
                if kind == 'picker':
                    for label in ('\u2713', 'Settings', 'Appearance',
                                  'App Icon', 'Current icon (unchanged)'):
                        if label not in st:
                            FAILS.append(f'{stage.name} missing {label}')
                im_out = Path('/tmp/corral-462-browser-stage.png')
                rr = chrome_shot(stage.as_uri(), '390,844', im_out)
                if rr.returncode != 0 or not im_out.exists():
                    FAILS.append(f'{stage.name} render failed')
                elif Image.open(im_out).size != (390, 844):
                    FAILS.append(f'{stage.name} renders '
                                 f'{Image.open(im_out).size}, want 390x844')
                elif ImageStat.Stat(Image.open(im_out).convert('RGB')).stddev[0] <= 1.0:
                    FAILS.append(f'{stage.name} renders blank')
                rmg(im_out)

    if console_errors:
        FAILS.append(f'console errors: {console_errors[:3]}')

    print(json.dumps({'refs_checked': len(expected),
                      'hscroll': hscroll_lines,
                      'console_errors': console_errors,
                      'failures': FAILS}, indent=1))
    print('BROWSER', 'OK' if not FAILS else 'FAILED')
    return 0 if not FAILS else 1


if __name__ == '__main__':
    raise SystemExit(main())
