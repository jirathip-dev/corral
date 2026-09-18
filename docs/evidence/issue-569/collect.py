#!/usr/bin/env python3
"""#569 evidence collector: assemble docs/evidence/issue-569/frames +
captures.json + measurements.log.

Two sources for the frames:
  * --frames DIR  copy the PNGs already rendered by SheetPolishTests in the
                  lane worktree (the normal path: the test writes them into
                  the simulator's app container, this copies them out);
  * --sim UDID    pull them straight out of the booted simulator's app
                  container (fallback when the frames were left in place).

Run from the repo root, e.g.:
    python3 docs/evidence/issue-569/collect.py --frames /tmp/g569-frames \
        --log /tmp/g569-focused5.log --log /tmp/g569-red.log \
        --log /tmp/g569-scratch-green.log
"""
import argparse
import hashlib
import json
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
OUT = Path(__file__).resolve().parent

parser = argparse.ArgumentParser()
parser.add_argument("--frames", type=Path)
parser.add_argument("--sim")
parser.add_argument("--log", action="append", default=[])
args = parser.parse_args()

assert bool(args.frames) != bool(args.sim), "give exactly one of --frames/--sim"
if args.frames:
    source = args.frames
    origin = f"lane worktree render output copied from {source}"
else:
    container = subprocess.check_output(
        ["xcrun", "simctl", "get_app_container", args.sim,
         "com.corral.fleetnotifier", "data"], text=True).strip()
    source = Path(container) / "Documents" / "g569-evidence"
    origin = f"simulator app container {container}"

files = sorted(source.glob("*.png"))
assert len(files) == 24, f"expected 24 captures (16 state + 8 chip), found {len(files)}"

frames = OUT / "frames"
frames.mkdir(parents=True, exist_ok=True)
captures = []
for path in files:
    target = frames / path.name
    shutil.copy(path, target)
    data = target.read_bytes()
    captures.append({
        "file": f"frames/{path.name}",
        "sha256": hashlib.sha256(data).hexdigest(),
        "bytes": len(data),
        "origin": str(path),
    })

(OUT / "captures.json").write_text(json.dumps({
    "surface": "RecentOutputSheet hosted over FleetView in a 390x844 pt "
               "XCTest UIWindow, 3x screenshots (1170x2532 px) — the #558 "
               "evidence path; the harness does not composite the "
               "presentationBackground material, so the sheet surface in "
               "these frames is the system sheet fill (white / #1c1c1e) "
               "rather than the app's flavored translucent backdrop",
    "source": origin,
    "state_frames": "day|night x default|ax3 x loading|empty|error|permission",
    "chip_frames": "latte|mocha x corral|sendmeter|synergy-apps|other",
    "captures": captures,
}, indent=2) + "\n")

transcript = []
for name in args.log:
    path = Path(name)
    if not path.exists():
        transcript.append(f"[missing log {path}]")
        continue
    transcript.append(f"===== {path}")
    for line in path.read_text(errors="replace").splitlines():
        if line.startswith("G569_") or "Executed " in line or "TEST SUCCEEDED" in line \
                or "TEST FAILED" in line or line.startswith("XCODEBUILD_EXIT") \
                or line.startswith("RED_RUN_EXIT") or line.startswith("SCRATCH_GREEN_EXIT"):
            transcript.append(line)
(OUT / "measurements.log").write_text("\n".join(transcript) + "\n")
print(json.dumps({"captures": len(captures), "measurement_lines": len(transcript)}, indent=2))
