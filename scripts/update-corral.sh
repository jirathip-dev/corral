#!/usr/bin/env bash
# update-corral.sh — pull main, rebuild, restart daemon, relaunch egui client.
#
# Run by the com.corral.corrald-update launchd agent (installed by
# setup-corrald.sh) on a schedule. Idempotent and cheap: exits quickly when
# there is nothing new, but ALWAYS compares the freshly built daemon binary
# against the binary launchd actually executes before deciding "up to date" —
# a binary-only change deploys even when the git history did not move.
#
# Safety rules (do not weaken):
#   - Only touches the MAIN checkout when it is clean and on `main`. A dirty
#     tree or a feature branch means someone is working there: skip this
#     cycle silently and retry next interval.
#   - Fast-forward pulls only; never --force, never merge.
#   - The daemon restarts ONLY when the binary it executes changed.
#   - The egui client is relaunched ONLY when it is running AND its binary
#     changed (killing a running editor window is a deliberate act — logged).
set -euo pipefail

REPO_DIR="${CORRAL_REPO_DIR:-$(cd -P "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)}"
SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# launchd runs this job with a minimal PATH (no /opt/homebrew/bin), so `gh`
# (git's HTTPS credential helper) is not found and `git fetch` fails. Derive a
# runtime PATH that prepends Homebrew's bin and cargo's bin when present; this
# is the primary fix (takes effect without reinstalling the launchd plist).
# Only source the lib when it is actually present: if it is ever missing
# (partial pull, release-bundle drift), fall back to the pre-fix behavior so a
# launchd run logs the normal skip line instead of dying silently before the
# log is even set up.
lib_path="$SCRIPT_DIR/lib-corral-update-path.sh"
# shellcheck disable=SC1090  # sourced path is dynamic (built from $SCRIPT_DIR)
if [[ -f "$lib_path" ]]; then
  source "$lib_path"
  corral_prepend_update_path
fi

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

file_hash() {  # sha256 of a file; empty when missing/unreadable
  [[ -f "$1" ]] || { echo ""; return 0; }
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" 2>/dev/null | awk '{print $1; exit}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" 2>/dev/null | awk '{print $1; exit}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$1" 2>/dev/null | awk '{print $NF; exit}'
  else
    echo ""
  fi
}

cd "$REPO_DIR"

# --- Guards: skip when this is not the actual source checkout ----------------
# `git rev-parse` walks up parent directories, so compare the resolved top
# level instead. A release copy nested in an unrelated git worktree must not
# fetch/pull that outer repository and try to cargo-build from the release dir.
if [[ "$(git -C "$REPO_DIR" rev-parse --show-toplevel 2>/dev/null || true)" != "$REPO_DIR" ]]; then
  log "skip: not a source checkout; release installs are updated with scripts/install-corral.sh"
  exit 0
fi
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
  # No new upstream commits, but do NOT exit here: a binary-only change (a
  # rebuild with a different compiler/feature, or a manual cargo build) still
  # has to deploy when the built binary differs from the binary launchd
  # executes. The cheap hash-compare below decides "up to date".
  log "no new upstream commits — checking binary drift"
else
  log "pulling origin/main: $(git log --oneline "$before..$after" | wc -l | tr -d ' ') new commit(s)"
  git pull --ff-only origin main 2>>"$LOG" || { log "pull failed — retry next cycle"; exit 0; }
fi

# --- Rebuild ---------------------------------------------------------------
# Build on every cycle: with nothing to build cargo is a fast no-op, and
# building is what makes a binary-only change (new toolchain, changed feature
# set) deployable even when the git history did not move.
BIN_DIR="$REPO_DIR/target/release"
DAEMON_BIN="$BIN_DIR/corrald"
UI_BIN="$BIN_DIR/corrald-ui"
daemon_before_hash="$(file_hash "$DAEMON_BIN")"
ui_before_mtime="$(file_mtime "$UI_BIN")"

log "building (workspace release)..."
cargo build --release 2>>"$LOG" || { log "build FAILED — keeping old binaries"; exit 0; }
log "build ok"
daemon_after_hash="$(file_hash "$DAEMON_BIN")"

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

# --- Deploy to the path launchd actually executes --------------------------
# source-mode installs point com.corral.corrald at the build output; on
# release-mode installs it points at the release bundle (e.g.
# ~/.local/share/corral/release/corrald), so a fresh build alone never
# reaches the running daemon. Resolution order: the loaded launchd job
# (ground truth of what launchd re-execs), then the plist file on disk,
# then the in-repo build output (source mode).
daemon_changed=0
job_loaded=0
job_line=""
if job_line="$(launchctl print "gui/$(id -u)/com.corral.corrald" 2>/dev/null)"; then
  job_loaded=1
fi
deploy_path=""
if [[ -n "$job_line" ]]; then
  deploy_path="$(awk '/^[[:space:]]*program = /{sub(/^[[:space:]]*program = /, ""); print; exit}' <<<"$job_line")"
fi
if [[ -z "$deploy_path" ]]; then
  deploy_path="$(plutil -extract ProgramArguments.0 raw "$HOME/Library/LaunchAgents/com.corral.corrald.plist" 2>/dev/null || true)"
fi
[[ -n "$deploy_path" ]] || deploy_path="$DAEMON_BIN"

if [[ "$deploy_path" == "$DAEMON_BIN" ]]; then
  # Source mode: launchd executes the build output directly, so the restart
  # decision is whether the rebuild changed that file in place.
  if [[ "$daemon_before_hash" != "$daemon_after_hash" ]]; then
    log "daemon binary changed: $deploy_path"
    daemon_changed=1
  fi
elif [[ "$daemon_after_hash" != "$(file_hash "$deploy_path")" ]]; then
  # Release mode: deterministic ship of the freshly built binary into the
  # path the plist names, before any kickstart.
  if install -m 755 "$DAEMON_BIN" "$deploy_path"; then
    log "deployed $DAEMON_BIN -> $deploy_path"
    daemon_changed=1
  else
    log "deploy FAILED: $DAEMON_BIN -> $deploy_path — keeping old binaries"
  fi
fi

# --- Restart the daemon only when the binary it executes changed ------------
if [[ "$daemon_changed" == "1" ]]; then
  if [[ "$job_loaded" == "1" ]]; then
    if launchctl kickstart -k "gui/$(id -u)/com.corral.corrald"; then
      log "restarted=yes"
    else
      log "restarted=no (kickstart failed)"
    fi
  else
    log "daemon job not loaded — skipped restart (run setup-corrald.sh); restarted=no"
  fi
else
  log "up to date ($(git log -1 --format='%h' HEAD)); deploy path $deploy_path; restarted=no"
fi

# --- Relaunch the egui client if it is running and changed -----------------
if [[ "$(file_mtime "$UI_BIN")" != "$ui_before_mtime" ]]; then
  reinstall_ui
fi

log "done: $(git log -1 --format='%h %s' HEAD)"
