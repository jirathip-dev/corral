#!/usr/bin/env bash
# Issue #340 doc-truth gate.
#
# Fails when docs/OPERATIONS.md reintroduces REMOVED daemon behavior as
# current contract:
#   - a `GET /fleets` read route (removed with the Fleet Ops CLI coupling,
#     #296, merged as #298; src/api/mod.rs has no /fleets route),
#   - a `corrald fleet ...` CLI subcommand (same removal; main.rs dispatches
#     only `digest`),
#   - "registered fleet name" / "fleet-ops CLI validated ..." identities in
#     the Issues grouping or start-worktree prose (issues are grouped and
#     pruned by the LIVE Herdr workspace.repo categories only, #237/#332).
#
# Run from the repo root:
#   bash docs/design/evidence/issue-340/doc-truth-gate.sh
set -u

doc="docs/OPERATIONS.md"
patterns=(
  "GET /fleets"
  "corrald fleet"
  "registered fleet name"
  "fleet-ops CLI validated"
)
status=0
for pattern in "${patterns[@]}"; do
  if grep -nF -- "$pattern" "$doc"; then
    status=1
  fi
done
if [ "$status" -ne 0 ]; then
  echo "doc-truth-gate: FAIL - docs/OPERATIONS.md claims removed fleet-ops behavior (matches above)." >&2
  exit 1
fi
echo "doc-truth-gate: PASS - docs/OPERATIONS.md names no GET /fleets route, corrald fleet subcommand, or fleet-ops-CLI-validated identity."
