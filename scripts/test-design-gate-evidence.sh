#!/usr/bin/env bash
# Hermetic tests for scripts/design-gate-evidence.sh. They cover the supplied
# PNG seam, complete-PNG rejection, visible provenance labels, explicit force
# overwrites, normalized conformance stability, Chrome/egui writer completion,
# argument validation, and the egui wake-command failure path.
#
# Run with one command:
#   bash scripts/test-design-gate-evidence.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SCRIPT="$SCRIPT_DIR/design-gate-evidence.sh"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3)}"
ORIGINAL_PATH="$PATH"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/corral-design-gate-test.XXXXXX")"
trap 'rm -rf -- "$WORK"' EXIT

fail() {
  echo "design-gate evidence test failed: $*" >&2
  exit 1
}

mkdir -p "$WORK/bin" "$WORK/output"

"$PYTHON_BIN" - "$WORK/prototype.png" "$WORK/ios-prototype.png" \
  "$WORK/live.png" "$WORK/composite.png" "$WORK/truncated.png" <<'PY'
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
write_png(Path(sys.argv[2]), 900, 900, (45, 212, 191))
write_png(Path(sys.argv[3]), 32, 24, (88, 166, 255))
write_png(Path(sys.argv[4]), 2400, 960, (13, 17, 23))
Path(sys.argv[5]).write_bytes(Path(sys.argv[3]).read_bytes()[:-12])
PY

cat > "$WORK/bin/chrome" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${CORRAL_TEST_CHROME_FAIL:-0}" == "1" ]]; then
  exit 23
fi
output=""
url=""
for argument in "$@"; do
  case "$argument" in
    --screenshot=*) output="${argument#--screenshot=}" ;;
    file://*) url="$argument" ;;
  esac
done
[[ -n "$output" && -n "$url" ]]
html_path="${url#file://}"
case "$output" in
  *prototype.png) cp -- "$CORRAL_TEST_PROTOTYPE_PNG" "$output" ;;
  *comparison.png)
    grep -q "issue #${CORRAL_TEST_EXPECTED_ISSUE}" "$html_path"
    grep -q "$CORRAL_TEST_EXPECTED_CAPTURE_KIND" "$html_path"
    cp -- "$CORRAL_TEST_COMPOSITE_PNG" "$output"
    ;;
  *) cp -- "$CORRAL_TEST_LIVE_PNG" "$output" ;;
esac
sleep 1
touch "$CORRAL_TEST_CHROME_FINISHED"
STUB
chmod +x "$WORK/bin/chrome"

cat > "$WORK/bin/curl" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
url=""
for argument in "$@"; do url="$argument"; done
case "$url" in
  */healthz) printf 'ok\n' ;;
  */snapshot) printf '%s\n' '{"agents":{"agent-1":{"state":"working","title":"fixture"}}}' ;;
  *) exit 22 ;;
esac
STUB
chmod +x "$WORK/bin/curl"

cat > "$WORK/bin/egui" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' 'native screenshot evidence selected live agent; fixture writer'
cp -- "$CORRAL_TEST_LIVE_PNG" "$CORRAL_UI_SCREENSHOT"
sleep 1
touch "$CORRAL_TEST_EGUI_FINISHED"
STUB
chmod +x "$WORK/bin/egui"

export CHROME_BIN="$WORK/bin/chrome"
export CORRAL_TEST_PROTOTYPE_PNG="$WORK/prototype.png"
export CORRAL_TEST_LIVE_PNG="$WORK/live.png"
export CORRAL_TEST_COMPOSITE_PNG="$WORK/composite.png"
export CORRAL_TEST_CHROME_FINISHED="$WORK/chrome-finished"
export CORRAL_TEST_EGUI_FINISHED="$WORK/egui-finished"
export CORRAL_TEST_EXPECTED_ISSUE=211
export CORRAL_TEST_EXPECTED_CAPTURE_KIND="explicit supplied PNG fixture"

normalized_conformance_sha() {
  "$PYTHON_BIN" - "$1" <<'PY'
from pathlib import Path
import hashlib
import re
import sys

text = Path(sys.argv[1]).read_text(encoding="utf-8")
normalized = re.sub(r"Generated: `[^`]+`", "Generated: `TIMESTAMP`", text)
normalized = normalized.replace(" --force", "")
print(hashlib.sha256(normalized.encode()).hexdigest())
PY
}

run_capture() {
  bash "$SCRIPT" \
    --issue 211 \
    --surface egui \
    --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
    --live-png "$WORK/live.png" \
    --output-root "$WORK/output" \
    --chrome-timeout-seconds 5 \
    "$@"
}

help_output="$(bash "$SCRIPT" --help)"
[[ "$help_output" == *"Usage:"* ]] || fail "help output has no usage synopsis"
[[ "$help_output" != *"#!/usr/bin/env bash"* ]] || fail "help output leaked shell source"
[[ "$help_output" != *"set -euo pipefail"* ]] || fail "help output leaked shell source"

if bash "$SCRIPT" --issue 211 --surface invalid --dry-run >"$WORK/bad-surface.log" 2>&1; then
  fail "invalid surface unexpectedly succeeded"
fi
grep -q -- "--surface must be egui or ios" "$WORK/bad-surface.log" \
  || fail "invalid surface error was not actionable"

if bash "$SCRIPT" --issue 205 --surface ios --ios-mode live --dry-run \
  >"$WORK/missing-ios-command.log" 2>&1; then
  fail "iOS live dry-run without command unexpectedly succeeded"
fi
grep -q -- "--ios-command" "$WORK/missing-ios-command.log" \
  || fail "missing iOS live command error was not actionable"

rm -f "$CORRAL_TEST_CHROME_FINISHED"
run_capture
[[ -f "$CORRAL_TEST_CHROME_FINISHED" ]] \
  || fail "Chrome writer was not allowed to exit cleanly"
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
before_conformance_sha="$(normalized_conformance_sha "$WORK/output/issue-211/conformance.md")"

if run_capture >"$WORK/no-force.log" 2>&1; then
  fail "existing evidence bundle was overwritten without --force"
fi
grep -q -- "pass --force" "$WORK/no-force.log" \
  || fail "overwrite refusal did not name --force"

rm -f "$CORRAL_TEST_CHROME_FINISHED"
run_capture --force
after_sha="$(shasum -a 256 "$WORK/output/issue-211/comparison.png" | awk '{print $1}')"
[[ "$before_sha" == "$after_sha" ]] || fail "rerun changed deterministic fixture output"
[[ -f "$CORRAL_TEST_CHROME_FINISHED" ]] \
  || fail "forced rerun did not wait for the Chrome writer"
after_conformance_sha="$(normalized_conformance_sha "$WORK/output/issue-211/conformance.md")"
[[ "$before_conformance_sha" == "$after_conformance_sha" ]] \
  || fail "normalized conformance changed across a forced rerun"

export CORRAL_TEST_CHROME_FAIL=1
if run_capture --force >"$WORK/failed.log" 2>&1; then
  fail "a failed browser capture unexpectedly succeeded"
fi
unset CORRAL_TEST_CHROME_FAIL
final_sha="$(shasum -a 256 "$WORK/output/issue-211/comparison.png" | awk '{print $1}')"
[[ "$final_sha" == "$after_sha" ]] || fail "failed rerun replaced the prior evidence bundle"

if run_capture --force --live-png "$WORK/truncated.png" \
  >"$WORK/truncated.log" 2>&1; then
  fail "truncated PNG unexpectedly passed validation"
fi
grep -E -q 'IEND|IDAT|truncated' "$WORK/truncated.log" \
  || fail "truncated PNG failure was not actionable"

export CORRAL_TEST_EXPECTED_ISSUE=205
export CORRAL_TEST_EXPECTED_CAPTURE_KIND="explicit supplied PNG fixture"
export CORRAL_TEST_PROTOTYPE_PNG="$WORK/ios-prototype.png"
bash "$SCRIPT" \
  --issue 205 \
  --surface ios \
  --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
  --ios-mode demo \
  --live-png "$WORK/live.png" \
  --output-root "$WORK/ios-output" \
  --chrome-timeout-seconds 5
grep -q '900x900' "$WORK/ios-output/issue-205/conformance.md" \
  || fail "iOS prototype render did not use the unclipped 900x900 viewport"

export CORRAL_TEST_PROTOTYPE_PNG="$WORK/prototype.png"
export CORRAL_TEST_EXPECTED_ISSUE=213
export CORRAL_TEST_EXPECTED_CAPTURE_KIND="native egui viewport screenshot"
rm -f "$CORRAL_TEST_EGUI_FINISHED"
PATH="$WORK/bin:$ORIGINAL_PATH" bash "$SCRIPT" \
  --issue 213 \
  --surface egui \
  --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
  --egui-binary "$WORK/bin/egui" \
  --live-agent agent-1 \
  --host-url http://fixture \
  --no-build \
  --delay-ms 1 \
  --timeout-seconds 5 \
  --output-root "$WORK/egui-output" \
  --chrome-timeout-seconds 5
[[ -f "$CORRAL_TEST_EGUI_FINISHED" ]] \
  || fail "egui writer was not allowed to exit cleanly"
grep -q 'native egui viewport screenshot' "$WORK/egui-output/issue-213/conformance.md" \
  || fail "egui capture provenance is missing"

export CORRAL_TEST_EXPECTED_ISSUE=214
if PATH="$WORK/bin:$ORIGINAL_PATH" bash "$SCRIPT" \
  --issue 214 \
  --surface egui \
  --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
  --egui-binary "$WORK/bin/egui" \
  --live-agent agent-1 \
  --host-url http://fixture \
  --no-build \
  --delay-ms 1 \
  --timeout-seconds 5 \
  --egui-wake-command 'exit 17' \
  --output-root "$WORK/wake-output" \
  --chrome-timeout-seconds 5 \
  >"$WORK/wake-failure.log" 2>&1; then
  fail "failed egui wake command unexpectedly succeeded"
fi
grep -q 'egui wake command failed' "$WORK/wake-failure.log" \
  || fail "wake-command failure was not actionable"
[[ ! -e "$WORK/wake-output/issue-214/comparison.png" ]] \
  || fail "wake-command failure published evidence"

shopt -s nullglob
staging_entries=(
  "$WORK/output/issue-211"/.design-gate.stage.*
  "$WORK/egui-output/issue-213"/.design-gate.stage.*
  "$WORK/wake-output/issue-214"/.design-gate.stage.*
)
if [[ "${#staging_entries[@]}" -ne 0 ]]; then
  fail "temporary staging directory survived a failed run"
fi

echo "OK: design-gate evidence validation, provenance, reruns, capture seams, and failure paths"
