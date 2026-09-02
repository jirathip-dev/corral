#!/usr/bin/env bash
# Issue #354 L4 doc-truth gate — read-only cut docs (docs/ + README).
#
# Fails when any of the REWRITTEN docs reintroduces REMOVED surface as
# current behavior:
#   - the mutating drive UI/flows (prompt / approve / interrupt / kill /
#     attach / start_worktree / read_issues), grant admin, step-up, the
#     grant CLI helper, the Issues/Terminal/Diff client surfaces,
#   - the removed fleet-ops route/CLI claims (GET /fleets, corrald fleet,
#     fleet-ops-CLI-validated identities; #296/#298/#340/#353),
#   - the pre-cut product copy ("Steer the fleet", approve hash claims).
#
# Patterns are CLAIM FORMS, not words: truthful removal prose ("... was
# removed in #354", "no /fleets route exists") stays legal. The daemon's
# daemon-retained read_diff capability (no client UI) is still named; the
# historical evidence/phase docs are NOT scanned (scope note in README).
#
# Run from the repo root:
#   bash docs/design/evidence/issue-354/doc-truth-gate.sh
set -u

files=(
  README.md
  docs/OPERATIONS.md
  docs/ARCHITECTURE.md
  docs/QUICKSTART.md
  docs/DEVELOPING.md
  docs/ios-showcase.md
)
patterns=(
  "GET /fleets"
  "corrald fleet"
  "registered fleet name"
  "fleet-ops CLI validated"
  "prompt, interrupt, approve"
  "signed drive: prompt"
  "Steer the fleet"
  "- **Steer it from your phone**"
  "Do something about it"
  "Approvals that can't go wrong"
  "Approve / Deny"
  "- **Prompt**"
  "- **Interrupt**"
  "- **Kill**"
  "- **Attach**"
  "Face ID"
  "X-Step-Up-Token"
  "step-up token"
  "grant editor"
  "device-grant editor"
  "Issues tab renders"
  "- **Worktree diff**"
  "corrald-grant.sh --"
)
status=0
for file in "${files[@]}"; do
  for pattern in "${patterns[@]}"; do
    if grep -nF -- "$pattern" "$file"; then
      status=1
    fi
  done
done
if [ "$status" -ne 0 ]; then
  echo "doc-truth-gate: FAIL - a rewritten doc reintroduces removed mutating/fleet surface (matches above)." >&2
  exit 1
fi
echo "doc-truth-gate: PASS - README + docs/ carry no removed mutating-drive, grant-admin, step-up, grant-CLI, Issues/Terminal/Diff, or GET /fleets claims as current behavior."
