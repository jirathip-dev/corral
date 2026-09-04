#!/usr/bin/env bash
# setup-corrald-linux.sh — one-shot corrald setup for Linux (systemd --user).
#
# Installs/updates a prebuilt corrald binary as a hardened per-user systemd
# service. This is the Linux counterpart of setup-corrald.sh (macOS/launchd):
# no root, no RPM, no container — everything lives under $HOME, which is the
# supported path on immutable OSes such as Bazzite.
#
# It is invoked by scripts/install-corral.sh --from-release staging (which has
# already checksum-verified and swapped the release bundle into place), but is
# also safe to run standalone. Idempotent: re-running with an unchanged binary
# rewrites nothing and does NOT restart a healthy service.
#
# Restart semantics (bounded): the unit restarts a crashed daemon after 2s
# (Restart=on-failure, RestartSec=2) and systemd's start limit caps that at
# 6 starts within 90s (StartLimitIntervalSec/StartLimitBurst in the unit).
# After the limit the unit fails and stays down until the user intervenes
# (systemctl --user reset-failed corrald; systemctl --user start corrald) —
# a crash loop can never churn forever. The daemon's own herdr-socket
# reconnects use the Rust adapter's capped exponential backoff (unchanged).
#
# Usage:
#   scripts/setup-corrald-linux.sh \
#     --from-release /path/to/corrald [--bind 127.0.0.1] [--port 8474] \
#     [--changed yes|no]
#
#   --from-release <binary>   prebuilt corrald binary (must be executable)
#   --bind <ip>               loopback by default; keep it loopback on Linux —
#                             remote iOS access is Tailscale HTTPS Serve (see
#                             docs/LINUX.md), never a public/tailnet bind.
#   --port <1-65535>          daemon HTTP port (default 8474)
#   --changed yes|no          "yes" when the binary at the unit's ExecStart
#                             path changed since the service last started
#                             (the caller, install-corral.sh, knows: it swaps
#                             the release directory). Triggers a restart when
#                             the service is active; "no" leaves a healthy
#                             running service untouched (idempotent reinstall).
#
# Env overrides: CORRAL_CONFIG_DIR (same convention as the macOS scripts).
set -euo pipefail

BIND="127.0.0.1"
PORT="8474"
FROM_RELEASE=""
CHANGED="no"

i=0
args=("$@")
while [[ $i -lt ${#args[@]} ]]; do
  case "${args[$i]}" in
    --from-release)
      i=$((i+1))
      if [[ $i -ge ${#args[@]} || -z "${args[$i]}" ]]; then
        echo "!! --from-release requires a path to a prebuilt corrald" >&2; exit 2
      fi
      FROM_RELEASE="${args[$i]}"
      ;;
    --bind)
      i=$((i+1))
      if [[ $i -ge ${#args[@]} || -z "${args[$i]}" ]]; then
        echo "!! --bind requires a value (IPv4/IPv6)" >&2; exit 2
      fi
      BIND="${args[$i]}"
      # IPv4 dotted-quad or IPv6 only — the daemon parses --bind as IpAddr and
      # refuses anything else, so reject a typo here before it crash-loops.
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
    --changed)
      i=$((i+1))
      if [[ $i -ge ${#args[@]} || -z "${args[$i]}" ]]; then
        echo "!! --changed requires yes or no" >&2; exit 2
      fi
      case "${args[$i]}" in
        yes|no) CHANGED="${args[$i]}" ;;
        *) echo "!! --changed requires yes or no (got: ${args[$i]})" >&2; exit 2 ;;
      esac
      ;;
    -h|--help) sed -n '2,39p' "$0"; exit 0 ;;
    *) echo "unknown arg: ${args[$i]}" >&2; exit 2 ;;
  esac
  i=$((i+1))
done

[[ -n "$FROM_RELEASE" ]] || { echo "!! --from-release is required" >&2; exit 2; }
FROM_RELEASE="$(cd "$(dirname "$FROM_RELEASE")" && pwd)/$(basename "$FROM_RELEASE")"
[[ -f "$FROM_RELEASE" && -x "$FROM_RELEASE" ]] || {
  echo "!! --from-release binary is missing or not executable: $FROM_RELEASE" >&2
  exit 2
}

# The daemon execs `git`/`gh` for the repo/GitHub planes (both tolerate their
# absence; the planes degrade, the daemon keeps serving). A systemd user
# manager does not inherit a login-shell PATH, so bake a deterministic one
# covering the standard Bazzite/usr locations (rpm-ostree layered tools).
DAEMON_PATH="/usr/local/bin:/usr/bin:/bin"

CONFIG_DIR="${CORRAL_CONFIG_DIR:-$HOME/.config/corral}"
# systemd user units live under $XDG_CONFIG_HOME/systemd/user (default
# ~/.config/systemd/user). XDG_CONFIG_HOME is honored here, not hardcoded.
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
UNIT="$UNIT_DIR/corrald.service"
# systemctl verbs take the full unit name (systemd also accepts the bare
# "corrald", but "corrald.service" keeps logs and assertions unambiguous).
LABEL="corrald.service"
# Bazzite's local herdr socket — same default the daemon itself uses and the
# macOS launchd plist wires (see scripts/setup-corrald.sh).
HERDR_SOCKET="$HOME/.config/herdr/herdr.sock"

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "!! required command not found: $1" >&2
    exit 1
  }
}
require_command systemctl
require_command curl

mkdir -p "$UNIT_DIR" "$CONFIG_DIR"

render_unit() {
  # Comment lines start with '#'. The daemon binary, socket, bind and port are
  # literal at install time (like the macOS plist); nothing here is user input
  # beyond the validated flags above.
  cat <<UNIT_EOF
# corrald systemd user unit — written by setup-corrald-linux.sh.
# Managed by scripts/install-corral.sh (install/update/uninstall). Manual
# edits are overwritten on the next install run.
[Unit]
Description=Corral host daemon (corrald)
# Bounded restart: at most 6 starts per 90s window (systemd start limit);
# past that the unit fails and stays down until a manual reset-failed/start.
StartLimitIntervalSec=90
StartLimitBurst=6

[Service]
Type=simple
ExecStart=$FROM_RELEASE --socket $HERDR_SOCKET --bind $BIND --port $PORT
Restart=on-failure
RestartSec=2
# The daemon shells out to git/gh for the repo/GitHub planes; give the user
# service a deterministic PATH instead of the manager's minimal default.
Environment=PATH=$DAEMON_PATH
Environment=CORRAL_CONFIG_DIR=$CONFIG_DIR
# Hardening: the daemon needs only its own files under $HOME (config), the
# herdr socket, loopback HTTP, and outbound HTTPS for the gh plane. It must
# keep writing $CONFIG_DIR, so ProtectHome/ProtectSystem are deliberately not
# used; NoNewPrivileges + PrivateTmp + the seccomp-ish flags below stay valid
# for unprivileged user managers.
NoNewPrivileges=true
PrivateTmp=true
RestrictSUIDSGID=true
ProtectClock=true
UMask=0077
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=default.target
UNIT_EOF
}

need_write=0
if [[ -f "$UNIT" ]] && cmp -s "$UNIT" <(render_unit); then
  need_write=0
else
  need_write=1
fi

if [[ "$need_write" == "1" ]]; then
  tmp_unit="$(mktemp "$UNIT.XXXXXX")"
  render_unit > "$tmp_unit"
  chmod 0644 "$tmp_unit"
  mv -f "$tmp_unit" "$UNIT"
  echo ">> Wrote systemd user unit: $UNIT"
else
  echo ">> systemd user unit unchanged: $UNIT"
fi

# Apply the (possibly rewritten) unit and learn the current service state.
# is-active/is-enabled exit non-zero for inactive/disabled/absent units.
systemctl --user daemon-reload
active=0
enabled=0
if systemctl --user is-active --quiet "$LABEL" 2>/dev/null; then
  active=1
fi
if systemctl --user is-enabled --quiet "$LABEL" 2>/dev/null; then
  enabled=1
fi

if [[ "$CHANGED" == "yes" && "$active" == "1" ]]; then
  echo ">> Restarting $LABEL (installed binary changed)"
  systemctl --user restart "$LABEL" \
    || { echo "!! systemctl --user restart $LABEL failed (see: journalctl --user -u $LABEL)" >&2; exit 1; }
  restarted="restarted=yes (binary changed)"
elif [[ "$active" == "1" ]]; then
  restarted="restarted=no (already running the current binary)"
elif [[ "$enabled" == "1" ]]; then
  echo ">> Starting $LABEL"
  systemctl --user start "$LABEL" \
    || { echo "!! systemctl --user start $LABEL failed (see: journalctl --user -u $LABEL)" >&2; exit 1; }
  restarted="started (unit enabled, was stopped)"
else
  echo ">> Enabling and starting $LABEL"
  systemctl --user enable --now "$LABEL" \
    || { echo "!! systemctl --user enable --now $LABEL failed (see: journalctl --user -u $LABEL)" >&2; exit 1; }
  restarted="enabled and started"
fi
echo "   $restarted"

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
    ok=1
    break
  fi
  sleep 1
done
if [[ "$ok" == "1" ]]; then
  echo "   ✓ corrald is UP at $URL"
else
  # Stop the unit: the installed binary did not come up healthy. The caller
  # (install-corral.sh) rolls the release directory back to the previous
  # version; leaving the unit stopped is deterministic and documented.
  systemctl --user stop "$LABEL" 2>/dev/null || true
  echo "   ✗ could not reach $URL — service stopped; previous release was restored by the installer" >&2
  echo "     diagnose with: journalctl --user -u $LABEL" >&2
  exit 1
fi

echo
echo ">> Installed prebuilt corrald (systemd --user)"
echo "   service: systemctl --user status $LABEL"
echo "   logs:    journalctl --user -u $LABEL"
echo "   config:  $CONFIG_DIR"
