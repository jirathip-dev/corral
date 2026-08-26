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
# Newly generated conformance.md files are the stable manifest contract: they
# omit wall-clock metadata, record the canonical generator path/hash and a
# typed invocation, and use repo-relative paths or stable external
# placeholders. Ordinary non-path arguments remain byte-for-byte; provenance
# notes and opaque command/path values receive only targeted known-root and
# disposable-path substitutions. Newly generated capture.log files are
# byte-oriented bounded views (64 KiB by default); exact head/tail bytes survive
# documented path substitution, and invalid UTF-8 is never decoded or replaced.
# Historical checked-in evidence may predate this contract and is labeled
# accordingly.
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
# with --force are safe: all work is staged in private sibling directories,
# existing evidence is untouched on failure, and the validated artifact set is
# published with a directory-level rename plus rollback of the old bundle.
#
# Dependencies: Bash 3+, Python 3, headless-capable Chrome/Chromium, and (for
# native captures) curl/cargo or hermes-sim-task as described above. Set
# CHROME_BIN or PYTHON_BIN to override discovery.

set -euo pipefail
IFS=$'\n\t'

canonical_script_path() {
  local candidate="$1"
  local directory
  local target
  local link_hops=0

  if [[ "$candidate" != /* ]]; then
    candidate="$PWD/$candidate"
  fi
  while [[ -L "$candidate" ]]; do
    link_hops=$((link_hops + 1))
    [[ "$link_hops" -le 40 ]] || return 1
    directory="$(cd -P "$(dirname "$candidate")" 2>/dev/null && pwd)" \
      || return 1
    target="$(readlink "$candidate")" || return 1
    if [[ "$target" == /* ]]; then
      candidate="$target"
    else
      candidate="$directory/$target"
    fi
  done
  directory="$(cd -P "$(dirname "$candidate")" 2>/dev/null && pwd)" \
    || return 1
  printf '%s/%s\n' "$directory" "$(basename "$candidate")"
}

SCRIPT_SOURCE="${BASH_SOURCE[0]}"
SCRIPT_PATH="$(canonical_script_path "$SCRIPT_SOURCE")" \
  || {
    printf 'error: could not resolve canonical BASH_SOURCE path: %s\n' "$SCRIPT_SOURCE" >&2
    exit 1
  }
SCRIPT_DIR="$(dirname "$SCRIPT_PATH")"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd -P)"
ORIGINAL_ARGS=("$@")

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
WORKTREES_ROOT="${CORRAL_WORKTREES_ROOT:-}"
DRY_RUN=0
STAGE=""

# Published capture.log is a byte-oriented, bounded view. The exact first and
# last bytes survive normalization; only known checkout/staging roots, generic
# disposable Herdr worktree roots/descendants, and an explicitly documented middle
# omission marker are changed. In particular, this must never decode with
# replacement characters. CORRAL_WORKTREES_ROOT, when set, identifies the
# configured root even when it contains spaces or does not use .herdr.
CAPTURE_LOG_MAX_BYTES=65536
CAPTURE_LOG_HEAD_BYTES=8192
CAPTURE_LOG_TAIL_BYTES=57344

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
  CORRAL_WORKTREES_ROOT       Worktree root to redact from generated logs.
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
if [[ -n "$WORKTREES_ROOT" && "$WORKTREES_ROOT" != /* ]]; then
  WORKTREES_ROOT="$PWD/$WORKTREES_ROOT"
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

repo_relative_path() {
  "$PYTHON_BIN" - "$1" "$REPO_DIR" "${2:-<external-path>}" <<'PY'
import os
import sys

candidate = os.path.realpath(sys.argv[1])
root = os.path.realpath(sys.argv[2])
external_label = sys.argv[3]
try:
    relative = os.path.relpath(candidate, root)
except ValueError:
    label = external_label
else:
    if relative == ".":
        label = "."
    elif relative == ".." or relative.startswith(".." + os.sep):
        label = external_label
    else:
        label = relative.replace(os.sep, "/")
sys.stdout.buffer.write(os.fsencode(label))
PY
}

SCRIPT_RELATIVE_PATH="$(repo_relative_path "$SCRIPT_PATH")"
PROTOTYPE_PATH_LABEL="$(repo_relative_path "$PROTOTYPE" "<external-input>")"
LIVE_INPUT_PATH_LABEL=""
if [[ -n "$LIVE_PNG" ]]; then
  LIVE_INPUT_PATH_LABEL="$(repo_relative_path "$LIVE_PNG" "<external-input>")"
fi
OUTPUT_PATH_LABEL="$(repo_relative_path "$OUTPUT_ROOT" "<external-output>")"

normalize_path_argument() {
  local value="$1"
  local external_label="${2:-<external-path>}"

  if ! NORMALIZED_PATH="$(repo_relative_path "$value" "$external_label")"; then
    die "could not normalize path argument"
  fi
}

recorded_invocation() {
  local argument
  local value_kind=""

  printf '%q' "$SCRIPT_RELATIVE_PATH"
  for argument in "$@"; do
    if [[ -n "$value_kind" ]]; then
      if [[ "$value_kind" == "provenance-note" ]]; then
        normalize_provenance_note "$argument"
        printf ' %q' "$NORMALIZED_NOTE"
      elif [[ "$value_kind" == "opaque" ]]; then
        normalize_opaque_argument "$argument"
        printf ' %q' "$NORMALIZED_ARGUMENT"
      elif [[ "$value_kind" == "launch-arg" ]]; then
        normalize_launch_argument "$argument"
        printf ' %q' "$NORMALIZED_ARGUMENT"
      elif [[ "$value_kind" == "raw" ]]; then
        printf ' %q' "$argument"
      else
        normalize_path_argument "$argument" "$value_kind"
        printf ' %q' "$NORMALIZED_PATH"
      fi
      value_kind=""
      continue
    fi
    # Non-path argv is emitted directly unless it is an opaque command or
    # launch argument. Those values are normalized as bytes below so a
    # disposable checkout/temp path inside a command cannot destabilize the
    # manifest; shell syntax and all other bytes remain intact.
    printf ' %q' "$argument"
    case "$argument" in
      --prototype|--egui-binary|--ios-app)
        value_kind="<external-input>"
        ;;
      --live-png)
        value_kind="<external-input>"
        ;;
      --output-root)
        value_kind="<external-output>"
        ;;
      --egui-wake-command|--ios-command)
        value_kind="opaque"
        ;;
      --ios-launch-arg)
        value_kind="launch-arg"
        ;;
      --issue|--surface|--live-agent|--host-url|--delay-ms|\
      --timeout-seconds|--chrome-timeout-seconds|--ios-bundle-id|\
      --ios-mode|--ios-delay-seconds)
        value_kind="raw"
        ;;
      --provenance-note)
        value_kind="provenance-note"
        ;;
    esac
  done
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
bit_depth = color_type = None
interlace_method = None
idat = []
seen_ihdr = False
seen_iend = False
seen_plte = False
seen_idat = False
idat_closed = False
seen_singletons = set()
known_critical = {b"IHDR", b"PLTE", b"IDAT", b"IEND"}
singleton_ancillary = {
    b"cHRM",
    b"gAMA",
    b"iCCP",
    b"sRGB",
    b"sBIT",
    b"tRNS",
    b"bKGD",
    b"hIST",
    b"pHYs",
    b"eXIf",
    b"oFFs",
    b"pCAL",
    b"sCAL",
    b"sTER",
    b"tIME",
}
before_plte = {b"cHRM", b"gAMA", b"iCCP", b"sRGB", b"sBIT"}
before_idat = before_plte | {b"pHYs", b"eXIf", b"oFFs", b"pCAL", b"sCAL", b"sTER"}
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
    if len(chunk_type) != 4 or any(
        not (65 <= value <= 90 or 97 <= value <= 122) for value in chunk_type
    ):
        raise SystemExit(f"{path} has an invalid PNG chunk type")
    if chunk_type[0] <= 90 and chunk_type not in known_critical:
        raise SystemExit(f"{path} has an unknown critical chunk: {chunk_type.decode('ascii')}")
    if not seen_ihdr and chunk_type != b"IHDR":
        raise SystemExit(f"{path} does not start with IHDR")
    if chunk_type in singleton_ancillary:
        if chunk_type in seen_singletons:
            raise SystemExit(f"{path} has a duplicate {chunk_type.decode('ascii')} chunk")
        seen_singletons.add(chunk_type)
    if chunk_type in before_plte and (seen_plte or seen_idat):
        raise SystemExit(f"{path} has {chunk_type.decode('ascii')} after PLTE or IDAT")
    if chunk_type in before_idat and seen_idat:
        raise SystemExit(f"{path} has {chunk_type.decode('ascii')} after IDAT")
    if chunk_type == b"IDAT" and idat_closed:
        raise SystemExit(f"{path} has non-consecutive IDAT chunks")
    if chunk_type != b"IDAT" and seen_idat:
        idat_closed = True
    if chunk_type == b"IHDR":
        if seen_ihdr or length != 13:
            raise SystemExit(f"{path} has an invalid IHDR")
        (
            width,
            height,
            bit_depth,
            color_type,
            compression_method,
            filter_method,
            interlace_method,
        ) = struct.unpack(">IIBBBBB", payload)
        if width == 0 or height == 0:
            raise SystemExit(f"{path} has an empty dimension")
        channel_counts = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}
        valid_depths = {
            0: {1, 2, 4, 8, 16},
            2: {8, 16},
            3: {1, 2, 4, 8},
            4: {8, 16},
            6: {8, 16},
        }
        if color_type not in channel_counts or bit_depth not in valid_depths.get(
            color_type, set()
        ):
            raise SystemExit(f"{path} has an invalid bit-depth/color-type pair")
        if compression_method != 0 or filter_method != 0:
            raise SystemExit(f"{path} has unsupported PNG compression or filter method")
        if interlace_method not in (0, 1):
            raise SystemExit(f"{path} has an invalid interlace method")
        seen_ihdr = True
    elif chunk_type == b"IDAT":
        if color_type == 3 and not seen_plte:
            raise SystemExit(f"{path} has indexed raster data before its PLTE chunk")
        idat.append(payload)
        seen_idat = True
    elif chunk_type == b"PLTE":
        if (
            seen_plte
            or seen_idat
            or color_type in (0, 4)
            or length == 0
            or length % 3 != 0
            or length > 768
        ):
            raise SystemExit(f"{path} has an invalid PLTE chunk")
        palette_entries = length // 3
        if color_type == 3 and palette_entries > (1 << bit_depth):
            raise SystemExit(f"{path} has too many PLTE entries for its bit depth")
        seen_plte = True
    elif chunk_type == b"tRNS":
        if color_type in (4, 6) or seen_idat:
            raise SystemExit(f"{path} has an invalid tRNS chunk order or color type")
        if color_type == 0 and length != 2:
            raise SystemExit(f"{path} has an invalid grayscale tRNS chunk")
        if color_type == 2 and length != 6:
            raise SystemExit(f"{path} has an invalid truecolor tRNS chunk")
        if color_type == 3:
            if not seen_plte:
                raise SystemExit(f"{path} has indexed tRNS data before its PLTE chunk")
            if length == 0 or length > palette_entries:
                raise SystemExit(f"{path} has an invalid indexed tRNS chunk")
    elif chunk_type == b"bKGD":
        if seen_idat or (color_type == 3 and not seen_plte):
            raise SystemExit(f"{path} has bKGD before PLTE or after IDAT")
    elif chunk_type == b"hIST":
        if color_type != 3 or not seen_plte or seen_idat:
            raise SystemExit(f"{path} has hIST without a preceding indexed PLTE")
    elif chunk_type == b"IEND":
        if length != 0:
            raise SystemExit(f"{path} has a non-empty IEND")
        seen_iend = True
        offset = crc_end
        break
    offset = crc_end

if (
    not seen_ihdr
    or width is None
    or height is None
    or bit_depth is None
    or color_type is None
    or interlace_method is None
):
    raise SystemExit(f"{path} has no IHDR")
if not seen_iend:
    raise SystemExit(f"{path} is missing IEND")
if offset != len(data):
    raise SystemExit(f"{path} has data after IEND")
if not idat:
    raise SystemExit(f"{path} has no IDAT data")
if color_type == 3 and not seen_plte:
    raise SystemExit(f"{path} has indexed raster data without a PLTE chunk")
decoder = zlib.decompressobj()
try:
    raw = decoder.decompress(b"".join(idat))
    raw += decoder.flush()
except zlib.error as error:
    raise SystemExit(f"{path} has incomplete IDAT data: {error}")
if not decoder.eof or decoder.unused_data:
    raise SystemExit(f"{path} has incomplete or trailing compressed IDAT data")

bits_per_pixel = channel_counts[color_type] * bit_depth


def pass_size(pass_width, pass_height):
    if pass_width == 0 or pass_height == 0:
        return 0
    row_bytes = (pass_width * bits_per_pixel + 7) // 8
    return (row_bytes + 1) * pass_height


if interlace_method == 0:
    expected_raw_bytes = pass_size(width, height)
else:
    expected_raw_bytes = 0
    for start_x, start_y, step_x, step_y in (
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ):
        pass_width = max(0, (width - start_x + step_x - 1) // step_x)
        pass_height = max(0, (height - start_y + step_y - 1) // step_y)
        expected_raw_bytes += pass_size(pass_width, pass_height)
if len(raw) != expected_raw_bytes:
    raise SystemExit(
        f"{path} has {len(raw)} decompressed raster bytes; "
        f"expected {expected_raw_bytes} for {width}x{height}"
    )


def paeth(left, above, upper_left):
    estimate = left + above - upper_left
    left_distance = abs(estimate - left)
    above_distance = abs(estimate - above)
    upper_left_distance = abs(estimate - upper_left)
    if left_distance <= above_distance and left_distance <= upper_left_distance:
        return left
    if above_distance <= upper_left_distance:
        return above
    return upper_left


def validate_filters(pass_width, pass_height, offset):
    if pass_width == 0 or pass_height == 0:
        return offset
    row_bytes = (pass_width * bits_per_pixel + 7) // 8
    filter_bytes_per_pixel = max(1, (bits_per_pixel + 7) // 8)
    previous = bytearray(row_bytes)
    for _ in range(pass_height):
        filter_type = raw[offset]
        if filter_type > 4:
            raise SystemExit(f"{path} has an invalid scanline filter: {filter_type}")
        source = raw[offset + 1 : offset + 1 + row_bytes]
        reconstructed = bytearray(row_bytes)
        for index, value in enumerate(source):
            left = reconstructed[index - filter_bytes_per_pixel] if index >= filter_bytes_per_pixel else 0
            above = previous[index]
            upper_left = (
                previous[index - filter_bytes_per_pixel]
                if index >= filter_bytes_per_pixel
                else 0
            )
            if filter_type == 0:
                reconstructed[index] = value
            elif filter_type == 1:
                reconstructed[index] = (value + left) & 0xFF
            elif filter_type == 2:
                reconstructed[index] = (value + above) & 0xFF
            elif filter_type == 3:
                reconstructed[index] = (value + ((left + above) // 2)) & 0xFF
            else:
                reconstructed[index] = (value + paeth(left, above, upper_left)) & 0xFF
        if color_type == 3:
            if bit_depth == 8:
                palette_indices = reconstructed
            else:
                palette_indices = (
                    (reconstructed[index // 8] >> (8 - bit_depth - (index % 8)))
                    & ((1 << bit_depth) - 1)
                    for index in range(0, pass_width * bit_depth, bit_depth)
                )
            if any(index >= palette_entries for index in palette_indices):
                raise SystemExit(f"{path} has a pixel outside its PLTE entries")
        previous = reconstructed
        offset += row_bytes + 1
    return offset


if interlace_method == 0:
    validated_bytes = validate_filters(width, height, 0)
else:
    validated_bytes = 0
    for start_x, start_y, step_x, step_y in (
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ):
        pass_width = max(0, (width - start_x + step_x - 1) // step_x)
        pass_height = max(0, (height - start_y + step_y - 1) // step_y)
        validated_bytes = validate_filters(pass_width, pass_height, validated_bytes)
if validated_bytes != len(raw):
    raise SystemExit(f"{path} has an invalid scanline layout")
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

GENERATOR_PATH="$SCRIPT_RELATIVE_PATH"
GENERATOR_SHA="$(sha256_file "$SCRIPT_PATH")"

run_byte_normalizer() {
  "$PYTHON_BIN" - "$@" <<'PY'
import os
from pathlib import Path
import re
import sys

mode = sys.argv[1]
source = sys.argv[2]
if mode == "capture":
    path = Path(source)
    data = path.read_bytes()
elif mode in ("note", "opaque"):
    data = os.fsencode(source)
else:
    raise SystemExit(f"unknown byte normalizer mode: {mode}")

repo = sys.argv[3]
repo_label = sys.argv[4]
worktrees_root = sys.argv[5]
worktrees_label = sys.argv[6]


def path_boundary(prefix):
    if prefix:
        # A known root may be followed by any non-path punctuation, but a
        # dotted/dashed component followed by a path boundary is a sibling
        # lookalike rather than the root itself.
        return (
            rb"(?=$|/|"
            rb"(?!(?:[.-][A-Za-z0-9_.-]+)(?=$|/|[^A-Za-z0-9_/]))"
            rb"[^A-Za-z0-9_/])"
        )
    # Do not treat a .bak sibling or a suffixed name as the path itself.
    return rb"(?=$|[/\x00\r\n\t \"'`:,;)\]}])"


def path_variants(path_value):
    variants = []
    for candidate in (path_value, os.path.abspath(path_value), os.path.realpath(path_value)):
        if candidate and candidate not in variants:
            variants.append(candidate)
    return variants


def replace_path(data_value, path_value, label, prefix=False):
    if not path_value:
        return data_value
    for variant in sorted(path_variants(path_value), key=len, reverse=True):
        if variant == os.path.sep:
            continue
        pattern = (
            rb"(?<![A-Za-z0-9_])"
            + re.escape(os.fsencode(variant))
            + path_boundary(prefix)
        )
        data_value = re.sub(pattern, os.fsencode(label), data_value)
    return data_value


def is_word_byte(value):
    return (
        48 <= value <= 57
        or 65 <= value <= 90
        or 97 <= value <= 122
        or value == 95
    )


def is_whitespace_byte(value):
    return value in (9, 11, 12, 32)


def terminal_path_end(data_value, path_start, component_start, line_end):
    # A quote immediately before the root unambiguously bounds the complete
    # terminal component, including spaces in a quoted worktree name.
    if path_start > 0 and data_value[path_start - 1] in (34, 39):
        quote = data_value[path_start - 1]
        index = path_start
        while index < line_end:
            if data_value[index] == quote:
                backslashes = 0
                cursor = index - 1
                while cursor >= path_start and data_value[cursor] == 92:
                    backslashes += 1
                    cursor -= 1
                if backslashes % 2 == 0:
                    return index
            index += 1

    end = line_end
    # Punctuation followed by whitespace (or a closing quote) is a diagnostic
    # separator, while punctuation embedded in an unquoted path is retained.
    for index in range(component_start, line_end):
        if data_value[index] not in (41, 44, 59, 58, 93, 125):
            continue
        next_value = data_value[index + 1] if index + 1 < line_end else None
        if next_value is None or is_whitespace_byte(next_value) or next_value in (34, 39):
            end = min(end, index)

    # Unquoted terminal paths with spaces are inherently ambiguous. Consume
    # the full terminal component; callers that need arbitrary diagnostic text
    # to follow a path must quote it or use punctuation as an explicit bound.
    return end


def redact_configured_worktree(data_value):
    if not worktrees_root:
        return data_value
    replacements = []
    for root in path_variants(worktrees_root):
        root_bytes = os.fsencode(root)
        if root == os.path.sep:
            continue
        search = 0
        while True:
            start = data_value.find(root_bytes, search)
            if start == -1:
                break
            root_end = start + len(root_bytes)
            if (
                (start == 0 or not is_word_byte(data_value[start - 1]))
                and root_end < len(data_value)
                and data_value[root_end] == 47
            ):
                line_end = len(data_value)
                for delimiter in (b"\x00", b"\r", b"\n"):
                    delimiter_end = data_value.find(delimiter, root_end)
                    if delimiter_end != -1 and delimiter_end < line_end:
                        line_end = delimiter_end
                first_slash = data_value.find(b"/", root_end + 1, line_end)
                if first_slash > root_end + 1:
                    second_slash = data_value.find(
                        b"/", first_slash + 1, line_end
                    )
                    if second_slash == -1:
                        second_slash = terminal_path_end(
                            data_value, start, first_slash + 1, line_end
                        )
                    if second_slash > first_slash + 1:
                        replacements.append((start, second_slash))
            search = root_end
    if not replacements:
        return data_value
    output = bytearray()
    cursor = 0
    for start, end in sorted(replacements):
        if start < cursor:
            continue
        output.extend(data_value[cursor:start])
        output.extend(b"<herdr-worktree>")
        cursor = end
    output.extend(data_value[cursor:])
    return bytes(output)


def is_token_boundary_byte(value):
    # Whitespace and assignment separators start a new diagnostic token. A
    # slash preceded by a component character remains part of that path,
    # including components whose names contain spaces.
    return value in (9, 32, 61)


def valid_path_component(component):
    return not any(delimiter in component for delimiter in (b"\x00", b"\r", b"\n"))


def replace_generic_worktrees(data_value, protected_paths=()):
    # Search for the fixed marker and find the eligible path start in the same
    # forward pass. A slash after a token separator starts a new candidate,
    # while a slash inside a space-containing path keeps its current candidate.
    # This avoids a backtracking expression over every slash in a long line.
    marker = b"/.herdr/worktrees/"
    protected_variants = [
        os.fsencode(variant)
        for path_value in protected_paths
        for variant in path_variants(path_value)
        if variant != os.path.sep
    ]
    replacements = []
    eligible_start = None
    index = 0
    while index < len(data_value):
        value = data_value[index]
        if value in (0, 10, 13):
            eligible_start = None
        elif value == 47:
            if index == 0 or not is_word_byte(data_value[index - 1]):
                if eligible_start is None or is_token_boundary_byte(
                    data_value[index - 1]
                ):
                    eligible_start = index
            if eligible_start is not None and data_value.startswith(marker, index):
                cursor = index + len(marker)
                line_end = len(data_value)
                for delimiter in (b"\x00", b"\r", b"\n"):
                    delimiter_end = data_value.find(delimiter, cursor)
                    if delimiter_end != -1 and delimiter_end < line_end:
                        line_end = delimiter_end
                first_slash = data_value.find(b"/", cursor, line_end)
                if first_slash > cursor and valid_path_component(
                    data_value[cursor:first_slash]
                ):
                    second_slash = data_value.find(
                        b"/", first_slash + 1, line_end
                    )
                    if second_slash == -1:
                        second_slash = terminal_path_end(
                            data_value, eligible_start, first_slash + 1, line_end
                        )
                    if second_slash > first_slash + 1 and valid_path_component(
                        data_value[first_slash + 1 : second_slash]
                    ):
                        start = eligible_start
                        end = second_slash
                        candidate = data_value[start:end]
                        is_known_sibling = any(
                            candidate.startswith(variant)
                            and candidate[len(variant) :].startswith((b".", b"-"))
                            for variant in protected_variants
                        )
                        if is_known_sibling:
                            eligible_start = None
                        elif not replacements or start >= replacements[-1][1]:
                            replacements.append((start, end))
                            eligible_start = None
                            index = end
        index += 1

    if not replacements:
        return data_value
    output = bytearray()
    cursor = 0
    for start, end in replacements:
        output.extend(data_value[cursor:start])
        output.extend(b"<herdr-worktree>")
        cursor = end
    output.extend(data_value[cursor:])
    return bytes(output)


def redact_ephemeral_paths(data_value):
    if mode not in ("note", "opaque"):
        return data_value
    roots = [
        "/tmp",
        "/private/tmp",
        "/var/folders",
        "/private/var/folders",
        os.environ.get("TMPDIR", "").rstrip("/")
    ]
    for root in sorted({value for value in roots if value}, key=len, reverse=True):
        pattern = (
            rb"(?<![A-Za-z0-9_])"
            + re.escape(os.fsencode(root))
            + rb"/[^\x00\r\n\t \"'`:,;&|()<>]+"
        )
        data_value = re.sub(pattern, b"<external-temp>", data_value)
    return data_value


# Longest specific paths go first so a concrete file/path wins before its
# repository root and before generic Herdr redaction. This keeps a checkout
# inside a Herdr worktree repo-relative instead of turning it into a generic
# placeholder.
known_paths = [
    (sys.argv[7], sys.argv[8], False),
    (sys.argv[9], sys.argv[10], False),
    (sys.argv[11], sys.argv[12], False),
    (sys.argv[13], sys.argv[14], True),
    (sys.argv[15], sys.argv[16], True),
    (repo, repo_label, True),
]
for path_value, label, prefix in sorted(
    known_paths, key=lambda item: len(os.fsencode(item[0])), reverse=True
):
    data = replace_path(data, path_value, label, prefix)

# Redact configured and generic Herdr paths only after known checkout/staging
# paths have had the opportunity to establish stable identities. A capture or
# note may mention only the configured root, without a repo/worktree suffix.
data = redact_configured_worktree(data)
data = replace_path(data, worktrees_root, worktrees_label, True)
data = replace_generic_worktrees(
    data, [path_value for path_value, _, _ in known_paths]
)
data = redact_ephemeral_paths(data)

if mode == "capture":
    max_bytes = int(sys.argv[17])
    head_bytes = int(sys.argv[18])
    tail_bytes = int(sys.argv[19])
if mode == "capture" and len(data) > max_bytes:
    head_size = min(head_bytes, len(data))
    tail_size = min(tail_bytes, len(data) - head_size)
    while True:
        omitted = len(data) - head_size - tail_size
        marker = (
            f"\n[... capture log truncated: omitted {omitted} bytes; "
            "exact head and tail bytes retained ...]\n"
        ).encode("ascii")
        available_tail = max_bytes - head_size - len(marker)
        if available_tail < 0:
            raise SystemExit("capture log bound is too small for its marker")
        if tail_size <= available_tail:
            break
        tail_size = available_tail
    tail = data[-tail_size:] if tail_size else b""
    data = data[:head_size] + marker + tail
    if len(data) > max_bytes:
        raise SystemExit("capture log exceeded its configured byte bound")

if mode == "capture":
    path.write_bytes(data)
else:
    sys.stdout.buffer.write(data)
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

normalize_capture_log() {
  run_byte_normalizer capture "$1" \
    "$REPO_DIR" "." \
    "$WORKTREES_ROOT" "<herdr-worktree>" \
    "$SCRIPT_PATH" "$SCRIPT_RELATIVE_PATH" \
    "$PROTOTYPE" "$PROTOTYPE_PATH_LABEL" \
    "$LIVE_PNG" "$LIVE_INPUT_PATH_LABEL" \
    "$OUTPUT_ROOT" "$OUTPUT_PATH_LABEL" \
    "$STAGE" "<stage>" \
    "$CAPTURE_LOG_MAX_BYTES" "$CAPTURE_LOG_HEAD_BYTES" \
    "$CAPTURE_LOG_TAIL_BYTES"
}

NORMALIZED_ARGUMENT=""
normalize_argument_bytes() {
  local mode="$1"
  local argument="$2"
  local normalized_file="$3"
  local label="$4"

  NORMALIZED_ARGUMENT=""
  if ! run_byte_normalizer "$mode" "$argument" \
    "$REPO_DIR" "." \
    "$WORKTREES_ROOT" "<herdr-worktree>" \
    "$SCRIPT_PATH" "$SCRIPT_RELATIVE_PATH" \
    "$PROTOTYPE" "$PROTOTYPE_PATH_LABEL" \
    "$LIVE_PNG" "$LIVE_INPUT_PATH_LABEL" \
    "$OUTPUT_ROOT" "$OUTPUT_PATH_LABEL" \
    "$STAGE" "<stage>" \
    0 0 0 >"$normalized_file"; then
    die "could not normalize $label"
  fi
  if ! IFS= read -r -d '' NORMALIZED_ARGUMENT < <(
    cat "$normalized_file"
    printf '\0'
  ); then
    die "could not read normalized $label"
  fi
}

normalize_provenance_note() {
  normalize_argument_bytes note "$1" \
    "$STAGE/.design-gate.note-normalized" "provenance note"
  NORMALIZED_NOTE="$NORMALIZED_ARGUMENT"
}

normalize_opaque_argument() {
  normalize_argument_bytes opaque "$1" \
    "$STAGE/.design-gate.opaque-normalized" "opaque invocation argument"
}

normalize_launch_argument() {
  local argument="$1"
  if [[ "$argument" == /* || "$argument" == ./* || "$argument" == ../* ]]; then
    if ! NORMALIZED_ARGUMENT="$(repo_relative_path "$argument" "<external-input>")"; then
      die "could not normalize launch argument"
    fi
  else
    normalize_opaque_argument "$argument"
  fi
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
import hashlib
import json
from pathlib import Path
import sys

source_path = Path(sys.argv[1]).resolve()
surface = sys.argv[2]
egui_tab = sys.argv[3]
output_path = Path(sys.argv[4])
source_bytes = source_path.read_bytes()
source = source_bytes.decode("utf-8")

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
print(f"{width} {height} {hashlib.sha256(source_bytes).hexdigest()}")
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
  local binary_path

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

  binary_path="$(repo_relative_path "$binary" "<external-input>")"
  CAPTURE_KIND="native egui viewport screenshot"
  LIVE_DESCRIPTION="real egui process launched from $binary_path against a loopback corrald; selected live agent $LIVE_AGENT from /snapshot"
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

publish_bundle() {
  local final_dir="$1"

  if [[ -e "$OUTPUT_DIR" || -L "$OUTPUT_DIR" ]]; then
    BACKUP_DIR="$(mktemp -d "$OUTPUT_ROOT/.design-gate.backup.XXXXXX")"
    rmdir -- "$BACKUP_DIR"
    if ! mv -f -- "$OUTPUT_DIR" "$BACKUP_DIR"; then
      rmdir -- "$BACKUP_DIR" 2>/dev/null || true
      BACKUP_DIR=""
      die "could not stage the existing evidence bundle for replacement"
    fi
  fi

  if [[ -e "$OUTPUT_DIR" || -L "$OUTPUT_DIR" ]]; then
    die "output path appeared during publication; the old evidence backup is retained"
  fi
  if ! mv -f -- "$final_dir" "$OUTPUT_DIR"; then
    die "could not publish the validated evidence bundle"
  fi
  for artifact in "${PUBLISHED_ARTIFACTS[@]}"; do
    [[ -s "$OUTPUT_DIR/$artifact" ]] \
      || die "output path changed during publication; the old evidence backup is retained"
  done
  FINAL_DIR=""
  PUBLISHED=1

  if [[ -n "$BACKUP_DIR" ]]; then
    if rm -rf -- "$BACKUP_DIR"; then
      BACKUP_DIR=""
    else
      warn "old evidence backup could not be removed: $BACKUP_DIR"
    fi
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
mkdir -p "$OUTPUT_ROOT" \
  || die "could not create output root: $OUTPUT_ROOT"
LOCK_DIR="$OUTPUT_ROOT/.design-gate.lock"
if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  die "could not acquire evidence publication lock: $LOCK_DIR (another run may be active)"
fi
STAGE=""
CAPTURE_PID=""
FINAL_DIR=""
BACKUP_DIR=""
PUBLISHED=0
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
  if [[ -n "${FINAL_DIR:-}" && -d "$FINAL_DIR" ]]; then
    rm -rf -- "$FINAL_DIR"
  fi
  if [[ -n "${BACKUP_DIR:-}" && -d "$BACKUP_DIR" ]]; then
    if [[ "$PUBLISHED" -eq 1 ]]; then
      if ! rm -rf -- "$BACKUP_DIR"; then
        cleanup_status=1
      else
        BACKUP_DIR=""
      fi
    elif [[ ! -e "$OUTPUT_DIR" && ! -L "$OUTPUT_DIR" ]]; then
      if ! mv -f -- "$BACKUP_DIR" "$OUTPUT_DIR"; then
        cleanup_status=1
      else
        BACKUP_DIR=""
      fi
    else
      warn "old evidence backup retained at $BACKUP_DIR because output path is occupied"
      cleanup_status=1
    fi
  fi
  if [[ -n "${LOCK_DIR:-}" && -d "$LOCK_DIR" ]]; then
    if ! rmdir -- "$LOCK_DIR"; then
      cleanup_status=1
    else
      LOCK_DIR=""
    fi
  fi
  return "$cleanup_status"
}
trap cleanup EXIT

if [[ -e "$OUTPUT_DIR" || -L "$OUTPUT_DIR" ]]; then
  [[ -d "$OUTPUT_DIR" ]] || die "output path exists but is not a directory: $OUTPUT_DIR"
  [[ "$FORCE" -eq 1 ]] \
    || die "evidence bundle already exists: $OUTPUT_DIR (pass --force to replace it)"
fi
STAGE="$(mktemp -d "$OUTPUT_ROOT/.design-gate.stage.XXXXXX")" \
  || die "could not create staging directory below $OUTPUT_ROOT"

PROTOTYPE_VIEW="$STAGE/prototype-view.html"
if ! prototype_size="$(make_prototype_view "$PROTOTYPE_VIEW")"; then
  die "could not prepare the prototype render; check the --prototype surface"
fi
IFS=' ' read -r PROTOTYPE_WIDTH PROTOTYPE_HEIGHT PROTOTYPE_SOURCE_SHA <<<"$prototype_size"
PROTOTYPE_SOURCE_SHA_CHECK="$(sha256_file "$PROTOTYPE")"
[[ "$PROTOTYPE_SOURCE_SHA" == "$PROTOTYPE_SOURCE_SHA_CHECK" ]] \
  || die "prototype changed while it was being prepared; refusing mismatched evidence"
run_chrome_screenshot "$PROTOTYPE_VIEW" "$STAGE/prototype.png" \
  "$PROTOTYPE_WIDTH" "$PROTOTYPE_HEIGHT" prototype

CAPTURE_KIND=""
LIVE_DESCRIPTION=""
CAPTURE_COMMAND=""
if [[ -n "$LIVE_PNG" ]]; then
  CAPTURE_KIND="explicit supplied PNG fixture"
  LIVE_DESCRIPTION="caller-supplied file; this run did not capture a live surface"
  CAPTURE_COMMAND="cp $LIVE_INPUT_PATH_LABEL <issue-dir>/live-after.png"
  cp -- "$LIVE_PNG" "$STAGE/live-after.png"
  LIVE_SOURCE_SHA="$(sha256_file "$STAGE/live-after.png")"
  printf 'supplied fixture: %s\n' "$LIVE_INPUT_PATH_LABEL" >"$STAGE/capture.log"
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

PROTOTYPE_SHA="$(sha256_file "$STAGE/prototype.png")"
LIVE_SHA="$(sha256_file "$STAGE/live-after.png")"
if [[ -n "$LIVE_PNG" ]]; then
  LIVE_SOURCE_PATH="$LIVE_INPUT_PATH_LABEL"
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
if [[ "$CHROME_BIN_EXPLICIT" -eq 1 ]]; then
  RENDERER_GUIDANCE='`CHROME_BIN` was explicitly set for this capture; use a complete GUI-capable Chrome/Chromium when the default renderer cannot complete.'
else
  RENDERER_GUIDANCE=""
fi
PROVENANCE_NOTE_DISPLAY=""
if [[ -n "$PROVENANCE_NOTE" ]]; then
  normalize_provenance_note "$PROVENANCE_NOTE"
  PROVENANCE_NOTE_DISPLAY="$NORMALIZED_NOTE"
fi
# Markdown backticks are literal printf text, not shell command substitutions.
# shellcheck disable=SC2016
{
  printf '# Issue #%s design-gate evidence\n\n' "$ISSUE"
  printf '## Contract\n\n'
  printf -- '- Newly generated bundle contract: this manifest is byte-stable for identical semantic inputs; wall-clock generation time is intentionally omitted.\n'
  printf -- '- Newly generated capture-log contract: `capture.log` is byte-bounded to %s bytes, retains exact head/tail bytes after documented path substitutions, and never decodes invalid UTF-8.\n' "$CAPTURE_LOG_MAX_BYTES"
  printf -- '- Provenance-note contract: arbitrary note bytes are preserved except for targeted substitutions of recognized repository, staging, and worktree roots.\n'
  printf -- '- This contract applies to bundles generated by this version; historical checked-in evidence may be a separately labeled sanitized summary.\n'
  printf '\n## Capture\n\n'
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
  if [[ -n "$PROVENANCE_NOTE_DISPLAY" ]]; then
    printf -- '- Operator/environment note: %s\n' "$PROVENANCE_NOTE_DISPLAY"
  fi
  if [[ -n "$RENDERER_GUIDANCE" ]]; then
    printf -- '- Renderer guidance: %s\n' "$RENDERER_GUIDANCE"
  fi
  printf '\n## Sources\n\n'
  printf -- '- Prototype source: `%s`\n' "$PROTOTYPE_PATH_LABEL"
  printf -- '- Prototype source SHA-256: `%s`\n' "$PROTOTYPE_SOURCE_SHA"
  printf -- '- Generator script (canonical `BASH_SOURCE[0]`): `%s`\n' "$GENERATOR_PATH"
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
  printf -- '- Repository HEAD: `%s`\n' "$GIT_SHA"
  printf -- '- Reproducible invocation: `'
  recorded_invocation "${ORIGINAL_ARGS[@]}"
  printf '`\n'
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

FINAL_DIR="$(mktemp -d "$OUTPUT_ROOT/.design-gate.final.XXXXXX")"
for artifact in "${PUBLISHED_ARTIFACTS[@]}"; do
  mv -f -- "$STAGE/$artifact" "$FINAL_DIR/$artifact"
done
rm -rf -- "$STAGE"
STAGE=""
publish_bundle "$FINAL_DIR"

log "wrote $OUTPUT_DIR"
log "prototype: $OUTPUT_DIR/prototype.png ($PROTOTYPE_DIMS)"
if [[ "$IOS_BEFORE_DIMS" != "not applicable" ]]; then
  log "before: $OUTPUT_DIR/ios-before-detail.png ($IOS_BEFORE_DIMS)"
fi
log "live: $OUTPUT_DIR/live-after.png ($LIVE_DIMS)"
log "comparison: $OUTPUT_DIR/comparison.png ($COMPARISON_DIMS)"
