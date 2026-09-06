#!/usr/bin/env python3
"""Verify Corral #427 prototypes, matrix, accessibility, and hashes."""
from __future__ import annotations

import argparse
import hashlib
import html
import json
import re
import subprocess
import tempfile
from pathlib import Path
from urllib.parse import urlencode

from PIL import Image, ImageDraw, ImageFont

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
PALETTE = {
    "latte": {
        "base": "#eff1f5", "mantle": "#e6e9ef", "crust": "#dce0e8",
        "surface0": "#ccd0da", "surface1": "#bcc0cc", "surface2": "#acb0be",
        "text": "#4c4f69", "subtext1": "#5c5f77", "mauve": "#8839ef",
    },
    "frappe": {
        "base": "#303446", "mantle": "#292c3c", "crust": "#232634",
        "surface0": "#414559", "surface1": "#51576d", "surface2": "#626880",
        "text": "#c6d0f5", "subtext1": "#b5bfe2", "mauve": "#ca9ee6",
    },
    "macchiato": {
        "base": "#24273a", "mantle": "#1e2030", "crust": "#181926",
        "surface0": "#363a4f", "surface1": "#494d64", "surface2": "#5b6078",
        "text": "#cad3f5", "subtext1": "#b8c0e0", "mauve": "#c6a0f6",
    },
    "mocha": {
        "base": "#1e1e2e", "mantle": "#181825", "crust": "#11111b",
        "surface0": "#313244", "surface1": "#45475a", "surface2": "#585b70",
        "text": "#cdd6f4", "subtext1": "#bac2de", "mauve": "#cba6f7",
    },
}


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
        raise RuntimeError("chrome-headless-shell unavailable")
    return candidates[0]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def relative_luminance(value: str) -> float:
    channels = [int(value[index:index + 2], 16) / 255 for index in (1, 3, 5)]
    linear = [channel / 12.92 if channel <= 0.04045 else ((channel + 0.055) / 1.055) ** 2.4 for channel in channels]
    return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]


def contrast(first: str, second: str) -> float:
    high, low = sorted((relative_luminance(first), relative_luminance(second)), reverse=True)
    return (high + 0.05) / (low + 0.05)


def contrast_checks() -> dict:
    pairs = (
        ("text", "base"),
        ("text", "mantle"),
        ("subtext1", "base"),
        ("subtext1", "mantle"),
        ("base", "mauve"),
        ("mauve", "base"),
    )
    checks = []
    for palette_name, tokens in PALETTE.items():
        for foreground, background in pairs:
            ratio = contrast(tokens[foreground], tokens[background])
            checks.append(
                {
                    "palette": palette_name,
                    "foreground": foreground,
                    "background": background,
                    "ratio": round(ratio, 2),
                    "minimum": 4.5,
                    "pass": ratio >= 4.5,
                }
            )
        # The existing dark flavors use surface1 for thick status headers.
        # Latte moves one token lighter to surface0 so normal-size text holds AA.
        status_background = "surface0" if palette_name == "latte" else "surface1"
        ratio = contrast(tokens["text"], tokens[status_background])
        checks.append(
            {
                "palette": palette_name,
                "foreground": "text",
                "background": status_background,
                "role": "status header",
                "ratio": round(ratio, 2),
                "minimum": 4.5,
                "pass": ratio >= 4.5,
            }
        )
    result = {"status": "pass" if all(item["pass"] for item in checks) else "fail", "checks": checks}
    (ROOT / "contrast-checks.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


def dump_dom(params: dict, width: int, height: int, chrome: Path) -> str:
    url = INDEX.resolve().as_uri() + "?" + urlencode(params)
    with tempfile.TemporaryDirectory(prefix="corral-427-dom-") as user_data:
        command = [
            str(chrome), "--headless", "--disable-gpu", "--no-first-run",
            "--hide-scrollbars", "--run-all-compositor-stages-before-draw",
            "--virtual-time-budget=1200", f"--user-data-dir={user_data}",
            f"--window-size={width},{height}", "--force-device-scale-factor=1",
            "--dump-dom", url,
        ]
        result = subprocess.run(command, text=True, capture_output=True, timeout=45)
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or "DOM render failed")
    return result.stdout


def html_data(dom: str) -> dict[str, str]:
    match = re.search(r"<html\b([^>]*)>", dom, re.IGNORECASE)
    if not match:
        return {}
    return {
        key: html.unescape(value)
        for key, value in re.findall(r'\b(data-[\w-]+)="([^"]*)"', match.group(1))
    }


def dom_checks(chrome: Path) -> dict:
    required = {
        "data-console-errors": "0",
        "data-hit-targets": "pass",
        "data-horizontal-overflow": "pass",
        "data-scope-order": "pass",
        "data-voiceover-scopes": "pass",
        "data-reset-visible": "pass",
        "data-scroll-reach": "pass",
        "data-selftest": "ready",
    }
    cases = []
    failures = []
    for variant in VARIANTS:
        for width, height in WIDTHS:
            for state in STATES:
                params = {"capture": "1", "variant": variant, "palette": "mocha", "state": state}
                cases.append((f"default:{variant}:{width}x{height}:{state}", params, width, height, False))
            cases.append((f"forced:{variant}:{width}x{height}", {"capture": "1", "variant": variant, "palette": "latte", "state": "both-filters", "forceOpen": "1"}, width, height, False))
            cases.append((f"a11y:{variant}:{width}x{height}", {"capture": "1", "variant": variant, "palette": "mocha", "state": "both-filters", "forceOpen": "1", "a11y": "1"}, width, height, False))
            cases.append((f"interaction:{variant}:{width}x{height}", {"capture": "1", "variant": variant, "palette": "mocha", "state": "populated", "forceOpen": "1", "selftest": "1"}, width, height, True))
    records = []
    for name, params, width, height, interaction in cases:
        data = {}
        for _attempt in range(3):
            data = html_data(dump_dom(params, width, height, chrome))
            if data.get("data-selftest") == "ready" and (
                not interaction or data.get("data-interaction-gate") in {"pass", "fail"}
            ):
                break
        bad = {key: {"expected": value, "actual": data.get(key)} for key, value in required.items() if data.get(key) != value}
        if interaction and data.get("data-interaction-gate") != "pass":
            bad["data-interaction-gate"] = {"expected": "pass", "actual": data.get("data-interaction-gate")}
        passed = not bad
        records.append({"case": name, "pass": passed, "failures": bad})
        if bad:
            failures.append({"case": name, "failures": bad})
    result = {"status": "pass" if not failures else "fail", "case_count": len(records), "failures": failures, "cases": records}
    (ROOT / "dom-verification.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


def matrix_path(variant: str, palette: str, width: int, height: int, state: str) -> Path:
    return SHOT_ROOT / "matrix" / (
        f"variant-{variant.lower()}__palette-{palette}__width-{width}x{height}__state-{state}.png"
    )


def image_checks() -> dict:
    expected = []
    for variant in VARIANTS:
        for palette in PALETTES:
            for width, height in WIDTHS:
                for state in STATES:
                    expected.append((matrix_path(variant, palette, width, height, state), (width, height), "matrix"))
    for variant in VARIANTS:
        for width, height, palette in ((390, 844, "latte"), (375, 812, "mocha")):
            path = SHOT_ROOT / "accessibility" / (
                f"variant-{variant.lower()}__palette-{palette}__width-{width}x{height}__"
                "state-both-filters__dynamic-type-accessibility.png"
            )
            expected.append((path, (width, height), "accessibility"))
    expected.append((SHOT_ROOT / "comparison" / "abc-comparison__palette-mocha__width-1440x1080.png", (1440, 1080), "comparison"))
    failures = []
    by_kind = {"matrix": 0, "accessibility": 0, "comparison": 0}
    for path, size, kind in expected:
        if not path.exists():
            failures.append({"path": str(path.relative_to(ROOT)), "error": "missing"})
            continue
        with Image.open(path) as image:
            if image.size != size:
                failures.append({"path": str(path.relative_to(ROOT)), "error": f"expected {size}, got {image.size}"})
            else:
                by_kind[kind] += 1
    return {"status": "pass" if not failures else "fail", "expected": len(expected), "verified": sum(by_kind.values()), "counts": by_kind, "failures": failures}


def font(size: int) -> ImageFont.ImageFont:
    candidates = [
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
    ]
    for candidate in candidates:
        try:
            return ImageFont.truetype(candidate, size)
        except OSError:
            pass
    return ImageFont.load_default()


def contact_sheets() -> list[str]:
    out_dir = SHOT_ROOT / "contact-sheets"
    out_dir.mkdir(parents=True, exist_ok=True)
    outputs = []
    label_font = font(15)
    row_font = font(18)
    for variant in VARIANTS:
        for width, height in WIDTHS:
            scale = 0.44
            tw, th = int(width * scale), int(height * scale)
            gap, top, left = 8, 28, 92
            canvas = Image.new("RGB", (left + len(STATES) * (tw + gap) + gap, 18 + len(PALETTES) * (top + th + gap)), "#11111b")
            draw = ImageDraw.Draw(canvas)
            for column, state in enumerate(STATES):
                draw.text((left + column * (tw + gap) + 2, 4), state.replace("-", " "), fill="#cdd6f4", font=label_font)
            for row, palette in enumerate(PALETTES):
                y = 18 + row * (top + th + gap)
                draw.text((8, y + 5), palette, fill="#cba6f7", font=row_font)
                for column, state in enumerate(STATES):
                    path = matrix_path(variant, palette, width, height, state)
                    with Image.open(path) as image:
                        thumb = image.convert("RGB").resize((tw, th), Image.Resampling.LANCZOS)
                    canvas.paste(thumb, (left + column * (tw + gap), y + top))
            target = out_dir / f"contact__variant-{variant.lower()}__width-{width}x{height}.png"
            canvas.save(target, optimize=True)
            outputs.append(str(target.relative_to(ROOT)))
    # Accessibility sheet: six large-enough frames for type/clipping review.
    samples = []
    for variant in VARIANTS:
        for width, height, palette in ((390, 844, "latte"), (375, 812, "mocha")):
            path = SHOT_ROOT / "accessibility" / (
                f"variant-{variant.lower()}__palette-{palette}__width-{width}x{height}__"
                "state-both-filters__dynamic-type-accessibility.png"
            )
            samples.append((variant, palette, width, height, path))
    scale = 0.62
    cell_w, cell_h = 290, 550
    canvas = Image.new("RGB", (cell_w * 3, cell_h * 2), "#11111b")
    draw = ImageDraw.Draw(canvas)
    for index, (variant, palette, width, height, path) in enumerate(samples):
        col, row = index // 2, index % 2
        x, y = col * cell_w, row * cell_h
        draw.text((x + 8, y + 8), f"{variant} · {palette} · {width}x{height}", fill="#cdd6f4", font=label_font)
        with Image.open(path) as image:
            thumb = image.convert("RGB").resize((int(width * scale), int(height * scale)), Image.Resampling.LANCZOS)
        canvas.paste(thumb, (x + 8, y + 34))
    target = out_dir / "contact__dynamic-type-accessibility.png"
    canvas.save(target, optimize=True)
    outputs.append(str(target.relative_to(ROOT)))
    return outputs


def source_contract_checks() -> dict:
    text = INDEX.read_text(encoding="utf-8")
    assertions = {
        "no generic Fleet title": ">Fleet<" not in text and 'navigationTitle("Fleet")' not in text,
        "explicit All hosts": "All hosts" in text,
        "explicit All repositories": "All repositories" in text,
        "active summary form": "Filters · ${n}" in text and "Bazzite · corral" in text,
        "settings remains": 'aria-label="Settings"' in text,
        "textual connecting": "connecting" in text,
        "textual offline and stale": "offline · stale 6m" in text,
        "reset all": "Reset all filters" in text,
        "scoped clears": "Clear host" in text and "Clear repository" in text,
        "native sheet role": 'role="dialog"' in text and "drag-handle" in text,
        "dynamic type mode": "body.a11y" in text,
        "reduced motion": "prefers-reduced-motion:reduce" in text,
        "minimum hit target contract": "min-width:44px;min-height:44px" in text,
    }
    return {"status": "pass" if all(assertions.values()) else "fail", "checks": assertions}


def write_manifest() -> dict:
    excluded = {"asset-manifest.json", "verification.json"}
    files = sorted(
        path for path in ROOT.rglob("*")
        if path.is_file() and path.name not in excluded and "__pycache__" not in path.parts
    )
    items = [{"path": str(path.relative_to(ROOT)), "bytes": path.stat().st_size, "sha256": sha256(path)} for path in files]
    result = {"algorithm": "sha256", "file_count": len(items), "files": items}
    (ROOT / "asset-manifest.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--skip-dom", action="store_true")
    args = parser.parse_args()
    contrast_result = contrast_checks()
    images = image_checks()
    contract = source_contract_checks()
    dom = {"status": "skipped", "case_count": 0, "failures": []} if args.skip_dom else dom_checks(chrome_shell())
    sheets = contact_sheets() if images["status"] == "pass" else []
    manifest = write_manifest()
    status = "pass" if all(item["status"] == "pass" for item in (contrast_result, images, contract, dom)) else "fail"
    result = {
        "status": status,
        "surface": "Monitor",
        "matrix": "3 variants × 7 states × 4 Catppuccin palettes × 2 widths",
        "image_verification": images,
        "dom_verification": {"status": dom["status"], "case_count": dom.get("case_count", 0), "failure_count": len(dom.get("failures", []))},
        "contrast_verification": {"status": contrast_result["status"], "check_count": len(contrast_result["checks"])},
        "source_contract": contract,
        "contact_sheets": sheets,
        "manifest_file_count": manifest["file_count"],
    }
    (ROOT / "verification.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))
    if status != "pass":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
