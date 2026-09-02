#!/bin/bash
# #324 probe: measure request/response bytes for first, unchanged, and
# changed read_tail reads against a SIMULATED contract-honoring provider.
#
# The live Herdr 0.8.2 provider does not support revisions (`agent.read`
# returns `revision: 0` regardless of `rev` — see README.md), so this probe
# exercises the REAL Corral adapter over a mock unix socket whose fixture
# provider implements the #324 contract. The byte counts are the raw wire
# transfer (JSON-RPC line including trailing newline) for each exchange.
#
# Run from the repo root:
#   docs/design/evidence/issue-324/probe.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

log="docs/design/evidence/issue-324/probe.log"
{
  echo "PROBE issue-324 $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "PROBE head: $(git rev-parse --short HEAD)"
  cargo test -p corrald --lib probe_read_tail_bytes_first_unchanged_changed \
    -- --nocapture
} > "$log" 2>&1
rc=$?
echo "PROBE_EXIT=$rc" | tee -a "$log"
grep -E '^(PROBE|test result)' "$log" || true
exit "$rc"
