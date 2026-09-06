#!/usr/bin/env python3
"""Capture the full Corral #427 design evidence matrix."""
from __future__ import annotations

import argparse
import concurrent.futures
import os
import shutil
import subprocess
import tempfile
from pathlib import Path
from urllib.parse import urlencode

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
INDEX = ROOT / "index.html"
SHOT_ROOT = ROOT / "screenshots"
VARIANTS = ("A", "B", "C")
PALETTES = ("latte", "frappe", "macchiato", "mocha")
WIDTHS = ((390, 844), (375, 812))
STATES = (
    "populated",
    "host-only",
    "repository-only",
    "both-filters",
    "connecting-host",
    "offline-stale-host",
    "zero-results",
)


def chrome_shell() -> Path:
    candidates = sorted(
        Path.home().glob(
            "Library/Caches/ms-playwright/chromium_headless_shell-*/"
            "chrome-headless-shell-mac-arm64/chrome-headless-shell"
        ),
        key=lambda p: p.stat().st_mtime,
        reverse=True,
    )
    if not candidates:
        raise SystemExit("chrome-headless-shell is not installed in the Playwright cache")
    return candidates[0]


def capture_one(spec: dict, chrome: Path, force: bool) -> tuple[str, str]:
    out = spec["out"]
    width, height = spec["width"], spec["height"]
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.exists() and not force:
        with Image.open(out) as image:
            if image.size == (width, height):
                return str(out.relative_to(ROOT)), "cached"
    url = spec["source"].resolve().as_uri()
    if spec.get("params"):
        url += "?" + urlencode(spec["params"])
    with tempfile.TemporaryDirectory(prefix="corral-427-chrome-") as user_data:
        cmd = [
            str(chrome),
            "--headless",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--hide-scrollbars",
            "--run-all-compositor-stages-before-draw",
            "--virtual-time-budget=900",
            "--force-device-scale-factor=1",
            f"--user-data-dir={user_data}",
            f"--window-size={width},{height}",
            f"--screenshot={out}",
            url,
        ]
        result = subprocess.run(cmd, text=True, capture_output=True, timeout=40)
        if result.returncode != 0:
            raise RuntimeError(
                f"capture failed for {out.name}: {result.stderr.strip() or result.stdout.strip()}"
            )
    if not out.exists():
        raise RuntimeError(f"renderer did not create {out}")
    with Image.open(out) as image:
        if image.size != (width, height):
            raise RuntimeError(f"{out.name}: expected {(width, height)}, got {image.size}")
    return str(out.relative_to(ROOT)), "rendered"


def matrix_specs() -> list[dict]:
    specs: list[dict] = []
    for variant in VARIANTS:
        for palette in PALETTES:
            for width, height in WIDTHS:
                for state in STATES:
                    label = (
                        f"variant-{variant.lower()}__palette-{palette}__"
                        f"width-{width}x{height}__state-{state}.png"
                    )
                    specs.append(
                        {
                            "source": INDEX,
                            "params": {
                                "capture": "1",
                                "variant": variant,
                                "palette": palette,
                                "state": state,
                            },
                            "width": width,
                            "height": height,
                            "out": SHOT_ROOT / "matrix" / label,
                        }
                    )
    return specs


def accessibility_specs() -> list[dict]:
    specs: list[dict] = []
    for variant in VARIANTS:
        for width, height, palette in ((390, 844, "latte"), (375, 812, "mocha")):
            label = (
                f"variant-{variant.lower()}__palette-{palette}__width-{width}x{height}__"
                "state-both-filters__dynamic-type-accessibility.png"
            )
            specs.append(
                {
                    "source": INDEX,
                    "params": {
                        "capture": "1",
                        "variant": variant,
                        "palette": palette,
                        "state": "both-filters",
                        "a11y": "1",
                        "forceOpen": "1",
                        "menu": "repo" if width == 375 else "host",
                        "scrollEnd": "1" if width == 375 else "0",
                    },
                    "width": width,
                    "height": height,
                    "out": SHOT_ROOT / "accessibility" / label,
                }
            )
    return specs


def comparison_spec() -> dict:
    return {
        "source": ROOT / "comparison.html",
        "params": {},
        "width": 1440,
        "height": 1080,
        "out": SHOT_ROOT / "comparison" / "abc-comparison__palette-mocha__width-1440x1080.png",
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--sample", action="store_true", help="capture one representative frame")
    parser.add_argument("--force", action="store_true", help="recapture existing outputs")
    parser.add_argument("--workers", type=int, default=6)
    args = parser.parse_args()
    chrome = chrome_shell()
    specs = matrix_specs() + accessibility_specs() + [comparison_spec()]
    if args.sample:
        specs = [s for s in specs if "variant-a__palette-mocha__width-390x844__state-both-filters" in s["out"].name]
    completed: list[tuple[str, str]] = []
    failures: list[str] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.workers)) as pool:
        futures = {pool.submit(capture_one, spec, chrome, args.force): spec for spec in specs}
        for future in concurrent.futures.as_completed(futures):
            spec = futures[future]
            try:
                completed.append(future.result())
            except Exception as exc:
                failures.append(f"{spec['out'].name}: {exc}")
    log = [
        "Corral #427 deterministic capture log",
        f"renderer: {chrome}",
        "command shape: chrome-headless-shell --headless --force-device-scale-factor=1 "
        "--window-size=W,H --screenshot=OUT file:///index.html?capture=1&...",
        f"requested: {len(specs)}",
        f"completed: {len(completed)}",
        f"failures: {len(failures)}",
        "",
    ]
    log.extend(f"{status}: {path}" for path, status in sorted(completed))
    if failures:
        log.extend(["", "FAILURES", *failures])
    (ROOT / "capture.log").write_text("\n".join(log) + "\n", encoding="utf-8")
    print(f"captured {len(completed)}/{len(specs)}; failures={len(failures)}")
    if failures:
        print("\n".join(failures))
        raise SystemExit(1)


if __name__ == "__main__":
    main()
