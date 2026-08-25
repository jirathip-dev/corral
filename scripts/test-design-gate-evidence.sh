#!/usr/bin/env bash
# Hermetic test for scripts/design-gate-evidence.sh. It exercises the explicit
# supplied-PNG seam with a fake headless browser, checks the stamped bundle and
# deterministic dimensions, proves a rerun replaces the same files, and makes
# sure a failed later run leaves the previous bundle intact.
#
# Run with one command:
#   bash scripts/test-design-gate-evidence.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SCRIPT="$SCRIPT_DIR/design-gate-evidence.sh"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3)}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/corral-design-gate-test.XXXXXX")"
trap 'rm -rf -- "$WORK"' EXIT

fail() {
  echo "design-gate evidence test failed: $*" >&2
  exit 1
}

mkdir -p "$WORK/bin" "$WORK/output"

"$PYTHON_BIN" - "$WORK/prototype.png" "$WORK/live.png" "$WORK/composite.png" <<'PY'
from pathlib import Path
import struct
import sys
import zlib


def write_png(path, width, height, rgb):
    rows = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))

    def chunk(name, payload):
        return (
            struct.pack(">I", len(payload))
            + name
            + payload
            + struct.pack(">I", zlib.crc32(name + payload) & 0xFFFFFFFF)
        )

    data = b"\x89PNG\r\n\x1a\n"
    data += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(rows))
    data += chunk(b"IEND", b"")
    path.write_bytes(data)


write_png(Path(sys.argv[1]), 1160, 631, (45, 212, 191))
write_png(Path(sys.argv[2]), 32, 24, (88, 166, 255))
write_png(Path(sys.argv[3]), 2400, 960, (13, 17, 23))
PY

cat > "$WORK/bin/chrome" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${CORRAL_TEST_CHROME_FAIL:-0}" == "1" ]]; then
  exit 23
fi
output=""
for argument in "$@"; do
  case "$argument" in
    --screenshot=*) output="${argument#--screenshot=}" ;;
  esac
done
[[ -n "$output" ]]
case "$output" in
  *prototype.png) cp -- "$CORRAL_TEST_PROTOTYPE_PNG" "$output" ;;
  *comparison.png) cp -- "$CORRAL_TEST_COMPOSITE_PNG" "$output" ;;
  *) cp -- "$CORRAL_TEST_LIVE_PNG" "$output" ;;
esac
STUB
chmod +x "$WORK/bin/chrome"

export CHROME_BIN="$WORK/bin/chrome"
export CORRAL_TEST_PROTOTYPE_PNG="$WORK/prototype.png"
export CORRAL_TEST_LIVE_PNG="$WORK/live.png"
export CORRAL_TEST_COMPOSITE_PNG="$WORK/composite.png"

run_capture() {
  bash "$SCRIPT" \
    --issue 211 \
    --surface egui \
    --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
    --live-png "$WORK/live.png" \
    --output-root "$WORK/output" \
    --chrome-timeout-seconds 5
}

run_capture
for artifact in prototype.png live-after.png comparison.png conformance.md capture.log; do
  [[ -s "$WORK/output/issue-211/$artifact" ]] || fail "missing artifact after first run: $artifact"
done

grep -q 'Issue #211' "$WORK/output/issue-211/conformance.md" \
  || fail "provenance does not identify issue #211"
grep -q 'explicit supplied PNG fixture' "$WORK/output/issue-211/conformance.md" \
  || fail "fixture provenance is not explicit"
grep -q '2400x960' "$WORK/output/issue-211/conformance.md" \
  || fail "composite dimensions are not recorded"

before_sha="$(shasum -a 256 "$WORK/output/issue-211/comparison.png" | awk '{print $1}')"
run_capture
after_sha="$(shasum -a 256 "$WORK/output/issue-211/comparison.png" | awk '{print $1}')"
[[ "$before_sha" == "$after_sha" ]] || fail "rerun changed deterministic fixture output"

export CORRAL_TEST_CHROME_FAIL=1
if run_capture >"$WORK/failed.log" 2>&1; then
  fail "a failed browser capture unexpectedly succeeded"
fi
unset CORRAL_TEST_CHROME_FAIL
final_sha="$(shasum -a 256 "$WORK/output/issue-211/comparison.png" | awk '{print $1}')"
[[ "$final_sha" == "$after_sha" ]] || fail "failed rerun replaced the prior evidence bundle"

shopt -s nullglob
staging_entries=("$WORK/output/issue-211"/.design-gate.stage.*)
if [[ "${#staging_entries[@]}" -ne 0 ]]; then
  fail "temporary staging directory survived a failed run"
fi

echo "OK: design-gate evidence fixture seam, dimensions, rerun, and failure preservation"
