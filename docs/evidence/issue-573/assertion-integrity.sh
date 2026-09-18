#!/bin/bash
# Generate docs/evidence/issue-573/assertion-integrity.txt (#573).
set -u
cd "$(git rev-parse --show-toplevel)" || exit 1
OUT=docs/evidence/issue-573/assertion-integrity.txt
BASE=21ada0a719491cda8a3764ee32c31574689a144e

{
  echo "# #573 assertion integrity — the protected #561 assertion is untouched"
  echo
  echo "base: $BASE"
  echo "head: $BASE + the #573 worktree diff (commit sha in the final lane report)"
  echo
  echo "## 1. The protected assertion, base vs head (byte-identical; diff must be empty)"
  git show "$BASE":src/adapters/git_plane_issue561_tests.rs | sed -n '389,392p' > /tmp/g573-assert-base.txt
  sed -n '389,392p' src/adapters/git_plane_issue561_tests.rs > /tmp/g573-assert-head.txt
  cat /tmp/g573-assert-head.txt
  if diff -q /tmp/g573-assert-base.txt /tmp/g573-assert-head.txt >/dev/null; then
    echo "ASSERTION_DIFF=EMPTY (byte-identical)"
  else
    echo "ASSERTION_DIFF=NONEMPTY"
    diff /tmp/g573-assert-base.txt /tmp/g573-assert-head.txt
  fi
  echo
  echo "## 2. Test file: additions only"
  echo "numstat (added / removed): $(git diff --numstat HEAD -- src/adapters/git_plane_issue561_tests.rs | tr '	' ' ')"
  removed=$(git diff -U0 HEAD -- src/adapters/git_plane_issue561_tests.rs | grep -c '^-[^-]')
  echo "removed or modified lines: $removed"
  echo
  echo "## 3. No removed/modified line anywhere in the diff mentions an assertion, tolerance, window or ignore"
  hits=$(git diff -U0 HEAD | grep '^-[^-]' | grep -cE 'assert|expect|timeout|Duration::from|ignore|panic!|DEBOUNCE|hydration_probes')
  if [ "$hits" -eq 0 ]; then
    echo "NONE (every such line in the diff is an addition: the two new #573 fixtures)"
  else
    echo "FOUND $hits removed/modified lines:"
    git diff -U0 HEAD | grep '^-[^-]' | grep -E 'assert|expect|timeout|Duration::from|ignore|panic!|DEBOUNCE|hydration_probes'
  fi
  echo
  echo "## 4. Production probe call sites unchanged"
  echo "probe_worktree_with_budget( at base: $(git show "$BASE":src/adapters/git_plane.rs | grep -c 'probe_worktree_with_budget(')"
  echo "probe_worktree_with_budget( at head: $(grep -c 'probe_worktree_with_budget(' src/adapters/git_plane.rs)"
  echo "probe_runs.record() at base: $(git show "$BASE":src/adapters/git_plane.rs | grep -c 'probe_runs.record()')"
  echo "probe_runs.record() at head: $(grep -c 'probe_runs.record()' src/adapters/git_plane.rs)"
  echo
  echo "## 5. sha256 of the changed files (head)"
  shasum -a 256 src/adapters/git_plane.rs src/adapters/git_plane_issue561_tests.rs
} > "$OUT" 2>&1
cat "$OUT"
