#!/usr/bin/env bash
# update-corral.sh — pull main, rebuild, restart daemon, relaunch egui client.
#
# Run by the com.corral.corrald-update launchd agent (installed by
# setup-corrald.sh) on a schedule. Idempotent and cheap: exits immediately
# when there is nothing new.
#
# Safety rules (do not weaken):
#   - Only touches the MAIN checkout when it is clean and on `main`. A dirty
#     tree or a feature branch means someone is working there: skip this
#     cycle silently and retry next interval.
#   - Fast-forward pulls only; never --force, never merge.
#   - The daemon restarts ONLY when its binary changed.
#   - The egui client is relaunched ONLY when it is running AND its binary
#     changed (killing a running editor window is a deliberate act — logged).
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_DIR="${CORRAL_CONFIG_DIR:-$HOME/.config/corral}"
LOG="$CONFIG_DIR/corral-update.log"
mkdir -p "$CONFIG_DIR"

log() { echo "$(date '+%Y-%m-%dT%H:%M:%S%z')  $*" >> "$LOG"; }

file_mtime() {  # epoch mtime; 0 when missing/unreadable
  if [[ "$(uname -s)" == "Darwin" ]]; then
    stat -f %m "$1" 2>/dev/null || echo 0
  else
    stat -c %Y "$1" 2>/dev/null || echo 0
  fi
}

cd "$REPO_DIR"

# --- Guards: skip when the main checkout is not in a pullable state ---------
if [[ "$(git branch --show-current)" != "main" ]]; then
  log "skip: not on main (branch=$(git branch --show-current))"
  exit 0
fi
if ! git status --porcelain --untracked-files=no | grep -q .; then
  : # clean (untracked files like orch briefs are fine)
else
  log "skip: working tree dirty — $(git status --porcelain --untracked-files=no | wc -l | tr -d ' ') modified file(s)"
  exit 0
fi

# --- Fetch and decide -------------------------------------------------------
before="$(git rev-parse HEAD)"
git fetch origin main -q 2>>"$LOG" || { log "skip: git fetch failed"; exit 0; }
after="$(git rev-parse origin/main)"
if [[ "$before" == "$after" ]]; then
  log "up to date ($(git log -1 --format='%h' HEAD))"
  exit 0
fi
log "pulling origin/main: $(git log --oneline "$before..$after" | wc -l | tr -d ' ') new commit(s)"

# --- Pull + rebuild ---------------------------------------------------------
git pull --ff-only origin main 2>>"$LOG" || { log "pull failed — retry next cycle"; exit 0; }

BIN_DIR="$REPO_DIR/target/release"
DAEMON_BIN="$BIN_DIR/corrald"
UI_BIN="$BIN_DIR/corrald-ui"
daemon_before_mtime="$(file_mtime "$DAEMON_BIN")"
ui_before_mtime="$(file_mtime "$UI_BIN")"

log "building (workspace release)..."
cargo build --release 2>>"$LOG" || { log "build FAILED — keeping old binaries"; exit 0; }
log "build ok"

# --- Desktop client: reinstall + relaunch if running -------------------------
# Mirrors install_desktop_client() in setup-corrald.sh: macOS -> Corral.app
# bundle, Linux -> ~/.local/bin + .desktop, other -> ~/.local/bin.
reinstall_ui() {
  local UI_BIN="$BIN_DIR/corrald-ui"
  case "$(uname -s)" in
    Darwin)
      local APP="/Applications/Corral.app"
      if [[ -d "$APP" ]]; then
        cp "$UI_BIN" "$APP/Contents/MacOS/corrald-ui"
        codesign -s - --force "$APP" 2>/dev/null || true
        if pgrep -f "$APP/Contents/MacOS/corrald-ui" >/dev/null 2>&1; then
          pkill -f "$APP/Contents/MacOS/corrald-ui" 2>/dev/null || true
          sleep 1
          nohup "$APP/Contents/MacOS/corrald-ui" >/dev/null 2>&1 &
          log "Corral.app relaunched (new binary)"
        else
          log "Corral.app not running — next launch uses the new binary"
        fi
        return 0
      fi
      ;;
    Linux)
      mkdir -p "$HOME/.local/bin"
      cp "$UI_BIN" "$HOME/.local/bin/corrald-ui"
      chmod +x "$HOME/.local/bin/corrald-ui"
      if pgrep -f "$HOME/.local/bin/corrald-ui" >/dev/null 2>&1; then
        pkill -f "$HOME/.local/bin/corrald-ui" 2>/dev/null || true
        sleep 1
        nohup "$HOME/.local/bin/corrald-ui" >/dev/null 2>&1 &
        log "corrald-ui relaunched (new binary)"
      fi
      return 0
      ;;
  esac
  # Fallback: raw binary (works everywhere)
  if pgrep -f "$UI_BIN" >/dev/null 2>&1; then
    pkill -f "$UI_BIN" 2>/dev/null || true
    sleep 1
    nohup "$UI_BIN" >/dev/null 2>&1 &
    log "egui client relaunched (new binary)"
  else
    log "egui client not running — next launch uses the new binary"
  fi
}

# --- Restart the daemon if its binary changed ------------------------------
if [[ "$(file_mtime "$DAEMON_BIN")" != "$daemon_before_mtime" ]]; then
  if launchctl print "gui/$(id -u)/com.corral.corrald" >/dev/null 2>&1; then
    launchctl kickstart -k "gui/$(id -u)/com.corral.corrald" && log "daemon restarted (new binary)"
  else
    log "daemon job not loaded — skipped restart (run setup-corrald.sh)"
  fi
fi

# --- Relaunch the egui client if it is running and changed -----------------
if [[ "$(file_mtime "$UI_BIN")" != "$ui_before_mtime" ]]; then
  reinstall_ui
fi

log "done: $(git log -1 --format='%h %s' HEAD)"
