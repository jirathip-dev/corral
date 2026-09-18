#!/bin/bash
# Generate docs/evidence/issue-573/red-witness/red-witness-hashes.txt (#573).
# Reads the recorded hashes from the raw reversion log so this file can never
# drift from the archived run.
set -u
cd "$(git rev-parse --show-toplevel)" || exit 1
OUT=docs/evidence/issue-573/red-witness/red-witness-hashes.txt
RED=docs/evidence/issue-573/red-witness/red-final.log.gz
MECH=docs/evidence/issue-573/mechanism/boot-initial-instrumented.log.gz

gzgrep() { gzip -dc "$1" | grep -m1 "$2" | sed 's/^[^=]*=//'; }

{
  echo "# #573 red-witness hashes"
  echo
  echo "Reversion driver: docs/evidence/issue-573/red-witness/red_witness.py"
  echo "  sha256 $(shasum -a 256 docs/evidence/issue-573/red-witness/red_witness.py | awk '{print $1}')"
  echo "Mechanism driver: docs/evidence/issue-573/red-witness/boot_initial_probe.py"
  echo "  sha256 $(shasum -a 256 docs/evidence/issue-573/red-witness/boot_initial_probe.py | awk '{print $1}')"
  echo
  echo "src/adapters/git_plane.rs (recorded by the reversion driver)"
  echo "  before_sha256   = $(gzgrep "$RED" '^before_sha256=')"
  echo "  mutated_sha256  = $(gzgrep "$RED" '^mutated_sha256=')"
  echo "  restored_sha256 = $(gzgrep "$RED" '^restored_sha256=')"
  echo "  current sha256  = $(shasum -a 256 src/adapters/git_plane.rs | awk '{print $1}')"
  echo
  echo "src/adapters/git_plane_issue561_tests.rs (the witness lives here; untouched by the reversion)"
  echo "  current sha256  = $(shasum -a 256 src/adapters/git_plane_issue561_tests.rs | awk '{print $1}')"
  echo
  echo "Raw reversion run: red-final.log.gz"
  echo "  sha256 $(shasum -a 256 "$RED" | awk '{print $1}')"
  echo "  raw_exit=$(gzgrep "$RED" '^raw_exit='), byte_identical_restore=$(gzgrep "$RED" '^byte_identical_restore=')"
  echo
  echo "Raw mechanism run (boot filter set printed at the pre-fix head): mechanism/boot-initial-instrumented.log.gz"
  echo "  sha256 $(shasum -a 256 "$MECH" | awk '{print $1}')"
  echo "  raw_exit=$(gzgrep "$MECH" '^raw_exit='), G573_BOOT_INITIAL=$(gzip -dc "$MECH" | grep -m1 'G573_BOOT_INITIAL=' | sed 's/^[^=]*=//')"
} > "$OUT" 2>&1
cat "$OUT"
