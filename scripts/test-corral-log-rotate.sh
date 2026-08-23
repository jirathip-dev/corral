#!/usr/bin/env bash
# Small, hermetic test for scripts/rotate-corral-logs.sh. Verifies a log under
# the cap is skipped, an over-cap log is rotated to a gzipped generation, KEEP
# generations are retained with the oldest dropped, the live log file is never
# deleted, corral-update.log gets the same treatment, and the launchd restart
# step is skipped via CORRAL_ROTATE_SKIP_KICKSTART (no real launchctl, no real
# daemon, nothing restarted from this test).
#
# Run with one command:
#   bash scripts/test-corral-log-rotate.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROTATOR="$SCRIPT_DIR/rotate-corral-logs.sh"
WORK="$(mktemp -d)"
trap 'rm -rf -- "$WORK"' EXIT

CONFIG="$WORK/config"
STUB_BIN="$WORK/bin"
mkdir -p "$CONFIG" "$STUB_BIN"

# A fake launchctl that records an invocation and bails; if the rotator honors
# CORRAL_ROTATE_SKIP_KICKSTART it must never be called.
cat > "$STUB_BIN/launchctl" <<EOF
#!/usr/bin/env bash
echo invoked > "$STUB_BIN/launchctl.called"
exit 99
EOF
chmod +x "$STUB_BIN/launchctl"

# Shrink the cap and force the kickstart skip so the test is deterministic and
# never touches the host's launchd domain.
export CORRAL_CONFIG_DIR="$CONFIG"
export CORRAL_LOG_MAX_BYTES=100
export CORRAL_LOG_KEEP=2
export CORRAL_ROTATE_SKIP_KICKSTART=1
export PATH="$STUB_BIN:$PATH"

fail() { echo "FAIL: $*" >&2; exit 1; }

make_log() { dd if=/dev/zero of="$1" bs="$2" count=1 2>/dev/null; }
bytes() { wc -c < "$1" | tr -d ' '; }

DAEMON_LOG="$CONFIG/corrald-launchd.log"
UPDATE_LOG="$CONFIG/corral-update.log"

# 1. Under-cap daemon log -> no rotation, live file untouched, no generations.
make_log "$DAEMON_LOG" 50
bash "$ROTATOR"
[[ -f "$DAEMON_LOG" ]] || fail "live daemon log missing after no-op"
[[ "$(bytes "$DAEMON_LOG")" == "50" ]] || fail "under-cap daemon log changed size"
[[ ! -e "$DAEMON_LOG.1" && ! -e "$DAEMON_LOG.1.gz" ]] || fail "under-cap daemon log was rotated"
[[ ! -e "$UPDATE_LOG.1" && ! -e "$UPDATE_LOG.1.gz" ]] || fail "missing update log produced a generation"
[[ ! -e "$STUB_BIN/launchctl.called" ]] || fail "launchctl was called on a no-op"

# 2. Over-cap daemon log -> rotated to .1.gz, live file recreated empty.
make_log "$DAEMON_LOG" 200
bash "$ROTATOR"
[[ -f "$DAEMON_LOG" ]] || fail "live daemon log missing after rotation (must never be deleted)"
[[ "$(bytes "$DAEMON_LOG")" == "0" ]] || fail "live daemon log not empty after rotation"
[[ -f "$DAEMON_LOG.1.gz" ]] || fail "daemon .1.gz not created"
[[ ! -e "$DAEMON_LOG.1" ]] || fail "daemon .1 should be gzipped away"
[[ "$(gzip -dc "$DAEMON_LOG.1.gz" | wc -c | tr -d ' ')" == "200" ]] || fail "daemon .1.gz does not preserve content"
[[ ! -e "$STUB_BIN/launchctl.called" ]] || fail "launchctl was called despite SKIP_KICKSTART=1"

# 3. Second rotation -> two generations kept, .1.gz/.2.gz both present.
make_log "$DAEMON_LOG" 200
bash "$ROTATOR"
[[ -f "$DAEMON_LOG.1.gz" && -f "$DAEMON_LOG.2.gz" ]] || fail "two generations not kept (.1.gz/.2.gz)"
[[ -f "$DAEMON_LOG" ]] || fail "live daemon log missing after second rotation"

# 4. Update log over cap -> same rotation; both logs independent.
make_log "$UPDATE_LOG" 200
bash "$ROTATOR"
[[ -f "$UPDATE_LOG.1.gz" ]] || fail "corral-update.log was not rotated"
[[ -f "$DAEMON_LOG.1.gz" ]] || fail "daemon generation lost after update-log rotation"

# 5. Third daemon rotation -> oldest (.2.gz) dropped, only .1.gz + .2.gz remain.
make_log "$DAEMON_LOG" 200
bash "$ROTATOR"
[[ -f "$DAEMON_LOG.1.gz" && -f "$DAEMON_LOG.2.gz" ]] || fail "KEEP=2 generations not retained"
[[ ! -e "$DAEMON_LOG.3.gz" ]] || fail "generation beyond KEEP was not dropped"
[[ -f "$DAEMON_LOG" ]] || fail "live daemon log missing after third rotation"

# 6. Live file never deleted across all runs; a final under-cap run stays put.
make_log "$DAEMON_LOG" 50
bash "$ROTATOR"
[[ -f "$DAEMON_LOG" && "$(bytes "$DAEMON_LOG")" == "50" ]] || fail "under-cap daemon log altered after triage"
[[ -f "$DAEMON_LOG.1.gz" ]] || fail "existing generations disappeared unexpectedly"

echo "OK: corral log rotation (skip-under-cap, rotate-over-cap, generations, live-file, update-log, kickstart-skip)"
