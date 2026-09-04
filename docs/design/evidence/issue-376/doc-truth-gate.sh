#!/usr/bin/env bash
# Issue #376 doc-truth gate — iOS-only product docs (README + docs/).
#
# Fails when any of the REWRITTEN docs reintroduces the REMOVED desktop/WASM
# product surface as current behavior:
#   - the desktop fleet board / windowed client (workspace member,
#     corrald-ui binary, clients/egui tree),
#   - the WASM/web demo build, wasm-pack tooling, eframe/wgpu renderer
#     stack, or any "egui mirror later" promise (#371/#372/#373 wording),
#   - live demo claims ("open the live web demo", Pages web demo).
#
# The historical design/evidence archives (docs/design/evidence/*,
# docs/corral/*, docs/design/* prototypes) are NOT scanned: they are dated
# records of removed surfaces (scope note in the issue-376 README). One
# technical supply-chain mention is deliberately allowed: deny policy text
# calls rustls-platform-verifier "wasm32-only" (its web-target triple) —
# that describes a remaining dependency, not a Corral web product.
#
# Run from the repo root:
#   bash docs/design/evidence/issue-376/doc-truth-gate.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

files=(
  README.md
  docs/OPERATIONS.md
  docs/ARCHITECTURE.md
  docs/QUICKSTART.md
  docs/DEVELOPING.md
  docs/ios-showcase.md
)
# Claim tokens: the removed product names, its renderer stack, and web-demo
# build/publish vocabulary. 'wasm32-only' (supply-chain technical text) and
# 'wasm32-unknown-unknown' do not appear in any scanned file by design.
patterns=(
  "egui"
  "corrald-ui"
  "clients/egui"
  "eframe"
  "wgpu"
  "epaint"
  "emath"
  "wasm-pack"
  "wasm demo"
  "WASM demo"
  "wasm build"
  "WASM build"
  "web demo"
  "Web demo"
  "live web demo"
)
status=0
for file in "${files[@]}"; do
  for pattern in "${patterns[@]}"; do
    if grep -niF -- "$pattern" "$ROOT/$file"; then
      status=1
    fi
  done
done
if [ "$status" -ne 0 ]; then
  echo "doc-truth-gate: FAIL - a rewritten doc reintroduces removed desktop/WASM surface (matches above)." >&2
  exit 1
fi
echo "doc-truth-gate: PASS - README + docs/ carry no desktop-board, corrald-ui, WASM/web-demo, or renderer-stack references as current behavior."
