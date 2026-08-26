#!/usr/bin/env bash
# Hermetic tests for scripts/design-gate-evidence.sh. They cover the supplied
# PNG seam, complete-PNG rejection, exit-during-validation rechecking, visible
# provenance labels, canonical symlink/wrapper identity (including spaces),
# stable repo-relative paths across cwd spellings, byte-stable conformance,
# lossless slash-prefixed argv, opaque command-path redaction, targeted note
# redaction, load-bearing path normalization failures, complete-but-lingering
# writers, TERM-ignoring child escalation, bounded raw-byte logs with invalid
# UTF-8 and configured worktree roots, a bounded generic-worktree scan,
# structural prototype rejection through real Chrome, Chrome trust-boundary
# flags, argument validation, locked atomic publication rollback, and the egui
# wake-command failure path.
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

ln -s "$SCRIPT" "$WORK/design-gate-link.sh"
ln -s "$SCRIPT" "$WORK/design gate.sh"
cat >"$WORK/design-gate-wrapper.sh" <<WRAPPER
#!/usr/bin/env bash
set -euo pipefail
exec "$WORK/design-gate-link.sh" "\$@"
WRAPPER
chmod +x "$WORK/design-gate-wrapper.sh"

"$PYTHON_BIN" - "$WORK/prototype.png" "$WORK/ios-prototype.png" \
  "$WORK/live.png" "$WORK/composite.png" "$WORK/truncated.png" \
  "$WORK/invalid-raster.png" "$WORK/invalid-filter.png" \
  "$WORK/interlaced.png" "$WORK/invalid-palette.png" \
  "$WORK/invalid-palette-size.png" "$WORK/invalid-palette-index.png" \
  "$WORK/invalid-grayscale-palette.png" "$WORK/nonconsecutive-idat.png" \
  "$WORK/valid-palette.png" "$WORK/unknown-critical.png" \
  "$WORK/indexed-trns-before-plte.png" "$WORK/trns-after-idat.png" \
  "$WORK/duplicate-trns.png" "$WORK/truecolor-trns-before-plte.png" \
  "$WORK/truecolor-bkgd-before-plte.png" "$WORK/invalid-reserved-chunk.png" \
  "$WORK/invalid-trns-sample.png" <<'PY'
from pathlib import Path
import struct
import sys
import zlib


def chunk(name, payload):
    return (
        struct.pack(">I", len(payload))
        + name
        + payload
        + struct.pack(">I", zlib.crc32(name + payload) & 0xFFFFFFFF)
    )


def write_png(path, width, height, rgb):
    rows = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))

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
invalid_raster = b"\x89PNG\r\n\x1a\n"
invalid_raster += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 10, 10, 8, 2, 0, 0, 0)
)
invalid_raster += chunk(b"IDAT", zlib.compress(b""))
invalid_raster += chunk(b"IEND", b"")
Path(sys.argv[6]).write_bytes(invalid_raster)
invalid_filter = b"\x89PNG\r\n\x1a\n"
invalid_filter += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
)
invalid_filter += chunk(b"IDAT", zlib.compress(b"\x05\x00\x00\x00"))
invalid_filter += chunk(b"IEND", b"")
Path(sys.argv[7]).write_bytes(invalid_filter)
interlaced = b"\x89PNG\r\n\x1a\n"
interlaced += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 1)
)
interlaced += chunk(b"IDAT", zlib.compress(b"\x00\x00\x00\x00"))
interlaced += chunk(b"IEND", b"")
Path(sys.argv[8]).write_bytes(interlaced)
invalid_palette = b"\x89PNG\r\n\x1a\n"
invalid_palette += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 3, 0, 0, 0)
)
invalid_palette += chunk(b"IDAT", zlib.compress(b"\x00\x00"))
invalid_palette += chunk(b"IEND", b"")
Path(sys.argv[9]).write_bytes(invalid_palette)
invalid_palette_size = b"\x89PNG\r\n\x1a\n"
invalid_palette_size += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 1, 3, 0, 0, 0)
)
invalid_palette_size += chunk(b"PLTE", b"\x00\x00\x00\xff\xff\xff\x80\x80\x80")
invalid_palette_size += chunk(b"IDAT", zlib.compress(b"\x00\x00"))
invalid_palette_size += chunk(b"IEND", b"")
Path(sys.argv[10]).write_bytes(invalid_palette_size)
invalid_palette_index = b"\x89PNG\r\n\x1a\n"
invalid_palette_index += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 3, 0, 0, 0)
)
invalid_palette_index += chunk(b"PLTE", b"\x00\x00\x00")
invalid_palette_index += chunk(b"IDAT", zlib.compress(b"\x00\x01"))
invalid_palette_index += chunk(b"IEND", b"")
Path(sys.argv[11]).write_bytes(invalid_palette_index)
invalid_grayscale_palette = b"\x89PNG\r\n\x1a\n"
invalid_grayscale_palette += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 0, 0, 0, 0)
)
invalid_grayscale_palette += chunk(b"PLTE", b"\x00\x00\x00")
invalid_grayscale_palette += chunk(b"IDAT", zlib.compress(b"\x00\x00"))
invalid_grayscale_palette += chunk(b"IEND", b"")
Path(sys.argv[12]).write_bytes(invalid_grayscale_palette)
nonconsecutive_idat = b"\x89PNG\r\n\x1a\n"
nonconsecutive_idat += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
)
compressed = zlib.compress(b"\x00\x00\x00\x00")
split = max(1, len(compressed) // 2)
nonconsecutive_idat += chunk(b"IDAT", compressed[:split])
nonconsecutive_idat += chunk(b"tEXt", b"comment\x00between IDAT chunks")
nonconsecutive_idat += chunk(b"IDAT", compressed[split:])
nonconsecutive_idat += chunk(b"IEND", b"")
Path(sys.argv[13]).write_bytes(nonconsecutive_idat)
valid_palette = b"\x89PNG\r\n\x1a\n"
valid_palette += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 1, 3, 0, 0, 0)
)
valid_palette += chunk(b"PLTE", b"\x00\x00\x00\xff\xff\xff")
valid_palette += chunk(b"IDAT", zlib.compress(b"\x00\x01"))
valid_palette += chunk(b"IEND", b"")
Path(sys.argv[14]).write_bytes(valid_palette)
unknown_critical = b"\x89PNG\r\n\x1a\n"
unknown_critical += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
)
unknown_critical += chunk(b"ABCD", b"")
unknown_critical += chunk(b"IDAT", zlib.compress(b"\x00\x00\x00\x00"))
unknown_critical += chunk(b"IEND", b"")
Path(sys.argv[15]).write_bytes(unknown_critical)
indexed_trns_before_plte = b"\x89PNG\r\n\x1a\n"
indexed_trns_before_plte += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 3, 0, 0, 0)
)
indexed_trns_before_plte += chunk(b"tRNS", b"\x00")
indexed_trns_before_plte += chunk(b"PLTE", b"\x00\x00\x00")
indexed_trns_before_plte += chunk(b"IDAT", zlib.compress(b"\x00\x00"))
indexed_trns_before_plte += chunk(b"IEND", b"")
Path(sys.argv[16]).write_bytes(indexed_trns_before_plte)
trns_after_idat = b"\x89PNG\r\n\x1a\n"
trns_after_idat += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
)
trns_after_idat += chunk(b"IDAT", zlib.compress(b"\x00\x00\x00\x00"))
trns_after_idat += chunk(b"tRNS", b"\x00\x00\x00\x00\x00\x00")
trns_after_idat += chunk(b"IEND", b"")
Path(sys.argv[17]).write_bytes(trns_after_idat)
duplicate_trns = b"\x89PNG\r\n\x1a\n"
duplicate_trns += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
)
duplicate_trns += chunk(b"tRNS", b"\x00\x00\x00\x00\x00\x00")
duplicate_trns += chunk(b"tRNS", b"\x00\x00\x00\x00\x00\x00")
duplicate_trns += chunk(b"IDAT", zlib.compress(b"\x00\x00\x00\x00"))
duplicate_trns += chunk(b"IEND", b"")
Path(sys.argv[18]).write_bytes(duplicate_trns)
truecolor_trns_before_plte = b"\x89PNG\r\n\x1a\n"
truecolor_trns_before_plte += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
)
truecolor_trns_before_plte += chunk(b"tRNS", b"\x00\x00\x00\x00\x00\x00")
truecolor_trns_before_plte += chunk(b"PLTE", b"\x00\x00\x00")
truecolor_trns_before_plte += chunk(
    b"IDAT", zlib.compress(b"\x00\x00\x00\x00")
)
truecolor_trns_before_plte += chunk(b"IEND", b"")
Path(sys.argv[19]).write_bytes(truecolor_trns_before_plte)
truecolor_bkgd_before_plte = b"\x89PNG\r\n\x1a\n"
truecolor_bkgd_before_plte += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
)
truecolor_bkgd_before_plte += chunk(b"bKGD", b"\x00\x00\x00\x00\x00\x00")
truecolor_bkgd_before_plte += chunk(b"PLTE", b"\x00\x00\x00")
truecolor_bkgd_before_plte += chunk(
    b"IDAT", zlib.compress(b"\x00\x00\x00\x00")
)
truecolor_bkgd_before_plte += chunk(b"IEND", b"")
Path(sys.argv[20]).write_bytes(truecolor_bkgd_before_plte)
invalid_reserved_chunk = b"\x89PNG\r\n\x1a\n"
invalid_reserved_chunk += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
)
invalid_reserved_chunk += chunk(b"abcd", b"")
invalid_reserved_chunk += chunk(
    b"IDAT", zlib.compress(b"\x00\x00\x00\x00")
)
invalid_reserved_chunk += chunk(b"IEND", b"")
Path(sys.argv[21]).write_bytes(invalid_reserved_chunk)
invalid_trns_sample = b"\x89PNG\r\n\x1a\n"
invalid_trns_sample += chunk(
    b"IHDR", struct.pack(">IIBBBBB", 1, 1, 1, 0, 0, 0, 0)
)
invalid_trns_sample += chunk(b"tRNS", b"\x00\x02")
invalid_trns_sample += chunk(b"IDAT", zlib.compress(b"\x00\x00"))
invalid_trns_sample += chunk(b"IEND", b"")
Path(sys.argv[22]).write_bytes(invalid_trns_sample)
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
if [[ "$mode" == "partial-then-linger" || "$mode" == "partial-stuck" || "$mode" == "race-during-validation" || "$mode" == "invalid-bytes" || "$mode" == "large-log" || "$mode" == "generic-path-chaff" ]]; then
  exec "$PYTHON_BIN" - "$CORRAL_TEST_LIVE_PNG" "$CORRAL_UI_SCREENSHOT" "$mode" <<'PY'
from pathlib import Path
import os
import signal
import sys
import time

source = Path(sys.argv[1]).read_bytes()
destination = Path(sys.argv[2])
mode = sys.argv[3]
if mode == "invalid-bytes":
    sys.stdout.buffer.write(b"raw diagnostic with invalid byte: \xff\n")
    sys.stdout.buffer.write(b"FAILURE: exact invalid-byte diagnostic\n")
    sys.stdout.buffer.flush()
    destination.write_bytes(source)
    raise SystemExit(0)
if mode == "large-log":
    configured_root = os.environ.get(
        "CORRAL_TEST_WORKTREES_ROOT", "/tmp/Configured Herdr Root"
    ).encode()
    repo_root = os.environ.get("CORRAL_TEST_REPO_ROOT", "/tmp/corral-repo").encode()
    output_root = os.environ.get("CORRAL_TEST_OUTPUT_ROOT", "/tmp/corral-output").encode()
    sys.stdout.buffer.write(b"capture header\n")
    sys.stdout.buffer.write(b"x" * 100000)
    sys.stdout.buffer.write(
        b"\nconfigured-worktree="
        + configured_root
        + b"/repo name/worktree name/ios\n"
        + b"configured-root="
        + configured_root
        + b"\n"
        + b"configured-sibling="
        + configured_root
        + b".bak/repo name/worktree name/ios\n"
        + b"generic-worktree=/prefix with spaces/.herdr/worktrees/repo name/worktree name/ios\n"
        + b"generic-bare=/h/.herdr/worktrees/a/b\n"
        + b"generic-bare-real=/Users/jirathip/.herdr/worktrees/corral/other-branch\n"
        + b"generic-bare-spaces=/prefix with spaces/.herdr/worktrees/repo name/worktree name\n"
        + b"generic-space-marker=/prefix with spaces/.herdr/worktrees/repo/my failed experiment\n"
        + b"generic-crash-name=/prefix with spaces/.herdr/worktrees/repo/my crashed experiment\n"
        + b"generic-unfamiliar=/prefix with spaces/.herdr/worktrees/repo/my feature branch became unreadable\n"
        + b"generic-crash-diagnostic=/prefix with spaces/.herdr/worktrees/repo/my feature branch crashed during capture\n"
        + b"same-line-two-paths=cp /tmp/x /prefix with spaces/.herdr/worktrees/r n/w n/f\n"
        + b"known-repo-child="
        + repo_root
        + b"/scripts\n"
        + b"output-sibling="
        + output_root
        + b"-backup/file\n"
        + b"output-dot-sibling="
        + output_root
        + b".bak/file\n"
        + b"configured-diagnostic="
        + configured_root
        + b"/repo/branch failed to compile\n"
        + b'quoted-diagnostic="'
        + configured_root
        + b'/repo/branch": permission denied\n'
        + b"generic-diagnostic=/Users/jirathip/.herdr/worktrees/corral/branch failed to compile\n"
        b"FAILURE: exact bounded-log diagnostic \xff\n"
    )
    sys.stdout.buffer.flush()
    destination.write_bytes(source)
    raise SystemExit(0)
if mode == "generic-path-chaff":
    sys.stdout.buffer.write(b" /seg name/sub dir/file.o" * 32000)
    sys.stdout.buffer.write(b"\nFAILURE: generic path scan completed\n")
    sys.stdout.buffer.flush()
    destination.write_bytes(source)
    raise SystemExit(0)
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

cat > "$WORK/bin/python-fail-path" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${2:-}" == *fail-normalization-app* ]]; then
  exit 77
fi
if [[ "${3:-}" == *fail-note-normalization* ]]; then
  exit 78
fi
exec "$CORRAL_TEST_REAL_PYTHON" "$@"
STUB
chmod +x "$WORK/bin/python-fail-path"

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

conformance_sha() {
  shasum -a 256 "$1" | awk '{print $1}'
}

run_capture_with() {
  local invocation="$1"
  local output_root="$2"
  shift 2
  bash "$invocation" \
    --issue 211 \
    --surface egui \
    --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
    --live-png "$WORK/live.png" \
    --output-root "$output_root" \
    --chrome-timeout-seconds 5 \
    "$@"
}

run_capture() {
  run_capture_with "$SCRIPT" "$WORK/output" "$@"
}

run_bounded() {
  local timeout_seconds="$1"
  shift
  "$PYTHON_BIN" - "$timeout_seconds" "$@" <<'PY'
import os
import signal
import subprocess
import sys

timeout = float(sys.argv[1])
command = sys.argv[2:]
process = subprocess.Popen(command, start_new_session=True)
try:
    return_code = process.wait(timeout=timeout)
except subprocess.TimeoutExpired:
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
    print(
        f"command exceeded {timeout:g}-second bound: {' '.join(command)}",
        file=sys.stderr,
    )
    raise SystemExit(124)
raise SystemExit(return_code)
PY
}

run_capture_from() {
  local cwd="$1"
  local invocation="$2"
  local output_root="$3"
  local prototype="$4"
  local live_png="$5"
  shift 5
  (
    cd "$cwd"
    bash "$invocation" \
      --issue 211 \
      --surface egui \
      --prototype "$prototype" \
      --live-png "$live_png" \
      --output-root "$output_root" \
      --chrome-timeout-seconds 5 \
      "$@"
  )
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
run_capture --force
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
REAL_CHROME_RESULT="SKIP: real Chrome unavailable for structural prototype regression"
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
  REAL_CHROME_RESULT="PASS: real Chrome structural prototype regressions"
else
  echo "$REAL_CHROME_RESULT" >&2
fi

for artifact in prototype.png live-after.png comparison.png conformance.md capture.log; do
  [[ -s "$WORK/output/issue-211/$artifact" ]] || fail "missing artifact after first run: $artifact"
done
mkdir "$WORK/output/.design-gate.lock"
if run_capture --force >"$WORK/lock-failure.log" 2>&1; then
  fail "publication lock contention unexpectedly succeeded"
fi
grep -q 'could not acquire evidence publication lock' "$WORK/lock-failure.log" \
  || fail "publication lock failure was not actionable"
rmdir "$WORK/output/.design-gate.lock"
"$PYTHON_BIN" - "$WORK/output/issue-211" <<'PY'
from pathlib import Path
import sys

entries = sorted(
    path.name
    for path in Path(sys.argv[1]).iterdir()
    if not path.name.startswith(".")
)
assert entries == [
    "capture.log",
    "comparison.png",
    "conformance.md",
    "live-after.png",
    "prototype.png",
], f"published bundle contains unexpected entries: {entries!r}"
PY

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
grep -q 'byte-stable for identical semantic inputs' "$WORK/output/issue-211/conformance.md" \
  || fail "manifest stability contract is not documented"
grep -q 'Generator script (canonical .*BASH_SOURCE' "$WORK/output/issue-211/conformance.md" \
  || fail "canonical generator identity is not recorded"
grep -F 'scripts/design-gate-evidence.sh' "$WORK/output/issue-211/conformance.md" \
  || fail "generator path was not made repo-relative"
prototype_source_sha="$(conformance_sha "$REPO_DIR/docs/design/corral-ux-prototype.html")"
grep -F -- "- Prototype source SHA-256: \`$prototype_source_sha\`" \
  "$WORK/output/issue-211/conformance.md" \
  || fail "prototype source hash did not describe the bytes used to render"
live_source_sha="$(conformance_sha "$WORK/live.png")"
grep -F -- "- Live input SHA-256: \`$live_source_sha\`" \
  "$WORK/output/issue-211/conformance.md" \
  || fail "live source hash did not describe the copied fixture bytes"
"$PYTHON_BIN" - "$WORK/output/issue-211/conformance.md" <<'PY'
from pathlib import Path
import sys

data = Path(sys.argv[1]).read_bytes()
assert b"sanitized summary.\n\n## Capture\n" in data
assert b"\\<external-input\\>" in data
assert b"\\<external-output\\>" in data
PY
for artifact in conformance.md capture.log; do
  if grep -F "$REPO_DIR" "$WORK/output/issue-211/$artifact"; then
    fail "$artifact retained the disposable checkout path"
  fi
  if grep -F "$WORK" "$WORK/output/issue-211/$artifact"; then
    fail "$artifact retained the disposable test path"
  fi
  if grep -F '.herdr/worktrees' "$WORK/output/issue-211/$artifact"; then
    fail "$artifact retained a Herdr worktree path"
  fi
done

run_capture_with "$WORK/design-gate-wrapper.sh" "$WORK/wrapper-output" --force
cmp "$WORK/output/issue-211/conformance.md" \
  "$WORK/wrapper-output/issue-211/conformance.md" \
  || fail "symlink/wrapper invocation changed the manifest"
cmp "$WORK/output/issue-211/capture.log" \
  "$WORK/wrapper-output/issue-211/capture.log" \
  || fail "symlink/wrapper invocation changed the normalized capture log"

run_capture_with "$WORK/design gate.sh" "$WORK/space-output" --force
cmp "$WORK/output/issue-211/conformance.md" \
  "$WORK/space-output/issue-211/conformance.md" \
  || fail "script path containing spaces changed the manifest"
cmp "$WORK/output/issue-211/capture.log" \
  "$WORK/space-output/issue-211/capture.log" \
  || fail "script path containing spaces changed the capture log"

relative_live_from_scripts="$($PYTHON_BIN - "$WORK/live.png" "$REPO_DIR/scripts" <<'PY'
import os
import sys
print(os.path.relpath(sys.argv[1], sys.argv[2]))
PY
)"
run_capture_from "$REPO_DIR" "scripts/design-gate-evidence.sh" \
  "$WORK/equivalent-output-a" \
  "./docs/design/corral-ux-prototype.html" "$WORK/live.png" --force
run_capture_from "$REPO_DIR/scripts" "../scripts/design-gate-evidence.sh" \
  "$WORK/equivalent-output-b" \
  "../docs/design/corral-ux-prototype.html" \
  "$relative_live_from_scripts" --force
cmp "$WORK/equivalent-output-a/issue-211/conformance.md" \
  "$WORK/equivalent-output-b/issue-211/conformance.md" \
  || fail "equivalent path spellings changed the manifest"
cmp "$WORK/equivalent-output-a/issue-211/capture.log" \
  "$WORK/equivalent-output-b/issue-211/capture.log" \
  || fail "equivalent path spellings changed the capture log"

slash_note=$'/operator/path is plain text\n'
wake_command=$'/usr/bin/osascript -e wake\n'
launch_arg='/literal/launch-argument'
literal_option_value='--prototype'
run_capture --force \
  --provenance-note "$slash_note" \
  --egui-wake-command "$wake_command" \
  --ios-launch-arg "$launch_arg" \
  --ios-launch-arg "$literal_option_value" \
  --output-root "$WORK/nonpath-output"
note_q="$(printf '%q' "$slash_note")"
wake_q="$(printf '%q' "$wake_command")"
grep -F -- "--provenance-note $note_q" \
  "$WORK/nonpath-output/issue-211/conformance.md" \
  || fail "slash-prefixed provenance note was normalized as a path"
grep -F -- "--egui-wake-command $wake_q" \
  "$WORK/nonpath-output/issue-211/conformance.md" \
  || fail "slash-prefixed wake command was normalized as a path"
grep -F -- '--ios-launch-arg --prototype --output-root \<' \
  "$WORK/nonpath-output/issue-211/conformance.md" \
  || fail "option-shaped launch argument changed path typing"
"$PYTHON_BIN" - "$WORK/nonpath-output/issue-211/conformance.md" "$slash_note" <<'PY'
from pathlib import Path
import os
import sys

data = Path(sys.argv[1]).read_bytes()
note = os.fsencode(sys.argv[2])
assert note in data, "trailing newline in non-path note was lost"
assert b"<external-path>" not in data
PY

opaque_command="$WORK/wake-window.sh PATH=$WORK/cache:/usr/bin --config=$WORK/a,/etc/stable"
run_capture --force \
  --egui-wake-command "$opaque_command" \
  --ios-command "$WORK/ios-command.sh" \
  --ios-launch-arg "$WORK/launch payload" \
  --output-root "$WORK/opaque-output"
"$PYTHON_BIN" - "$WORK/opaque-output/issue-211/conformance.md" "$WORK" <<'PY'
from pathlib import Path
import os
import sys

data = Path(sys.argv[1]).read_bytes()
work = os.fsencode(sys.argv[2])
assert work not in data, "opaque command or launch argument leaked the disposable path"
assert b"external-temp" in data, "opaque disposable paths were not normalized"
assert b"external-input" in data, "absolute launch arguments were not normalized"
assert b"PATH=\\<external-temp\\>:/usr/bin" in data, "opaque colon-separated content was not preserved"
assert b"--config=\\<external-temp\\>\\,/etc/stable" in data, "opaque comma-separated content was not preserved"
PY

known_worktrees_root="$WORK/known herdr root"
redaction_note="${REPO_DIR}/docs/design/corral-ux-prototype.html"$'\n'"${known_worktrees_root}/repo name/worktree name/ios"$'\n'
redaction_note="${redaction_note}"$'\xff\n'
export CORRAL_WORKTREES_ROOT="$known_worktrees_root"
run_capture --force \
  --provenance-note "$redaction_note" \
  --output-root "$WORK/note-redaction-output"
unset CORRAL_WORKTREES_ROOT
"$PYTHON_BIN" - \
  "$WORK/note-redaction-output/issue-211/conformance.md" \
  "$REPO_DIR" "$known_worktrees_root" <<'PY'
from pathlib import Path
import os
import sys

data = Path(sys.argv[1]).read_bytes()
repo = os.fsencode(sys.argv[2])
worktrees_root = os.fsencode(sys.argv[3])
assert repo not in data, "provenance note leaked the absolute repository root"
assert worktrees_root not in data, "provenance note leaked the configured worktree root"
assert b"docs/design/corral-ux-prototype.html" in data
assert b"<herdr-worktree>/ios" in data
assert b"\xff\n" in data, "invalid provenance-note bytes were replaced"
PY

boundary_note="boundary=${REPO_DIR}. ${REPO_DIR}> ${REPO_DIR}! ${REPO_DIR}? ${REPO_DIR}, ${REPO_DIR}: ${REPO_DIR}; quote=\"${REPO_DIR}\""
sibling_note="siblings=${REPO_DIR}.bak/x ${REPO_DIR}-backup/x ${REPO_DIR}.bak ${REPO_DIR}-backup"
run_capture --force \
  --provenance-note "$boundary_note $sibling_note" \
  --output-root "$WORK/boundary-output"
expected_boundary_note="boundary=.. .> .! .? ., .: .; quote=\".\" siblings=${REPO_DIR}.bak/x ${REPO_DIR}-backup/x ${REPO_DIR}.bak ${REPO_DIR}-backup"
expected_boundary_q="$(printf '%q' "$expected_boundary_note")"
grep -F -- "--provenance-note $expected_boundary_q" \
  "$WORK/boundary-output/issue-211/conformance.md" \
  || fail "known-root punctuation boundary or sibling guard changed"

if PYTHON_BIN="$WORK/bin/python-fail-path" \
  PATH="$WORK/bin:$ORIGINAL_PATH" bash "$SCRIPT" \
    --issue 211 \
    --surface egui \
    --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
    --live-png "$WORK/live.png" \
    --ios-app "$WORK/fail-normalization-app" \
    --output-root "$WORK/path-normalization-failure-output" \
    --chrome-timeout-seconds 5 \
    --force >"$WORK/path-normalization-failure.log" 2>&1; then
  fail "path-normalizer failure was swallowed"
fi
grep -q 'could not normalize path argument' "$WORK/path-normalization-failure.log" \
  || fail "path-normalizer failure did not remain load-bearing"

if PYTHON_BIN="$WORK/bin/python-fail-path" \
  PATH="$WORK/bin:$ORIGINAL_PATH" bash "$SCRIPT" \
    --issue 211 \
    --surface egui \
    --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
    --live-png "$WORK/live.png" \
    --provenance-note "$WORK/fail-note-normalization" \
    --output-root "$WORK/note-normalization-failure-output" \
    --chrome-timeout-seconds 5 \
    --force >"$WORK/note-normalization-failure.log" 2>&1; then
  fail "provenance-note normalizer failure was swallowed"
fi
grep -q 'could not normalize provenance note' \
  "$WORK/note-normalization-failure.log" \
  || fail "provenance-note normalizer failure did not remain load-bearing"

before_sha="$(shasum -a 256 "$WORK/output/issue-211/comparison.png" | awk '{print $1}')"
before_conformance_sha="$(conformance_sha "$WORK/output/issue-211/conformance.md")"

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
after_conformance_sha="$(conformance_sha "$WORK/output/issue-211/conformance.md")"
[[ "$before_conformance_sha" == "$after_conformance_sha" ]] \
  || fail "normalized conformance changed across a forced rerun"

export CORRAL_TEST_CHROME_FAIL=1
if run_capture --force >"$WORK/failed.log" 2>&1; then
  fail "a failed browser capture unexpectedly succeeded"
fi
unset CORRAL_TEST_CHROME_FAIL
final_sha="$(shasum -a 256 "$WORK/output/issue-211/comparison.png" | awk '{print $1}')"
[[ "$final_sha" == "$after_sha" ]] || fail "failed rerun replaced the prior evidence bundle"

cat >"$WORK/bin/mv" <<MV_STUB
#!/usr/bin/env bash
set -euo pipefail
last_argument=""
for argument in "\$@"; do last_argument="\$argument"; done
if [[ "\$last_argument" == "$WORK/output/issue-211" && ! -e "$WORK/mv-failed" ]]; then
  touch "$WORK/mv-failed"
  exit 73
fi
exec /bin/mv "\$@"
MV_STUB
chmod +x "$WORK/bin/mv"
if PATH="$WORK/bin:$ORIGINAL_PATH" run_capture --force \
  >"$WORK/publication-failure.log" 2>&1; then
  fail "forced publication unexpectedly succeeded after injected directory rename failure"
fi
grep -q 'could not publish the validated evidence bundle' \
  "$WORK/publication-failure.log" \
  || {
    sed -n '1,120p' "$WORK/publication-failure.log" >&2
    fail "directory publication failure was not actionable"
  }
rollback_sha="$(shasum -a 256 "$WORK/output/issue-211/comparison.png" | awk '{print $1}')"
[[ "$rollback_sha" == "$after_sha" ]] \
  || fail "directory publication failure did not restore prior evidence"
rollback_conformance_sha="$(conformance_sha "$WORK/output/issue-211/conformance.md")"
[[ "$rollback_conformance_sha" == "$after_conformance_sha" ]] \
  || fail "directory publication failure changed prior conformance"
rm -f "$WORK/bin/mv" "$WORK/mv-failed"

cat >"$WORK/bin/mv" <<MV_COLLISION_STUB
#!/usr/bin/env bash
set -euo pipefail
last_argument=""
for argument in "\$@"; do last_argument="\$argument"; done
if [[ "\$last_argument" == "$WORK/output/issue-211" && ! -e "$WORK/mv-collision" ]]; then
  mkdir "$WORK/output/issue-211"
  touch "$WORK/mv-collision"
  exit 73
fi
exec /bin/mv "\$@"
MV_COLLISION_STUB
chmod +x "$WORK/bin/mv"
if PATH="$WORK/bin:$ORIGINAL_PATH" run_capture --force \
  >"$WORK/publication-collision.log" 2>&1; then
  fail "forced publication unexpectedly succeeded after destination collision"
fi
grep -q 'could not publish the validated evidence bundle' \
  "$WORK/publication-collision.log" \
  || fail "destination collision failure was not actionable"
shopt -s nullglob
collision_backups=("$WORK/output"/.design-gate.backup.*)
[[ "${#collision_backups[@]}" -eq 1 ]] \
  || fail "destination collision did not retain exactly one old evidence backup"
/bin/rm -rf -- "$WORK/output/issue-211"
/bin/mv -- "${collision_backups[0]}" "$WORK/output/issue-211"
collision_sha="$(shasum -a 256 "$WORK/output/issue-211/comparison.png" | awk '{print $1}')"
[[ "$collision_sha" == "$after_sha" ]] \
  || fail "destination collision recovery did not preserve prior evidence"
rm -f "$WORK/bin/mv" "$WORK/mv-collision"

if run_capture --force --live-png "$WORK/truncated.png" \
  >"$WORK/truncated.log" 2>&1; then
  fail "truncated PNG unexpectedly passed validation"
fi
grep -E -q 'IEND|IDAT|truncated' "$WORK/truncated.log" \
  || fail "truncated PNG failure was not actionable"

if run_capture --force --live-png "$WORK/invalid-raster.png" \
  >"$WORK/invalid-raster.log" 2>&1; then
  fail "invalid PNG raster unexpectedly passed validation"
fi
grep -E -q 'decompressed raster|expected' "$WORK/invalid-raster.log" \
  || fail "invalid PNG raster failure was not actionable"

if run_capture --force --live-png "$WORK/invalid-filter.png" \
  >"$WORK/invalid-filter.log" 2>&1; then
  fail "invalid PNG filter unexpectedly passed validation"
fi
grep -q 'invalid scanline filter' "$WORK/invalid-filter.log" \
  || fail "invalid PNG filter failure was not actionable"

run_capture --force --live-png "$WORK/interlaced.png" \
  --output-root "$WORK/interlaced-output"
grep -q '1x1' "$WORK/interlaced-output/issue-211/conformance.md" \
  || fail "valid interlaced PNG was not accepted"

run_capture --force --live-png "$WORK/valid-palette.png" \
  --output-root "$WORK/valid-palette-output"
grep -q '1x1' "$WORK/valid-palette-output/issue-211/conformance.md" \
  || fail "valid indexed PNG was not accepted"

if run_capture --force --live-png "$WORK/invalid-palette.png" \
  >"$WORK/invalid-palette.log" 2>&1; then
  fail "indexed PNG without a palette unexpectedly passed validation"
fi
grep -q 'indexed raster data' "$WORK/invalid-palette.log" \
  || fail "indexed PNG palette failure was not actionable"

if run_capture --force --live-png "$WORK/invalid-palette-size.png" \
  >"$WORK/invalid-palette-size.log" 2>&1; then
  fail "indexed PNG with an oversized palette unexpectedly passed validation"
fi
grep -q 'too many PLTE entries' "$WORK/invalid-palette-size.log" \
  || fail "indexed PNG palette-size failure was not actionable"

if run_capture --force --live-png "$WORK/invalid-palette-index.png" \
  >"$WORK/invalid-palette-index.log" 2>&1; then
  fail "indexed PNG with an out-of-range pixel unexpectedly passed validation"
fi
grep -q 'outside its PLTE entries' "$WORK/invalid-palette-index.log" \
  || fail "indexed PNG pixel-range failure was not actionable"

if run_capture --force --live-png "$WORK/invalid-grayscale-palette.png" \
  >"$WORK/invalid-grayscale-palette.log" 2>&1; then
  fail "grayscale PNG with a PLTE unexpectedly passed validation"
fi
grep -q 'invalid PLTE chunk' "$WORK/invalid-grayscale-palette.log" \
  || fail "grayscale PNG palette failure was not actionable"

if run_capture --force --live-png "$WORK/nonconsecutive-idat.png" \
  >"$WORK/nonconsecutive-idat.log" 2>&1; then
  fail "non-consecutive IDAT chunks unexpectedly passed validation"
fi
grep -q 'non-consecutive IDAT' "$WORK/nonconsecutive-idat.log" \
  || fail "non-consecutive IDAT failure was not actionable"

if run_capture --force --live-png "$WORK/unknown-critical.png" \
  >"$WORK/unknown-critical.log" 2>&1; then
  fail "PNG with an unknown critical chunk unexpectedly passed validation"
fi
grep -q 'unknown critical chunk' "$WORK/unknown-critical.log" \
  || fail "unknown critical chunk failure was not actionable"

if run_capture --force --live-png "$WORK/indexed-trns-before-plte.png" \
  >"$WORK/indexed-trns-before-plte.log" 2>&1; then
  fail "indexed PNG with tRNS before PLTE unexpectedly passed validation"
fi
grep -q 'tRNS data before' "$WORK/indexed-trns-before-plte.log" \
  || fail "indexed tRNS ordering failure was not actionable"

if run_capture --force --live-png "$WORK/trns-after-idat.png" \
  >"$WORK/trns-after-idat.log" 2>&1; then
  fail "PNG with tRNS after IDAT unexpectedly passed validation"
fi
grep -q 'invalid tRNS chunk order' "$WORK/trns-after-idat.log" \
  || fail "tRNS-after-IDAT failure was not actionable"

if run_capture --force --live-png "$WORK/duplicate-trns.png" \
  >"$WORK/duplicate-trns.log" 2>&1; then
  fail "PNG with duplicate tRNS chunks unexpectedly passed validation"
fi
grep -q 'duplicate tRNS' "$WORK/duplicate-trns.log" \
  || fail "duplicate tRNS failure was not actionable"

if run_capture --force --live-png "$WORK/truecolor-trns-before-plte.png" \
  >"$WORK/truecolor-trns-before-plte.log" 2>&1; then
  fail "truecolor tRNS before PLTE unexpectedly passed validation"
fi
grep -q 'invalid PLTE chunk' "$WORK/truecolor-trns-before-plte.log" \
  || fail "truecolor tRNS/PLTE ordering failure was not actionable"

if run_capture --force --live-png "$WORK/truecolor-bkgd-before-plte.png" \
  >"$WORK/truecolor-bkgd-before-plte.log" 2>&1; then
  fail "truecolor bKGD before PLTE unexpectedly passed validation"
fi
grep -q 'invalid PLTE chunk' "$WORK/truecolor-bkgd-before-plte.log" \
  || fail "truecolor bKGD/PLTE ordering failure was not actionable"

if run_capture --force --live-png "$WORK/invalid-reserved-chunk.png" \
  >"$WORK/invalid-reserved-chunk.log" 2>&1; then
  fail "PNG with an invalid chunk reserved bit unexpectedly passed validation"
fi
grep -q 'reserved bit' "$WORK/invalid-reserved-chunk.log" \
  || fail "invalid chunk reserved-bit failure was not actionable"

if run_capture --force --live-png "$WORK/invalid-trns-sample.png" \
  >"$WORK/invalid-trns-sample.log" 2>&1; then
  fail "PNG with an out-of-range tRNS sample unexpectedly passed validation"
fi
grep -q 'tRNS sample outside' "$WORK/invalid-trns-sample.log" \
  || fail "out-of-range tRNS sample failure was not actionable"

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

export CORRAL_TEST_EXPECTED_ISSUE=219
rm -f "$CORRAL_TEST_EGUI_PID_FILE"
export CORRAL_TEST_EGUI_MODE=invalid-bytes
PATH="$WORK/bin:$ORIGINAL_PATH" bash "$SCRIPT" \
  --issue 219 \
  --surface egui \
  --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
  --egui-binary "$WORK/bin/egui" \
  --live-agent agent-1 \
  --host-url http://fixture \
  --no-build \
  --delay-ms 1 \
  --timeout-seconds 5 \
  --output-root "$WORK/invalid-bytes-output" \
  --chrome-timeout-seconds 5
"$PYTHON_BIN" - "$WORK/invalid-bytes-output/issue-219/capture.log" <<'PY'
from pathlib import Path
import sys

data = Path(sys.argv[1]).read_bytes()
assert b"\xff" in data, "invalid UTF-8 byte was replaced"
assert b"FAILURE: exact invalid-byte diagnostic" in data
PY
unset CORRAL_TEST_EGUI_MODE

export CORRAL_TEST_EXPECTED_ISSUE=220
rm -f "$CORRAL_TEST_EGUI_PID_FILE"
export CORRAL_TEST_EGUI_MODE=large-log
export CORRAL_TEST_WORKTREES_ROOT="/tmp/Configured Herdr Root"
export CORRAL_TEST_REPO_ROOT="$REPO_DIR"
export CORRAL_TEST_OUTPUT_ROOT="$WORK/large-log-output"
export CORRAL_WORKTREES_ROOT="$CORRAL_TEST_WORKTREES_ROOT"
PATH="$WORK/bin:$ORIGINAL_PATH" bash "$SCRIPT" \
  --issue 220 \
  --surface egui \
  --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
  --egui-binary "$WORK/bin/egui" \
  --live-agent agent-1 \
  --host-url http://fixture \
  --no-build \
  --delay-ms 1 \
  --timeout-seconds 5 \
  --output-root "$WORK/large-log-output" \
  --chrome-timeout-seconds 5
"$PYTHON_BIN" - "$WORK/large-log-output/issue-220/capture.log" "$WORK/large-log-output" <<'PY'
from pathlib import Path
import os
import sys

data = Path(sys.argv[1]).read_bytes()
output_root = os.fsencode(sys.argv[2])
assert len(data) <= 65536, f"capture log exceeded bound: {len(data)}"
assert b"capture log truncated" in data
assert b"FAILURE: exact bounded-log diagnostic" in data
assert b".herdr/worktrees" not in data, "generic Herdr marker leaked"
assert b"<herdr-worktree>/ios" in data, "full configured worktree was not redacted"
assert b"configured-root=<herdr-worktree>\n" in data, "bare configured root was not redacted"
assert b"configured-sibling=/tmp/Configured Herdr Root.bak/repo name/worktree name/ios" in data, "configured sibling was over-redacted"
assert b"generic-bare=<herdr-worktree>\n" in data, "bare generic worktree root was not redacted"
assert b"generic-bare-real=<herdr-worktree>\n" in data, "real Herdr worktree root was not redacted"
assert b"generic-bare-spaces=<herdr-worktree>\n" in data, "space-containing worktree name was not fully redacted"
assert b"generic-space-marker=<herdr-worktree>\n" in data, "marker word inside a worktree name was leaked"
assert b"generic-crash-name=<herdr-worktree>\n" in data, "crash marker inside a worktree name was leaked"
assert b"generic-unfamiliar=<herdr-worktree>\n" in data, "unquoted path suffix was leaked"
assert b"generic-crash-diagnostic=<herdr-worktree>\n" in data, "unquoted diagnostic suffix was leaked"
assert b"same-line-two-paths=cp /tmp/x <herdr-worktree>/f\n" in data, "same-line source path was over-redacted"
assert b"known-repo-child=./scripts\n" in data, "specific repo path lost to generic Herdr redaction"
assert b"output-sibling=" + output_root + b"-backup/file" in data
assert b"output-dot-sibling=" + output_root + b".bak/file" in data
assert b"configured-diagnostic=<herdr-worktree>\n" in data, "unquoted configured path suffix was leaked"
assert b'quoted-diagnostic="<herdr-worktree>": permission denied\n' in data, "quoted worktree redaction consumed its diagnostic delimiter"
assert b"generic-diagnostic=<herdr-worktree>\n" in data, "unquoted generic path suffix was leaked"
PY
unset CORRAL_TEST_REPO_ROOT CORRAL_TEST_WORKTREES_ROOT CORRAL_TEST_OUTPUT_ROOT CORRAL_WORKTREES_ROOT
unset CORRAL_TEST_EGUI_MODE

export CORRAL_TEST_EXPECTED_ISSUE=221
rm -f "$CORRAL_TEST_EGUI_PID_FILE"
export CORRAL_TEST_EGUI_MODE=generic-path-chaff
run_bounded 15 env PATH="$WORK/bin:$ORIGINAL_PATH" bash "$SCRIPT" \
  --issue 221 \
  --surface egui \
  --prototype "$REPO_DIR/docs/design/corral-ux-prototype.html" \
  --egui-binary "$WORK/bin/egui" \
  --live-agent agent-1 \
  --host-url http://fixture \
  --no-build \
  --delay-ms 1 \
  --timeout-seconds 5 \
  --output-root "$WORK/path-chaff-output" \
  --chrome-timeout-seconds 5
"$PYTHON_BIN" - "$WORK/path-chaff-output/issue-221/capture.log" <<'PY'
from pathlib import Path
import sys

data = Path(sys.argv[1]).read_bytes()
assert b"capture log truncated" in data
assert b"FAILURE: generic path scan completed" in data
PY
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
  "$WORK"/*/.design-gate.stage.*
  "$WORK"/*/.design-gate.final.*
  "$WORK"/*/.design-gate.backup.*
)
if [[ "${#staging_entries[@]}" -ne 0 ]]; then
  fail "temporary staging directory survived a failed run"
fi
lock_entries=("$WORK"/*/.design-gate.lock)
if [[ "${#lock_entries[@]}" -ne 0 ]]; then
  fail "publication lock survived a completed or failed run"
fi

printf 'Real Chrome structural prototype regression: %s\n' "$REAL_CHROME_RESULT"
echo "OK: design-gate evidence validation, complete-PNG contract, bounded cleanup, trust boundary, capture seams, and failure paths"
