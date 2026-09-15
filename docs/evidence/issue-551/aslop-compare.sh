#!/bin/bash
# #551 G6 identity comparison of the advisory anti-slop tool.
# The HEAD log is produced by run-gates.sh (G6). This extracts the BASE tree,
# runs the same tool over it, then diffs the (file, rule) sets with per-key
# counts. Zero ADDED is the delta claim; the absolute totals are advisory.
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
BASE=b6b69f377883d6b2507f8418b6b4dc34f7452145
cd "$ROOT" || exit 126
rm -rf /tmp/g551-base
mkdir -p /tmp/g551-base
git archive "$BASE" | tar -x -C /tmp/g551-base
echo "BASE_EXTRACT_EXIT=$?"
swift run --package-path ios/tools/anti-slop-swift anti-slop \
  /tmp/g551-base/ios/FleetNotifier /tmp/g551-base/ios/FleetNotifierTests \
  > /tmp/g551-aslop-base.log 2>&1
echo "ASLOP_BASE_EXIT=$?"
python3 docs/evidence/issue-551/aslop-normalize.py compare
python3 docs/evidence/issue-551/aslop-normalize.py render base > docs/evidence/issue-551/aslop-base-rules.txt
python3 docs/evidence/issue-551/aslop-normalize.py render head > docs/evidence/issue-551/aslop-head-rules.txt
echo "ASLOP_COMPARE_DONE"
