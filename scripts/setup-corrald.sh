#!/usr/bin/env bash
# setup-corrald.sh — one-shot corrald daemon setup (build + launchd + first run)
#
# Idempotent: safe to re-run. Works for any user (uses $HOME, no hardcoded paths).
# Does NOT modify source; only builds, creates the config dir, installs a
# launchd agent, and stages the desktop client through the icon-aware installer.
#
# Usage:
#   scripts/setup-corrald.sh            # build + run under launchd on 127.0.0.1:8474
#   scripts/setup-corrald.sh --from-release <binary-path>  # prebuilt corrald, no cargo
#   scripts/setup-corrald.sh --bind 100.67.222.5   # Tailscale/private IP (desktop/daemon
#                                                  # only; iOS needs Tailscale Serve —
#                                                  # see docs/OPERATIONS.md)
#   scripts/setup-corrald.sh --uninstall
#
# Prereqs (source mode): rustup (pinned toolchain auto-installs), herdr running
# (optional; corrald serves without it, just shows no agents). Release mode uses
# a prebuilt corrald + sibling corrald-ui and needs no Rust toolchain.
set -euo pipefail

BIND="127.0.0.1"
PORT="8474"
UNINSTALL=0
FROM_RELEASE=""

# Parse args with an index loop so --bind can consume its value.
i=0
args=("$@")
while [[ $i -lt ${#args[@]} ]]; do
  case "${args[$i]}" in
    --bind)
      i=$((i+1))
      if [[ $i -ge ${#args[@]} || -z "${args[$i]}" ]]; then
        echo "!! --bind requires a value (IPv4/IPv6)" >&2; exit 2
      fi
      BIND="${args[$i]}"
      # Validate: IPv4 dotted-quad or IPv6 (the daemon parses --bind as
      # IpAddr and PANICS on a hostname — reject anything that isn't an
      # IP so a typo can't crash-loop under KeepAlive).
      if ! [[ "$BIND" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] && ! [[ "$BIND" =~ ^[0-9a-fA-F:]+$ ]]; then
        echo "!! invalid --bind address (IPv4 or IPv6 only): $BIND" >&2; exit 2
      fi
      ;;
    --port)
      i=$((i+1))
      if [[ $i -ge ${#args[@]} || -z "${args[$i]}" || ! "${args[$i]}" =~ ^[0-9]+$ || "${args[$i]}" -lt 1 || "${args[$i]}" -gt 65535 ]]; then
        echo "!! --port requires a value from 1-65535" >&2; exit 2
      fi
      PORT="${args[$i]}"
      ;;
    --from-release)
      i=$((i+1))
      if [[ $i -ge ${#args[@]} || -z "${args[$i]}" ]]; then
        echo "!! --from-release requires a path to a prebuilt corrald" >&2; exit 2
      fi
      FROM_RELEASE="${args[$i]}"
      ;;
    --uninstall) UNINSTALL=1 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown arg: ${args[$i]}" >&2; exit 2 ;;
  esac
  i=$((i+1))
done

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FROM_RELEASE_DIR=""
BIN="$REPO_DIR/target/release/corrald"
CONFIG_DIR="${CORRAL_CONFIG_DIR:-$HOME/.config/corral}"
PLIST="$HOME/Library/LaunchAgents/com.corral.corrald.plist"
UPDATE_PLIST="$HOME/Library/LaunchAgents/com.corral.corrald-update.plist"
ROTATE_PLIST="$HOME/Library/LaunchAgents/com.corral.corrald-rotate.plist"
LABEL="com.corral.corrald"
ROTATE_LABEL="com.corral.corrald-rotate"
LOG="$CONFIG_DIR/corrald-launchd.log"

if [[ -n "$FROM_RELEASE" ]]; then
  FROM_RELEASE="$(cd "$(dirname "$FROM_RELEASE")" && pwd)/$(basename "$FROM_RELEASE")"
  [[ -f "$FROM_RELEASE" && -x "$FROM_RELEASE" ]] || {
    echo "!! --from-release binary is missing or not executable: $FROM_RELEASE" >&2
    exit 2
  }
  FROM_RELEASE_DIR="$(dirname "$FROM_RELEASE")"
  BIN="$FROM_RELEASE"
  if [[ -x "$FROM_RELEASE_DIR/corrald-ui" && -f "$FROM_RELEASE_DIR/scripts/setup-corrald.sh" ]]; then
    REPO_DIR="$FROM_RELEASE_DIR"
  fi
fi

if [[ "$UNINSTALL" == "1" ]]; then
  echo ">> Uninstalling corrald launchd agents"
  launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
  launchctl bootout "gui/$(id -u)" "$UPDATE_PLIST" 2>/dev/null || true
  launchctl bootout "gui/$(id -u)" "$ROTATE_PLIST" 2>/dev/null || true
  rm -f "$PLIST"
  rm -f "$UPDATE_PLIST"
  rm -f "$ROTATE_PLIST"
  echo ">> Removed $PLIST, $UPDATE_PLIST and $ROTATE_PLIST. Config/keys kept at $CONFIG_DIR (delete manually to wipe)."
  exit 0
fi

if [[ -n "$FROM_RELEASE" ]]; then
  echo ">> Using prebuilt corrald: $BIN"
else
  echo ">> Building corrald (release)..."
  cargo build --release --manifest-path "$REPO_DIR/Cargo.toml"
fi

mkdir -p "$CONFIG_DIR"

if [[ "$(uname -s)" == "Darwin" ]]; then
echo ">> Installing launchd agent: $PLIST"
# bootout any previously-loaded job FIRST — launchd does NOT re-read a
# rewritten plist on kickstart, so a re-run with changed --bind would
# silently restart the old config. bootout + bootstrap applies the new file.
launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
launchctl enable "gui/$(id -u)/$LABEL" 2>/dev/null || true
cat > "$PLIST" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN</string>
    <string>--socket</string>
    <string>$HOME/.config/herdr/herdr.sock</string>
    <string>--bind</string>
    <string>$BIND</string>
    <string>--port</string>
    <string>$PORT</string>
  </array>
  <key>WorkingDirectory</key><string>$REPO_DIR</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>SoftResourceLimits</key>
  <dict>
    <key>NumberOfFiles</key><integer>10240</integer>
  </dict>
  <key>HardResourceLimits</key>
  <dict>
    <key>NumberOfFiles</key><integer>10240</integer>
  </dict>
  <key>StandardOutPath</key><string>$LOG</string>
  <key>StandardErrorPath</key><string>$LOG</string>
</dict>
</plist>
PLIST_EOF
plutil -lint "$PLIST" >/dev/null

echo ">> Loading under launchd..."
# bootstrap is the only way to apply a (possibly changed) plist; a genuine
# failure must be fatal, not swallowed. bootout can return before launchd
# finishes teardown on some macOS builds, so retry once after 1s.
launchctl bootstrap "gui/$(id -u)" "$PLIST" 2>&1 \
  || { sleep 1; launchctl bootstrap "gui/$(id -u)" "$PLIST" 2>&1; } \
  || { echo "!! launchctl bootstrap failed — see output above" >&2; exit 1; }

echo ">> Health check:"
ok=0
# IPv6 needs brackets in a URL: http://[::1]:8474/healthz
if [[ "$BIND" == *:* ]]; then
  URL="http://[$BIND]:$PORT/healthz"
else
  URL="http://$BIND:$PORT/healthz"
fi
for i in 1 2 3 4 5 6 7 8 9 10; do
  if curl -fsS "$URL" >/dev/null 2>&1; then
    ok=1; break
  fi
  sleep 1
done
if [[ "$ok" == "1" ]]; then
  echo "   ✓ corrald is UP at $URL"
else
  echo "   ✗ could not reach $URL — check $LOG" >&2
  exit 1
fi

echo
echo ">> Installing auto-update agent (com.corral.corrald-update)..."
chmod +x "$REPO_DIR/scripts/update-corral.sh"
# Belt-and-suspenders: bake a launchd-usable PATH into the update plist so the
# agent starts with gh/cargo resolvable. The script-top derivation in
# update-corral.sh is the primary fix (works on the next run without needing to
# re-run setup); this only helps fresh installs and setup re-runs.
SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# Only source the lib when present: a release bundle missing it must not abort
# setup after the daemon plist was already written — fall back to the current
# PATH and continue.
lib_path="$SCRIPT_DIR/lib-corral-update-path.sh"
# shellcheck disable=SC1090  # sourced path is dynamic (built from $SCRIPT_DIR)
if [[ -f "$lib_path" ]]; then
  source "$lib_path"
  corral_prepend_update_path
fi
UPDATE_PATH="$PATH"
# Escape XML metacharacters so an exotic prefix (containing &, <, >) cannot make
# plutil -lint fail on the generated plist.
UPDATE_PATH="${UPDATE_PATH//&/&amp;}"
UPDATE_PATH="${UPDATE_PATH//</&lt;}"
UPDATE_PATH="${UPDATE_PATH//>/&gt;}"
launchctl bootout "gui/$(id -u)" "$UPDATE_PLIST" 2>/dev/null || true
launchctl enable "gui/$(id -u)/com.corral.corrald-update" 2>/dev/null || true
cat > "$UPDATE_PLIST" <<UPDATE_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.corral.corrald-update</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>$REPO_DIR/scripts/update-corral.sh</string>
  </array>
  <key>WorkingDirectory</key><string>$REPO_DIR</string>
  <key>RunAtLoad</key><true/>
  <key>StartCalendarInterval</key>
  <dict>
    <key>Minute</key><integer>17</integer>
  </dict>
  <key>ProcessType</key><string>Background</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>$UPDATE_PATH</string>
  </dict>
  <key>AbandonProcessGroup</key><true/>
  <key>StandardOutPath</key><string>$CONFIG_DIR/corral-update.log</string>
  <key>StandardErrorPath</key><string>$CONFIG_DIR/corral-update.log</string>
</dict>
</plist>
UPDATE_EOF
plutil -lint "$UPDATE_PLIST" >/dev/null
if launchctl bootstrap "gui/$(id -u)" "$UPDATE_PLIST" 2>&1 \
  || { sleep 1; launchctl bootstrap "gui/$(id -u)" "$UPDATE_PLIST" 2>&1; }; then
  echo "   ✓ update agent loaded (checks hourly at :17)"
else
  echo "   ✗ update agent bootstrap failed — see output above" >&2
fi

echo
echo ">> Installing log-rotation agent ($ROTATE_LABEL)..."
chmod +x "$REPO_DIR/scripts/rotate-corral-logs.sh"
# Escape XML metacharacters so an exotic config dir (containing &, <, >) cannot
# make plutil -lint fail on the generated plist, matching the update agent.
CONFIG_DIR_ESCAPED="${CONFIG_DIR//&/&amp;}"
CONFIG_DIR_ESCAPED="${CONFIG_DIR_ESCAPED//</&lt;}"
CONFIG_DIR_ESCAPED="${CONFIG_DIR_ESCAPED//>/&gt;}"
launchctl bootout "gui/$(id -u)" "$ROTATE_PLIST" 2>/dev/null || true
launchctl enable "gui/$(id -u)/$ROTATE_LABEL" 2>/dev/null || true
cat > "$ROTATE_PLIST" <<ROTATE_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$ROTATE_LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>$REPO_DIR/scripts/rotate-corral-logs.sh</string>
  </array>
  <key>WorkingDirectory</key><string>$REPO_DIR</string>
  <key>RunAtLoad</key><true/>
  <key>StartInterval</key><integer>1800</integer>
  <key>ProcessType</key><string>Background</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>CORRAL_CONFIG_DIR</key>
    <string>$CONFIG_DIR_ESCAPED</string>
  </dict>
  <key>AbandonProcessGroup</key><true/>
  <key>StandardOutPath</key><string>$CONFIG_DIR_ESCAPED/corral-rotate.log</string>
  <key>StandardErrorPath</key><string>$CONFIG_DIR_ESCAPED/corral-rotate.log</string>
</dict>
</plist>
ROTATE_EOF
plutil -lint "$ROTATE_PLIST" >/dev/null
if launchctl bootstrap "gui/$(id -u)" "$ROTATE_PLIST" 2>&1 \
  || { sleep 1; launchctl bootstrap "gui/$(id -u)" "$ROTATE_PLIST" 2>&1; }; then
  echo "   ✓ log-rotation agent loaded (checks every 30 min; 50 MiB cap, 2 gz generations)"
else
  echo "   ✗ log-rotation agent bootstrap failed — see output above" >&2
fi
else
  echo ">> Skipping launchd agent (macOS only)"
fi

# --- Desktop client install (platform-aware) ---------------------------------
install_desktop_client() {
  local UI_BIN="$REPO_DIR/target/release/corrald-ui"
  if [[ -n "$FROM_RELEASE" ]]; then
    UI_BIN="$FROM_RELEASE_DIR/corrald-ui"
    if [[ ! -x "$UI_BIN" ]]; then
      echo "!! prebuilt corrald-ui binary is missing or not executable: $UI_BIN" >&2
      return 1
    fi
  elif [[ ! -x "$UI_BIN" ]]; then
    echo ">> corrald-ui binary missing — building..." >&2
    cargo build --release 2>>"$LOG" || { echo "   ✗ build failed" >&2; return 1; }
  fi
  CORRAL_INSTALL_PLATFORM="$(uname -s)" \
    bash "$REPO_DIR/scripts/install-corral-ui.sh" --binary "$UI_BIN"
}

install_desktop_client

echo
echo ">> Next:"
if [[ -n "$FROM_RELEASE" ]]; then
  echo "   - board:           open ${CORRAL_MACOS_APP_DEST:-/Applications/Corral.app}"
else
  echo "   - client (egui):   cargo run -p corrald-ui --release   (auto-registers on localhost)"
  echo "   - device grant:    scripts/corrald-grant.sh --key <key_id> --caps read_tail,prompt"
fi
echo "   - view config:     ls -la $CONFIG_DIR"
echo "   - logs:            tail -f $LOG"
echo "   - log rotation:    50 MiB cap, 2 gz generations, every 30 min ($CONFIG_DIR/corral-rotate.log)"
