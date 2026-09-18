#!/usr/bin/env python3
"""#572 RED witness: re-globalise the probe counter, run the witness tests, restore.

The mutation restores the pre-#572 shape exactly: ONE counter shared by every
plane under test (and therefore by every test in the binary). It is applied to
the worktree, exercised, then reverted with `git checkout --` and proven
byte-identical by sha256. Nothing is committed in the mutated state.

Usage:
    RED_LOG=/tmp/red.log python3 red_witness.py [worktree]

Exit status: 0 when the mutation reproduced the RED (tests failed) AND the
file was restored byte-identically; 1 otherwise.
"""
import hashlib
import os
import pathlib
import subprocess
import sys

WT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "/Users/jirathip/.herdr/worktrees/corral/impl572-testiso")
GP = WT / "src/adapters/git_plane.rs"
LOG = os.environ.get("RED_LOG", "/tmp/g572-red-witness.log")

OLD = """impl ProbeRuns {
    /// A fresh counter for one plane under test.
    #[cfg(test)]
    fn new() -> Self {
        Self(Arc::new(AtomicU64::new(0)))
    }
"""
NEW = """impl ProbeRuns {
    /// A fresh counter for one plane under test.
    #[cfg(test)]
    fn new() -> Self {
        // #572 RED mutation: the counter is process-global again (the
        // pre-#572 shape - every plane under test shares one counter).
        static GLOBAL: std::sync::OnceLock<Arc<AtomicU64>> = std::sync::OnceLock::new();
        Self(GLOBAL.get_or_init(|| Arc::new(AtomicU64::new(0))).clone())
    }
"""


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    before = sha(GP)
    print(f"sha256_before={before}")

    text = GP.read_text()
    if text.count(OLD) != 1:
        print("FAIL: mutation anchor not unique")
        return 1
    GP.write_text(text.replace(OLD, NEW))
    print(f"sha256_mutated={sha(GP)}")

    with open(LOG, "wb") as handle:
        proc = subprocess.run(
            ["cargo", "test", "--lib", "-p", "corrald", "probe_accounting_"],
            cwd=WT,
            stdout=handle,
            stderr=subprocess.STDOUT,
        )
    print(f"RED_EXIT={proc.returncode}")

    # Restore from the committed blob and prove byte identity.
    subprocess.run(["git", "checkout", "--", "src/adapters/git_plane.rs"], cwd=WT, check=True)
    after = sha(GP)
    print(f"sha256_after={after}")
    print(f"RESTORED_IDENTICAL={after == before}")
    status = subprocess.run(
        ["git", "status", "--short"], cwd=WT, capture_output=True, text=True
    ).stdout.strip()
    print(f"git_status_after_restore={status!r}")
    return 0 if (after == before and proc.returncode != 0) else 1


if __name__ == "__main__":
    sys.exit(main())
