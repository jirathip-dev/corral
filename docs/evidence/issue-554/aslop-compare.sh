#!/bin/bash
# #554 G6 identity comparison of the advisory anti-slop tool.
# The HEAD log is produced by run-gates.sh (G6). This extracts the BASE tree
# (this lane's base commit), runs the same tool over it, then diffs the
# (file, rule) sets with per-key counts. Zero ADDED is the delta claim; the
# absolute totals are advisory.
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
BASE=97ed9e03cb4d265da02ac28bad1685000b159f80
cd "$ROOT" || exit 126
rm -rf /tmp/g554-base
mkdir -p /tmp/g554-base
git archive "$BASE" | tar -x -C /tmp/g554-base
echo "BASE_EXTRACT_EXIT=$?"
swift run --package-path ios/tools/anti-slop-swift anti-slop \
  /tmp/g554-base/ios/FleetNotifier /tmp/g554-base/ios/FleetNotifierTests \
  > /tmp/g554-aslop-base.log 2>&1
echo "ASLOP_BASE_EXIT=$?"
python3 docs/evidence/issue-554/aslop-normalize.py compare
python3 docs/evidence/issue-554/aslop-normalize.py render base > docs/evidence/issue-554/aslop-base-rules.txt
python3 docs/evidence/issue-554/aslop-normalize.py render head > docs/evidence/issue-554/aslop-head-rules.txt
echo "ASLOP_COMPARE_DONE"
