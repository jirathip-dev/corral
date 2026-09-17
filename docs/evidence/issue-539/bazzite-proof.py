#!/usr/bin/env python3
"""Run only inside a disposable root on Bazzite; never mutate live Corral."""
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys

root = Path(sys.argv[1]).resolve()
assert root.name.startswith("corral539-proof-")
assert (root / ".created-for-539").is_file()
out = root / "results"
out.mkdir()
commands = []


def run(argv, **kwargs):
    result = subprocess.run(argv, timeout=240, **kwargs)
    commands.append({"argv": argv, "raw_exit": result.returncode})
    return result


def snapshot():
    home = Path.home()
    paths = [home / ".local/share/corral", home / ".config/corral",
             home / ".config/systemd/user/corrald.service", home / ".local/bin/gh"]
    entries = {}
    for base in paths:
        assert base.exists(), base
        for path in [base, *sorted(base.rglob("*"))] if base.is_dir() else [base]:
            info = path.lstat()
            # Hash path identifiers as well: do not publish device/key filenames.
            identity = hashlib.sha256(str(path.relative_to(home)).encode()).hexdigest()
            entry = {"mode": info.st_mode, "size": info.st_size, "mtime_ns": info.st_mtime_ns}
            if stat.S_ISREG(info.st_mode):
                entry["sha256"] = hashlib.file_digest(path.open("rb"), "sha256").hexdigest()
            elif stat.S_ISLNK(info.st_mode):
                entry["link_sha256"] = hashlib.sha256(os.readlink(path).encode()).hexdigest()
            entries[identity] = entry
    active = run(["systemctl", "--user", "is-active", "corrald"], capture_output=True, text=True)
    detail = run(["systemctl", "--user", "show", "corrald", "--property=MainPID",
                  "--property=ActiveEnterTimestampMonotonic", "--property=ActiveState"],
                 capture_output=True, text=True)
    return {"roots": [str(p) for p in paths], "entries": entries,
            "manifest_sha256": hashlib.sha256(json.dumps(entries, sort_keys=True).encode()).hexdigest(),
            "service_exit": active.returncode, "service": active.stdout.strip(),
            "service_details": detail.stdout, "service_details_exit": detail.returncode}


before = snapshot()
assert before["service_exit"] == 0 and before["service"] == "active"
for name in ["home", "tmp"]:
    (root / name).mkdir()
env = {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TMPDIR": str(root / "tmp"),
       "CORRAL_TEST_EVIDENCE_DIR": str(out / "scenario-logs")}
probe = run(["/bin/bash", "--noprofile", "--norc", "-c", "command -v gh"],
            env=env, capture_output=True, text=True)
assert probe.returncode == 1 and probe.stdout == ""
with (out / "suite.log").open("w") as log:
    suite = run(["/bin/bash", "scripts/test-install-corral-linux.sh"],
                cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT)
after = snapshot()
forbidden = out / "scenario-logs/forbidden.log"
receipt = {"before": before, "after": after, "live_unchanged": before == after,
           "suite_exit": suite.returncode, "forbidden_bytes": forbidden.stat().st_size,
           "absent_path": env["PATH"], "command_v_gh_stdout": probe.stdout,
           "command_v_gh_exit": probe.returncode, "commands": commands,
           "created_root": str(root), "sandbox_tmp_empty": not any((root / "tmp").iterdir()),
           "isolation": "fake HOME; installer/config overrides; stub systemctl/launchctl/curl; no real daemon launched"}
(out / "isolation.json").write_text(json.dumps(receipt, indent=2) + "\n")
print(json.dumps({k: v for k, v in receipt.items() if k not in ("before", "after", "commands")}, indent=2))
assert suite.returncode == 0
assert before == after, "live paths/service changed during proof; inspect before/after, do not restore live state"
assert forbidden.stat().st_size == 0
assert receipt["sandbox_tmp_empty"]
