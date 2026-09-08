#!/usr/bin/env bash
# issue-442 — run every final check in order, each to a named log with its
# RAW exit status recorded in logs/exit-codes.log. Never pipes a check
# through grep for its verdict. Run from docs/design/evidence/issue-442/.
#
#   bash scripts/run-gates.sh
#
# Steps: build -> reproducibility RED/GREEN -> two-run capture+promotion ->
# measure -> dimensions -> existing mutations -> privacy -> verify+manifest ->
# independent manifest check. No later render can invalidate comparison.
set -u
cd "$(dirname "$0")/.." || exit 2
[ "$(basename "$PWD")" = "issue-442" ] || { echo "wrong dir: $PWD"; exit 2; }
export PYTHONDONTWRITEBYTECODE=1
mkdir -p logs
: > logs/exit-codes.log
overall=0

step() {  # step <name> <logfile> <cmd...>
  local name="$1" log="$2"; shift 2
  "$@" > "$log" 2>&1
  local rc=$?
  echo "$name exit=$rc log=$log" | tee -a logs/exit-codes.log
  [ "$rc" -eq 0 ] || overall=1
}

rm -f manifest.sha256
step build      logs/build.log      python3 scripts/build.py
step repro-mutation logs/repro-red-green.log python3 scripts/repro-mutate.py
step reproducibility logs/reproducibility.log python3 scripts/repro.py --promote
# No render after comparison: canonical PNGs are the exact compared run A.
cp logs/reproducibility.log logs/capture.log
step measure    logs/measure.log    python3 scripts/measure.py
step dimensions logs/dimensions.log python3 scripts/check-dimensions.py
step mutation logs/verify-mutation.log python3 scripts/mutate.py
step privacy logs/privacy-scan.log bash scripts/privacy-scan.sh
step verify     logs/verify.log     python3 scripts/verify.py
# independent manifest check with a different tool (shasum, not verify.py)
step manifest-check logs/manifest-check.log shasum -a 256 -c manifest.sha256

echo "overall exit=$overall" | tee -a logs/exit-codes.log
exit $overall
