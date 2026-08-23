#!/usr/bin/env bash
# rotate-corral-logs.sh — size-cap rotation for corral's user logs.
#
# corrald runs under launchd with KeepAlive and writes StandardOutPath /
# StandardErrorPath to $CONFIG_DIR/corrald-launchd.log. launchd holds that fd
# open for the lifetime of the daemon, so a rename-based rotation MUST restart
# the daemon afterwards (launchctl kickstart -k gui/$UID/com.corral.corrald):
# launchd reopens the path at offset 0 on restart, which quiesces the renamed
# inode so it can be gzipped. The update agent (com.corral.corrald-update) is a
# run-and-exit job, so corral-update.log is not held open persistently and needs
# no restart. Rotation never deletes the live log file.
#
# Idempotent: a no-op while both logs are under the cap. Cap defaults to 50 MiB
# and keeps 2 gzipped generations (.1.gz, .2.gz); .2.gz is dropped when the
# next rotation happens.
#
# Env overrides (used by setup-corrald.sh's launchd agent and by tests):
#   CORRAL_CONFIG_DIR            config dir (default $HOME/.config/corral)
#   CORRAL_LOG_MAX_BYTES         cap in bytes (default 52428800 = 50 MiB)
#   CORRAL_LOG_KEEP              generations kept (default 2, min 1)
#   CORRAL_ROTATE_SKIP_KICKSTART 1 skips the launchd restart (tests only)
#
# Usage:
#   scripts/rotate-corral-logs.sh
set -euo pipefail

CONFIG_DIR="${CORRAL_CONFIG_DIR:-$HOME/.config/corral}"
MAX_BYTES="${CORRAL_LOG_MAX_BYTES:-52428800}"
KEEP="${CORRAL_LOG_KEEP:-2}"
SKIP_KICKSTART="${CORRAL_ROTATE_SKIP_KICKSTART:-0}"

DAEMON_LOG="$CONFIG_DIR/corrald-launchd.log"
UPDATE_LOG="$CONFIG_DIR/corral-update.log"
DAEMON_LABEL="com.corral.corrald"

# Validate tunables so a stray env value cannot make the arithmetic below
# (which runs under `set -e`) fail with a confusing error.
if ! [[ "$MAX_BYTES" =~ ^[0-9]+$ ]] || (( MAX_BYTES < 1 )); then
  MAX_BYTES=52428800
fi
if ! [[ "$KEEP" =~ ^[0-9]+$ ]] || (( KEEP < 1 )); then
  KEEP=2
fi

file_size() {
  if [[ "$(uname -s)" == "Darwin" ]]; then
    stat -f %z "$1" 2>/dev/null || echo 0
  else
    stat -c %s "$1" 2>/dev/null || echo 0
  fi
}

# rotate_one <logpath> -> 0 if rotated, 1 if not (missing or under cap).
# Shifts existing gzipped generations, renames the live log to .1, and creates
# a fresh empty log at the original path. Does NOT gzip .1 — a process may
# still hold that inode; the caller compresses it after any needed restart.
rotate_one() {
  local logpath="$1"
  local size
  local i
  [[ -f "$logpath" ]] || return 1
  size="$(file_size "$logpath")"
  if ! [[ "$size" =~ ^[0-9]+$ ]] || (( size < MAX_BYTES )); then
    return 1
  fi

  # Drop the oldest generation beyond KEEP.
  if [[ -f "$logpath.$KEEP.gz" ]]; then
    rm -f -- "$logpath.$KEEP.gz"
  fi
  # Shift .(i).gz -> .(i+1).gz for i = KEEP-1 .. 1 so the previous newest
  # generation becomes the next-oldest after this rotation.
  for ((i = KEEP - 1; i >= 1; i--)); do
    if [[ -f "$logpath.$i.gz" ]]; then
      mv -f -- "$logpath.$i.gz" "$logpath.$((i + 1)).gz"
    fi
  done

  # Rename the live log out of the way; NEVER delete the live file. launchd
  # still holds the renamed inode until the daemon is restarted, which is why
  # the kickstart below must happen before we gzip.
  mv -f -- "$logpath" "$logpath.1"
  # Create an empty file at the original path so it always exists (for the
  # daemon-not-loaded case, and so launchd reopens it at offset 0 on restart).
  : > "$logpath"
  echo "rotated $logpath (size=${size}B, keep=${KEEP} gz)"
  return 0
}

# compress <logpath> gzips logpath.1 when present; no-op otherwise.
compress_oldest() {
  local logpath="$1"
  [[ -f "$logpath.1" ]] || return 0
  gzip -f "$logpath.1"
}

mkdir -p "$CONFIG_DIR"

rotated_daemon=0
rotated_update=0
if rotate_one "$DAEMON_LOG"; then
  rotated_daemon=1
fi
if rotate_one "$UPDATE_LOG"; then
  rotated_update=1
fi

# A renamed daemon log is still being written to by the running daemon's open
# fd until launchd restarts it. kickstart -k kills the old process and launchd
# reopens the path at offset 0, closing the old inode so it is safe to gzip.
daemon_compressable=1
if [[ "$rotated_daemon" == 1 && "$SKIP_KICKSTART" != "1" ]]; then
  if launchctl print "gui/$(id -u)/$DAEMON_LABEL" >/dev/null 2>&1; then
    if launchctl kickstart -k "gui/$(id -u)/$DAEMON_LABEL"; then
      echo "restarted $DAEMON_LABEL"
    else
      echo "warning: kickstart $DAEMON_LABEL failed; not compressing rotated log" >&2
      daemon_compressable=0
    fi
  else
    echo "daemon job not loaded; skipped restart (log dir exists)"
  fi
fi

# Compress the just-rotated generations now that nobody writes to them.
if [[ "$rotated_daemon" == 1 && "$daemon_compressable" == 1 ]]; then
  compress_oldest "$DAEMON_LOG"
fi
if [[ "$rotated_update" == 1 ]]; then
  compress_oldest "$UPDATE_LOG"
fi
