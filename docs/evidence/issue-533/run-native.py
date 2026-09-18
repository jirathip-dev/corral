"""Run one #533 native leg, retaining raw output and bounded status."""

import hashlib
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
EVIDENCE = Path(__file__).resolve().parent
LOG = Path("/tmp/g533-tests.log")
step = sys.argv[1]
assert step in {"red", "focused", "full", "mutation", "restore"}
command = [
    "xcodebuild", "-project", "ios/FleetNotifier.xcodeproj", "-scheme", "FleetNotifier",
    "-configuration", "Debug", "-destination", "platform=iOS Simulator,name=iPhone 16",
    "-derivedDataPath", "/tmp/g533-dd", "CODE_SIGNING_ALLOWED=NO",
    "-parallel-testing-enabled", "NO",
]
if step == "full":
    command += ["-only-testing:FleetNotifierTests"]
elif step in {"mutation", "restore"}:
    command += ["-only-testing:FleetNotifierTests/ThemeStoreTests/testExternalEnvironmentWriteRefreshesTheResolvedPalette"]
else:
    command += ["-only-testing:FleetNotifierTests/ThemeStoreTests", "-only-testing:FleetNotifierTests/ThemePaletteTests"]
command += ["test"]
# The installed shim has no iPhone 16 default. Use its owned-simulator
# lifecycle explicitly so the brief's exact destination remains unchanged.
launch = [
    "flock", "/tmp/corral-heavy-gate.lock", "hermes-sim-task",
    "--device-type", "com.apple.CoreSimulator.SimDeviceType.iPhone-16",
    "--name", "iPhone 16", "--", *command,
]
receipt = {
    "step": step,
    "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True, timeout=30).strip(),
    "command": shlex.join(command),
    "launch": shlex.join(launch),
    "sha256": {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in (
        "ios/FleetNotifier/UI/AppTheme.swift", "ios/FleetNotifierTests/ThemeTests.swift",
    )},
}
with LOG.open("ab") as log:
    start = log.tell()
    log.write(("\nG533_BEGIN " + json.dumps(receipt, sort_keys=True) + "\n").encode())
    log.flush()
    process = subprocess.Popen(launch, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    try:
        status = process.wait(timeout=1500)
        receipt["deadline_exceeded"] = False
    except subprocess.TimeoutExpired:
        receipt["deadline_exceeded"] = True
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=60)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=30)
        status = 124
    receipt["exit"] = status
    log.write(f"\n{step}_exit={status}\nG533_END {step}\n".encode())
    end = log.tell()
with LOG.open("rb") as log:
    log.seek(start)
    (EVIDENCE / f"amend1-{step}.log").write_bytes(log.read(end - start))
(EVIDENCE / f"amend1-{step}.json").write_text(json.dumps(receipt, indent=2) + "\n")
print(f"{step}_exit={status}", flush=True)
sys.exit(status)
