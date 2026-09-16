#!/bin/bash
# #554 exact probe driver — one shared-lock invocation of the whole battery.
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT" || exit 126
echo "HEAD=$(git rev-parse HEAD)"
echo "DF_START: $(df -h / | tail -1)"
flock /tmp/n.lock python3 docs/evidence/issue-554/probes.py
echo "PROBES_EXIT=$?"
echo "DF_END: $(df -h / | tail -1)"
