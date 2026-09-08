#!/usr/bin/env python3
"""issue-442 — exact 390x844 dimension + count verifier (independent of
render.py and verify.py: reads the PNG IHDR directly, no renderer).

    PYTHONDONTWRITEBYTECODE=1 python3 scripts/check-dimensions.py
"""
from __future__ import annotations

import json
import struct
import sys
from pathlib import Path

EVID = Path(__file__).resolve().parent.parent
W, H = 390, 844
REQUIRED = [f"herd-{v}-{p}-{f}-{W}x{H}.png"
            for v in ("v1", "v2") for p in ("mocha", "latte")
            for f in ("blocked-heavy", "dense-fleet", "disconnected")]


def size(p: Path):
    with open(p, "rb") as fh:
        head = fh.read(24)
    if head[:8] != b"\x89PNG\r\n\x1a\n":
        return None
    return struct.unpack(">II", head[16:24])


def main() -> int:
    pngs = sorted(EVID.glob(f"*-{W}x{H}.png"))
    specs = json.loads((EVID / "stage" / "specs.json").read_text())
    bad = 0
    for p in pngs:
        s = size(p)
        flag = "OK " if s == (W, H) else "BAD"
        bad += flag == "BAD"
        print(f"{flag} {p.name} {s[0]}x{s[1]}" if s else f"BAD {p.name} not-png")
    missing = [r for r in REQUIRED if not (EVID / r).exists()]
    print(f"count: {len(pngs)} PNGs (stage specs: {len(specs)})")
    print(f"required 12: {12 - len(missing)}/12 present"
          + (f" MISSING {missing}" if missing else ""))
    if bad or missing or len(pngs) != len(specs):
        print("DIMENSIONS FAIL")
        return 1
    print(f"DIMENSIONS OK: {len(pngs)}/{len(pngs)} exactly {W}x{H}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
