#!/usr/bin/env bash
# setup-corrald.sh — one-shot corrald daemon setup (build + launchd + first run)
#
# Idempotent: safe to re-run. Works for any user (uses $HOME, no hardcoded paths).
# Does NOT modify source; only builds, creates the config dir, and installs a
# launchd agent.
#
# Usage:
#   scripts/setup-corrald.sh            # build + run under launchd on 127.0.0.1:8474
#   scripts/setup-corrald.sh --bind 100.67.222.5   # Tailscale/private IP (desktop/daemon
#                                                  # only; iOS needs Tailscale Serve —
#                                                  # see docs/OPERATIONS.md)
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
    --uninstall) UNINSTALL=1 ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "unknown arg: ${args[$i]}" >&2; exit 2 ;;
  esac
  i=$((i+1))
done

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$REPO_DIR/target/release/corrald"
CONFIG_DIR="${CORRAL_CONFIG_DIR:-$HOME/.config/corral}"
PLIST="$HOME/Library/LaunchAgents/com.corral.corrald.plist"
LABEL="com.corral.corrald"
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
# bootout any previously-loaded job FIRST — launchd does NOT re-read a
# rewritten plist on kickstart, so a re-run with changed --bind would
# silently restart the old config. bootout + bootstrap applies the new file.
launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
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
echo ">> Next:"
echo "   - client (egui):   cargo run -p corrald-ui --release   (auto-registers on localhost)"
echo "   - device grant:    scripts/corrald-grant.sh --key <key_id> --caps read_tail,prompt"
echo "   - view config:     ls -la $CONFIG_DIR"
echo "   - logs:            tail -f $LOG"
