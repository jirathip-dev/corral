"""Temporarily no-op refresh, always restore its exact bytes, then re-test."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
EVIDENCE = Path(__file__).resolve().parent
SOURCE = ROOT / "ios/FleetNotifier/UI/AppTheme.swift"
before = SOURCE.read_bytes()
committed = subprocess.check_output(
    ["git", "show", "HEAD:ios/FleetNotifier/UI/AppTheme.swift"], cwd=ROOT, timeout=30,
)
assert before == committed
start = before.index(b"    private func refreshFromDefaults() {\n")
end = before.index(b"    var palette: CatppuccinPalette {", start)
mutated = before[:start] + b"    private func refreshFromDefaults() {}\n\n" + before[end:]
receipt = {
    "mutation": "replace only refreshFromDefaults() with a no-op",
    "before_sha256": hashlib.sha256(before).hexdigest(),
    "mutated_sha256": hashlib.sha256(mutated).hexdigest(),
}
try:
    SOURCE.write_bytes(mutated)
    receipt["mutation_exit"] = subprocess.run(
        [sys.executable, "-B", str(EVIDENCE / "run-native.py"), "mutation"],
        cwd=ROOT, check=False,
    ).returncode
finally:
    SOURCE.write_bytes(before)
    os.utime(SOURCE, None)
    restored = SOURCE.read_bytes()
    assert restored == before
    receipt["restored_sha256"] = hashlib.sha256(restored).hexdigest()
    (EVIDENCE / "amend1-mutation-restore.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt, sort_keys=True), flush=True)
receipt["restore_exit"] = subprocess.run(
    [sys.executable, "-B", str(EVIDENCE / "run-native.py"), "restore"],
    cwd=ROOT, check=False,
).returncode
assert SOURCE.read_bytes() == before
receipt["after_green_sha256"] = hashlib.sha256(SOURCE.read_bytes()).hexdigest()
(EVIDENCE / "amend1-mutation-restore.json").write_text(json.dumps(receipt, indent=2) + "\n")
print(json.dumps(receipt, sort_keys=True), flush=True)
sys.exit(0 if receipt["mutation_exit"] == 65 and receipt["restore_exit"] == 0 else 1)
