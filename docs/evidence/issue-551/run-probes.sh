#!/bin/bash
# #551 G8 mutation battery wrapper — the script this lane actually ran.
# ONE /tmp/n.lock invocation is held across the whole battery.
# probes.py builds a disposable worktree from HEAD, injects ONE candidate defect
# at a time, demands an XCTest assertion RED (exit 65), restores the bytes with
# SHA-256 + `git diff --exit-code` proof, and finishes with the pristine GREEN
# battery. Exit 0 means every assertion and every restore proof held.
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT" || exit 126
echo "HEAD=$(git rev-parse HEAD)"
echo "DF_BEFORE: $(df -h / | tail -1)"
git status --porcelain -- ios/
rm -rf /tmp/g551-probe-dd /tmp/g551-probe-tree
flock /tmp/n.lock python3 docs/evidence/issue-551/probes.py
echo "PROBES_EXIT=$?"
rm -rf /tmp/g551-probe-dd
echo "DF_AFTER: $(df -h / | tail -1)"
