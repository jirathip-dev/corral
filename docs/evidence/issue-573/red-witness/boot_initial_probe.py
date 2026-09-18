#!/usr/bin/env python3
"""#573 mechanism probe: at the PRE-FIX head (the production access-event
filter reverted), arm the witness fixture AND print the watcher boot's
`initial` short-circuit set. The filter must print `initial=[]` (the path is
KNOWN) while the probe still escapes (assertion left: 1) -> the escaping
probe comes from the registration-event path, not from the boot filter.

Both mutations are reverted together and the file is restored
byte-identically (hash printed before/after).

Usage: python3 boot_initial_probe.py <worktree-root>
"""
import hashlib
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
SRC = ROOT / "src" / "adapters" / "git_plane.rs"

# Mutation 1: revert the fix hunk (same text as red_witness.py).
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

# Mutation 2: print the short-circuit set the boot filter computed.
ANCHOR = """        for worktree in initial {
            self.debounce(worktree, sink.clone());
        }
"""
PROBE = """        #[cfg(test)]
        eprintln!("G573_BOOT_INITIAL={:?}", initial);
        for worktree in initial {
            self.debounce(worktree, sink.clone());
        }
"""


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


original = SRC.read_bytes()
before = sha256(original)
for name, needle in (("filter hunk", FILTER), ("boot anchor", ANCHOR)):
    if needle.encode() not in original:
        sys.exit(f"FATAL: {name} not found; refusing to run")
mutated = original.replace(FILTER.encode(), b"").replace(ANCHOR.encode(), PROBE.encode())
SRC.write_bytes(mutated)
print(f"before_sha256={before}", flush=True)
print(f"mutated_sha256={sha256(mutated)}", flush=True)
try:
    proc = subprocess.run(
        [
            "cargo",
            "test",
            "--lib",
            "-p",
            "corrald",
            "g573_watcher_boot_ignores_registration_open_events",
            "--",
            "--nocapture",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    print(f"raw_exit={proc.returncode}", flush=True)
    print("---- stdout ----", flush=True)
    print(proc.stdout, flush=True)
    print("---- stderr ----", flush=True)
    print(proc.stderr, flush=True)
finally:
    SRC.write_bytes(original)
    after = sha256(SRC.read_bytes())
    print(f"restored_sha256={after}", flush=True)
    print(f"byte_identical_restore={after == before}", flush=True)
