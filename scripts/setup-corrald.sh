#!/usr/bin/env bash
# setup-corrald.sh — one-shot corrald daemon setup (build + launchd + first run)
#
# Idempotent: safe to re-run. Works for any user (uses $HOME, no hardcoded paths).
# Does NOT modify source; only builds, creates the config dir, and installs a
# launchd agent.
#
# Usage:
#   scripts/setup-corrald.sh            # build + run under launchd on 127.0.0.1:8474
#   scripts/setup-corrald.sh --bind 100.67.222.5   # bind a Tailscale/private IP (needs #65)
#   scripts/setup-corrald.sh --uninstall
#
# Prereqs: rustup (pinned toolchain auto-installs), herdr running (optional;
# corrald serves without it, just shows no agents).
set -euo pipefail

BIND="127.0.0.1"
PORT="8474"
UNINSTALL=0

# Parse args with an index loop so --bind can consume its value.
i=0
args=("$@")
while [[ $i -lt ${#args[@]} ]]; do
  case "${args[$i]}" in
    --bind)
      i=$((i+1))
      BIND="${args[$i]:-127.0.0.1}"
      ;;
    --uninstall) UNINSTALL=1 ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "unknown arg: ${args[$i]}" >&2; exit 2 ;;
  esac
  i=$((i+1))
done

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$REPO_DIR/target/release/corrald"
CONFIG_DIR="${CORRAL_CONFIG_DIR:-$HOME/.config/corral}"
PLIST="$HOME/Library/LaunchAgents/com.jirathip.corrald.plist"
LABEL="com.jirathip.corrald"
LOG="$CONFIG_DIR/corrald-launchd.log"

if [[ "$UNINSTALL" == "1" ]]; then
  echo ">> Uninstalling corrald launchd agent"
  launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
  rm -f "$PLIST"
  echo ">> Removed $PLIST. Config/keys kept at $CONFIG_DIR (delete manually to wipe)."
  exit 0
fi

echo ">> Building corrald (release)..."
cargo build --release --manifest-path "$REPO_DIR/Cargo.toml"

mkdir -p "$CONFIG_DIR"

echo ">> Installing launchd agent: $PLIST"
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
  <key>StandardOutPath</key><string>$LOG</string>
  <key>StandardErrorPath</key><string>$LOG</string>
</dict>
</plist>
PLIST_EOF
plutil -lint "$PLIST" >/dev/null

echo ">> Loading under launchd..."
launchctl bootstrap "gui/$(id -u)" "$PLIST" 2>/dev/null || launchctl kickstart -k "gui/$(id -u)/$LABEL" 2>/dev/null || true

sleep 2
echo ">> Health check:"
if curl -fsS "http://$BIND:$PORT/healthz" >/dev/null 2>&1; then
  echo "   ✓ corrald is UP at http://$BIND:$PORT/healthz"
else
  echo "   ⚠ could not reach http://$BIND:$PORT/healthz — check $LOG"
fi

echo
echo ">> Next:"
echo "   - client (egui):   cargo run -p corrald-ui --release   (auto-registers on localhost)"
echo "   - device grant:    scripts/corrald-grant.sh --key <key_id> --caps read_tail,prompt"
echo "   - view config:     ls -la $CONFIG_DIR"
echo "   - logs:            tail -f $LOG"
