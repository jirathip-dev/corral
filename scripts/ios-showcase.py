#!/usr/bin/env python3
"""Capture, validate, and render the public iOS showcase artifact."""
from __future__ import annotations

import argparse
import html
import json
import re
import shutil
import subprocess
import time
import zlib
from datetime import datetime, timezone
from pathlib import Path

WIDTH, HEIGHT = 390, 844
ALLOWLIST = {
    "board": ("-demoMode",),
    "detail": ("-corralDemoDetail",),
    "issues": ("-corralDemoIssues",),
    "issue-detail": ("-corralDemoIssues", "-corralDemoIssuesDetail", "267"),
}
EXPECTED = {f"{name}.png" for name in ALLOWLIST} | {"metadata.json", "index.html"}
DENYLIST = (
    b"AKIA", b"BEGIN PRIVATE KEY", b"GITHUB_TOKEN", b"ASC_API_KEY",
    b"/Users/", b"/home/", b"dev_", b"registration-token", b"admin-token",
)
UUID_RE = re.compile(rb"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}\b")


def run(*args: str) -> None:
    subprocess.run(args, check=True)


def png_size(path: Path) -> tuple[int, int]:
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path.name}: invalid PNG signature")
    pos, ihdr, compressed, saw_end = 8, None, b"", False
    while pos < len(data):
        if pos + 12 > len(data):
            raise ValueError(f"{path.name}: truncated PNG chunk")
        length = int.from_bytes(data[pos:pos + 4], "big")
        kind = data[pos + 4:pos + 8]
        end = pos + 12 + length
        if end > len(data):
            raise ValueError(f"{path.name}: truncated PNG data")
        body = data[pos + 8:pos + 8 + length]
        crc = int.from_bytes(data[pos + 8 + length:end], "big")
        actual = zlib.crc32(kind + body) & 0xffffffff
        if crc != actual:
            raise ValueError(f"{path.name}: bad {kind.decode(errors='replace')} CRC")
        if kind == b"IHDR":
            if length != 13:
                raise ValueError(f"{path.name}: malformed IHDR")
            ihdr = body
        elif kind == b"IDAT":
            compressed += body
        elif kind == b"IEND":
            saw_end = True
            if end != len(data):
                raise ValueError(f"{path.name}: data follows IEND")
            break
        pos = end
    if ihdr is None or not saw_end:
        raise ValueError(f"{path.name}: incomplete PNG")
    width = int.from_bytes(ihdr[0:4], "big")
    height = int.from_bytes(ihdr[4:8], "big")
    bit_depth, color_type = ihdr[8], ihdr[9]
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}.get(color_type)
    if channels is None or bit_depth != 8:
        raise ValueError(f"{path.name}: unsupported PNG encoding")
    try:
        decoded = zlib.decompress(compressed)
    except zlib.error as exc:
        raise ValueError(f"{path.name}: IDAT does not decode ({exc})") from exc
    expected = height * (1 + width * channels)
    if len(decoded) != expected:
        raise ValueError(f"{path.name}: decoded pixel stream has wrong length")
    return width, height


def validate(root: Path) -> None:
    if not root.is_dir():
        raise ValueError(f"artifact directory does not exist: {root}")
    actual = {p.name for p in root.iterdir() if p.is_file()}
    if any(p.is_dir() for p in root.iterdir()) or actual != EXPECTED:
        raise ValueError(f"expected exactly {sorted(EXPECTED)}, found {sorted(actual)}")
    metadata = json.loads((root / "metadata.json").read_text())
    for key in ("commit_sha", "captured_at_utc", "testflight_build"):
        if key not in metadata:
            raise ValueError(f"metadata missing {key}")
    for filename in sorted(p for p in actual if p.endswith(".png")):
        size = png_size(root / filename)
        if size != (WIDTH, HEIGHT):
            raise ValueError(f"{filename}: expected {WIDTH}x{HEIGHT}, found {size[0]}x{size[1]}")
    for path in root.iterdir():
        data = path.read_bytes()
        if any(marker in data for marker in DENYLIST) or UUID_RE.search(data):
            raise ValueError(f"secret/private identifier detected in {path.name}")
    print(f"validated {len(ALLOWLIST)} PNGs at {WIDTH}x{HEIGHT}; allowlist and secret scan passed")


def gallery(root: Path) -> None:
    metadata = json.loads((root / "metadata.json").read_text())
    cards = []
    for name, args in ALLOWLIST.items():
        cards.append(
            f'<article><h2>{html.escape(name.replace("-", " ").title())}</h2>'
            f'<img src="{name}.png" alt="FleetNotifier {html.escape(name)} simulator screen">'
            '<p>Simulator demo from the TestFlight source revision</p></article>'
        )
    page = f'''<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Corral iOS showcase</title>
<style>:root{{color-scheme:dark;--bg:#0d1117;--panel:#161b22;--line:#30363d;--teal:#56d4c7;--muted:#8b949e}}*{{box-sizing:border-box}}body{{margin:0;background:var(--bg);color:#e6edf3;font:15px system-ui,sans-serif}}main{{max-width:1180px;margin:auto;padding:32px 20px}}header{{border-bottom:1px solid var(--line);margin-bottom:24px}}h1{{margin:0 0 8px;color:var(--teal)}}h2{{font-size:16px;text-transform:capitalize;margin:0 0 12px}}p{{color:var(--muted)}}.meta{{font:12px ui-monospace,monospace;overflow-wrap:anywhere}}.grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:20px}}article{{background:var(--panel);border:1px solid var(--line);border-radius:10px;padding:14px}}img{{display:block;width:100%;height:auto;border-radius:6px}}</style></head><body><main><header><h1>Corral · iOS</h1><p>Simulator demo from the TestFlight source revision</p><p class="meta">commit {html.escape(metadata['commit_sha'])}<br>captured {html.escape(metadata['captured_at_utc'])}<br>TestFlight build {html.escape(str(metadata['testflight_build']))}</p></header><section class="grid">{''.join(cards)}</section></main></body></html>'''
    (root / "index.html").write_text(page)


def capture(args: argparse.Namespace) -> None:
    out = Path(args.output)
    out.mkdir(parents=True, exist_ok=True)
    for old in out.iterdir():
        if old.is_file(): old.unlink()
    subprocess.run(("xcrun", "simctl", "boot", args.udid), check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    run("xcrun", "simctl", "bootstatus", args.udid, "-b")
    run("xcrun", "simctl", "install", args.udid, args.app)
    for name, launch_args in ALLOWLIST.items():
        subprocess.run(("xcrun", "simctl", "terminate", args.udid, "com.corral.fleetnotifier"), check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        run("xcrun", "simctl", "launch", args.udid, "com.corral.fleetnotifier", *launch_args)
        time.sleep(args.wait)
        run("xcrun", "simctl", "io", args.udid, "screenshot", str(out / f"{name}.png"))
        run("sips", "-z", str(HEIGHT), str(WIDTH), str(out / f"{name}.png"), "--out", str(out / f"{name}.png"))
    metadata = {"commit_sha": args.sha, "captured_at_utc": args.captured_at, "testflight_build": args.build}
    (out / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    gallery(out)
    validate(out)


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    val = sub.add_parser("validate"); val.add_argument("artifact", type=Path)
    gen = sub.add_parser("gallery"); gen.add_argument("artifact", type=Path)
    cap = sub.add_parser("capture")
    cap.add_argument("--app", required=True); cap.add_argument("--udid", required=True)
    cap.add_argument("--output", required=True); cap.add_argument("--sha", required=True)
    cap.add_argument("--captured-at", required=True); cap.add_argument("--build", default="unavailable")
    cap.add_argument("--wait", type=float, default=3)
    args = parser.parse_args()
    try:
        if args.command == "validate": validate(args.artifact)
        elif args.command == "gallery": gallery(args.artifact)
        else: capture(args)
    except (OSError, ValueError, json.JSONDecodeError, subprocess.CalledProcessError) as exc:
        parser.error(str(exc))


if __name__ == "__main__": main()
