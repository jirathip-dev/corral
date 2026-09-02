#!/usr/bin/env bash
# update-corral.sh — build fetched origin/main, restart daemon, relaunch egui.
#
# Run by the com.corral.corrald-update launchd agent (installed by
# setup-corrald.sh) on a schedule. Idempotent and cheap: exits quickly when
# there is nothing new, but ALWAYS compares the freshly built daemon binary
# against the binary launchd actually executes before deciding "up to date" —
# a binary-only change deploys even when the git history did not move.
#
# Safety rules (do not weaken):
#   - The developer checkout is read-only input. Dirty trees and feature
#     branches are safe because the fetched revision is archived into a
#     disposable source checkout before cargo runs.
#   - Never pull, checkout, reset, clean, or merge the developer checkout.
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
# (partial install, release-bundle drift), the explicit release-required path
# below keeps the failure visible instead of silently skipping an update.
lib_path="$SCRIPT_DIR/lib-corral-update-path.sh"
# shellcheck disable=SC1090  # sourced path is dynamic (built from $SCRIPT_DIR)
if [[ -f "$lib_path" ]]; then
  source "$lib_path"
  corral_prepend_update_path
fi

CONFIG_DIR="${CORRAL_CONFIG_DIR:-$HOME/.config/corral}"
LOG="$CONFIG_DIR/corral-update.log"
UI_STAMP_FILE="$CONFIG_DIR/ui-artifact.sha256"
MACOS_APP_DEST="${CORRAL_MACOS_APP_DEST:-/Applications/Corral.app}"
LINUX_PREFIX="${CORRAL_LINUX_PREFIX:-$HOME/.local}"
OTHER_PREFIX="${CORRAL_OTHER_PREFIX:-$HOME/.local}"
mkdir -p "$CONFIG_DIR"

log() { echo "$(date '+%Y-%m-%dT%H:%M:%S%z')  $*" >> "$LOG"; }

if ! REPO_DIR="$(cd -P "$REPO_DIR" 2>/dev/null && pwd -P)"; then
  log "release-required: repository path does not exist; use scripts/install-corral.sh"
  exit 1
fi

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

ui_deploy_path() {
  case "$(uname -s)" in
    Darwin) printf '%s/Contents/MacOS/corrald-ui' "$MACOS_APP_DEST" ;;
    Linux) printf '%s/bin/corrald-ui' "$LINUX_PREFIX" ;;
    *) printf '%s/bin/corrald-ui' "$OTHER_PREFIX" ;;
  esac
}

ui_stamp_hash() {
  # Canonical identity of the last successfully deployed UI source artifact.
  # codesign rewrites the deployed executable after copy, so the deployed
  # bytes are never a stable comparison target.
  [[ -s "$UI_STAMP_FILE" ]] || { echo ""; return 0; }
  tr -d '[:space:]' < "$UI_STAMP_FILE" 2>/dev/null || true
}

write_ui_stamp() {
  local stamp="$1"
  local tmp
  tmp="$(mktemp "${UI_STAMP_FILE}.XXXXXX")"
  printf '%s\n' "$stamp" > "$tmp"
  chmod 600 "$tmp"
  mv -f "$tmp" "$UI_STAMP_FILE"
}

# --- Resolve source checkout and fetch the build revision ---------------------
# `git rev-parse` walks up parent directories, so compare the resolved top
# level instead. A release copy nested in an unrelated git worktree must not
# fetch/pull that outer repository and try to cargo-build from the release dir.
if [[ "$(git -C "$REPO_DIR" rev-parse --show-toplevel 2>/dev/null || true)" != "$REPO_DIR" ]]; then
  log "release-required: updater is not running from a source checkout; use scripts/install-corral.sh"
  exit 1
fi

before="$(git -C "$REPO_DIR" rev-parse HEAD)"
if ! git -C "$REPO_DIR" fetch origin main -q 2>>"$LOG"; then
  log "release-required: could not fetch origin/main; use scripts/install-corral.sh with a published release"
  exit 1
fi
after="$(git -C "$REPO_DIR" rev-parse origin/main)"
if [[ "$before" == "$after" ]]; then
  # No new upstream commits, but do NOT exit here: a binary-only change (a
  # rebuild with a different compiler/feature, or a manual cargo build) still
  # has to deploy when the built binary differs from the binary launchd
  # executes. The cheap hash-compare below decides "up to date".
  log "no new upstream commits — checking binary drift"
else
  log "building origin/main: $(git -C "$REPO_DIR" log --oneline "$before..$after" | wc -l | tr -d ' ') new commit(s)"
fi

# Build a fetched tree outside the developer checkout. `git archive` excludes
# dirty and untracked files and leaves the primary worktree's branch, index,
# and files untouched.
SOURCE_CHECKOUT="$(mktemp -d "${TMPDIR:-/tmp}/corral-update-source.XXXXXX")"
cleanup_source() { rm -rf -- "$SOURCE_CHECKOUT"; }
trap cleanup_source EXIT
if ! git -C "$REPO_DIR" archive --format=tar "$after" | tar -xf - -C "$SOURCE_CHECKOUT"; then
  log "release-required: could not materialize origin/main in an isolated checkout"
  exit 1
fi

# --- Rebuild ---------------------------------------------------------------
BIN_DIR="$SOURCE_CHECKOUT/target/release"
DAEMON_BIN="$BIN_DIR/corrald"
UI_BIN="$BIN_DIR/corrald-ui"
UI_DEPLOY_PATH="$(ui_deploy_path)"
daemon_before_hash="$(file_hash "$REPO_DIR/target/release/corrald")"
ui_before_hash="$(ui_stamp_hash)"

log "building origin/main in isolated checkout..."
(cd "$SOURCE_CHECKOUT" && CORRAL_BUILD_ID="$after" cargo build --release) 2>>"$LOG" || {
  log "release-required: origin/main build FAILED — use scripts/install-corral.sh with a published release"
  exit 1
}
log "build ok"
daemon_after_hash="$(file_hash "$DAEMON_BIN")"
ui_after_hash="$(file_hash "$UI_BIN")"

# --- Desktop client: reinstall + relaunch if running -------------------------
# Mirrors install_desktop_client() in setup-corrald.sh: macOS -> Corral.app
# bundle, Linux -> ~/.local/bin + .desktop, other -> ~/.local/bin. Returns 0
# after a successful deploy and stamp update; 1 when there was nothing to
# deploy (app not installed), so the caller never stamps a failed/partial
# install.
reinstall_ui() {
  local UI_BIN="$BIN_DIR/corrald-ui"
  local platform ui_backup
  platform="$(uname -s)"
  ui_backup=""
  if [[ "$platform" == "Darwin" && ! -d "$MACOS_APP_DEST" ]]; then
    log "Corral.app not installed — skipped UI update (run scripts/install-corral-ui.sh)"
    return 1
  fi
  mkdir -p "$(dirname "$UI_DEPLOY_PATH")"
  # Keep the prior working binary until the new one is deployed, signed,
  # relaunched, and startup-certified; every failure path rolls back to it,
  # so the app is never left half-deployed or dead-but-marked-current.
  ui_backup="$(mktemp "${UI_DEPLOY_PATH}.backup.XXXXXX")"
  cp -p "$UI_DEPLOY_PATH" "$ui_backup" 2>/dev/null || true
  if ! cp "$UI_BIN" "$UI_DEPLOY_PATH"; then
    rm -f "$ui_backup"
    log "release-required: UI deploy FAILED: $UI_BIN -> $UI_DEPLOY_PATH"
    exit 1
  fi
  if [[ "$platform" == "Darwin" ]]; then
    if ! codesign -s - --force "$MACOS_APP_DEST" 2>/dev/null; then
      log "release-required: UI signing FAILED"
      if [[ -f "$ui_backup" ]]; then
        mv -f "$ui_backup" "$UI_DEPLOY_PATH"
        codesign -s - --force "$MACOS_APP_DEST" 2>/dev/null || true
      else
        rm -f "$ui_backup"
      fi
      exit 1
    fi
  else
    chmod +x "$UI_DEPLOY_PATH"
  fi
  if pgrep -f "$UI_DEPLOY_PATH" >/dev/null 2>&1; then
    pkill -f "$UI_DEPLOY_PATH" 2>/dev/null || true
    sleep 1
    nohup "$UI_DEPLOY_PATH" >/dev/null 2>&1 &
    if [[ "$platform" == "Darwin" ]]; then
      log "Corral.app relaunched (new binary), certifying startup"
    else
      log "corrald-ui relaunched (new binary), certifying startup"
    fi
  else
    # App was not running: nothing to certify; next launch uses the new
    # binary. Deploy + stamp is the established not-running semantics.
    if [[ "$platform" == "Darwin" ]]; then
      log "Corral.app not running — next launch uses the new binary"
    else
      log "corrald-ui not running — next launch uses the new binary"
    fi
    rm -f "$ui_backup"
    write_ui_stamp "$ui_after_hash"
    return 0
  fi
  # Certify startup before stamping: `cmd &` returns immediately, so a
  # binary that crashes on launch would otherwise be marked current while
  # the client stays dead. Poll the running probe for a bounded window.
  local alive=0
  for _ in $(seq 1 12); do
    if pgrep -f "$UI_DEPLOY_PATH" >/dev/null 2>&1; then
      alive=1
      break
    fi
    sleep 0.5
  done
  if [[ "$alive" == "1" ]]; then
    log "UI client startup certified (new binary)"
    rm -f "$ui_backup"
    write_ui_stamp "$ui_after_hash"
    return 0
  fi
  # The freshly deployed binary did not survive startup: roll back to the
  # prior working binary, attempt to relaunch it, and fail closed without
  # advancing the canonical stamp.
  log "release-required: UI relaunch FAILED — rolled back"
  if [[ -f "$ui_backup" ]]; then
    mv -f "$ui_backup" "$UI_DEPLOY_PATH"
    if [[ "$platform" == "Darwin" ]]; then
      codesign -s - --force "$MACOS_APP_DEST" 2>/dev/null || true
    fi
  else
    rm -f "$ui_backup"
  fi
  nohup "$UI_DEPLOY_PATH" >/dev/null 2>&1 &
  log "restored previous UI binary relaunched"
  exit 1
}

# --- Deploy to the path launchd actually executes --------------------------
# source-mode installs point com.corral.corrald at the build output; on
# release-mode installs it points at the release bundle (e.g.
# ~/.local/share/corral/release/corrald), so a fresh build alone never
# reaches the running daemon. Resolution order: the loaded launchd job
# (ground truth of what launchd re-execs), then the plist file on disk,
# then the in-repo build output (source mode).
daemon_changed=0
deploy_failed=0
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
[[ -n "$deploy_path" ]] || deploy_path="$REPO_DIR/target/release/corrald"

if [[ "$deploy_path" == "$REPO_DIR/target/release/corrald" ]]; then
  # A source-mode plist still points at the primary checkout's ignored target
  # artifact. Install the isolated build there only after it is complete; no
  # tracked developer file is changed and no checkout operation is performed.
  if [[ "$daemon_before_hash" != "$daemon_after_hash" ]]; then
    if install -m 755 "$DAEMON_BIN" "$deploy_path"; then
      log "deployed isolated $DAEMON_BIN -> $deploy_path"
      daemon_changed=1
    else
      log "release-required: deploy FAILED: $DAEMON_BIN -> $deploy_path"
      deploy_failed=1
    fi
  fi
elif [[ "$daemon_after_hash" != "$(file_hash "$deploy_path")" ]]; then
  # Release mode: deterministic ship of the freshly built binary into the
  # path the plist names, before any kickstart.
  if install -m 755 "$DAEMON_BIN" "$deploy_path"; then
    log "deployed $DAEMON_BIN -> $deploy_path"
    daemon_changed=1
  else
    log "deploy FAILED: $DAEMON_BIN -> $deploy_path — keeping old binaries"
    deploy_failed=1
  fi
fi

if [[ "$deploy_failed" == "1" ]]; then
  log "release-required: could not install the fetched origin/main artifact"
  exit 1
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
  log "up to date ($(git -C "$REPO_DIR" log -1 --format='%h' "$after")); deploy path $deploy_path; restarted=no"
fi

# --- Relaunch the egui client when the source artifact changed -------------
# Compare against the pre-sign source stamp: codesign rewrites the deployed
# executable after copy, so the deployed bytes would look changed forever.
if [[ "$ui_after_hash" != "$ui_before_hash" ]]; then
  if ! reinstall_ui; then
    log "UI artifact changed but was not deployed; canonical stamp not updated"
  fi
fi

log "done: $(git -C "$REPO_DIR" log -1 --format='%h %s' "$after")"
