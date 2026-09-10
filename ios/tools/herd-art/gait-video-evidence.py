#!/usr/bin/env python3
"""#448 runtime video evidence: real phone-sized actual-app herd video showing
the working trot, idle/stepping and grazing behavior, plus the Reduce Motion
and disconnected static guards.

One owned simulator (never erased); capture installs the Debug app built from
the committed candidate, launches the existing DEBUG evidence routes, records
`simctl io recordVideo` per scenario, takes time-stamped screenshots, and
writes a manifest with build/source provenance (head, source digest, app
binary sha256, per-artifact sha256). Artifacts stay outside the source tree.

Scenarios:
  working-idle-grazing  -corralHerdEvidence -corral456FullScreenEvidence
      live clock: day/night/next-paddock full-screen holds; working horses
      trot, idle horses step and enter their deterministic grazing windows.
  reduce-motion         + -corralDemoReduceMotion
      app Reduce Motion forced: static pose, clock never ticks.
  disconnected          + -corralHerdOffline
      every source disconnected: unknown/static outage frame.
  scene-background      launch -> Settings (background) -> relaunch (foreground)
      scene-phase observation only (the backgrounded surface is not rendered).
"""
import argparse
import hashlib
import json
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
BUNDLE = "com.corral.fleetnotifier"


def sh(*command, timeout=120):
    return subprocess.run(command, capture_output=True, text=True, timeout=timeout)


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def probe_seconds(path):
    result = sh("ffprobe", "-v", "error", "-show_entries", "format=duration",
                "-of", "default=noprint_wrappers=1:nokey=1", str(path))
    try:
        return round(float(result.stdout.strip()), 3)
    except ValueError:
        return None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--udid", required=True)
    parser.add_argument("--app", required=True, type=Path)
    parser.add_argument("--head", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    shots_dir = args.output / "screenshots"
    shots_dir.mkdir(exist_ok=True)
    sim = args.udid
    state = sh("xcrun", "simctl", "list", "devices").stdout
    assert "Booted" in state, "owned simulator must be booted by the caller"

    def container():
        return Path(sh("xcrun", "simctl", "get_app_container", sim, BUNDLE, "data").stdout.strip())

    def clear_markers():
        markers = container() / "Documents" / "ux-evidence"
        if markers.exists():
            shutil.rmtree(markers)

    def markers_seen():
        markers = container() / "Documents" / "ux-evidence"
        if not markers.exists():
            return {}
        return {path.stem: path.stat().st_mtime for path in markers.glob("*.marker")}

    def launch(extra):
        sh("xcrun", "simctl", "terminate", sim, BUNDLE)
        time.sleep(0.4)
        stamp = time.time()
        result = sh("xcrun", "simctl", "launch", sim, BUNDLE, *extra)
        return stamp, result.stdout.strip() or result.stderr.strip()

    def screenshot(name):
        path = shots_dir / f"{name}.png"
        sh("xcrun", "simctl", "io", sim, "screenshot", "--type", "png", str(path))
        return {"file": path.name, "sha256": sha256(path), "epoch": round(time.time(), 3)}

    def record_scenario(name, extra, duration, cadence, first_shot):
        entry = {"name": name, "launch_args": extra, "video": None, "screenshots": [], "markers": {}}
        clear_markers()
        video = args.output / f"{name}.mov"
        if video.exists():
            video.unlink()
        recorder = subprocess.Popen(
            ["xcrun", "simctl", "io", sim, "recordVideo", "--codec=h264",
             "--mask=ignored", "--force", str(video)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1.2)
        record_epoch, launch_output = launch(extra)
        entry["record_epoch"] = round(record_epoch, 3)
        entry["launch_epoch"] = round(record_epoch, 3)
        entry["launch_output"] = launch_output
        end = time.time() + duration
        next_shot = record_epoch + first_shot
        while time.time() < end:
            now = time.time()
            if now >= next_shot:
                entry["screenshots"].append(screenshot(f"{name}-{round(now - record_epoch, 2):07.2f}"))
                next_shot = now + cadence
            time.sleep(0.05)
        entry["markers"] = {k: round(v, 3) for k, v in markers_seen().items()}
        recorder.send_signal(signal.SIGINT)
        try:
            recorder.wait(timeout=30)
        except subprocess.TimeoutExpired:
            recorder.kill()
        entry["video"] = {"file": video.name, "sha256": sha256(video), "seconds": probe_seconds(video)}
        print(json.dumps({"scenario": name, "video": entry["video"],
                          "screenshots": len(entry["screenshots"]),
                          "markers": sorted(entry["markers"])}, sort_keys=True), flush=True)
        return entry

    manifest = {
        "issue": 448,
        "head": args.head,
        "device": "iPhone 16 (393x852 pt; 1179x2556 @3x) — this host's only simulator",
        "app": str(args.app),
        "app_binary_sha256": sha256(args.app / "FleetNotifier"),
        "xcodebuild": sh("xcodebuild", "-version").stdout.splitlines()[0],
        "scenarios": [],
    }
    sys.path.insert(0, str(ROOT))
    from ios.release_source_manifest import release_source_digest  # noqa: E402
    manifest["release_source_digest"] = release_source_digest(ROOT)

    sh("xcrun", "simctl", "install", sim, str(args.app))
    manifest["scenarios"].append(record_scenario(
        "working-idle-grazing", ["-corralHerdEvidence", "-corral456FullScreenEvidence"], 36, 0.75, 2.5))
    manifest["scenarios"].append(record_scenario(
        "reduce-motion", ["-corralHerdEvidence", "-corral456FullScreenEvidence", "-corralDemoReduceMotion"],
        13, 1.2, 3.0))
    manifest["scenarios"].append(record_scenario(
        "disconnected", ["-corralHerdEvidence", "-corral456FullScreenEvidence", "-corralHerdOffline"],
        11, 1.2, 3.0))

    scene = {"name": "scene-background", "launch_args": ["-corralHerdEvidence", "-corral456FullScreenEvidence"],
             "screenshots": [], "markers": {}}
    clear_markers()
    _, launch_output = launch(scene["launch_args"])
    scene["launch_output"] = launch_output
    time.sleep(5)
    scene["screenshots"].append(screenshot("scene-1-herd"))
    background = sh("xcrun", "simctl", "launch", sim, "com.apple.Preferences")
    scene["background_launch"] = background.stdout.strip() or background.stderr.strip()
    time.sleep(3)
    scene["screenshots"].append(screenshot("scene-2-backgrounded"))
    foreground = sh("xcrun", "simctl", "launch", sim, BUNDLE)
    scene["foreground_launch"] = foreground.stdout.strip() or foreground.stderr.strip()
    time.sleep(2)
    scene["screenshots"].append(screenshot("scene-3-foreground-return"))
    manifest["scenarios"].append(scene)

    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print("manifest:", args.output / "manifest.json")


if __name__ == "__main__":
    main()
