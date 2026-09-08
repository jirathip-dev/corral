#!/usr/bin/env python3
"""issue-442 measure — the phone-readability / scalability numbers quoted in
README.md, read from the exact stage DOM the PNGs were captured from.

    PYTHONDONTWRITEBYTECODE=1 python3 scripts/measure.py
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
EVID = HERE.parent
sys.path.insert(0, str(HERE))
from verify import Browser  # noqa: E402

STAGES = ["herd-v1-mocha-blocked-heavy", "herd-v2-mocha-blocked-heavy",
          "herd-v1-mocha-dense-fleet", "herd-v2-mocha-dense-fleet"]

JS = """(() => {
  const vh = 844, vw = 390;
  const inView = el => { const r = el.getBoundingClientRect();
    return r.bottom > 0 && r.top < vh && r.right > 0 && r.left < vw && r.width > 0; };
  const horses = Array.from(document.querySelectorAll('.horse-btn'));
  const blocked = horses.filter(h => h.dataset.state === 'blocked');
  const pads = Array.from(document.querySelectorAll('.paddock'));
  const heads = pads.map(p => p.querySelector('.paddock-name')).filter(inView);
  const np = Array.from(document.querySelectorAll('.nameplate')).filter(inView)
    .map(n => parseFloat(getComputedStyle(n).fontSize));
  const svgW = Array.from(document.querySelectorAll('.horse-svg')).filter(inView)
    .map(s => Math.round(s.getBoundingClientRect().width));
  const strip = document.querySelector('.paddock-strip');
  const map = document.querySelector('.map');
  const small = Array.from(document.querySelectorAll('button,.opt')).filter(inView)
    .filter(e => { const r = e.getBoundingClientRect(); return r.width < 43.5 || r.height < 43.5; }).length;
  return {horsesTotal: horses.length, horsesVisibleFirstScreen: horses.filter(inView).length,
          blockedTotal: blocked.length, blockedVisibleFirstScreen: blocked.filter(inView).length,
          paddocksTotal: pads.length, paddockHeadersVisible: heads.length,
          nameplateFontPx: [...new Set(np)].sort(), horseWidthsPx: [...new Set(svgW)].sort((a,b)=>a-b),
          swipesToSeeAllPaddocks: strip ? pads.length - 1 : 0,
          verticalScreens: map ? +(map.scrollHeight / map.clientHeight).toFixed(2) : 1,
          showAllRows: document.querySelectorAll('.more').length,
          targetsUnder44: small};
})()"""


def main() -> int:
    b = Browser()
    out = {}
    try:
        for name in STAGES:
            b.goto(EVID / "stage" / f"{name}.html")
            out[name] = b.js(JS)
            print(name, json.dumps(out[name]))
    finally:
        b.close()
    print("MEASURE OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
