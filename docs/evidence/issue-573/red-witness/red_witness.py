#!/usr/bin/env python3
"""#573 RED witness driver: revert ONLY the production access-event filter
(the fix hunk in src/adapters/git_plane.rs) and run the deterministic boot
witness. The witness must fail with `left: 1, right: 0` while the filter is
reverted, and the file must be restored byte-identically afterwards.

Usage: python3 red_witness.py <worktree-root>
Requires CARGO_TARGET_DIR to be set (scratch cache).
"""
import hashlib
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
SRC = ROOT / "src" / "adapters" / "git_plane.rs"

# The fix hunk: dropping it restores the pre-#573 event handling exactly.
FILTER = """            // An access is not a change (#573). inotify reports IN_OPEN for
            // every directory the recursive watch pass opens under a
            // commondir (and for every file a git command reads, including our
            // own probes), so treating access events as change signals
            // hydrates known paths at boot and re-triggers itself afterwards.
            // Only create/remove/modify events can move a worktree's facts.
            if matches!(event.kind, notify::EventKind::Access(_)) {
                continue;
            }
"""


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


original = SRC.read_bytes()
before = sha256(original)
if FILTER.encode() not in original:
    sys.exit("FATAL: fix hunk not found; refusing to run a non-revert")
mutated = original.replace(FILTER.encode(), b"")
SRC.write_bytes(mutated)
print(f"before_sha256={before}", flush=True)
print(f"mutated_sha256={sha256(mutated)}", flush=True)

cmd = [
    "cargo",
    "test",
    "--lib",
    "-p",
    "corrald",
    "g573_watcher_boot_ignores_registration_open_events",
    "--",
    "--nocapture",
]
proc = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
print(f"cmd={' '.join(cmd)}", flush=True)
print(f"raw_exit={proc.returncode}", flush=True)
print("---- stdout ----", flush=True)
print(proc.stdout, flush=True)
print("---- stderr ----", flush=True)
print(proc.stderr, flush=True)

SRC.write_bytes(original)
after = sha256(SRC.read_bytes())
print(f"restored_sha256={after}", flush=True)
print(f"byte_identical_restore={after == before}", flush=True)
sys.exit(0 if after == before else 2)
