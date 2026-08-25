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
#   docs/design/evidence/issue-<N>/live-after.png
#   docs/design/evidence/issue-<N>/comparison.png
#   docs/design/evidence/issue-<N>/conformance.md
#   docs/design/evidence/issue-<N>/capture.log
#
# The default egui prototype is the approved HTML design source at
# docs/design/corral-ux-prototype.html. Its desktop .desk surface is rendered
# through headless Chrome at 1160×631. A custom HTML target can be supplied with
# --prototype; egui targets must contain .desk and iOS targets must contain
# .phone. The source is wrapped without changing the checked-in prototype.
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
# a stale frame.
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
# --live-png is an explicit fixture seam for tests or a previously captured
# frame. Its provenance says that the PNG was supplied rather than captured by
# this run; it never silently becomes live evidence. --dry-run validates the
# interface and prints the planned capture without writing an evidence bundle.
# Re-runs are safe: all work is staged in a private temporary directory below
# the target issue directory, existing evidence is untouched on failure, and
# the validated files are replaced at the end using atomic file renames.
#
# Dependencies: Bash 3+, Python 3, headless-capable Chrome/Chromium, and (for
# native captures) curl/cargo or hermes-sim-task as described above. Set
# CHROME_BIN or PYTHON_BIN to override discovery.

set -euo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SCRIPT_NAME="$(basename "$0")"
ORIGINAL_ARGS=("$@")

ISSUE=""
SURFACE=""
PROTOTYPE=""
LIVE_PNG=""
LIVE_AGENT=""
HOST_URL="${CORRAL_UI_HOST_URL:-http://127.0.0.1:8474}"
EGUI_BINARY=""
EGUI_DELAY_MS="8000"
EGUI_WAKE_COMMAND="${CORRAL_EGUI_WAKE_COMMAND:-}"
CAPTURE_TIMEOUT_SECONDS="90"
CHROME_TIMEOUT_SECONDS="30"
BUILD_EGUI=1
BUILD_IOS=1
IOS_APP=""
IOS_BUNDLE_ID="com.corral.fleetnotifier"
IOS_MODE="live"
IOS_DELAY_SECONDS="4"
IOS_COMMAND=""
IOS_LAUNCH_ARGS=()
PROVENANCE_NOTE=""
OUTPUT_ROOT="$REPO_DIR/docs/design/evidence"
DRY_RUN=0

usage() {
  sed -n '1,82p' "$SCRIPT_DIR/$SCRIPT_NAME"
  cat <<'USAGE'

Options:
  --issue N                  Target issue number (required).
  --surface egui|ios         Surface to capture (required).
  --prototype PATH           Approved HTML prototype override.
  --live-agent ID             egui agent id, checked against /snapshot.
  --live-png PATH             Explicit supplied PNG fixture seam.
  --host-url URL              egui health endpoint (default: 127.0.0.1:8474).
  --egui-binary PATH          corrald-ui binary override.
  --delay-ms N                egui screenshot delay (default: 8000).
  --egui-wake-command SHELL   Explicit eframe wake/input command.
  --timeout-seconds N         Native capture timeout (default: 90).
  --chrome-timeout-seconds N  Headless Chrome timeout (default: 30).
  --ios-app PATH              Prebuilt .app; otherwise build through Herdr.
  --ios-bundle-id ID         Bundle id (default: com.corral.fleetnotifier).
  --ios-mode live|demo        Live requires --ios-command; demo is explicit.
  --ios-command SHELL         Prepare/launch live app inside hermes-sim-task.
  --ios-launch-arg ARG        Repeatable simulator launch argument.
  --ios-delay-seconds N       Wait before simulator screenshot (default: 4).
  --provenance-note TEXT      Extra operator/environment note in provenance.
  --output-root PATH          Evidence root override (test seam).
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
    PROTOTYPE="$REPO_DIR/docs/design/corral-ux-prototype.html"
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

PYTHON_BIN="${PYTHON_BIN:-}"
if [[ -z "$PYTHON_BIN" ]]; then
  PYTHON_BIN="$(command -v python3 || true)"
fi
[[ -n "$PYTHON_BIN" && -x "$PYTHON_BIN" ]] \
  || die "Python 3 is required; set PYTHON_BIN to an executable Python 3"

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

path = Path(sys.argv[1])
data = path.read_bytes()
if data[:8] != b"\x89PNG\r\n\x1a\n":
    raise SystemExit(f"{path} is not a PNG")
if len(data) < 24:
    raise SystemExit(f"{path} is truncated")
width, height = struct.unpack(">II", data[16:24])
if width == 0 or height == 0:
    raise SystemExit(f"{path} has an empty dimension")
print(f"{width}x{height}")
PY
}

assert_png() {
  local path="$1"
  [[ -s "$path" ]] || die "capture did not produce a non-empty PNG: $path"
  png_dimensions "$path" >/dev/null \
    || die "capture is not a valid PNG: $path"
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

make_prototype_view() {
  local output="$1"
  "$PYTHON_BIN" - "$PROTOTYPE" "$SURFACE" "$output" <<'PY'
from html import escape
from pathlib import Path
import sys

source_path = Path(sys.argv[1]).resolve()
surface = sys.argv[2]
output_path = Path(sys.argv[3])
source = source_path.read_text(encoding="utf-8")

if "</head>" not in source.lower():
    raise SystemExit(f"prototype has no </head> element: {source_path}")

if surface == "egui":
    if ".desk" not in source:
        raise SystemExit(
            f"egui prototype must contain a .desk surface: {source_path}; "
            "pass the approved egui HTML with --prototype"
        )
    style = """
html, body { width: 1160px !important; height: 631px !important; overflow: hidden !important; }
body { padding: 8px !important; }
body > h1, body > .sub { display: none !important; }
body > .rack { display: block !important; width: 1080px !important; }
body > .rack > .frame { display: none !important; }
body > .rack > .frame:has(.desk) {
  display: block !important;
  width: 1080px !important;
  max-width: 1080px !important;
  margin: 0 !important;
}
"""
    width, height = 1160, 631
elif surface == "ios":
    if ".phone" not in source:
        raise SystemExit(
            f"iOS prototype must contain a .phone surface: {source_path}; "
            "pass the approved iOS HTML with --prototype"
        )
    style = """
html, body { width: 900px !important; height: 820px !important; overflow: hidden !important; }
body { padding: 8px !important; }
body > h1, body > .sub { display: none !important; }
body > .rack { display: flex !important; flex-wrap: nowrap !important; width: 840px !important; gap: 28px !important; }
body > .rack > .frame:has(.desk) { display: none !important; }
body > .rack > .frame { flex: 0 0 auto !important; }
"""
    width, height = 900, 820
else:
    raise SystemExit(f"unsupported surface: {surface}")

base_href = escape(source_path.parent.as_uri() + "/", quote=True)
injection = (
    f'<base href="{base_href}">\n'
    '<meta name="design-gate-render" content="generated without editing the source">\n'
    f'<style id="design-gate-surface">{style}</style>\n'
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
  local deadline
  local dimensions

  mkdir -p "$profile"
  url="$(file_url "$html_path")"
  log "rendering $label with headless Chrome at ${width}x${height}"
  "$CHROME_BIN" \
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
    --window-size="$width,$height" \
    --user-data-dir="$profile" \
    --no-first-run \
    --no-default-browser-check \
    --virtual-time-budget=1000 \
    --screenshot="$output_path" \
    "$url" >"$chrome_log" 2>&1 &
  chrome_pid=$!
  deadline=$((SECONDS + 10#$CHROME_TIMEOUT_SECONDS))
  while [[ ! -s "$output_path" && $SECONDS -lt $deadline ]]; do
    if ! kill -0 "$chrome_pid" 2>/dev/null; then
      break
    fi
    sleep 1
  done
  if [[ ! -s "$output_path" ]]; then
    kill "$chrome_pid" 2>/dev/null || true
    wait "$chrome_pid" 2>/dev/null || true
    tail -40 "$chrome_log" >&2 || true
    die "headless Chrome did not produce $label within ${CHROME_TIMEOUT_SECONDS}s"
  fi
  if kill -0 "$chrome_pid" 2>/dev/null; then
    kill "$chrome_pid" 2>/dev/null || true
  fi
  wait "$chrome_pid" 2>/dev/null || true
  assert_png "$output_path"
  dimensions="$(png_dimensions "$output_path")"
  [[ "$dimensions" == "${width}x${height}" ]] \
    || die "$label PNG dimensions are $dimensions; expected ${width}x${height}"
}

make_composite_html() {
  local output="$1"
  "$PYTHON_BIN" - "$STAGE/prototype.png" "$STAGE/live-after.png" "$output" "$ISSUE" "$SURFACE" <<'PY'
from html import escape
from pathlib import Path
import sys

prototype = Path(sys.argv[1]).resolve().as_uri()
live = Path(sys.argv[2]).resolve().as_uri()
output = Path(sys.argv[3])
issue = escape(sys.argv[4])
surface = escape(sys.argv[5])

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
      <article class="panel"><div class="label">Live board · captured surface</div><div class="viewport"><img src="{live}" alt="Live board screenshot"></div></article>
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
  local deadline
  local exit_status
  local binary

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

  CAPTURE_KIND="native egui viewport screenshot"
  LIVE_DESCRIPTION="real egui process launched from $binary against $HOST_URL"
  LIVE_DESCRIPTION+="; selected live agent $LIVE_AGENT from /snapshot"
  CAPTURE_COMMAND="CORRAL_UI_SCREENSHOT=<issue-dir>/live-after.png CORRAL_UI_SCREENSHOT_DELAY_MS=$EGUI_DELAY_MS"
  CAPTURE_COMMAND+=" CORRAL_UI_SCREENSHOT_AGENT=$LIVE_AGENT"
  CAPTURE_COMMAND+=" $binary"
  if [[ -n "$EGUI_WAKE_COMMAND" ]]; then
    CAPTURE_COMMAND+="; wake with caller command"
  fi

  log "capturing live egui board; health check passed"
  if [[ -n "$LIVE_AGENT" ]]; then
    log "selected target: $LIVE_AGENT"
  fi
  CORRAL_UI_SCREENSHOT_AGENT="$LIVE_AGENT" \
    CORRAL_UI_SCREENSHOT="$STAGE/live-after.png" \
    CORRAL_UI_SCREENSHOT_DELAY_MS="$EGUI_DELAY_MS" \
    RUST_LOG="${RUST_LOG:-info}" "$binary" >"$STAGE/capture.log" 2>&1 &
  ui_pid=$!
  CAPTURE_PID="$ui_pid"
  if [[ -n "$EGUI_WAKE_COMMAND" ]]; then
    log "running explicit egui wake command"
    sleep 1
    if ! CORRAL_UI_SCREENSHOT_PID="$ui_pid" \
      CORRAL_UI_SCREENSHOT_PATH="$STAGE/live-after.png" \
      bash -c "$EGUI_WAKE_COMMAND" >>"$STAGE/capture.log" 2>&1; then
      kill "$ui_pid" 2>/dev/null || true
      wait "$ui_pid" 2>/dev/null || true
      die "egui wake command failed; no live screenshot claim was made"
    fi
  fi
  deadline=$((SECONDS + 10#$CAPTURE_TIMEOUT_SECONDS))
  while [[ ! -s "$STAGE/live-after.png" && $SECONDS -lt $deadline ]]; do
    if ! kill -0 "$ui_pid" 2>/dev/null; then
      break
    fi
    sleep 1
  done
  if [[ ! -s "$STAGE/live-after.png" ]]; then
    kill "$ui_pid" 2>/dev/null || true
    wait "$ui_pid" 2>/dev/null || true
    tail -80 "$STAGE/capture.log" >&2 || true
    die "egui did not produce a screenshot within ${CAPTURE_TIMEOUT_SECONDS}s"
  fi
  if kill -0 "$ui_pid" 2>/dev/null; then
    kill "$ui_pid" 2>/dev/null || true
  fi
  set +e
  wait "$ui_pid"
  exit_status=$?
  set -e
  CAPTURE_PID=""
  if [[ "$exit_status" -ne 0 ]]; then
    warn "egui exited with status $exit_status after writing the screenshot; see capture.log"
  fi
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
  local command_text
  local launch_arg
  local launch_args_q=""
  local has_demo_mode=0

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

  if [[ -n "${IOS_LAUNCH_ARGS[*]-}" ]]; then
    for launch_arg in "${IOS_LAUNCH_ARGS[@]}"; do
      if [[ "$launch_arg" == "-demoMode" ]]; then
        has_demo_mode=1
      fi
    done
  fi
  if [[ "$IOS_MODE" == "demo" && "$has_demo_mode" -eq 0 ]]; then
    if [[ -n "${IOS_LAUNCH_ARGS[*]-}" ]]; then
      IOS_LAUNCH_ARGS=("-demoMode" "${IOS_LAUNCH_ARGS[@]}")
    else
      IOS_LAUNCH_ARGS=("-demoMode")
    fi
  fi
  if [[ -n "${IOS_LAUNCH_ARGS[*]-}" ]]; then
    for launch_arg in "${IOS_LAUNCH_ARGS[@]}"; do
      launch_args_q+=" $(shell_quote "$launch_arg")"
    done
  fi

  app_q="$(shell_quote "$app_path")"
  bundle_q="$(shell_quote "$IOS_BUNDLE_ID")"
  project_q="$(shell_quote "$REPO_DIR/ios/FleetNotifier.xcodeproj")"
  derived_q="$(shell_quote "$STAGE/ios-derived-data")"
  output_q="$(shell_quote "$STAGE/live-after.png")"
  command_text=$'set -euo pipefail\n'
  if [[ -n "$app_path" ]]; then
    command_text+="xcrun simctl install \"\$SIMULATOR_UDID\" $app_q"$'\n'
  else
    command_text+="xcodebuild -project $project_q -scheme FleetNotifier -configuration Debug -destination \"id=\$SIMULATOR_UDID\" -derivedDataPath $derived_q CODE_SIGNING_ALLOWED=NO build"$'\n'
    command_text+="app_path=$derived_q/Build/Products/Debug-iphonesimulator/FleetNotifier.app"$'\n'
    command_text+="xcrun simctl install \"\$SIMULATOR_UDID\" \"\$app_path\""$'\n'
  fi
  if [[ "$IOS_MODE" == "demo" ]]; then
    command_text+="xcrun simctl launch \"\$SIMULATOR_UDID\" $bundle_q$launch_args_q"$'\n'
  else
    command_text+="$IOS_COMMAND"$'\n'
  fi
  command_text+="sleep $(shell_quote "$IOS_DELAY_SECONDS")"$'\n'
  command_text+="xcrun simctl io \"\$SIMULATOR_UDID\" screenshot $output_q"$'\n'

  CAPTURE_KIND="iOS simulator screenshot via hermes-sim-task"
  if [[ "$IOS_MODE" == "demo" ]]; then
    LIVE_DESCRIPTION="explicit Debug -demoMode simulator fixture; not live-daemon evidence"
  else
    LIVE_DESCRIPTION="caller-prepared iOS live app in a private Herdr-owned simulator"
  fi
  CAPTURE_COMMAND="hermes-sim-task --shell <install/build, prepare, launch, and screenshot command>"
  log "capturing iOS surface through hermes-sim-task ($IOS_MODE mode)"
  if ! hermes-sim-task --shell "$command_text" >"$STAGE/capture.log" 2>&1; then
    tail -120 "$STAGE/capture.log" >&2 || true
    die "hermes-sim-task iOS capture failed; the private simulator was owned by the wrapper"
  fi
  assert_png "$STAGE/live-after.png"
}

print_dry_run() {
  local output_dir="$OUTPUT_ROOT/issue-$ISSUE"
  log "dry run"
  log "surface: $SURFACE"
  log "prototype: $PROTOTYPE"
  log "output: $output_dir/{prototype.png,live-after.png,comparison.png,conformance.md,capture.log}"
  log "Chrome: $CHROME_BIN"
  if [[ -n "$LIVE_PNG" ]]; then
    log "live source: explicit supplied PNG fixture $LIVE_PNG"
  elif [[ "$SURFACE" == "egui" ]]; then
    log "live source: native corrald-ui capture after health check at $HOST_URL"
  else
    log "live source: hermes-sim-task iOS capture in $IOS_MODE mode"
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
}

if [[ "$DRY_RUN" -eq 1 ]]; then
  print_dry_run
  exit 0
fi

mkdir -p "$OUTPUT_ROOT"
OUTPUT_DIR="$OUTPUT_ROOT/issue-$ISSUE"
mkdir -p "$OUTPUT_DIR"
STAGE="$(mktemp -d "$OUTPUT_DIR/.design-gate.stage.XXXXXX")"
CAPTURE_PID=""
cleanup() {
  if [[ -n "${CAPTURE_PID:-}" ]]; then
    kill "$CAPTURE_PID" 2>/dev/null || true
    wait "$CAPTURE_PID" 2>/dev/null || true
  fi
  if [[ -n "${STAGE:-}" && -d "$STAGE" ]]; then
    rm -rf -- "$STAGE"
  fi
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

COMPOSITE_HTML="$STAGE/comparison.html"
make_composite_html "$COMPOSITE_HTML"
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
GIT_SHA="$(git -C "$REPO_DIR" rev-parse HEAD 2>/dev/null || printf 'unknown')"
GENERATED_AT="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
COMMAND_LINE="$(printf '%q ' "$SCRIPT_DIR/$SCRIPT_NAME" "${ORIGINAL_ARGS[@]}")"

# Markdown backticks are literal printf text, not shell command substitutions.
# shellcheck disable=SC2016
{
  printf '# Issue #%s design-gate evidence\n\n' "$ISSUE"
  printf 'Generated: `%s`\n\n' "$GENERATED_AT"
  printf '## Capture\n\n'
  printf -- '- Surface: `%s`\n' "$SURFACE"
  printf -- '- Capture kind: %s\n' "$CAPTURE_KIND"
  printf -- '- Live description: %s\n' "$LIVE_DESCRIPTION"
  printf -- '- Command: `%s`\n' "$CAPTURE_COMMAND"
  if [[ "$SURFACE" == "egui" && -z "$LIVE_PNG" ]]; then
    printf -- '- Host health URL: `%s` (checked before capture)\n' "$HOST_URL"
  else
    printf -- '- Host health URL: not checked for this supplied capture\n'
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
  printf '\n## Sources\n\n'
  printf -- '- Prototype source: `%s`\n' "$PROTOTYPE"
  printf -- '- Prototype source SHA-256: `%s`\n' "$PROTOTYPE_SOURCE_SHA"
  printf -- '- Generator SHA-256: `%s`\n' "$GENERATOR_SHA"
  printf -- '- Live input: `%s`\n' "$LIVE_SOURCE_PATH"
  printf -- '- Live input SHA-256: `%s`\n' "$LIVE_SOURCE_SHA"
  printf -- '- Repository HEAD: `%s`\n' "$GIT_SHA"
  printf -- '- Reproducible invocation: `%s`\n' "$COMMAND_LINE"
  printf '\n## Artifacts\n\n'
  printf -- '| File | Dimensions | SHA-256 |\n| --- | --- | --- |\n'
  printf -- '| `prototype.png` | `%s` | `%s` |\n' "$PROTOTYPE_DIMS" "$PROTOTYPE_SHA"
  printf -- '| `live-after.png` | `%s` | `%s` |\n' "$LIVE_DIMS" "$LIVE_SHA"
  printf -- '| `comparison.png` | `%s` | `%s` |\n' "$COMPARISON_DIMS" "$COMPARISON_SHA"
  printf '\nThe comparison header is stamped with the target issue number. A supplied PNG or iOS Debug demo is explicitly labeled above and must not be read as proof of a live daemon session.\n'
} >"$STAGE/conformance.md"

for artifact in prototype.png live-after.png comparison.png conformance.md capture.log; do
  [[ -s "$STAGE/$artifact" ]] || die "validated artifact is missing or empty: $artifact"
done

for artifact in prototype.png live-after.png comparison.png conformance.md capture.log; do
  mv -f -- "$STAGE/$artifact" "$OUTPUT_DIR/$artifact"
done
rm -rf -- "$STAGE"
STAGE=""

log "wrote $OUTPUT_DIR"
log "prototype: $OUTPUT_DIR/prototype.png ($PROTOTYPE_DIMS)"
log "live: $OUTPUT_DIR/live-after.png ($LIVE_DIMS)"
log "comparison: $OUTPUT_DIR/comparison.png ($COMPARISON_DIMS)"
