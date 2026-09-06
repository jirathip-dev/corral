#!/usr/bin/env python3
"""#416 translucent-sheet evidence analysis (stdlib + Pillow only).

What the iOS 26.5 SIMULATOR can and cannot show (established by the #416
mechanism probes documented in the README):

- UIKit on this runtime does NOT composite the presenting view behind the
  sheet card — the region under the card shows only the app window's flat
  backdrop, so NO backdrop recipe can make board rows appear "through" the
  sheet interior ON THIS SIM. The recipes are therefore verified by
  (a) the code contract (real glass/material layers, no opaque fills —
     SheetTranslucencyWiringTests + SheetBackdropTests),
  (b) the surface-tone measurements below (the fallback material visibly
     frosts the surface; the glass branch tracks the base tone),
  (c) real-device / iOS-17-25 rendering, where the presenter content IS
     composited behind the sheet — the re-locked low tints are what make
     the through-show perceptible there (this script cannot capture it).

Measurements here:

1. geometry: each sheet frame's top edge + the busy board above it
   (medium detent evidence runs).
2. branch A/B on IDENTICAL geometry: native-glass vs forced-fallback
   surface tone over the same sheet region — the fallback material
   (ultraThinMaterial + 80 % tint) must render measurably lighter/frosted
   vs the glass tint (mean delta >> 0 on the light flavor).
3. opaque control: an opaque whole-sheet `theme.base` fill would be flat
   base tone everywhere; card interiors (opaque, intentional) stay flat
   single tones (hierarchy control).
"""
import math
import statistics
import sys
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent


def region_values(img, y0f, y1f, x0f, x1f):
    w, h = img.size
    vals = []
    for y in range(int(h * y0f), int(h * y1f), 3):
        for x in range(int(w * x0f), int(w * x1f), 5):
            r, g, b = img.getpixel((x, y))
            vals.append(r + g + b)
    return vals


def stats(vals):
    return (round(statistics.mean(vals), 1), round(statistics.pstdev(vals), 1))


def load(name):
    return Image.open(HERE / f"{name}-390x844.png").convert("RGB")


def main():
    out = []
    def log(s):
        print(s)
        out.append(s)

    # 1. Sheet geometry + busy board above (native glass run).
    for name, flavor in (("phase-2-recents-mocha", "mocha"),
                         ("phase-3-recents-latte", "latte"),
                         ("phase-5-settings-mocha", "mocha"),
                         ("phase-6-settings-latte", "latte"),
                         ("phase-7-addhost-entry-mocha", "mocha"),
                         ("phase-8-addhost-confirm-latte", "latte")):
        img = load(name)
        w, h = img.size
        top = None
        for y in range(int(h * 0.28), int(h * 0.72), 2):
            xs = []
            for x in range(int(w * 0.35), int(w * 0.65), 2):
                p = img.getpixel((x, y))
                if abs(p[0] - p[1]) < 30 and abs(p[1] - p[2]) < 30 and 90 < sum(p) / 3 < 215:
                    xs.append(x)
            if xs and max(xs) - min(xs) > int(w * 0.06):
                top = y / h
                break
        # busy content evidence: the 0.30-0.44 strip must carry strong tonal
        # structure (board rows and/or the underlying sheet content the
        # presented sheet floats over), not a flat empty backdrop.
        above = stats(region_values(img, 0.30, 0.44, 0.05, 0.95))
        log(f"[geometry {name}] sheet top ~{top:.0%} of height; "
            f"0.30-0.44 strip stdev={above[1]} (busy content -> large)")

    # 2. Branch A/B on identical geometry: native glass vs forced fallback.
    for name, y0, y1, label in (
            ("phase-2-recents-mocha", 0.845, 0.855, "recents mocha inter-card gap"),
            ("phase-3-recents-latte", 0.845, 0.855, "recents latte inter-card gap"),
            ("phase-5-settings-mocha", 0.60, 0.90, "settings mocha form surface"),
            ("phase-6-settings-latte", 0.60, 0.90, "settings latte form surface")):
        glass = stats(region_values(load(name), y0, y1, 0.10, 0.90))
        fb = stats(region_values(load(f"{name}-fallback"), y0, y1, 0.10, 0.90))
        delta = fb[0] - glass[0]
        log(f"[branch A/B {label}] glass mean={glass[0]} stdev={glass[1]} | "
            f"fallback mean={fb[0]} stdev={fb[1]} | mean delta={delta:+.1f} "
            f"({'frosted material lift' if delta > 3 else 'delta within noise'})")

    # 3. Opaque control: card interiors stay flat single tones (opaque
    # content surfaces preserve the hierarchy).
    for name, label, probe in (
            ("phase-2-recents-mocha", "mocha block card", (0.5, 0.72)),
            ("phase-3-recents-latte", "latte block card", (0.5, 0.75))):
        img = load(name)
        w, h = img.size
        x, y = int(w * probe[0]), int(h * probe[1])
        vals = [sum(img.getpixel((x, yy))) for yy in range(y - 2, y + 3)]
        log(f"[opaque control {label}] card tone stdev={round(statistics.pstdev(vals), 1)} "
            f"(opaque card ~ flat)")

    (HERE / "analysis.txt").write_text("\n".join(out) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
