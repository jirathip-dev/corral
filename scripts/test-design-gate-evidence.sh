#!/usr/bin/env bash
# Hermetic tests for scripts/design-gate-evidence.sh. They cover the supplied
# PNG seam, complete-PNG rejection, exit-during-validation rechecking, visible
# provenance labels, explicit force overwrites, normalized conformance
# stability, complete-but-lingering writers, TERM-ignoring child escalation,
# structural prototype rejection through real Chrome, Chrome trust-boundary
# flags, argument validation, and the egui wake-command failure path.
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
if [[ -n "${CORRAL_TEST_CHROME_PID_FILE:-}" ]]; then
  printf '%s\n' "$$" >"$CORRAL_TEST_CHROME_PID_FILE"
fi
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
  *prototype.png)
    cp -- "$CORRAL_TEST_PROTOTYPE_PNG" "$output"
    if [[ -n "${CORRAL_TEST_PROTOTYPE_HTML:-}" ]]; then
      cp -- "$html_path" "$CORRAL_TEST_PROTOTYPE_HTML"
    fi
    ;;
  *comparison.png)
    grep -q "issue #${CORRAL_TEST_EXPECTED_ISSUE}" "$html_path"
    grep -q "$CORRAL_TEST_EXPECTED_CAPTURE_KIND" "$html_path"
    cp -- "$CORRAL_TEST_COMPOSITE_PNG" "$output"
    ;;
  *) cp -- "$CORRAL_TEST_LIVE_PNG" "$output" ;;
esac
if [[ -n "${CORRAL_TEST_CHROME_ARGS_FILE:-}" ]]; then
  printf '%s\n' "$*" >"$CORRAL_TEST_CHROME_ARGS_FILE"
fi
touch "$CORRAL_TEST_CHROME_FINISHED"
if [[ "${CORRAL_TEST_CHROME_LINGER:-0}" == "1" ]]; then
  exec "$PYTHON_BIN" - <<'PY'
import signal
import time

signal.signal(signal.SIGTERM, signal.SIG_IGN)
while True:
    time.sleep(1)
PY
fi
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
if [[ -n "${CORRAL_TEST_EGUI_PID_FILE:-}" ]]; then
  printf '%s\n' "$$" >"$CORRAL_TEST_EGUI_PID_FILE"
fi
printf '%s\n' 'native screenshot evidence selected live agent; fixture writer'
printf '%s\n' 'requesting viewport screenshot'
printf '%s\n' 'screenshot event received'
printf '%s\n' 'screenshot saved — exiting'
if [[ -n "${CORRAL_UI_WINDOW_DIAGNOSTIC_LOG:-}" ]]; then
  printf '%s\n' '{"action":"dispatch_evaluation","cg_owner_pid_match":true,"cg_window_list":[{"bounds":{"height":100.0,"width":100.0,"x":0.0,"y":0.0},"layer":0,"onscreen":true,"placement":0,"window_number":9}],"exact_pid_match":true,"frontmost":true,"frontmost_application_matches_target":true,"frontmost_application_pid":42,"key_window":true,"main_window":true,"non_target_window_count":3,"pid":42,"probe_ok":true,"process_visible":true,"reason_code":"dispatch_ready","visible_gate":true,"frontmost_gate":true,"window_visible":true}' >"$CORRAL_UI_WINDOW_DIAGNOSTIC_LOG"
fi
if [[ -n "${CORRAL_TEST_UI_CONFIG_ROOT:-}" ]]; then
  case "${CORRAL_UI_CONFIG_DIR:-}" in
    "$CORRAL_TEST_UI_CONFIG_ROOT"/.design-gate.stage.*/ui-config) ;;
    *)
      printf '%s\n' "egui did not receive an isolated staged config directory: ${CORRAL_UI_CONFIG_DIR:-<unset>}" >&2
      exit 1
      ;;
  esac
  [[ -s "${CORRAL_UI_CONFIG_DIR}/config.json" ]] \
    || { printf '%s\n' 'egui staged config is missing the seeded config.json' >&2; exit 1; }
fi
mode="${CORRAL_TEST_EGUI_MODE:-normal}"
if [[ "$mode" == "partial-then-linger" || "$mode" == "partial-stuck" || "$mode" == "race-during-validation" ]]; then
  exec "$PYTHON_BIN" - "$CORRAL_TEST_LIVE_PNG" "$CORRAL_UI_SCREENSHOT" "$mode" <<'PY'
from pathlib import Path
import os
import signal
import sys
import time

source = Path(sys.argv[1]).read_bytes()
destination = Path(sys.argv[2])
mode = sys.argv[3]
split = max(1, len(source) // 2)
destination.write_bytes(source[:split])
if mode == "race-during-validation":
    marker = Path(os.environ["CORRAL_TEST_PNG_RACE_VALIDATE_STARTED"])
    deadline = time.monotonic() + 5
    while not marker.exists() and time.monotonic() < deadline:
        time.sleep(0.01)
    if not marker.exists():
        raise SystemExit("PNG race validator did not start")
    with destination.open("ab") as stream:
        stream.write(source[split:])
    Path(os.environ["CORRAL_TEST_PNG_RACE_WRITER_FINISHED"]).touch()
elif mode == "partial-then-linger":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    time.sleep(0.5)
    with destination.open("ab") as stream:
        stream.write(source[split:])
    Path(os.environ["CORRAL_TEST_EGUI_FINISHED"]).touch()
    while True:
        time.sleep(1)
else:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    time.sleep(10)
PY
else
  cp -- "$CORRAL_TEST_LIVE_PNG" "$CORRAL_UI_SCREENSHOT"
  touch "$CORRAL_TEST_EGUI_FINISHED"
fi
if [[ "$mode" == "term-ignore" ]]; then
  exec "$PYTHON_BIN" - <<'PY'
import signal
import time

signal.signal(signal.SIGTERM, signal.SIG_IGN)
while True:
    time.sleep(1)
PY
fi
STUB
chmod +x "$WORK/bin/egui"

cat > "$WORK/bin/python-race" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${CORRAL_TEST_PNG_RACE:-0}" == "1" \
  && "${1:-}" == "-" \
  && "${2:-}" == *"live-after.png" \
  && ! -e "$CORRAL_TEST_PNG_RACE_INTERCEPTED" ]]; then
  touch "$CORRAL_TEST_PNG_RACE_INTERCEPTED"
  # Run the real validator against the partial file first. Its rejection is
  # part of this seam; then let the writer finish and exit while reporting the
  # first validation as failed so the production recheck is exercised.
  if "$CORRAL_TEST_REAL_PYTHON" "$@" >/dev/null 2>&1; then
    echo "race validator unexpectedly accepted the partial PNG" >&2
    exit 1
  fi
  touch "$CORRAL_TEST_PNG_RACE_VALIDATE_STARTED"
  deadline=$((SECONDS + 5))
  while [[ ! -e "$CORRAL_TEST_PNG_RACE_WRITER_FINISHED" \
    && $SECONDS -lt $deadline ]]; do
    sleep 0.01
  done
  [[ -e "$CORRAL_TEST_PNG_RACE_WRITER_FINISHED" ]]
  exit 1
fi
exec "$CORRAL_TEST_REAL_PYTHON" "$@"
STUB
chmod +x "$WORK/bin/python-race"

cat > "$WORK/malformed-prototype.html" <<'HTML'
<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Malformed design gate fixture</title><style>.desk {}</style></head>
<body>
  <main class="rack">
    <div class="wrapper">
      <section class="frame"><div class="desk"></div></section>
    </div>
  </main>
</body>
</html>
HTML

cat > "$WORK/template-prototype.html" <<'HTML'
<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Template design gate fixture</title><style>.desk {}</style></head>
<body>
  <main class="rack">
    <section class="frame">
      <template><div class="desk"></div></template>
    </section>
  </main>
</body>
</html>
HTML

export CHROME_BIN="$WORK/bin/chrome"
export PYTHON_BIN
export CORRAL_TEST_REAL_PYTHON="$PYTHON_BIN"
export CORRAL_TEST_PROTOTYPE_PNG="$WORK/prototype.png"
export CORRAL_TEST_LIVE_PNG="$WORK/live.png"
export CORRAL_TEST_COMPOSITE_PNG="$WORK/composite.png"
export CORRAL_TEST_CHROME_FINISHED="$WORK/chrome-finished"
export CORRAL_TEST_EGUI_FINISHED="$WORK/egui-finished"
export CORRAL_TEST_CHROME_ARGS_FILE="$WORK/chrome-args"
export CORRAL_TEST_CHROME_PID_FILE="$WORK/chrome.pid"
export CORRAL_TEST_EGUI_PID_FILE="$WORK/egui.pid"
export CORRAL_TEST_PROTOTYPE_HTML="$WORK/prototype-view.html"
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

assert_stopped() {
  local label="$1"
  local pid_file="$2"
  [[ -s "$pid_file" ]] || fail "$label did not publish its pid"
  local pid
  pid="$(<"$pid_file")"
  if ps -p "$pid" >/dev/null 2>&1; then
    fail "$label child pid $pid survived bounded cleanup"
  fi
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

rm -f "$CORRAL_TEST_CHROME_FINISHED" "$CORRAL_TEST_CHROME_PID_FILE" "$CORRAL_TEST_CHROME_ARGS_FILE"
export CORRAL_TEST_CHROME_LINGER=1
run_capture
unset CORRAL_TEST_CHROME_LINGER
[[ -f "$CORRAL_TEST_CHROME_FINISHED" ]] \
  || fail "Chrome writer was not allowed to exit cleanly"
assert_stopped "lingering Chrome" "$CORRAL_TEST_CHROME_PID_FILE"
grep -q -- "--remote-debugging-address=127.0.0.1" "$CORRAL_TEST_CHROME_ARGS_FILE" \
  || fail "Chrome DevTools endpoint was not explicitly loopback-bound"
grep -q -- "--remote-allow-origins=http://127.0.0.1" "$CORRAL_TEST_CHROME_ARGS_FILE" \
  || fail "Chrome DevTools origin was not narrowed to loopback"
if grep -q ':has(' "$CORRAL_TEST_PROTOTYPE_HTML"; then
  fail "generated prototype view still has a load-bearing :has() selector"
fi
grep -q 'design-gate-surface-script' "$CORRAL_TEST_PROTOTYPE_HTML" \
  || fail "generated prototype view has no surface-selection script"
grep -q 'design-gate-target' "$CORRAL_TEST_PROTOTYPE_HTML" \
  || fail "generated prototype view has no explicit target marker"

REAL_CHROME_BIN=""
for candidate in \
  "$(command -v google-chrome 2>/dev/null || true)" \
  "$(command -v google-chrome-stable 2>/dev/null || true)" \
  "$(command -v chromium 2>/dev/null || true)" \
  "$(command -v chromium-browser 2>/dev/null || true)" \
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  "/Applications/Chromium.app/Contents/MacOS/Chromium" \
  "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"; do
  if [[ -n "$candidate" && -x "$candidate" ]]; then
    REAL_CHROME_BIN="$candidate"
    break
  fi
done
if [[ -n "$REAL_CHROME_BIN" ]]; then
  assert_rejected_prototype() {
    local name="$1"
    local issue="$2"
    local prototype="$3"
    local output_root="$WORK/$name-output"
    local log_path="$WORK/$name.log"

    if CHROME_BIN="$REAL_CHROME_BIN" bash "$SCRIPT" \
      --issue "$issue" \
      --surface egui \
      --prototype "$prototype" \
      --live-png "$WORK/live.png" \
      --output-root "$output_root" \
      --chrome-timeout-seconds 5 \
      >"$log_path" 2>&1; then
      fail "$name prototype unexpectedly published evidence"
    fi
    grep -q 'body > .rack > .frame' "$log_path" \
      || fail "$name prototype failure did not identify the required structure"
    for artifact in prototype.png live-after.png comparison.png conformance.md; do
      [[ ! -e "$output_root/issue-$issue/$artifact" ]] \
        || fail "$name prototype published $artifact"
    done
  }

  assert_rejected_prototype malformed 217 "$WORK/malformed-prototype.html"
  assert_rejected_prototype template 218 "$WORK/template-prototype.html"
else
  echo "SKIP: real Chrome unavailable for structural prototype regression" >&2
fi

for artifact in prototype.png live-after.png comparison.png conformance.md capture.log; do
  [[ -s "$WORK/output/issue-211/$artifact" ]] || fail "missing artifact after first run: $artifact"
done

grep -q 'Issue #211' "$WORK/output/issue-211/conformance.md" \
  || fail "provenance does not identify issue #211"
grep -q 'explicit supplied PNG fixture' "$WORK/output/issue-211/conformance.md" \
  || fail "fixture provenance is not explicit"
grep -q '2400x960' "$WORK/output/issue-211/conformance.md" \
  || fail "composite dimensions are not recorded"
grep -q 'complete, CRC-checked PNG' "$WORK/output/issue-211/conformance.md" \
  || fail "complete-PNG success contract is not recorded"
grep -q 'loopback-only' "$WORK/output/issue-211/conformance.md" \
  || fail "Chrome trust boundary is not recorded"
grep -F -q '| `capture.log` | `n/a` |' "$WORK/output/issue-211/conformance.md" \
  || fail "capture.log row is not recorded in the artifact table"
grep -E -q '\| `capture\.log` \| `n/a` \| `[0-9a-f]{64}` \|' \
  "$WORK/output/issue-211/conformance.md" \
  || fail "capture.log SHA-256 is not recorded in the artifact table"
grep -q 'scripts/design-gate-evidence.sh --issue 211 --surface egui' \
  "$WORK/output/issue-211/conformance.md" \
  || fail "conformance invocation was not normalized to a stable command"

"$PYTHON_BIN" - "$SCRIPT_DIR/verify-design-gate-egui-evidence.py" "$WORK" <<'PY'
import hashlib
import importlib.util
from pathlib import Path
import struct
import sys
import zlib

spec = importlib.util.spec_from_file_location("verify_egui_evidence", sys.argv[1])
if spec is None or spec.loader is None:
    raise SystemExit("could not load the egui evidence verifier")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

bundle = Path(sys.argv[2]) / "artifact-manifest-regression"
bundle.mkdir()


def write_png(path, width, height):
    rows = b"".join(b"\x00" + b"\x12\x34\x56" * width for _ in range(height))

    def chunk(name, payload):
        return (
            struct.pack(">I", len(payload))
            + name
            + payload
            + struct.pack(">I", zlib.crc32(name + payload) & 0xFFFFFFFF)
        )

    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )


write_png(bundle / "prototype.png", 1160, 631)
write_png(bundle / "live-after.png", 2640, 1720)
write_png(bundle / "comparison.png", 2400, 960)
(bundle / "capture.log").write_text("native capture\n", encoding="utf-8")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def table():
    return "\n".join(
        [
            f"| `prototype.png` | `1160x631` | `{digest(bundle / 'prototype.png')}` |",
            f"| `live-after.png` | `2640x1720` | `{digest(bundle / 'live-after.png')}` |",
            f"| `comparison.png` | `2400x960` | `{digest(bundle / 'comparison.png')}` |",
            f"| `capture.log` | `n/a` | `{digest(bundle / 'capture.log')}` |",
        ]
    )


module.verify_artifact_manifest(bundle, table(), "manifest regression")


def expect_failure(label, callback):
    try:
        callback()
    except SystemExit:
        return
    raise SystemExit(f"{label} unexpectedly passed")


expect_failure(
    "swapped dimensions",
    lambda: module.verify_artifact_manifest(
        bundle,
        table().replace("| `prototype.png` | `1160x631` |", "| `prototype.png` | `2640x1720` |"),
        "swapped dimensions",
    ),
)
write_png(bundle / "live-after.png", 32, 24)
expect_failure(
    "undersized live PNG",
    lambda: module.verify_artifact_manifest(
        bundle,
        table().replace("| `live-after.png` | `2640x1720` |", "| `live-after.png` | `32x24` |"),
        "undersized live PNG",
    ),
)
write_png(bundle / "live-after.png", 2640, 1720)
old_table = table()
(bundle / "live-after.png").write_bytes((bundle / "live-after.png").read_bytes() + b"changed")
expect_failure(
    "artifact hash mismatch",
    lambda: module.verify_artifact_manifest(bundle, old_table, "artifact hash mismatch"),
)
print("verified exact artifact dimensions, recorded hashes, and negative paths")
PY

"$PYTHON_BIN" - "$SCRIPT_DIR/design-gate-content-identity.py" "$REPO_DIR/Cargo.lock" <<'PY'
import importlib.util
from pathlib import Path
import sys

spec = importlib.util.spec_from_file_location("content_identity", sys.argv[1])
if spec is None or spec.loader is None:
    raise SystemExit("could not load the content identity helper")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
lock = Path(sys.argv[2]).read_text(encoding="utf-8")
original = module.renderer_dependency_fingerprint(lock)
eframe_change = lock.replace(
    'name = "eframe"\nversion = "0.36.1"',
    'name = "eframe"\nversion = "0.36.2"',
    1,
)
assert eframe_change != lock
assert module.renderer_dependency_fingerprint(eframe_change) != original
unrelated_change = lock.replace(
    'name = "lazy_static"\nversion = "1.5.0"',
    'name = "lazy_static"\nversion = "1.5.1"',
    1,
)
assert unrelated_change != lock
assert module.renderer_dependency_fingerprint(unrelated_change) == original
print("verified narrow eframe/wgpu lockfile fingerprint")
PY

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
mkdir -p "$WORK/ui-config-seed"
printf '%s\n' '{"host_url":"http://fixture"}' >"$WORK/ui-config-seed/config.json"
export CORRAL_UI_CONFIG_SEED_DIR="$WORK/ui-config-seed"
export CORRAL_TEST_UI_CONFIG_ROOT="$WORK/egui-output/issue-213"
rm -f "$CORRAL_TEST_EGUI_FINISHED" "$CORRAL_TEST_EGUI_PID_FILE"
export CORRAL_TEST_EGUI_MODE=partial-then-linger
egui_partial_start_ns="$($PYTHON_BIN -c 'import time; print(time.monotonic_ns())')"
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
egui_partial_end_ns="$($PYTHON_BIN -c 'import time; print(time.monotonic_ns())')"
egui_partial_elapsed_ms="$(( (egui_partial_end_ns - egui_partial_start_ns) / 1000000 ))"
(( egui_partial_elapsed_ms >= 400 )) \
  || fail "egui accepted the partial PNG before the writer completed (${egui_partial_elapsed_ms}ms)"
[[ -f "$CORRAL_TEST_EGUI_FINISHED" ]] \
  || fail "egui writer was not allowed to exit cleanly"
assert_stopped "partial then lingering egui" "$CORRAL_TEST_EGUI_PID_FILE"
grep -q 'native egui viewport screenshot' "$WORK/egui-output/issue-213/conformance.md" \
  || fail "egui capture provenance is missing"
unset CORRAL_TEST_EGUI_MODE
unset CORRAL_UI_CONFIG_SEED_DIR CORRAL_TEST_UI_CONFIG_ROOT

export CORRAL_TEST_EXPECTED_ISSUE=217
export CORRAL_TEST_PNG_RACE=1
export CORRAL_TEST_PNG_RACE_INTERCEPTED="$WORK/png-race-intercepted"
export CORRAL_TEST_PNG_RACE_VALIDATE_STARTED="$WORK/png-race-validate-started"
export CORRAL_TEST_PNG_RACE_WRITER_FINISHED="$WORK/png-race-writer-finished"
rm -f \
  "$CORRAL_TEST_EGUI_FINISHED" \
  "$CORRAL_TEST_EGUI_PID_FILE" \
  "$CORRAL_TEST_PNG_RACE_INTERCEPTED" \
  "$CORRAL_TEST_PNG_RACE_VALIDATE_STARTED" \
  "$CORRAL_TEST_PNG_RACE_WRITER_FINISHED"
export CORRAL_TEST_EGUI_MODE=race-during-validation
PYTHON_BIN="$WORK/bin/python-race" PATH="$WORK/bin:$ORIGINAL_PATH" bash "$SCRIPT" \
  --issue 217 \
  --surface egui \
  --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
  --egui-binary "$WORK/bin/egui" \
  --live-agent agent-1 \
  --host-url http://fixture \
  --no-build \
  --delay-ms 1 \
  --timeout-seconds 5 \
  --output-root "$WORK/png-race-output" \
  --chrome-timeout-seconds 5
[[ -f "$CORRAL_TEST_PNG_RACE_INTERCEPTED" ]] \
  || fail "PNG race validator seam was not exercised"
[[ -f "$CORRAL_TEST_PNG_RACE_VALIDATE_STARTED" ]] \
  || fail "PNG race did not validate the partial file before writer completion"
[[ -f "$CORRAL_TEST_PNG_RACE_WRITER_FINISHED" ]] \
  || fail "PNG race writer did not finish during validation"
[[ -s "$WORK/png-race-output/issue-217/comparison.png" ]] \
  || fail "exit-during-validation recheck did not publish complete evidence"
unset CORRAL_TEST_EGUI_MODE CORRAL_TEST_PNG_RACE

export CORRAL_TEST_EXPECTED_ISSUE=215
rm -f "$CORRAL_TEST_EGUI_FINISHED" "$CORRAL_TEST_EGUI_PID_FILE"
export CORRAL_TEST_EGUI_MODE=term-ignore
PATH="$WORK/bin:$ORIGINAL_PATH" bash "$SCRIPT" \
  --issue 215 \
  --surface egui \
  --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
  --egui-binary "$WORK/bin/egui" \
  --live-agent agent-1 \
  --host-url http://fixture \
  --no-build \
  --delay-ms 1 \
  --timeout-seconds 5 \
  --output-root "$WORK/term-ignore-output" \
  --chrome-timeout-seconds 5
[[ -f "$CORRAL_TEST_EGUI_FINISHED" ]] \
  || fail "TERM-ignoring egui writer did not publish its complete PNG"
assert_stopped "TERM-ignoring egui" "$CORRAL_TEST_EGUI_PID_FILE"
unset CORRAL_TEST_EGUI_MODE

export CORRAL_TEST_EXPECTED_ISSUE=216
rm -f "$CORRAL_TEST_EGUI_FINISHED" "$CORRAL_TEST_EGUI_PID_FILE"
export CORRAL_TEST_EGUI_MODE=partial-stuck
if PATH="$WORK/bin:$ORIGINAL_PATH" bash "$SCRIPT" \
  --issue 216 \
  --surface egui \
  --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
  --egui-binary "$WORK/bin/egui" \
  --live-agent agent-1 \
  --host-url http://fixture \
  --no-build \
  --delay-ms 1 \
  --timeout-seconds 1 \
  --output-root "$WORK/partial-stuck-output" \
  --chrome-timeout-seconds 5 \
  >"$WORK/partial-stuck.log" 2>&1; then
  fail "stuck partial PNG unexpectedly succeeded"
fi
grep -q 'complete PNG' "$WORK/partial-stuck.log" \
  || fail "stuck partial PNG failure did not name the complete-PNG contract"
assert_stopped "stuck partial egui" "$CORRAL_TEST_EGUI_PID_FILE"
[[ ! -e "$WORK/partial-stuck-output/issue-216/comparison.png" ]] \
  || fail "stuck partial PNG was published as evidence"
unset CORRAL_TEST_EGUI_MODE

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
  "$WORK/term-ignore-output/issue-215"/.design-gate.stage.*
  "$WORK/partial-stuck-output/issue-216"/.design-gate.stage.*
  "$WORK/wake-output/issue-214"/.design-gate.stage.*
)
if [[ "${#staging_entries[@]}" -ne 0 ]]; then
  fail "temporary staging directory survived a failed run"
fi

echo "OK: design-gate evidence validation, complete-PNG contract, bounded cleanup, trust boundary, capture seams, and failure paths"
