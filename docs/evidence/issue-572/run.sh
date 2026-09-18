#!/usr/bin/env bash
# #572 evidence reproducer. Every command here is one this lane actually ran
# (the raw logs in this directory are its output). It never pushes, and the
# only write to a tracked file is red_witness.py's mutation, which restores it
# byte-identically (sha256-checked).
#
#   CARGO_TARGET_DIR=/tmp/g572-target   (the lane's cache; remove afterwards)
#
# Usage: bash docs/evidence/issue-572/run.sh [worktree]
set -u

WT="${1:-/Users/jirathip/.herdr/worktrees/corral/impl572-testiso}"
BASE=2e8ce2cedeb627bcff168a13cf2b41ee2fe79fbb
OUT="${OUT:-/tmp/g572-run}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/g572-target}"
mkdir -p "$OUT"

# Raw exit capture: the exit of "$@" is appended to its own log.
run() {
  local log="$1"
  shift
  "$@" > "$log" 2>&1
  local code=$?
  echo "EXIT=$code" >> "$log"
  return 0
}

cd "$WT"
echo "head=$(git rev-parse HEAD)  (fix head should be f0a108b)"

# --- 1. base three-arity reproduction (the issue's table) ------------------
git worktree add --detach /tmp/g572-base "$BASE"
cd /tmp/g572-base
export CARGO_TARGET_DIR=/tmp/g572-base-target
run "$OUT/base-default.log" cargo test --lib -p corrald g561
run "$OUT/base-t2.log"      cargo test --lib -p corrald g561 -- --test-threads=2
run "$OUT/base-t4.log"      cargo test --lib -p corrald g561 -- --test-threads=4
run "$OUT/base-lib-t2.log"  cargo test --lib -p corrald -- --test-threads=2
run "$OUT/base-lib-default.log" cargo test --lib -p corrald
cd "$WT"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/g572-target}"
git worktree remove --force /tmp/g572-base
rm -rf /tmp/g572-base-target

# --- 2. the deterministic witness, GREEN -----------------------------------
run "$OUT/witness-green.log" cargo test --lib -p corrald probe_accounting_

# --- 3. the deterministic witness, RED (re-globalised counter) -------------
# Applies the mutation, runs, restores byte-identically, prints the hashes.
RED_LOG="$OUT/red-witness.log" python3 docs/evidence/issue-572/red-witness/red_witness.py "$WT" \
  | tee "$OUT/red-witness-hashes.txt"

# --- 4. the fix head's acceptance runs -------------------------------------
run "$OUT/after-default.log"   cargo test --lib -p corrald g561
run "$OUT/after-t2.log"        cargo test --lib -p corrald g561 -- --test-threads=2
run "$OUT/after-t4.log"        cargo test --lib -p corrald g561 -- --test-threads=4
run "$OUT/after-module-t2.log" cargo test --lib -p corrald adapters::git_plane::tests -- --test-threads=2
run "$OUT/after-lib-default.log" cargo test --lib -p corrald
run "$OUT/after-lib-t2.log"    cargo test --lib -p corrald -- --test-threads=2

echo "done; logs in $OUT"
