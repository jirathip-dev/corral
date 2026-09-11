#!/usr/bin/env python3
"""#448 runtime video evidence: real phone-sized actual-app herd video showing
the working trot, idle/stepping and grazing behavior, plus the Reduce Motion
and disconnected static guards.

One owned simulator (never erased); capture installs the Debug app built from
the committed candidate, launches the existing DEBUG evidence routes, records
`simctl io recordVideo` per scenario, takes time-stamped screenshots, and
writes a manifest with build/source provenance (head, source digest, app
binary sha256, per-artifact sha256). Artifacts stay outside the source tree.

The manifest's device block is never a hardcoded claim: it is resolved live
from the selected simulator (name + model via `simctl list devices/devicetypes`)
and from the screenshots this run actually captured (every screenshot must
agree on one positive pixel size). The run fails explicitly when the selected
simulator's identity (udid/name/device type, all required non-blank) or the
captured dimensions cannot be resolved.

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
import struct
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


def fail(message):
    raise SystemExit(f"gait-video-evidence FAIL: {message}")


def simctl_json(*arguments):
    result = sh("xcrun", "simctl", *arguments, "-j")
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as error:
        fail(f"simctl {' '.join(arguments)} did not return JSON (exit {result.returncode}): {error}")


def find_device(devices, udid):
    for runtime in devices.get("devices", {}).values():
        for device in runtime:
            if device.get("udid") == udid:
                return device
    fail(f"selected simulator {udid} is absent from the simctl device list")


def device_identity(device, devicetypes):
    if device.get("state") != "Booted":
        fail(f"owned simulator {device.get('udid')} must be booted by the caller, state={device.get('state')!r}")
    udid = device.get("udid")
    if not isinstance(udid, str) or not udid.strip():
        fail(f"selected simulator record has no usable udid: {udid!r}")
    name = device.get("name")
    if not isinstance(name, str) or not name.strip():
        fail(f"selected simulator {udid} has no usable name: {name!r}")
    type_identifier = device.get("deviceTypeIdentifier")
    if not isinstance(type_identifier, str) or not type_identifier.strip():
        fail(f"selected simulator {udid} has no usable device type identifier: {type_identifier!r}")
    matched = [entry for entry in devicetypes.get("devicetypes", [])
               if entry.get("identifier") == type_identifier]
    if not matched:
        fail(f"cannot resolve device type {type_identifier!r} for simulator {udid}")
    entry = matched[0]
    model = entry.get("name")
    if not isinstance(model, str) or not model.strip():
        fail(f"device type {type_identifier!r} has no usable name: {model!r}")
    model_identifier = entry.get("modelIdentifier")
    if not isinstance(model_identifier, str) or not model_identifier.strip():
        fail(f"device type {type_identifier!r} has no usable modelIdentifier: {model_identifier!r}")
    return {
        "udid": udid,
        "name": name,
        "model": model,
        "model_identifier": model_identifier,
        "device_type_identifier": type_identifier,
    }


def png_pixels(path):
    try:
        with open(path, "rb") as handle:
            header = handle.read(24)
    except OSError as error:
        fail(f"cannot read captured artifact {path}: {error}")
    if len(header) < 24 or header[:8] != b"\x89PNG\r\n\x1a\n" or header[12:16] != b"IHDR":
        fail(f"captured artifact is not a PNG screenshot: {path}")
    width, height = struct.unpack(">II", header[16:24])
    if width == 0 or height == 0:
        fail(f"captured artifact has a zero pixel dimension: {path} ({width}x{height})")
    return (width, height)


def capture_pixels(paths):
    sizes = {}
    for path in paths:
        sizes.setdefault(png_pixels(path), []).append(Path(path).name)
    if not sizes:
        fail("no captured screenshot artifacts; pixel dimensions cannot be derived")
    if len(sizes) != 1:
        fail("captured artifacts disagree on pixel dimensions: "
             + ", ".join(f"{size[0]}x{size[1]} ({len(names)} shots)"
                         for size, names in sorted(sizes.items())))
    return next(iter(sizes))


def device_metadata(identity, pixels):
    return {**identity, "pixels": {"width": pixels[0], "height": pixels[1]}}


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
    device = find_device(simctl_json("list", "devices"), sim)
    identity = device_identity(device, simctl_json("list", "devicetypes"))

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
        result = sh("xcrun", "simctl", "launch", sim, BUNDLE, *extra)
        return time.time(), result.stdout.strip() or result.stderr.strip()

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
        record_epoch = time.time()
        time.sleep(1.2)
        launch_epoch, launch_output = launch(extra)
        entry["record_epoch"] = round(record_epoch, 3)
        entry["launch_epoch"] = round(launch_epoch, 3)
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

    screenshots = [shots_dir / shot["file"]
                   for scenario in manifest["scenarios"] for shot in scenario["screenshots"]]
    manifest["device"] = device_metadata(identity, capture_pixels(screenshots))
    print(json.dumps({"device": manifest["device"]}, sort_keys=True), flush=True)

    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print("manifest:", args.output / "manifest.json")


if __name__ == "__main__":
    main()
