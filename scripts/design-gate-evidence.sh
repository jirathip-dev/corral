#!/usr/bin/env bash
# design-gate-evidence.sh — render a design target, capture the live surface,
# and write a stamped side-by-side evidence bundle.
#
# Usage:
#   scripts/design-gate-evidence.sh --issue 206 --surface egui \
#     --live-agent herdr:AGENT_ID
#   scripts/design-gate-evidence.sh --issue 205 --surface ios \
#     --ios-mode demo
#
# Output:
#   docs/design/evidence/issue-<N>/prototype.png
#   docs/design/evidence/issue-<N>/ios-before-detail.png (iOS demo only)
#   docs/design/evidence/issue-<N>/live-after.png
#   docs/design/evidence/issue-<N>/comparison.png
#   docs/design/evidence/issue-<N>/conformance.md
#   docs/design/evidence/issue-<N>/capture.log
#
# The default egui prototype is the approved HTML design source at
# docs/design/corral-ux-prototype.html. Its desktop .desk surface is rendered
# through headless Chrome at 1160×631. A custom HTML target can be supplied with
# --prototype; the requested .desk or .phone must be contained by a direct
# body > .rack > .frame chain. The iOS surface is rendered at 900×900 so the
# complete phone frame remains visible. The source is wrapped without changing
# the checked-in prototype.
#
# egui capture is deliberately live-only by default. The script requires a
# healthy --host-url (default http://127.0.0.1:8474), builds
# target/release/corrald-ui when needed, and launches it with the existing
# CORRAL_UI_SCREENSHOT env-gated native viewport capture. --live-agent is
# optional, but when supplied it must be present in /snapshot and the app log
# must prove that the real agent was selected. The app's existing config must
# already be registered; this script never creates demo data. If the host's
# window server needs an input event to wake eframe, --egui-wake-command (or
# CORRAL_EGUI_WAKE_COMMAND) runs an explicit caller-owned command while the
# process is alive; the command receives CORRAL_UI_SCREENSHOT_PID and
# CORRAL_UI_SCREENSHOT_PATH and failure is fatal rather than silently claiming
# a stale frame. A capture succeeds only after the output is a fully validated
# PNG; process exit is not part of that success contract. Once a complete PNG
# exists, the script terminates only its direct child with TERM, waits a short
# grace period, escalates to KILL, and validates the final file again. This
# accepts a complete PNG from a lingering writer without accepting partial data.
#
# iOS capture is simulator-only and always runs through hermes-sim-task, which
# owns and cleans up its private simulator. The script never calls
# "simctl delete all". --ios-mode demo is an explicit Debug fixture and adds
# -demoMode when no launch argument was supplied; it is not live-daemon
# evidence. --ios-mode live requires --ios-command, a caller-owned command
# that launches/prepares the live app inside the temporary simulator using
# $SIMULATOR_UDID. This prevents a fresh registration screen or fabricated demo
# state from being mislabeled as a live board. The app is built through the
# same Herdr-routed xcodebuild path when --ios-app is omitted.
#
# Chrome's temporary DevTools endpoint is an ephemeral port bound explicitly
# to 127.0.0.1, with a private profile and a loopback-only allowed origin. It
# is used only for the scoped Browser.close request; the local process and the
# approved checkout HTML are the trust boundary. No remote page is loaded.
#
# --live-png is an explicit fixture seam for tests or a previously captured
# frame. Its provenance says that the PNG was supplied rather than captured by
# this run; it never silently becomes live evidence. --dry-run validates the
# interface and prints the planned capture without writing an evidence bundle.
# Existing evidence is never overwritten unless --force is explicit. Re-runs
# with --force are safe: all work is staged in a private temporary directory
# below the target issue directory, existing evidence is untouched on failure,
# and the validated files are replaced at the end using atomic file renames.
#
# Dependencies: Bash 3+, Python 3, headless-capable Chrome/Chromium, and (for
# native captures) curl/cargo or hermes-sim-task as described above. Set
# CHROME_BIN or PYTHON_BIN to override discovery.

set -euo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SCRIPT_NAME="$(basename "$0")"

ISSUE=""
SURFACE=""
PROTOTYPE=""
LIVE_PNG=""
LIVE_AGENT=""
HOST_URL="${CORRAL_UI_HOST_URL:-http://127.0.0.1:8474}"
EGUI_BINARY=""
DAEMON_BINARY=""
FIXTURE_REGISTRY=""
EGUI_DELAY_MS="8000"
EGUI_TAB="${CORRAL_UI_SCREENSHOT_TAB:-board}"
EGUI_WAKE_COMMAND="${CORRAL_EGUI_WAKE_COMMAND:-}"
CAPTURE_TIMEOUT_SECONDS="90"
CHROME_TIMEOUT_SECONDS="30"
# A complete PNG is the evidence contract. Process cleanup is deliberately
# short and bounded; these are not user-tunable so a caller cannot accidentally
# turn a failed capture into an unbounded child leak.
CAPTURE_TERM_GRACE_SECONDS="2"
CAPTURE_KILL_GRACE_SECONDS="2"
CAPTURE_POLL_INTERVAL_SECONDS="0.1"
CHROME_DEVTOOLS_ADDRESS="127.0.0.1"
CHROME_DEVTOOLS_ORIGIN="http://127.0.0.1"
FORCE=0
BUILD_EGUI=1
BUILD_IOS=1
IOS_APP=""
IOS_BUNDLE_ID="com.corral.fleetnotifier"
IOS_MODE="live"
IOS_DELAY_SECONDS="4"
IOS_COMMAND=""
IOS_LAUNCH_ARGS=()
IOS_BEFORE_LAUNCH_ARGS=()
PROVENANCE_NOTE=""
OUTPUT_ROOT="$REPO_DIR/docs/design/evidence"
DRY_RUN=0

usage() {
  cat <<'USAGE'
Usage:
  scripts/design-gate-evidence.sh --issue 206 --surface egui \
    --live-agent herdr:AGENT_ID
  scripts/design-gate-evidence.sh --issue 205 --surface ios \
    --ios-mode demo

Options:
  --issue N                  Target issue number (required).
  --surface egui|ios         Surface to capture (required).
  --prototype PATH           Approved HTML prototype override.
  --live-agent ID             egui agent id, checked against /snapshot.
  --live-png PATH             Explicit supplied PNG fixture seam.
  --host-url URL              egui health endpoint (default: 127.0.0.1:8474).
  --egui-binary PATH          corrald-ui binary override.
  --daemon-binary PATH        Runtime corrald binary to hash in provenance.
  --fixture-registry PATH     Runtime fleets.json to hash in provenance.
  --egui-tab board|issues|registry|settings
                              Native/prototype tab to capture (default: board).
  --delay-ms N                egui screenshot delay (default: 8000).
  --egui-wake-command SHELL   Explicit eframe wake/input command.
  --timeout-seconds N         Native capture timeout (default: 90).
  --chrome-timeout-seconds N  Headless Chrome timeout (default: 30).
  --ios-app PATH              Prebuilt .app; otherwise build through Herdr.
  --ios-bundle-id ID         Bundle id (default: com.corral.fleetnotifier).
  --ios-mode live|demo        Live requires --ios-command; demo is explicit.
  --ios-command SHELL         Prepare/launch live app inside hermes-sim-task.
  --ios-launch-arg ARG        Repeatable simulator launch argument.
  --ios-before-launch-arg ARG Repeatable before-frame launch argument (iOS).
  --ios-delay-seconds N       Wait before simulator screenshot (default: 4).
  --provenance-note TEXT      Extra operator/environment note in provenance.
  --output-root PATH          Evidence root override (test seam).
  --force                     Permit replacement of an existing issue bundle.
  --no-build                  Refuse automatic egui/iOS builds.
  --dry-run                   Validate and print the planned operation only.
  -h, --help                  Show this help.

Environment:
  CHROME_BIN                  Chrome/Chromium executable override.
  PYTHON_BIN                  Python 3 executable override.
USAGE
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

log() {
  printf 'design-gate: %s\n' "$*"
}

warn() {
  printf 'design-gate: warning: %s\n' "$*" >&2
}

require_value() {
  [[ $# -ge 2 && -n "${2:-}" ]] || die "$1 requires a value"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --issue)
      require_value "$1" "${2:-}"
      ISSUE="$2"
      shift 2
      ;;
    --surface)
      require_value "$1" "${2:-}"
      SURFACE="$2"
      shift 2
      ;;
    --prototype)
      require_value "$1" "${2:-}"
      PROTOTYPE="$2"
      shift 2
      ;;
    --live-agent)
      require_value "$1" "${2:-}"
      LIVE_AGENT="$2"
      shift 2
      ;;
    --live-png)
      require_value "$1" "${2:-}"
      LIVE_PNG="$2"
      shift 2
      ;;
    --host-url)
      require_value "$1" "${2:-}"
      HOST_URL="$2"
      shift 2
      ;;
    --egui-binary)
      require_value "$1" "${2:-}"
      EGUI_BINARY="$2"
      shift 2
      ;;
    --daemon-binary)
      require_value "$1" "${2:-}"
      DAEMON_BINARY="$2"
      shift 2
      ;;
    --fixture-registry)
      require_value "$1" "${2:-}"
      FIXTURE_REGISTRY="$2"
      shift 2
      ;;
    --egui-tab)
      require_value "$1" "${2:-}"
      EGUI_TAB="$2"
      shift 2
      ;;
    --delay-ms)
      require_value "$1" "${2:-}"
      EGUI_DELAY_MS="$2"
      shift 2
      ;;
    --egui-wake-command)
      require_value "$1" "${2:-}"
      EGUI_WAKE_COMMAND="$2"
      shift 2
      ;;
    --timeout-seconds)
      require_value "$1" "${2:-}"
      CAPTURE_TIMEOUT_SECONDS="$2"
      shift 2
      ;;
    --chrome-timeout-seconds)
      require_value "$1" "${2:-}"
      CHROME_TIMEOUT_SECONDS="$2"
      shift 2
      ;;
    --ios-app)
      require_value "$1" "${2:-}"
      IOS_APP="$2"
      shift 2
      ;;
    --ios-bundle-id)
      require_value "$1" "${2:-}"
      IOS_BUNDLE_ID="$2"
      shift 2
      ;;
    --ios-mode)
      require_value "$1" "${2:-}"
      IOS_MODE="$2"
      shift 2
      ;;
    --ios-command)
      require_value "$1" "${2:-}"
      IOS_COMMAND="$2"
      shift 2
      ;;
    --ios-launch-arg)
      require_value "$1" "${2:-}"
      IOS_LAUNCH_ARGS+=("$2")
      shift 2
      ;;
    --ios-before-launch-arg)
      require_value "$1" "${2:-}"
      IOS_BEFORE_LAUNCH_ARGS+=("$2")
      shift 2
      ;;
    --ios-delay-seconds)
      require_value "$1" "${2:-}"
      IOS_DELAY_SECONDS="$2"
      shift 2
      ;;
    --provenance-note)
      require_value "$1" "${2:-}"
      PROVENANCE_NOTE="$2"
      shift 2
      ;;
    --output-root)
      require_value "$1" "${2:-}"
      OUTPUT_ROOT="$2"
      shift 2
      ;;
    --force)
      FORCE=1
      shift
      ;;
    --no-build)
      BUILD_EGUI=0
      BUILD_IOS=0
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option '$1' (use --help)"
      ;;
  esac
done

[[ -n "$ISSUE" ]] || die "--issue is required"
[[ "$ISSUE" =~ ^[1-9][0-9]*$ ]] || die "--issue must be a positive decimal number"
[[ "$SURFACE" == "egui" || "$SURFACE" == "ios" ]] \
  || die "--surface must be egui or ios"
[[ "$IOS_MODE" == "live" || "$IOS_MODE" == "demo" ]] \
  || die "--ios-mode must be live or demo"
if [[ "$SURFACE" == "egui" ]]; then
  case "$EGUI_TAB" in
    board|issues|registry|settings) ;;
    *) die "--egui-tab must be board, issues, registry, or settings" ;;
  esac
fi

# Issue #205's approved evidence bundle contains both reproducible frames. A
# caller can override the arguments explicitly, but the safe Debug route is
# the default for the issue so a future capture cannot silently fall back to a
# fleet list or a copied fixture.
if [[ "$ISSUE" == "205" && "$SURFACE" == "ios" && "$IOS_MODE" == "demo" ]]; then
  has_detail_argument=0
  if (( ${#IOS_LAUNCH_ARGS[@]} > 0 )); then
    for launch_arg in "${IOS_LAUNCH_ARGS[@]}"; do
      if [[ "$launch_arg" == "-corralDemoDetail" ]]; then
        has_detail_argument=1
        break
      fi
    done
  fi
  if [[ "$has_detail_argument" -eq 0 ]]; then
    if (( ${#IOS_LAUNCH_ARGS[@]} > 0 )); then
      IOS_LAUNCH_ARGS=("-corralDemoDetail" "${IOS_LAUNCH_ARGS[@]}")
    else
      IOS_LAUNCH_ARGS=("-corralDemoDetail")
    fi
  fi
  if [[ "${#IOS_BEFORE_LAUNCH_ARGS[@]}" -eq 0 ]]; then
    IOS_BEFORE_LAUNCH_ARGS=("-corralDemoDetail" "-corralDemoBefore")
  fi
fi

is_decimal() {
  [[ "$1" =~ ^[0-9]+$ ]]
}

is_positive_decimal() {
  is_decimal "$1" && ((10#$1 > 0))
}

is_nonnegative_decimal() {
  is_decimal "$1"
}

is_positive_decimal "$EGUI_DELAY_MS" || die "--delay-ms must be a positive decimal number"
is_positive_decimal "$CAPTURE_TIMEOUT_SECONDS" \
  || die "--timeout-seconds must be a positive decimal number"
is_positive_decimal "$CHROME_TIMEOUT_SECONDS" \
  || die "--chrome-timeout-seconds must be a positive decimal number"
is_nonnegative_decimal "$IOS_DELAY_SECONDS" \
  || die "--ios-delay-seconds must be a non-negative decimal number"

if [[ -z "$PROTOTYPE" ]]; then
  if [[ "$SURFACE" == "egui" ]]; then
    if [[ "$ISSUE" == "206" && -f "$REPO_DIR/docs/design/corral-ux-egui-redesign-prototype.html" ]]; then
      PROTOTYPE="$REPO_DIR/docs/design/corral-ux-egui-redesign-prototype.html"
    else
      PROTOTYPE="$REPO_DIR/docs/design/corral-ux-prototype.html"
    fi
  elif [[ -f "$REPO_DIR/docs/design/corral-ux-transcript-chat-prototype.html" ]]; then
    PROTOTYPE="$REPO_DIR/docs/design/corral-ux-transcript-chat-prototype.html"
  else
    PROTOTYPE="$REPO_DIR/docs/design/corral-ux-prototype.html"
    warn "the approved #205 transcript prototype is not present at this base; using the current token-compatible prototype. Pass --prototype to target #205 exactly."
  fi
fi

if [[ "$PROTOTYPE" != /* ]]; then
  PROTOTYPE="$PWD/$PROTOTYPE"
fi
if [[ "$OUTPUT_ROOT" != /* ]]; then
  OUTPUT_ROOT="$PWD/$OUTPUT_ROOT"
fi
if [[ -n "$EGUI_BINARY" && "$EGUI_BINARY" != /* ]]; then
  EGUI_BINARY="$PWD/$EGUI_BINARY"
fi
if [[ -n "$DAEMON_BINARY" && "$DAEMON_BINARY" != /* ]]; then
  DAEMON_BINARY="$PWD/$DAEMON_BINARY"
fi
if [[ -n "$FIXTURE_REGISTRY" && "$FIXTURE_REGISTRY" != /* ]]; then
  FIXTURE_REGISTRY="$PWD/$FIXTURE_REGISTRY"
fi
if [[ -n "$IOS_APP" && "$IOS_APP" != /* ]]; then
  IOS_APP="$PWD/$IOS_APP"
fi
if [[ -n "$LIVE_PNG" && "$LIVE_PNG" != /* ]]; then
  LIVE_PNG="$PWD/$LIVE_PNG"
fi

[[ -f "$PROTOTYPE" ]] || die "prototype does not exist: $PROTOTYPE"
if [[ "$SURFACE" == "egui" ]]; then
  grep -q '\.desk' "$PROTOTYPE" \
    || die "egui prototype must contain a .desk surface: $PROTOTYPE"
else
  grep -q '\.phone' "$PROTOTYPE" \
    || die "iOS prototype must contain a .phone surface: $PROTOTYPE"
fi
[[ -z "$LIVE_PNG" || -f "$LIVE_PNG" ]] \
  || die "--live-png does not exist: $LIVE_PNG"
[[ -z "$LIVE_AGENT" || -z "$LIVE_PNG" ]] \
  || die "--live-agent cannot be combined with the explicit --live-png fixture seam"
[[ "$SURFACE" == "egui" || -z "$LIVE_AGENT" ]] \
  || die "--live-agent is only valid for the egui surface"
[[ -z "$DAEMON_BINARY" || -f "$DAEMON_BINARY" ]] \
  || die "--daemon-binary does not exist: $DAEMON_BINARY"
[[ -z "$FIXTURE_REGISTRY" || -f "$FIXTURE_REGISTRY" ]] \
  || die "--fixture-registry does not exist: $FIXTURE_REGISTRY"

PYTHON_BIN="${PYTHON_BIN:-}"
if [[ -z "$PYTHON_BIN" ]]; then
  PYTHON_BIN="$(command -v python3 || true)"
fi
[[ -n "$PYTHON_BIN" && -x "$PYTHON_BIN" ]] \
  || die "Python 3 is required; set PYTHON_BIN to an executable Python 3"

CHROME_BIN_EXPLICIT=0
if [[ -n "${CHROME_BIN:-}" ]]; then
  CHROME_BIN_EXPLICIT=1
fi
CHROME_BIN="${CHROME_BIN:-}"
if [[ -n "$CHROME_BIN" ]]; then
  [[ -x "$CHROME_BIN" ]] || die "CHROME_BIN is not executable: $CHROME_BIN"
else
  for candidate in \
    "$(command -v google-chrome 2>/dev/null || true)" \
    "$(command -v google-chrome-stable 2>/dev/null || true)" \
    "$(command -v chromium 2>/dev/null || true)" \
    "$(command -v chromium-browser 2>/dev/null || true)" \
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
    "/Applications/Chromium.app/Contents/MacOS/Chromium" \
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"; do
    if [[ -n "$candidate" && -x "$candidate" ]]; then
      CHROME_BIN="$candidate"
      break
    fi
  done
fi
[[ -n "$CHROME_BIN" ]] \
  || die "headless Chrome/Chromium not found; install it or set CHROME_BIN"

python_check() {
  "$PYTHON_BIN" - <<'PY'
import sys
if sys.version_info < (3, 8):
    raise SystemExit("Python 3.8 or newer is required")
PY
}

python_check || die "Python 3.8 or newer is required"

CONTENT_IDENTITY_HELPER="$SCRIPT_DIR/design-gate-content-identity.py"
[[ -f "$CONTENT_IDENTITY_HELPER" ]] \
  || die "implementation identity helper is missing: $CONTENT_IDENTITY_HELPER"

implementation_identity() {
  if [[ "$ISSUE" == "205" ]]; then
    "$PYTHON_BIN" "$CONTENT_IDENTITY_HELPER" "$REPO_DIR" --issue 205
  else
    # Preserve the historical #206 identity for the generic/test seams and
    # other design-gate issue bundles.
    "$PYTHON_BIN" "$CONTENT_IDENTITY_HELPER" "$REPO_DIR"
  fi
}

absolute_path() {
  local value="$1"
  if [[ "$value" = /* ]]; then
    printf '%s\n' "$value"
  else
    printf '%s/%s\n' "$PWD" "$value"
  fi
}

shell_quote() {
  printf '%q' "$1"
}

file_url() {
  "$PYTHON_BIN" - "$1" <<'PY'
from pathlib import Path
import sys
print(Path(sys.argv[1]).resolve().as_uri())
PY
}

png_dimensions() {
  "$PYTHON_BIN" - "$1" <<'PY'
from pathlib import Path
import struct
import sys
import zlib

path = Path(sys.argv[1])
data = path.read_bytes()
if data[:8] != b"\x89PNG\r\n\x1a\n":
    raise SystemExit(f"{path} is not a PNG")
offset = 8
width = height = None
idat = []
seen_ihdr = False
seen_iend = False
while offset < len(data):
    if len(data) - offset < 12:
        raise SystemExit(f"{path} has a truncated PNG chunk")
    length = struct.unpack(">I", data[offset:offset + 4])[0]
    chunk_type = data[offset + 4:offset + 8]
    payload_start = offset + 8
    payload_end = payload_start + length
    crc_end = payload_end + 4
    if crc_end > len(data):
        raise SystemExit(f"{path} has a truncated PNG chunk payload")
    payload = data[payload_start:payload_end]
    expected_crc = struct.unpack(">I", data[payload_end:crc_end])[0]
    actual_crc = zlib.crc32(chunk_type + payload) & 0xFFFFFFFF
    if actual_crc != expected_crc:
        raise SystemExit(f"{path} has an invalid {chunk_type.decode('ascii', 'replace')} CRC")
    if not seen_ihdr and chunk_type != b"IHDR":
        raise SystemExit(f"{path} does not start with IHDR")
    if chunk_type == b"IHDR":
        if seen_ihdr or length != 13:
            raise SystemExit(f"{path} has an invalid IHDR")
        width, height = struct.unpack(">II", payload[:8])
        if width == 0 or height == 0:
            raise SystemExit(f"{path} has an empty dimension")
        seen_ihdr = True
    elif chunk_type == b"IDAT":
        idat.append(payload)
    elif chunk_type == b"IEND":
        if length != 0:
            raise SystemExit(f"{path} has a non-empty IEND")
        seen_iend = True
        offset = crc_end
        break
    offset = crc_end

if not seen_ihdr or width is None or height is None:
    raise SystemExit(f"{path} has no IHDR")
if not seen_iend:
    raise SystemExit(f"{path} is missing IEND")
if offset != len(data):
    raise SystemExit(f"{path} has data after IEND")
if not idat:
    raise SystemExit(f"{path} has no IDAT data")
decoder = zlib.decompressobj()
try:
    decoder.decompress(b"".join(idat))
    decoder.flush()
except zlib.error as error:
    raise SystemExit(f"{path} has incomplete IDAT data: {error}")
if not decoder.eof or decoder.unused_data:
    raise SystemExit(f"{path} has incomplete or trailing compressed IDAT data")
print(f"{width}x{height}")
PY
}

assert_png() {
  local path="$1"
  [[ -s "$path" ]] || die "capture did not produce a non-empty PNG: $path"
  png_dimensions "$path" >/dev/null \
    || die "capture is not a valid PNG: $path"
}

child_is_owned() {
  local pid="$1"
  local parent_pid
  [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 1
  parent_pid="$(ps -p "$pid" -o ppid= 2>/dev/null | awk 'NR == 1 { print $1 }')"
  [[ "$parent_pid" == "$$" ]]
}

child_is_running() {
  local pid="$1"
  local process_state
  [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 1
  kill -0 "$pid" 2>/dev/null || return 1
  # A direct child remains waitable as a zombie until reaped. Treating Z as
  # stopped keeps the TERM/KILL deadline honest and lets wait reap it below.
  process_state="$(ps -p "$pid" -o stat= 2>/dev/null | awk 'NR == 1 { print $1 }')"
  [[ -n "$process_state" && "$process_state" != Z* ]]
}

reap_capture_child() {
  local pid="$1"
  set +e
  wait "$pid" 2>/dev/null
  set -e
  if [[ "${CAPTURE_PID:-}" == "$pid" ]]; then
    CAPTURE_PID=""
  fi
  return 0
}

terminate_owned_child() {
  local pid="$1"
  local label="$2"
  local deadline

  [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 0
  if ! child_is_running "$pid"; then
    reap_capture_child "$pid"
    return 0
  fi
  child_is_owned "$pid" \
    || {
      printf 'error: refusing to terminate non-owned %s child pid %s\n' "$label" "$pid" >&2
      return 1
    }

  log "requesting TERM for lingering $label child (pid $pid)"
  kill -TERM "$pid" 2>/dev/null || true
  deadline=$((SECONDS + 10#$CAPTURE_TERM_GRACE_SECONDS))
  while child_is_running "$pid" && [[ $SECONDS -lt $deadline ]]; do
    sleep "$CAPTURE_POLL_INTERVAL_SECONDS"
  done 2>/dev/null
  if child_is_running "$pid"; then
    log "escalating to KILL for owned $label child (pid $pid)"
    kill -KILL "$pid" 2>/dev/null || true
    deadline=$((SECONDS + 10#$CAPTURE_KILL_GRACE_SECONDS))
    while child_is_running "$pid" && [[ $SECONDS -lt $deadline ]]; do
      sleep "$CAPTURE_POLL_INTERVAL_SECONDS"
    done 2>/dev/null
  fi
  if child_is_running "$pid"; then
    printf 'error: owned %s child pid %s survived TERM and KILL\n' "$label" "$pid" >&2
    return 1
  fi
  # A killed direct child can remain as a zombie until its parent waits for
  # it. Reap only after the bounded liveness check so an already-dead child
  # cannot be reported as surviving and a genuinely stuck child cannot turn
  # cleanup into an unbounded wait.
  reap_capture_child "$pid"
}

PNG_WAIT_REASON=""
wait_for_complete_png() {
  local path="$1"
  local label="$2"
  local timeout_seconds="$3"
  local deadline
  local dimensions

  PNG_WAIT_REASON=""
  deadline=$((SECONDS + 10#$timeout_seconds))
  while :; do
    if [[ -s "$path" ]]; then
      if dimensions="$(png_dimensions "$path" 2>&1)"; then
        return 0
      fi
      PNG_WAIT_REASON="$dimensions"
    else
      PNG_WAIT_REASON="$label has not written a non-empty PNG"
    fi

    # A writer that exits before publishing a complete PNG is normally a hard
    # failure; revalidate once because the writer may have completed between
    # the first validator call and this liveness check.
    if [[ -z "${CAPTURE_PID:-}" ]] || ! child_is_running "$CAPTURE_PID"; then
      if [[ -s "$path" ]]; then
        if dimensions="$(png_dimensions "$path" 2>&1)"; then
          return 0
        fi
        PNG_WAIT_REASON="$dimensions"
      fi
      return 1
    fi
    if [[ $SECONDS -ge $deadline ]]; then
      return 1
    fi
    sleep "$CAPTURE_POLL_INTERVAL_SECONDS"
  done
}

sha256_file() {
  "$PYTHON_BIN" - "$1" <<'PY'
import hashlib
from pathlib import Path
import sys

digest = hashlib.sha256()
with Path(sys.argv[1]).open("rb") as stream:
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        digest.update(block)
print(digest.hexdigest())
PY
}

normalize_capture_log() {
  "$PYTHON_BIN" - "$1" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
raw = path.read_text(encoding="utf-8", errors="replace")
lines = [line.rstrip(" \t") for line in raw.splitlines()]
normalized = "\n".join(lines)
if raw.endswith(("\n", "\r")):
    normalized += "\n"
path.write_text(normalized, encoding="utf-8")
PY
}

validate_native_probe() {
  "$PYTHON_BIN" - "$1" <<'PY'
import json
from pathlib import Path
import sys

path = Path(sys.argv[1])
if not path.is_file():
    raise SystemExit(f"native window probe log is missing: {path}")

records = []
for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
    try:
        record = json.loads(line)
    except json.JSONDecodeError as error:
        raise SystemExit(f"native window probe line {line_number} is not JSON: {error}")
    if not isinstance(record, dict):
        raise SystemExit(f"native window probe line {line_number} is not an object")
    records.append(record)

if not records:
    raise SystemExit("native window probe emitted no observations")

def ready(record):
    if record.get("action") != "dispatch_evaluation":
        return False
    required = {
        "probe_ok": True,
        "exact_pid_match": True,
        "process_visible": True,
        "window_visible": True,
        "frontmost": True,
        "key_window": True,
        "main_window": True,
        "frontmost_application_matches_target": True,
        "cg_owner_pid_match": True,
        "visible_gate": True,
        "frontmost_gate": True,
        "reason_code": "dispatch_ready",
    }
    if any(record.get(key) != value for key, value in required.items()):
        return False
    if record.get("frontmost_application_pid") != record.get("pid"):
        return False
    non_target_count = record.get("non_target_window_count")
    if not isinstance(non_target_count, int) or isinstance(non_target_count, bool):
        return False
    windows = record.get("cg_window_list")
    if not isinstance(windows, list) or not windows:
        return False
    allowed_window_keys = {
        "placement",
        "window_number",
        "layer",
        "onscreen",
        "bounds",
    }
    return all(
        isinstance(window, dict) and set(window).issubset(allowed_window_keys)
        for window in windows
    )

if not any(ready(record) for record in records):
    raise SystemExit(
        "native window probe never observed an exact-PID, visible, frontmost, "
        "key/main window before screenshot dispatch"
    )
print(f"verified native window readiness observations: {len(records)}")
PY
}

gracefully_close_chrome() {
  local profile="$1"
  [[ -s "$profile/DevToolsActivePort" ]] || return 1
  "$PYTHON_BIN" - "$profile" <<'PY'
import base64
import json
from pathlib import Path
import secrets
import socket
import sys
import time
from urllib.request import urlopen
from urllib.parse import urlsplit

profile = Path(sys.argv[1])
active_port = profile / "DevToolsActivePort"
deadline = time.monotonic() + 5
while time.monotonic() < deadline and not active_port.is_file():
    time.sleep(0.05)
if not active_port.is_file():
    raise SystemExit("Chrome did not publish DevToolsActivePort")
lines = active_port.read_text(encoding="utf-8").splitlines()
if len(lines) < 2:
    raise SystemExit("Chrome DevToolsActivePort is incomplete")
port = int(lines[0])
_browser_path = lines[1]
with urlopen(f"http://127.0.0.1:{port}/json/version", timeout=2) as response:
    version = json.load(response)
parts = urlsplit(version["webSocketDebuggerUrl"])
if parts.hostname != "127.0.0.1":
    raise SystemExit(
        f"Chrome DevTools endpoint escaped the loopback boundary: {parts.hostname!r}"
    )
key = base64.b64encode(secrets.token_bytes(16)).decode("ascii")
request = (
    f"GET {parts.path} HTTP/1.1\r\n"
    f"Host: 127.0.0.1:{port}\r\n"
    "Upgrade: websocket\r\n"
    "Connection: Upgrade\r\n"
    f"Sec-WebSocket-Key: {key}\r\n"
    "Sec-WebSocket-Version: 13\r\n"
    "Origin: http://127.0.0.1\r\n\r\n"
).encode("ascii")
payload = json.dumps({"id": 1, "method": "Browser.close"}).encode("utf-8")
mask = secrets.token_bytes(4)
masked = bytes(byte ^ mask[index % 4] for index, byte in enumerate(payload))
length = len(masked)
if length < 126:
    header = bytes((0x81, 0x80 | length))
elif length < 65536:
    header = bytes((0x81, 0xFE)) + length.to_bytes(2, "big")
else:
    raise SystemExit("Chrome close command is unexpectedly large")
with socket.create_connection((parts.hostname, parts.port), timeout=2) as connection:
    connection.sendall(request)
    response = b""
    while b"\r\n\r\n" not in response:
        chunk = connection.recv(4096)
        if not chunk:
            raise SystemExit("Chrome closed the DevTools handshake")
        response += chunk
    if b" 101 " not in response.split(b"\r\n", 1)[0]:
        raise SystemExit("Chrome rejected the DevTools handshake")
    connection.sendall(header + mask + masked)
PY
}

make_prototype_view() {
  local output="$1"
  "$PYTHON_BIN" - "$PROTOTYPE" "$SURFACE" "$EGUI_TAB" "$output" <<'PY'
from html import escape
from html.parser import HTMLParser
import json
from pathlib import Path
import sys

source_path = Path(sys.argv[1]).resolve()
surface = sys.argv[2]
egui_tab = sys.argv[3]
output_path = Path(sys.argv[4])
source = source_path.read_text(encoding="utf-8")

if "</head>" not in source.lower():
    raise SystemExit(f"prototype has no </head> element: {source_path}")

target_class = "desk" if surface == "egui" else "phone"
target_selector = f".{target_class}"


class SurfaceParser(HTMLParser):
    _void_tags = {
        "area",
        "base",
        "br",
        "col",
        "embed",
        "hr",
        "img",
        "input",
        "link",
        "meta",
        "param",
        "source",
        "track",
        "wbr",
    }

    def __init__(self, target_class):
        super().__init__(convert_charrefs=True)
        self.target_class = target_class
        self.stack = []
        self.template_depth = 0
        self.found = False

    @staticmethod
    def _classes(attrs):
        class_value = next(
            (value for name, value in attrs if name.lower() == "class"), ""
        )
        return set((class_value or "").split())

    def _has_valid_frame_ancestor(self):
        for index, node in enumerate(self.stack[:-1]):
            if index < 2:
                continue
            frame_parent = self.stack[index - 1]
            frame_grandparent = self.stack[index - 2]
            if (
                "frame" in node["classes"]
                and "rack" in frame_parent["classes"]
                and frame_grandparent["tag"] == "body"
            ):
                return True
        return False

    def handle_starttag(self, tag, attrs):
        tag = tag.lower()
        node = {"tag": tag, "classes": self._classes(attrs)}
        self.stack.append(node)
        # The template element itself remains in the DOM; only its content is
        # inert and unreachable by the frame's querySelector.
        if (
            self.template_depth == 0
            and self.target_class in node["classes"]
            and self._has_valid_frame_ancestor()
        ):
            self.found = True
        if tag == "template":
            self.template_depth += 1
        if node["tag"] in self._void_tags:
            self.stack.pop()

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)
        if tag.lower() not in self._void_tags and self.stack:
            self.handle_endtag(tag)

    def handle_endtag(self, tag):
        tag = tag.lower()
        for index in range(len(self.stack) - 1, -1, -1):
            if self.stack[index]["tag"] == tag:
                self.template_depth -= sum(
                    node["tag"] == "template" for node in self.stack[index:]
                )
                del self.stack[index:]
                return


parser = SurfaceParser(target_class)
parser.feed(source)
parser.close()
if not parser.found:
    raise SystemExit(
        f"{surface} prototype must contain {target_selector} inside "
        f"body > .rack > .frame: {source_path}"
    )

if surface == "egui":
    style = """
html, body { width: 1160px !important; height: 631px !important; overflow: hidden !important; }
body { padding: 8px !important; }
    body > h1, body > .sub { display: none !important; }
    body > .rack { display: block !important; width: 1080px !important; }
    body > .rack > .frame { display: none !important; }
    body > .rack > .frame.design-gate-target {
      display: block !important;
      width: 1080px !important;
      max-width: 1080px !important;
      margin: 0 !important;
    }
    #design-gate-error {
      display: block !important;
      margin: 24px !important;
      padding: 18px !important;
      border: 2px solid #f85149 !important;
      background: #3d1618 !important;
      color: #ffb4ab !important;
      font: 700 18px system-ui, sans-serif !important;
    }
    """
    width, height = 1160, 631
elif surface == "ios":
    style = """
html, body { width: 900px !important; height: 900px !important; overflow: hidden !important; }
    body { padding: 8px !important; }
    body > h1, body > .sub { display: none !important; }
    body > .rack { display: flex !important; flex-wrap: nowrap !important; width: 840px !important; gap: 28px !important; }
    body > .rack > .frame { flex: 0 0 auto !important; }
    body > .rack > .frame { display: none !important; }
    body > .rack > .frame.design-gate-target { display: block !important; }
    #design-gate-error {
      display: block !important;
      margin: 24px !important;
      padding: 18px !important;
      border: 2px solid #f85149 !important;
      background: #3d1618 !important;
      color: #ffb4ab !important;
      font: 700 18px system-ui, sans-serif !important;
    }
    """
    width, height = 900, 900
else:
    raise SystemExit(f"unsupported surface: {surface}")

base_href = escape(source_path.parent.as_uri() + "/", quote=True)
target_error = escape(
    f"Design-gate render failed: no frame containing {target_selector} was found."
)
surface_script = f"""
<script id="design-gate-surface-script">
(() => {{
  const targetSelector = {json.dumps(target_selector)};
  const requestedTab = {json.dumps(egui_tab)};
  const markSurface = () => {{
    const frames = Array.from(document.querySelectorAll("body > .rack > .frame"));
    const targets = frames.filter((frame) => frame.querySelector(targetSelector));
    if (targets.length === 0) {{
      const error = document.createElement("div");
      error.id = "design-gate-error";
      error.textContent = {json.dumps(target_error)};
      document.body.prepend(error);
      return;
    }}
    targets.forEach((frame) => frame.classList.add("design-gate-target"));
  }};
  const selectRequestedTab = () => {{
    if ({json.dumps(surface)} !== "egui") return;
    const tab = document.querySelector(`[data-tab="${{requestedTab}}"]`);
    if (tab) tab.click();
  }};
  if (document.readyState === "loading") {{
    document.addEventListener("DOMContentLoaded", () => {{
      markSurface();
      selectRequestedTab();
    }}, {{ once: true }});
  }} else {{
    markSurface();
    selectRequestedTab();
  }}
}})();
</script>
"""
injection = (
    f'<base href="{base_href}">\n'
    '<meta name="design-gate-render" content="generated without editing the source">\n'
    f'<style id="design-gate-surface">{style}</style>\n'
    f'{surface_script}'
)
head_end = source.lower().index("</head>")
derived = source[:head_end] + injection + source[head_end:]
output_path.write_text(derived, encoding="utf-8")
print(f"{width} {height}")
PY
}

run_chrome_screenshot() {
  local html_path="$1"
  local output_path="$2"
  local width="$3"
  local height="$4"
  local label="$5"
  local profile="$STAGE/chrome-profile-$label"
  local chrome_log="$STAGE/chrome-$label.log"
  local url
  local chrome_pid
  local dimensions

  mkdir -p "$profile"
  url="$(file_url "$html_path")"
  log "rendering $label with headless Chrome at ${width}x${height}"
  "$CHROME_BIN" \
    --password-store=basic \
    --headless=new \
    --disable-gpu \
    --disable-background-networking \
    --disable-component-update \
    --disable-domain-reliability \
    --disable-crash-reporter \
    --disable-extensions \
    --disable-sync \
    --hide-scrollbars \
    --force-device-scale-factor=1 \
    --run-all-compositor-stages-before-draw \
    --allow-file-access-from-files \
    --remote-debugging-address="$CHROME_DEVTOOLS_ADDRESS" \
    --remote-debugging-port=0 \
    --remote-allow-origins="$CHROME_DEVTOOLS_ORIGIN" \
    --window-size="$width,$height" \
    --user-data-dir="$profile" \
    --no-first-run \
    --no-default-browser-check \
    --virtual-time-budget=1000 \
    --screenshot="$output_path" \
    "$url" >"$chrome_log" 2>&1 &
  chrome_pid=$!
  CAPTURE_PID="$chrome_pid"
  if ! wait_for_complete_png "$output_path" "$label" "$CHROME_TIMEOUT_SECONDS"; then
    tail -40 "$chrome_log" >&2 || true
    die "headless Chrome did not publish a complete PNG for $label within ${CHROME_TIMEOUT_SECONDS}s: ${PNG_WAIT_REASON}"
  fi
  if gracefully_close_chrome "$profile"; then
    log "requested loopback-only DevTools Browser.close after $label completed"
  else
    warn "could not request loopback-only DevTools shutdown for $label; using owned-child cleanup"
  fi
  terminate_owned_child "$chrome_pid" "$label" \
    || die "could not clean up the owned headless Chrome child for $label"
  assert_png "$output_path"
  dimensions="$(png_dimensions "$output_path")"
  [[ "$dimensions" == "${width}x${height}" ]] \
    || die "$label PNG dimensions are $dimensions; expected ${width}x${height}"
}

make_composite_html() {
  local output="$1"
  local capture_kind="$2"
  "$PYTHON_BIN" - "$STAGE/prototype.png" "$STAGE/live-after.png" "$output" "$ISSUE" "$SURFACE" "$capture_kind" <<'PY'
from html import escape
from pathlib import Path
import sys

prototype = Path(sys.argv[1]).resolve().as_uri()
live = Path(sys.argv[2]).resolve().as_uri()
output = Path(sys.argv[3])
issue = escape(sys.argv[4])
surface = escape(sys.argv[5])
capture_kind = escape(sys.argv[6])

output.write_text(
    f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Corral design gate evidence — issue #{issue}</title>
<style>
  * {{ box-sizing: border-box; }}
  html, body {{ margin: 0; width: 2400px; height: 960px; overflow: hidden; }}
  body {{ background: #05070a; color: #e6edf3; font-family: -apple-system, BlinkMacSystemFont, system-ui, sans-serif; }}
  .canvas {{ width: 2400px; height: 960px; padding: 24px; }}
  h1 {{ margin: 0; font-size: 24px; letter-spacing: .04em; text-transform: uppercase; color: #e6edf3; }}
  .sub {{ margin-top: 6px; color: #8b949e; font-size: 14px; }}
  .stamp {{ color: #2dd4bf; font-weight: 700; }}
  .panels {{ display: grid; grid-template-columns: 1166px 1166px; gap: 20px; margin-top: 18px; }}
  .panel {{ width: 1166px; height: 800px; padding: 14px; border: 1px solid #30363d; border-radius: 14px; background: #10151c; }}
  .label {{ height: 28px; color: #8b949e; font-size: 12px; letter-spacing: .12em; text-transform: uppercase; text-align: center; }}
  .viewport {{ width: 1136px; height: 744px; display: flex; align-items: center; justify-content: center; overflow: hidden; border: 1px solid #30363d; border-radius: 10px; background: #0d1117; }}
  img {{ display: block; max-width: 1120px; max-height: 728px; object-fit: contain; }}
</style>
</head>
<body>
  <main class="canvas">
    <h1>Corral design gate <span class="stamp">· issue #{issue}</span></h1>
    <div class="sub">prototype ↔ live · surface: {surface} · deterministic capture bundle</div>
    <section class="panels">
      <article class="panel"><div class="label">Prototype · approved HTML render</div><div class="viewport"><img src="{prototype}" alt="Prototype render"></div></article>
      <article class="panel"><div class="label">Live board · {capture_kind}</div><div class="viewport"><img src="{live}" alt="Live board screenshot"></div></article>
    </section>
  </main>
</body>
</html>
""",
    encoding="utf-8",
)
PY
}

require_egui_dependencies() {
  command -v curl >/dev/null 2>&1 || die "curl is required for an egui live health check"
  if [[ -n "$EGUI_BINARY" ]]; then
    [[ -x "$EGUI_BINARY" ]] || die "egui binary is not executable: $EGUI_BINARY"
  elif [[ ! -x "$REPO_DIR/target/release/corrald-ui" && "$BUILD_EGUI" -eq 0 ]]; then
    die "target/release/corrald-ui is missing; build it or omit --no-build"
  else
    command -v cargo >/dev/null 2>&1 \
      || die "cargo is required to build target/release/corrald-ui"
  fi
}

capture_egui() {
  local health
  local snapshot_path="$STAGE/snapshot.json"
  local ui_pid
  local binary
  local native_probe_helper="${CORRAL_UI_WINDOW_PROBE_HELPER:-}"
  local native_probe_log="$STAGE/native-window-probe.jsonl"
  local ui_config_seed_dir="${CORRAL_UI_CONFIG_SEED_DIR:-}"

  require_egui_dependencies
  health="$(curl --fail --silent --show-error --max-time 5 "$HOST_URL/healthz")" \
    || die "egui live host is not healthy at $HOST_URL/healthz; start corrald first"
  [[ "${health//$'\n'/}" == "ok" ]] \
    || die "unexpected health response from $HOST_URL/healthz: $health"
  curl --fail --silent --show-error --max-time 5 "$HOST_URL/snapshot" >"$snapshot_path" \
    || die "egui live host snapshot failed at $HOST_URL/snapshot"

  if [[ -z "$LIVE_AGENT" ]]; then
    LIVE_AGENT="$($PYTHON_BIN - "$snapshot_path" <<'PY'
import json
from pathlib import Path
import sys

snapshot = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
candidates = sorted(
    agent_id
    for agent_id, agent in snapshot.get("agents", {}).items()
    if isinstance(agent, dict)
)
if not candidates:
    raise SystemExit("/snapshot contains no agents to select")
print(candidates[0])
PY
)"
    [[ -n "$LIVE_AGENT" ]] || die "could not select a live agent from /snapshot"
    log "auto-selected first live agent from /snapshot: $LIVE_AGENT"
  fi

  "$PYTHON_BIN" - "$snapshot_path" "$LIVE_AGENT" <<'PY'
import json
from pathlib import Path
import sys

snapshot = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
agent_id = sys.argv[2]
agent = snapshot.get("agents", {}).get(agent_id)
if not isinstance(agent, dict):
    available = sorted(snapshot.get("agents", {}).keys())
    raise SystemExit(
        f"agent {agent_id!r} is not present in /snapshot; available count={len(available)}"
    )
print(
    f"{agent_id} state={agent.get('state', 'unknown')} "
    f"title={agent.get('title', 'unknown')}"
)
PY

  binary="$EGUI_BINARY"
  if [[ -z "$binary" ]]; then
    binary="$REPO_DIR/target/release/corrald-ui"
  fi
  if [[ ! -x "$binary" ]]; then
    log "building egui release binary"
    (cd "$REPO_DIR" && cargo build --release -p corrald-ui)
  fi
  [[ -x "$binary" ]] || die "egui binary was not produced: $binary"
  EGUI_BINARY="$binary"

  if [[ "$(uname -s)" == "Darwin" ]]; then
    if [[ -z "$native_probe_helper" ]]; then
      command -v swiftc >/dev/null 2>&1 \
        || die "swiftc is required to compile the exact-PID CoreGraphics probe"
      native_probe_helper="$STAGE/native-window-probe"
      swiftc -O "$REPO_DIR/scripts/native-window-probe.swift" \
        -o "$native_probe_helper" \
        || die "could not compile the exact-PID CoreGraphics probe"
    fi
    [[ -x "$native_probe_helper" ]] \
      || die "native window probe helper is not executable: $native_probe_helper"
  fi

  CAPTURE_KIND="native egui viewport screenshot"
  LIVE_DESCRIPTION="real egui process launched against a loopback corrald; selected live agent $LIVE_AGENT from /snapshot"
  CAPTURE_COMMAND="CORRAL_UI_SCREENSHOT=<issue-dir>/live-after.png CORRAL_UI_SCREENSHOT_DELAY_MS=$EGUI_DELAY_MS CORRAL_UI_SCREENSHOT_TAB=$EGUI_TAB"
  CAPTURE_COMMAND+=" CORRAL_UI_SCREENSHOT_AGENT=$LIVE_AGENT"
  CAPTURE_COMMAND+=" CORRAL_UI_DISABLE_KEYRING=1"
  CAPTURE_COMMAND+=" CORRAL_UI_CONFIG_DIR=<stage>/ui-config <corrald-ui>"
  if [[ -n "$EGUI_WAKE_COMMAND" ]]; then
    CAPTURE_COMMAND+="; wake with caller command"
  fi
  CAPTURE_COMMAND+="; exact-PID native visible/frontmost/key/main/on-screen probe"

  log "capturing live egui board; health check passed"
  if [[ -n "$LIVE_AGENT" ]]; then
    log "selected target: $LIVE_AGENT"
  fi
  local ui_config_dir="$STAGE/ui-config"
  mkdir -p "$ui_config_dir"
  if [[ -n "$ui_config_seed_dir" ]]; then
    [[ -d "$ui_config_seed_dir" ]] \
      || die "egui UI config seed directory does not exist: $ui_config_seed_dir"
    [[ -f "$ui_config_seed_dir/config.json" ]] \
      || die "egui UI config seed is missing config.json: $ui_config_seed_dir"
    cp -p -- "$ui_config_seed_dir/config.json" "$ui_config_dir/config.json"
    for key_path in "$ui_config_seed_dir"/keys/*.key; do
      [[ -f "$key_path" ]] || continue
      mkdir -p "$ui_config_dir/keys"
      cp -p -- "$key_path" "$ui_config_dir/keys/"
    done
  fi
  CORRAL_UI_SCREENSHOT_AGENT="$LIVE_AGENT" \
    CORRAL_UI_SCREENSHOT="$STAGE/live-after.png" \
  CORRAL_UI_SCREENSHOT_DELAY_MS="$EGUI_DELAY_MS" \
  CORRAL_UI_SCREENSHOT_TAB="$EGUI_TAB" \
  CORRAL_UI_SCREENSHOT_WAKE_COMMAND="$EGUI_WAKE_COMMAND" \
    CORRAL_UI_DISABLE_KEYRING=1 \
    CORRAL_UI_CONFIG_DIR="$ui_config_dir" \
    CORRAL_UI_WINDOW_PROBE_HELPER="$native_probe_helper" \
    CORRAL_UI_WINDOW_DIAGNOSTIC_LOG="$native_probe_log" \
    RUST_LOG="${RUST_LOG:-info}" "$binary" >"$STAGE/capture.log" 2>&1 &
  ui_pid=$!
  CAPTURE_PID="$ui_pid"
  if [[ -n "$EGUI_WAKE_COMMAND" ]]; then
    log "running explicit egui wake command"
    sleep 1
    if ! CORRAL_UI_SCREENSHOT_PID="$ui_pid" \
      CORRAL_UI_SCREENSHOT_PATH="$STAGE/live-after.png" \
      bash -c "$EGUI_WAKE_COMMAND" >>"$STAGE/capture.log" 2>&1; then
      tail -40 "$STAGE/capture.log" >&2 || true
      terminate_owned_child "$ui_pid" "egui" \
        || die "egui wake command failed and owned-child cleanup failed"
      die "egui wake command failed; no live screenshot claim was made"
    fi
  fi
  if ! wait_for_complete_png "$STAGE/live-after.png" "egui" "$CAPTURE_TIMEOUT_SECONDS"; then
    tail -80 "$STAGE/capture.log" >&2 || true
    die "egui did not publish a complete PNG within ${CAPTURE_TIMEOUT_SECONDS}s: ${PNG_WAIT_REASON}"
  fi
  terminate_owned_child "$ui_pid" "egui" \
    || die "could not clean up the owned egui child"
  validate_native_probe "$native_probe_log" \
    || {
      tail -80 "$STAGE/capture.log" >&2 || true
      die "egui capture did not prove exact-PID native window readiness"
    }
  {
    printf '\n--- native window probe records ---\n'
    cat "$native_probe_log"
  } >>"$STAGE/capture.log"
  grep -F -- "requesting viewport screenshot" "$STAGE/capture.log" \
    || die "egui capture did not dispatch a viewport screenshot"
  grep -F -- "screenshot event received" "$STAGE/capture.log" \
    || die "egui capture did not receive a Screenshot event"
  grep -F -- "screenshot saved — exiting" "$STAGE/capture.log" \
    || die "egui capture did not save the Screenshot event PNG"
  if [[ -n "$LIVE_AGENT" ]] \
    && ! grep -q "native screenshot evidence selected live agent" "$STAGE/capture.log"; then
    tail -80 "$STAGE/capture.log" >&2 || true
    die "egui screenshot log did not prove selection of $LIVE_AGENT"
  fi
  assert_png "$STAGE/live-after.png"
}

capture_ios() {
  local app_path="$IOS_APP"
  local app_q
  local bundle_q
  local project_q
  local derived_q
  local output_q
  local before_output_q
  local command_text
  local launch_arg
  local before_launch_args_q=""
  local after_launch_args_q=""
  local before_has_demo_mode=0
  local after_has_demo_mode=0
  local -a before_args=()
  local -a after_args=()

  command -v hermes-sim-task >/dev/null 2>&1 \
    || die "hermes-sim-task is required for iOS capture; do not run simctl directly"
  if [[ "$IOS_MODE" == "live" && -z "$IOS_COMMAND" ]]; then
    die "iOS live capture requires --ios-command to prepare/launch the live app inside the temporary simulator; use --ios-mode demo only for an explicit Debug fixture"
  fi
  if [[ -n "$app_path" ]]; then
    [[ -d "$app_path" ]] || die "--ios-app is not an app bundle directory: $app_path"
    [[ "$app_path" == *.app ]] || die "--ios-app must point to a .app bundle: $app_path"
  elif [[ "$BUILD_IOS" -eq 0 ]]; then
    die "--ios-app is required with --no-build"
  fi

  if [[ "$IOS_MODE" == "demo" ]]; then
    if (( ${#IOS_BEFORE_LAUNCH_ARGS[@]} > 0 )); then
      before_args=("${IOS_BEFORE_LAUNCH_ARGS[@]}")
    fi
    if (( ${#IOS_LAUNCH_ARGS[@]} > 0 )); then
      after_args=("${IOS_LAUNCH_ARGS[@]}")
    fi
    if (( ${#before_args[@]} > 0 )); then
      for launch_arg in "${before_args[@]}"; do
        if [[ "$launch_arg" == "-demoMode" ]]; then
          before_has_demo_mode=1
        fi
      done
    fi
    if (( ${#after_args[@]} > 0 )); then
      for launch_arg in "${after_args[@]}"; do
        if [[ "$launch_arg" == "-demoMode" ]]; then
          after_has_demo_mode=1
        fi
      done
    fi
    if [[ "$before_has_demo_mode" -eq 0 ]]; then
      before_args=("-demoMode" "${before_args[@]}")
    fi
    if [[ "$after_has_demo_mode" -eq 0 ]]; then
      after_args=("-demoMode" "${after_args[@]}")
    fi
    if (( ${#before_args[@]} > 0 )); then
      for launch_arg in "${before_args[@]}"; do
        before_launch_args_q+=" $(shell_quote "$launch_arg")"
      done
    fi
    if (( ${#after_args[@]} > 0 )); then
      for launch_arg in "${after_args[@]}"; do
        after_launch_args_q+=" $(shell_quote "$launch_arg")"
      done
    fi
  fi

  app_q="$(shell_quote "$app_path")"
  bundle_q="$(shell_quote "$IOS_BUNDLE_ID")"
  project_q="$(shell_quote "$REPO_DIR/ios/FleetNotifier.xcodeproj")"
  derived_q="$(shell_quote "$STAGE/ios-derived-data")"
  output_q="$(shell_quote "$STAGE/live-after.png")"
  before_output_q="$(shell_quote "$STAGE/ios-before-detail.png")"
  command_text=$'set -euo pipefail\n'
  if [[ -n "$app_path" ]]; then
    command_text+="xcrun simctl install \"\$SIMULATOR_UDID\" $app_q"$'\n'
  else
    command_text+="xcodebuild -project $project_q -scheme FleetNotifier -configuration Debug -destination \"id=\$SIMULATOR_UDID\" -derivedDataPath $derived_q CODE_SIGNING_ALLOWED=NO build"$'\n'
    command_text+="app_path=$derived_q/Build/Products/Debug-iphonesimulator/FleetNotifier.app"$'\n'
    command_text+="xcrun simctl install \"\$SIMULATOR_UDID\" \"\$app_path\""$'\n'
  fi
  if [[ "$IOS_MODE" == "demo" ]]; then
    command_text+="printf '%s\\n' 'capture: before frame via DEBUG demo route'"$'\n'
    command_text+="xcrun simctl launch \"\$SIMULATOR_UDID\" $bundle_q$before_launch_args_q"$'\n'
    command_text+="sleep $(shell_quote "$IOS_DELAY_SECONDS")"$'\n'
    command_text+="xcrun simctl io \"\$SIMULATOR_UDID\" screenshot $before_output_q"$'\n'
    command_text+="xcrun simctl terminate \"\$SIMULATOR_UDID\" $bundle_q || true"$'\n'
    command_text+="printf '%s\\n' 'capture: after frame via DEBUG transcript-chat route'"$'\n'
    command_text+="xcrun simctl launch \"\$SIMULATOR_UDID\" $bundle_q$after_launch_args_q"$'\n'
  else
    command_text+="$IOS_COMMAND"$'\n'
  fi
  command_text+="sleep $(shell_quote "$IOS_DELAY_SECONDS")"$'\n'
  command_text+="xcrun simctl io \"\$SIMULATOR_UDID\" screenshot $output_q"$'\n'

  if [[ "$IOS_MODE" == "demo" ]]; then
    CAPTURE_KIND="iOS simulator before/after screenshots via hermes-sim-task (Debug demo fixture)"
    LIVE_DESCRIPTION="after frame uses the permanent Debug detail route; before frame uses its opt-in legacy presentation; neither is live-daemon evidence"
  else
    CAPTURE_KIND="iOS simulator screenshot via hermes-sim-task (caller-prepared live app)"
    LIVE_DESCRIPTION="caller-prepared iOS live app in a private Herdr-owned simulator"
  fi
  CAPTURE_COMMAND="hermes-sim-task --shell <install/build, before/after launch, and screenshot command>"
  if [[ "$IOS_MODE" == "demo" ]]; then
    CAPTURE_COMMAND+="; before launch args:${before_launch_args_q}; after launch args:${after_launch_args_q}"
  fi
  log "capturing iOS surface through hermes-sim-task ($IOS_MODE mode)"
  if ! hermes-sim-task --shell "$command_text" >"$STAGE/capture.log" 2>&1; then
    tail -120 "$STAGE/capture.log" >&2 || true
    die "hermes-sim-task iOS capture failed; the private simulator was owned by the wrapper"
  fi
  assert_png "$STAGE/live-after.png"
  if [[ "$IOS_MODE" == "demo" && "${#IOS_BEFORE_LAUNCH_ARGS[@]}" -gt 0 ]]; then
    assert_png "$STAGE/ios-before-detail.png"
  fi
}

print_dry_run() {
  local output_dir="$OUTPUT_ROOT/issue-$ISSUE"
  log "dry run"
  log "surface: $SURFACE"
  log "prototype: $PROTOTYPE"
  log "output: $output_dir/{prototype.png,ios-before-detail.png,live-after.png,comparison.png,conformance.md,capture.log}"
  log "Chrome: $CHROME_BIN"
  if [[ -n "$LIVE_PNG" ]]; then
    log "live source: explicit supplied PNG fixture $LIVE_PNG"
  elif [[ "$SURFACE" == "egui" ]]; then
    log "live source: native corrald-ui capture after health check at $HOST_URL"
    log "egui tab: $EGUI_TAB"
  else
    log "live source: hermes-sim-task iOS capture in $IOS_MODE mode"
    if [[ "${#IOS_BEFORE_LAUNCH_ARGS[@]}" -gt 0 ]]; then
      log "before source: permanent Debug launch route with ${#IOS_BEFORE_LAUNCH_ARGS[@]} launch arguments"
    fi
  fi
  if [[ "$SURFACE" == "ios" && "$IOS_MODE" == "live" ]]; then
    [[ -n "$IOS_COMMAND" ]] \
      || die "dry run: iOS live mode still requires --ios-command"
  fi
  if [[ "$SURFACE" == "egui" && -n "$LIVE_AGENT" ]]; then
    log "egui agent: $LIVE_AGENT (will be checked against /snapshot)"
  fi
  if [[ -n "$EGUI_WAKE_COMMAND" ]]; then
    log "egui wake: explicit caller command supplied"
  fi
  if [[ -e "$output_dir" && "$FORCE" -eq 0 ]]; then
    log "overwrite: blocked unless --force is supplied"
  fi
}

if [[ "$DRY_RUN" -eq 1 ]]; then
  print_dry_run
  exit 0
fi

IMPLEMENTATION_IDENTITY="$(implementation_identity)" \
  || die "could not compute the implementation content identity"
IMPLEMENTATION_CONTENT_DIGEST="${IMPLEMENTATION_IDENTITY%%$'\n'*}"
IMPLEMENTATION_MANIFEST="${IMPLEMENTATION_IDENTITY#*$'\n'}"
[[ "$IMPLEMENTATION_CONTENT_DIGEST" == sha256:* ]] \
  || die "implementation identity did not return a sha256 digest"

OUTPUT_DIR="$OUTPUT_ROOT/issue-$ISSUE"
if [[ -e "$OUTPUT_DIR" ]]; then
  [[ -d "$OUTPUT_DIR" ]] || die "output path exists but is not a directory: $OUTPUT_DIR"
  [[ "$FORCE" -eq 1 ]] \
    || die "evidence bundle already exists: $OUTPUT_DIR (pass --force to replace it)"
fi
mkdir -p "$OUTPUT_ROOT"
mkdir -p "$OUTPUT_DIR"
STAGE="$(mktemp -d "$OUTPUT_DIR/.design-gate.stage.XXXXXX")"
CAPTURE_PID=""
cleanup() {
  local cleanup_status=0
  local remove_attempt
  if [[ -n "${CAPTURE_PID:-}" ]]; then
    if ! terminate_owned_child "$CAPTURE_PID" "capture"; then
      cleanup_status=1
    fi
  fi
  if [[ -n "${STAGE:-}" && -d "$STAGE" ]]; then
    # Chrome can finish closing a profile helper just after Browser.close and
    # briefly race the directory removal on macOS. Retry only this owned,
    # stage-local path for a bounded interval; never broaden cleanup to a
    # parent directory or a user's browser profile.
    for remove_attempt in {1..20}; do
      rm -rf -- "$STAGE" || true
      if [[ ! -e "$STAGE" ]]; then
        break
      fi
      sleep 0.25
    done
    if [[ -e "$STAGE" ]]; then
      printf 'error: private design-gate stage survived bounded cleanup: %s\n' "$STAGE" >&2
      cleanup_status=1
    fi
  fi
  return "$cleanup_status"
}
trap cleanup EXIT

PROTOTYPE_VIEW="$STAGE/prototype-view.html"
if ! prototype_size="$(make_prototype_view "$PROTOTYPE_VIEW")"; then
  die "could not prepare the prototype render; check the --prototype surface"
fi
IFS=' ' read -r PROTOTYPE_WIDTH PROTOTYPE_HEIGHT <<<"$prototype_size"
run_chrome_screenshot "$PROTOTYPE_VIEW" "$STAGE/prototype.png" \
  "$PROTOTYPE_WIDTH" "$PROTOTYPE_HEIGHT" prototype

CAPTURE_KIND=""
LIVE_DESCRIPTION=""
CAPTURE_COMMAND=""
if [[ -n "$LIVE_PNG" ]]; then
  CAPTURE_KIND="explicit supplied PNG fixture"
  LIVE_DESCRIPTION="caller-supplied file; this run did not capture a live surface"
  CAPTURE_COMMAND="cp $LIVE_PNG <issue-dir>/live-after.png"
  cp -- "$LIVE_PNG" "$STAGE/live-after.png"
  printf 'supplied fixture: %s\n' "$LIVE_PNG" >"$STAGE/capture.log"
  assert_png "$STAGE/live-after.png"
elif [[ "$SURFACE" == "egui" ]]; then
  capture_egui
else
  capture_ios
fi
normalize_capture_log "$STAGE/capture.log"

COMPOSITE_HTML="$STAGE/comparison.html"
make_composite_html "$COMPOSITE_HTML" "$CAPTURE_KIND"
run_chrome_screenshot "$COMPOSITE_HTML" "$STAGE/comparison.png" 2400 960 comparison

PROTOTYPE_SOURCE_SHA="$(sha256_file "$PROTOTYPE")"
GENERATOR_SHA="$(sha256_file "$SCRIPT_DIR/$SCRIPT_NAME")"
PROTOTYPE_SHA="$(sha256_file "$STAGE/prototype.png")"
LIVE_SHA="$(sha256_file "$STAGE/live-after.png")"
if [[ -n "$LIVE_PNG" ]]; then
  LIVE_SOURCE_PATH="$LIVE_PNG"
  LIVE_SOURCE_SHA="$(sha256_file "$LIVE_PNG")"
else
  LIVE_SOURCE_PATH="generated by the capture command above"
  LIVE_SOURCE_SHA="not applicable (generated capture)"
fi
COMPARISON_SHA="$(sha256_file "$STAGE/comparison.png")"
PROTOTYPE_DIMS="$(png_dimensions "$STAGE/prototype.png")"
LIVE_DIMS="$(png_dimensions "$STAGE/live-after.png")"
COMPARISON_DIMS="$(png_dimensions "$STAGE/comparison.png")"
IOS_BEFORE_SHA="not applicable"
IOS_BEFORE_DIMS="not applicable"
if [[ -s "$STAGE/ios-before-detail.png" ]]; then
  IOS_BEFORE_SHA="$(sha256_file "$STAGE/ios-before-detail.png")"
  IOS_BEFORE_DIMS="$(png_dimensions "$STAGE/ios-before-detail.png")"
fi
GIT_SHA="$(git -C "$REPO_DIR" rev-parse HEAD 2>/dev/null || printf 'unknown')"
IDENTITY_AFTER="$(implementation_identity)" \
  || die "could not re-check the implementation content identity"
[[ "${IDENTITY_AFTER%%$'\n'*}" == "$IMPLEMENTATION_CONTENT_DIGEST" ]] \
  || die "implementation content changed during capture; evidence was not published"
if [[ -n "$EGUI_BINARY" && -f "$EGUI_BINARY" ]]; then
  LIVE_BINARY_SHA="$(sha256_file "$EGUI_BINARY")"
else
  LIVE_BINARY_SHA="not applicable"
fi
if [[ -n "$DAEMON_BINARY" ]]; then
  DAEMON_BINARY_SHA="$(sha256_file "$DAEMON_BINARY")"
else
  DAEMON_BINARY_SHA="not applicable (not supplied)"
fi
if [[ -n "$FIXTURE_REGISTRY" ]]; then
  FIXTURE_REGISTRY_SHA="$(sha256_file "$FIXTURE_REGISTRY")"
else
  FIXTURE_REGISTRY_SHA="not applicable (not supplied)"
fi
CAPTURE_LOG_SHA="$(sha256_file "$STAGE/capture.log")"
GENERATED_AT="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
if [[ "$PROTOTYPE" == "$REPO_DIR/"* ]]; then
  PROTOTYPE_DISPLAY="${PROTOTYPE#"$REPO_DIR/"}"
else
  PROTOTYPE_DISPLAY="$PROTOTYPE"
fi
if [[ "$SURFACE" == "egui" && "$ISSUE" == "206" && -z "$LIVE_PNG" ]]; then
  COMMAND_LINE="scripts/test-design-gate-egui-integration.sh --publish"
else
  COMMAND_LINE="scripts/design-gate-evidence.sh --issue $ISSUE --surface $SURFACE"
  if [[ -n "$PROTOTYPE_DISPLAY" ]]; then
    COMMAND_LINE+=" --prototype $PROTOTYPE_DISPLAY"
  fi
  if [[ "$SURFACE" == "egui" ]]; then
    COMMAND_LINE+=" --egui-tab $EGUI_TAB"
  else
    COMMAND_LINE+=" --ios-mode $IOS_MODE"
    if (( ${#IOS_LAUNCH_ARGS[@]} > 0 )); then
      for launch_arg in "${IOS_LAUNCH_ARGS[@]}"; do
        COMMAND_LINE+=" --ios-launch-arg $(shell_quote "$launch_arg")"
      done
    fi
    if (( ${#IOS_BEFORE_LAUNCH_ARGS[@]} > 0 )); then
      for launch_arg in "${IOS_BEFORE_LAUNCH_ARGS[@]}"; do
        COMMAND_LINE+=" --ios-before-launch-arg $(shell_quote "$launch_arg")"
      done
    fi
  fi
  if [[ -n "$LIVE_PNG" ]]; then
    COMMAND_LINE+=" --live-png <supplied-png>"
  fi
fi
if [[ "$CHROME_BIN_EXPLICIT" -eq 1 ]]; then
  COMMAND_LINE="CHROME_BIN=$(shell_quote "$CHROME_BIN") $COMMAND_LINE"
fi
if [[ "$CHROME_BIN_EXPLICIT" -eq 1 ]]; then
  RENDERER_GUIDANCE='`CHROME_BIN` was explicitly set for this capture; use a complete GUI-capable Chrome/Chromium when the default renderer cannot complete.'
else
  RENDERER_GUIDANCE=""
fi
# Markdown backticks are literal printf text, not shell command substitutions.
# shellcheck disable=SC2016
{
  printf '# Issue #%s design-gate evidence\n\n' "$ISSUE"
  printf 'Generated: `%s`\n\n' "$GENERATED_AT"
  printf '## Capture\n\n'
  printf -- '- Surface: `%s`\n' "$SURFACE"
  printf -- '- Capture kind: %s\n' "$CAPTURE_KIND"
  if [[ "$SURFACE" == "egui" ]]; then
    printf -- '- Egui tab: `%s` (native and approved prototype were both opened on this tab)\n' "$EGUI_TAB"
  fi
  printf -- '- Live description: %s\n' "$LIVE_DESCRIPTION"
  printf -- '- Command: `%s`\n' "$CAPTURE_COMMAND"
  printf -- '- Completion contract: the writer had to publish a complete, CRC-checked PNG before owned-child cleanup; a lingering writer is terminated with TERM then bounded KILL, and the final PNG is validated again.\n'
  printf -- '- Chrome trust boundary: temporary DevTools is loopback-only on `127.0.0.1`, uses an ephemeral port/private profile, and receives only `Browser.close`; local approved HTML and the owned process are trusted inputs.\n'
  if [[ "$SURFACE" == "egui" && -z "$LIVE_PNG" ]]; then
    printf -- '- Host health URL: `loopback corrald /healthz` (checked before capture)\n'
  elif [[ "$SURFACE" == "ios" ]]; then
    printf -- '- Host health URL: not applicable (iOS simulator capture)\n'
  else
    printf -- '- Host health URL: not checked for this explicit supplied PNG fixture\n'
  fi
  if [[ -n "$LIVE_AGENT" ]]; then
    printf -- '- Selected live agent: `%s` (validated against `/snapshot`)\n' "$LIVE_AGENT"
  else
    printf -- '- Selected live agent: none\n'
  fi
  if [[ "$SURFACE" == "ios" ]]; then
    printf -- '- Simulator ownership: `hermes-sim-task`; no simulator deletion command is used.\n'
    printf -- '- iOS mode: `%s`\n' "$IOS_MODE"
  fi
  if [[ -n "$PROVENANCE_NOTE" ]]; then
    printf -- '- Operator/environment note: %s\n' "$PROVENANCE_NOTE"
  fi
  if [[ -n "$RENDERER_GUIDANCE" ]]; then
    printf -- '- Renderer guidance: %s\n' "$RENDERER_GUIDANCE"
  fi
  printf '\n## Sources\n\n'
  printf -- '- Prototype source: `%s`\n' "$PROTOTYPE_DISPLAY"
  printf -- '- Prototype source SHA-256: `%s`\n' "$PROTOTYPE_SOURCE_SHA"
  printf -- '- Generator SHA-256: `%s`\n' "$GENERATOR_SHA"
  printf -- '- Live input: `%s`\n' "$LIVE_SOURCE_PATH"
  printf -- '- Live input SHA-256: `%s`\n' "$LIVE_SOURCE_SHA"
  printf -- '- Repository HEAD at capture (context only; not the evidence identity): `%s`\n' "$GIT_SHA"
  printf -- '- Implementation content digest: `%s`\n' "$IMPLEMENTATION_CONTENT_DIGEST"
  if [[ "$ISSUE" == "205" ]]; then
    printf -- '- Implementation identity scope: issue #205 iOS transcript implementation and tests, the applicable egui transcript mirror, release wiring/docs, capture generator, approved transcript prototype, and a narrow selected eframe/wgpu Cargo.lock package fingerprint; generated evidence is excluded.\n'
  else
    printf -- '- Implementation identity scope: egui client, native capture/probe/verifier tooling, approved prototype, and a narrow selected eframe/wgpu Cargo.lock package fingerprint; unrelated workspace/daemon files and generated evidence are excluded.\n'
  fi
  printf -- '- Renderer executable: `%s` (private profile, loopback-only DevTools, and owned cleanup)\n' "$CHROME_BIN"
  printf -- '- Native UI binary SHA-256: `%s`\n' "$LIVE_BINARY_SHA"
  printf -- '- Daemon binary SHA-256: `%s`\n' "$DAEMON_BINARY_SHA"
  printf -- '- Fixture registry SHA-256: `%s`\n' "$FIXTURE_REGISTRY_SHA"
  printf -- '- Reproducible invocation: `%s`\n' "$COMMAND_LINE"
  printf '\n### Implementation manifest\n\n%s\n' "$IMPLEMENTATION_MANIFEST"
  printf '\n## Artifacts\n\n'
  printf -- '| File | Dimensions | SHA-256 |\n| --- | --- | --- |\n'
  printf -- '| `prototype.png` | `%s` | `%s` |\n' "$PROTOTYPE_DIMS" "$PROTOTYPE_SHA"
  if [[ "$IOS_BEFORE_DIMS" != "not applicable" ]]; then
    printf -- '| `ios-before-detail.png` | `%s` | `%s` |\n' "$IOS_BEFORE_DIMS" "$IOS_BEFORE_SHA"
  fi
  printf -- '| `live-after.png` | `%s` | `%s` |\n' "$LIVE_DIMS" "$LIVE_SHA"
  printf -- '| `comparison.png` | `%s` | `%s` |\n' "$COMPARISON_DIMS" "$COMPARISON_SHA"
  printf -- '| `capture.log` | `n/a` | `%s` |\n' "$CAPTURE_LOG_SHA"
  printf '\nThe comparison header is stamped with the target issue number. A supplied PNG or iOS Debug demo is explicitly labeled above and must not be read as proof of a live daemon session.\n'
} >"$STAGE/conformance.md"

PUBLISHED_ARTIFACTS=(prototype.png live-after.png comparison.png conformance.md capture.log)
if [[ "$IOS_BEFORE_DIMS" != "not applicable" ]]; then
  PUBLISHED_ARTIFACTS=(prototype.png ios-before-detail.png live-after.png comparison.png conformance.md capture.log)
fi

for artifact in "${PUBLISHED_ARTIFACTS[@]}"; do
  [[ -s "$STAGE/$artifact" ]] || die "validated artifact is missing or empty: $artifact"
done

for artifact in "${PUBLISHED_ARTIFACTS[@]}"; do
  mv -f -- "$STAGE/$artifact" "$OUTPUT_DIR/$artifact"
done
rm -rf -- "$STAGE"
STAGE=""

log "wrote $OUTPUT_DIR"
log "prototype: $OUTPUT_DIR/prototype.png ($PROTOTYPE_DIMS)"
if [[ "$IOS_BEFORE_DIMS" != "not applicable" ]]; then
  log "before: $OUTPUT_DIR/ios-before-detail.png ($IOS_BEFORE_DIMS)"
fi
log "live: $OUTPUT_DIR/live-after.png ($LIVE_DIMS)"
log "comparison: $OUTPUT_DIR/comparison.png ($COMPARISON_DIMS)"
