#!/usr/bin/env python3
"""issue-442 render — capture every stage HTML to an exact 390x844 PNG.

    PYTHONDONTWRITEBYTECODE=1 python3 scripts/render.py

Renderer: chrome-headless-shell (Playwright cache; no install step).
Method: one headless invocation per stage (chaining hangs this build),
--window-size=390,844 --force-device-scale-factor=2 -> 780x1688
intermediate in a temp dir, then `sips -z 844 390` down to the exact
390x844 standard. The stage HTML pins html/body to 390x844 + overflow
hidden so the document canvas equals the viewport (the full-canvas trap).
Output dimensions are verified by reading the PNG IHDR — the flag is never
trusted. Exit non-zero if any PNG is not exactly 390x844.
"""
from __future__ import annotations

import argparse
import glob
import json
import os
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
EVID = HERE.parent
assert EVID.name == "issue-442", EVID
W, H = 390, 844


def find_shell() -> str:
    env = os.environ.get("CHROME_HEADLESS_SHELL")
    if env and Path(env).exists():
        return env
    pats = sorted(glob.glob(os.path.expanduser(
        "~/Library/Caches/ms-playwright/chromium_headless_shell-*/"
        "chrome-headless-shell-mac-arm64/chrome-headless-shell")),
        key=lambda p: os.stat(p).st_mtime, reverse=True)
    if not pats:
        sys.exit("chrome-headless-shell not found; set CHROME_HEADLESS_SHELL")
    return pats[0]


def png_size(path: Path) -> tuple[int, int]:
    with open(path, "rb") as fh:
        head = fh.read(24)
    assert head[:8] == b"\x89PNG\r\n\x1a\n", f"not a PNG: {path}"
    w, h = struct.unpack(">II", head[16:24])
    return w, h


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-dir", type=Path, default=EVID)
    args = parser.parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    shell = find_shell()
    specs = json.loads((EVID / "stage" / "specs.json").read_text())
    print(f"renderer: {shell}")
    print(f"stages: {len(specs)}")
    print("capture pose: CSS paused from first style resolution; delay=-500ms; transitions disabled")
    bad = []
    with tempfile.TemporaryDirectory() as tmp:
        for spec in specs:
            name = spec["name"]
            src = EVID / "stage" / f"{name}.html"
            # Capture-only copy: never disable motion in the shipped HTML.
            # Paused before first paint, fixed active time 500ms; RM animation:none survives.
            source = src.read_text()
            assert "<script" not in source.lower(), "timed JS needs explicit capture control"
            assert source.count("</head>") == 1
            frozen = Path(tmp) / f"{name}.html"
            frozen.write_text(source.replace("</head>", "<style>*,*::before,*::after{animation-play-state:paused!important;animation-delay:-500ms!important;transition:none!important}</style></head>"))
            src = frozen
            out2x = Path(tmp) / f"{name}-2x.png"
            out = args.output_dir / f"{name}-{W}x{H}.png"
            udd = tempfile.mkdtemp(prefix="hs-", dir=tmp)
            cmd = [shell, "--headless", "--disable-gpu", "--no-first-run",
                   "--hide-scrollbars", f"--user-data-dir={udd}",
                   f"--window-size={W},{H}", "--force-device-scale-factor=2",
                   "--virtual-time-budget=2500",
                   f"--screenshot={out2x}", f"file://{src}"]
            r = subprocess.run(cmd, capture_output=True, text=True)
            if r.returncode != 0 or not out2x.exists():
                print(f"FAIL render {name}: rc={r.returncode}\n{r.stderr[-800:]}")
                bad.append(name)
                continue
            w2, h2 = png_size(out2x)
            if (w2, h2) != (W * 2, H * 2):
                print(f"FAIL 2x size {name}: {w2}x{h2} (expected {W*2}x{H*2})")
                bad.append(name)
                continue
            r2 = subprocess.run(["sips", "-z", str(H), str(W), str(out2x),
                                 "--out", str(out)], capture_output=True,
                                text=True)
            if r2.returncode != 0:
                print(f"FAIL sips {name}: {r2.stderr[-400:]}")
                bad.append(name)
                continue
            w, h = png_size(out)
            status = "OK " if (w, h) == (W, H) else "BAD"
            if status == "BAD":
                bad.append(name)
            print(f"{status} {out.name}  {w}x{h}  (2x {w2}x{h2})")
    if bad:
        print(f"RENDER FAIL: {len(bad)} stage(s): {bad}")
        return 1
    print(f"RENDER OK: {len(specs)} PNGs at exactly {W}x{H}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
