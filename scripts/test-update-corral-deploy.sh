#!/usr/bin/env bash
# test-update-corral-deploy.sh — sandbox dry-run of scripts/update-corral.sh
# deploy + hash-compare logic (issue #248).
#
# The live com.corral.corrald daemon is NOT touched. git/cargo/launchctl are
# stubbed; the daemon plist, repo, binaries, config dir and logs all live in
# throwaway temp dirs. Verifies:
#   1. release-mode drift  -> deployed <built> -> <launchd path>, restarted=yes
#   2. release-mode same   -> no copy, no kickstart, restarted=no (no forced restart)
#   3. binary-only change, no new commits -> still deployed + restarted=yes
#   4. no new commits + identical -> "up to date" only AFTER the drift check
#   5. source mode (plist == build path) -> rebuild => restarted=yes, no cp
#   6. launchctl print unavailable -> plist-file fallback; restarted=no (job not loaded)
#
# Run with one command:
#   bash scripts/test-update-corral-deploy.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf -- "$WORK"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

# --- Stubs -------------------------------------------------------------------
mkdir -p "$WORK/bin" "$WORK/repo/target/release" "$WORK/home" "$WORK/deploy"
REPO="$WORK/repo"
BUILT="$REPO/target/release/corrald"
DEPLOY1="$WORK/deploy/corrald"
DEPLOY2="$WORK/deploy2/corrald"

# git stub: script invokes `git -C <dir> ...` and plain `git ...`.
cat > "$WORK/bin/git" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    -C) shift 2 ;;
    *) args+=("$1"); shift ;;
  esac
done
case "${args[0]:-}" in
  rev-parse)
    case "${args[1]:-}" in
      --show-toplevel) echo "$GIT_STUB_TOPLEVEL" ;;
      HEAD) echo "$GIT_STUB_BEFORE" ;;
      origin/main) echo "$GIT_STUB_AFTER" ;;
      *) exit 0 ;;
    esac
    ;;
  branch) echo "main" ;;
  status) : ;;                       # clean (no porcelain output)
  fetch|pull) : ;;
  log) echo "${GIT_STUB_LOG:-deadbeef fix}" ;;
  *) : ;;
esac
STUB
chmod +x "$WORK/bin/git"

# cargo stub: optional rewrite makes the built daemon binary change (simulates
# a rebuild producing a new binary, e.g. a different toolchain).
cat > "$WORK/bin/cargo" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${CARGO_STUB_REWRITE:-0}" == "1" ]]; then
  (echo "rebuilt-$(date +%s)" >> target/release/corrald) 2>/dev/null || true
fi
exit 0
STUB
chmod +x "$WORK/bin/cargo"

# launchctl stub: print prints the `program = <path>` line launchd uses;
# kickstart records the call into $LAUNCHCTL_KICKSTART_LOG.
cat > "$WORK/bin/launchctl" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  print)
    [[ "${LAUNCHCTL_PRINT_OK:-1}" == "1" ]] || exit 1
    [[ -n "${LAUNCHCTL_PROGRAM:-}" ]] || exit 1
    echo "    program = $LAUNCHCTL_PROGRAM"
    ;;
  kickstart)
    echo "kickstart $*" >> "${LAUNCHCTL_KICKSTART_LOG:?}"
    [[ "${LAUNCHCTL_KICKSTART_FAIL:-0}" == "0" ]]
    ;;
  *) exit 0 ;;
esac
STUB
chmod +x "$WORK/bin/launchctl"

# --- Helpers ------------------------------------------------------------------
hash_of() { shasum -a 256 "$1" | awk '{print $1}'; }

# run_scenario <name> [env=value ...] -- runs update-corral.sh with stubs.
run_scenario() {
  local name="$1"; shift
  local cfg="$WORK/cfg-$name"
  rm -rf "$cfg"; mkdir -p "$cfg"
  local log="$cfg/corral-update.log"
  local kicks="$cfg/kickstarts"
  : > "$kicks"
  local env_args=()
  local kv
  for kv in "$@"; do env_args+=("$kv"); done
  env CORRAL_REPO_DIR="$REPO" CORRAL_CONFIG_DIR="$cfg" HOME="$WORK/home" \
    PATH="$WORK/bin:$PATH" \
    GIT_STUB_TOPLEVEL="$REPO" GIT_STUB_BEFORE="1111111" \
    LAUNCHCTL_KICKSTART_LOG="$kicks" \
    "${env_args[@]}" \
    bash "$SCRIPT_DIR/update-corral.sh"
  SCEN_LOG="$log"
  SCEN_KICKS="$kicks"
}

assert_log() {
  grep -qF "$1" "$SCEN_LOG" \
    || fail "expected log line '$1' in $SCEN_LOG; log: $(cat "$SCEN_LOG")"
}
refute_log() {
  if grep -qF "$1" "$SCEN_LOG"; then
    fail "unexpected log line '$1' in $SCEN_LOG"
  fi
  return 0
}
assert_kickstart_count() {
  local n
  n="$(wc -l < "$SCEN_KICKS" | tr -d ' ')"
  [[ "$n" == "$1" ]] || fail "expected $1 kickstart(s), got $n ($(cat "$SCEN_KICKS"))"
}

# --- Scenario 1: release-mode drift with new commits — deploy + restart ------
printf 'built-v1' > "$BUILT"
printf 'installed-old' > "$DEPLOY1"
run_scenario s1 \
  GIT_STUB_AFTER="2222222" GIT_STUB_LOG="deadbeef fix" \
  LAUNCHCTL_PROGRAM="$DEPLOY1"
assert_log "pulling origin/main: 1 new commit(s)"
assert_log "deployed $BUILT -> $DEPLOY1"
assert_log "restarted=yes"
assert_kickstart_count 1
[[ "$(hash_of "$DEPLOY1")" == "$(hash_of "$BUILT")" ]] \
  || fail "s1: deploy path binary != built binary"
echo "OK s1: release-mode drift deploys to launchd path + restarts"

# --- Scenario 2: release-mode identical with new commits — no restart ---------
printf 'same-v1' > "$BUILT"
printf 'same-v1' > "$DEPLOY1"
run_scenario s2 \
  GIT_STUB_AFTER="2222222" GIT_STUB_LOG="deadbeef fix" \
  LAUNCHCTL_PROGRAM="$DEPLOY1"
refute_log "deployed "
assert_log "up to date (deadbeef fix); deploy path $DEPLOY1; restarted=no"
assert_kickstart_count 0
echo "OK s2: no forced restart when shipped binary unchanged"

# --- Scenario 3: binary-only change, NO new commits — still deploys -----------
printf 'manual-rebuild-v2' > "$BUILT"
printf 'installed-old' > "$DEPLOY1"
run_scenario s3 \
  GIT_STUB_AFTER="1111111" GIT_STUB_LOG="1111111 fix" \
  LAUNCHCTL_PROGRAM="$DEPLOY1"
assert_log "no new upstream commits — checking binary drift"
assert_log "deployed $BUILT -> $DEPLOY1"
assert_log "restarted=yes"
assert_kickstart_count 1
[[ "$(hash_of "$DEPLOY1")" == "$(hash_of "$BUILT")" ]] \
  || fail "s3: deploy path binary != built binary"
echo "OK s3: binary-only change ships despite no new commits"

# --- Scenario 4: no new commits + identical — "up to date" AFTER the check ----
printf 'same-v2' > "$BUILT"
printf 'same-v2' > "$DEPLOY1"
run_scenario s4 \
  GIT_STUB_AFTER="1111111" GIT_STUB_LOG="1111111 fix" \
  LAUNCHCTL_PROGRAM="$DEPLOY1"
assert_log "no new upstream commits — checking binary drift"
assert_log "up to date (1111111 fix); deploy path $DEPLOY1; restarted=no"
assert_kickstart_count 0
echo "OK s4: no early up-to-date exit before the drift check"

# --- Scenario 5: source mode (plist == build path) — rebuild triggers restart --
printf 'src-v1' > "$BUILT"
run_scenario s5 \
  GIT_STUB_AFTER="2222222" GIT_STUB_LOG="deadbeef fix" \
  LAUNCHCTL_PROGRAM="$BUILT" CARGO_STUB_REWRITE="1"
assert_log "daemon binary changed: $BUILT"
assert_log "restarted=yes"
refute_log "deployed "
assert_kickstart_count 1
echo "OK s5: source mode restarts when rebuild changed the binary it executes"

# --- Scenario 6: launchctl print unavailable -> plist fallback, job not loaded -
mkdir -p "$WORK/home6/Library/LaunchAgents"
PLIST="$WORK/home6/Library/LaunchAgents/com.corral.corrald.plist"
cat > "$PLIST" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.corral.corrald</string>
  <key>ProgramArguments</key>
  <array>
    <string>$DEPLOY2</string>
  </array>
</dict>
</plist>
PLIST_EOF
plutil -lint "$PLIST" >/dev/null || fail "s6: generated plist is malformed"
mkdir -p "$(dirname "$DEPLOY2")"
printf 'plist-target-old' > "$DEPLOY2"
printf 'plist-built-v1' > "$BUILT"
rm -rf "$WORK/cfg-s6"; mkdir -p "$WORK/cfg-s6"
: > "$WORK/cfg-s6/kickstarts"
env CORRAL_REPO_DIR="$REPO" CORRAL_CONFIG_DIR="$WORK/cfg-s6" \
  HOME="$WORK/home6" PATH="$WORK/bin:$PATH" \
  GIT_STUB_TOPLEVEL="$REPO" GIT_STUB_BEFORE="1111111" GIT_STUB_AFTER="1111111" \
  GIT_STUB_LOG="1111111 fix" \
  LAUNCHCTL_PRINT_OK="0" \
  LAUNCHCTL_KICKSTART_LOG="$WORK/cfg-s6/kickstarts" \
  bash "$SCRIPT_DIR/update-corral.sh"
SCEN_LOG="$WORK/cfg-s6/corral-update.log"
SCEN_KICKS="$WORK/cfg-s6/kickstarts"
assert_log "deployed $BUILT -> $DEPLOY2"
assert_log "daemon job not loaded — skipped restart (run setup-corrald.sh); restarted=no"
assert_kickstart_count 0
[[ "$(hash_of "$DEPLOY2")" == "$(hash_of "$BUILT")" ]] \
  || fail "s6: plist fallback deploy path binary != built binary"
echo "OK s6: plist-file fallback deploys; no restart when job is not loaded"

echo "OK: update-corral deploy + hash-compare logic (all 6 sandbox scenarios)"
