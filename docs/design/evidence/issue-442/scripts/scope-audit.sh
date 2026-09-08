#!/usr/bin/env bash
# issue-442 — post-commit repository checks: git diff --check against the
# exact dispatch base and a scope audit proving tracked changes are confined
# to docs/design/evidence/issue-442/. Run from the worktree root AFTER
# committing. Logs -> docs/design/evidence/issue-442/logs/.
#
#   bash docs/design/evidence/issue-442/scripts/scope-audit.sh
BASE=6544bb6b9a4abe9c11f4048a2bc3c2eea1bed4ea
DIR=docs/design/evidence/issue-442
set -u
cd "$(git rev-parse --show-toplevel)" || exit 2
L="$DIR/logs"; mkdir -p "$L"

git diff --check "$BASE..HEAD" > "$L/git-diff-check.log" 2>&1
rc1=$?
echo "git-diff-check exit=$rc1" | tee -a "$L/exit-codes.log"

{
  echo "base $BASE .. HEAD $(git rev-parse HEAD)"
  git diff --stat "$BASE..HEAD" | tail -1
  echo "files outside $DIR/:"
  git diff --name-only "$BASE..HEAD" | grep -v "^$DIR/"
  out=$(git diff --name-only "$BASE..HEAD" | grep -vc "^$DIR/")
  prod=$(git diff --name-only "$BASE..HEAD" | grep -Ec '\.swift$|\.rs$|Cargo|\.github/|\.yml$|\.plist$|Package\.swift|fastlane|project\.yml')
  echo "outside-scope count=$out"
  echo "swift/rust/ci/config touched=$prod"
  [ "$out" -eq 0 ] && [ "$prod" -eq 0 ]
} > "$L/scope-audit.log" 2>&1
rc2=$?
cat "$L/scope-audit.log"
echo "scope-audit exit=$rc2" | tee -a "$L/exit-codes.log"
[ $rc1 -eq 0 ] && [ $rc2 -eq 0 ]
