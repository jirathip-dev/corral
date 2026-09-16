#!/usr/bin/env bash
# Hermetic regression test for the daemon launchd plist emitted by
# setup-corrald.sh. It stubs launchd and curl, then parses the generated
# plist to verify the shared PATH contract, XML escaping, executable
# resolution, secret omission, and idempotence.
#
# Run with one command:
#   bash scripts/test-daemon-launchd-env.sh
set -euo pipefail
# The quoted printf arguments below intentionally write literal shell scripts
# into the hermetic fixture; their $ expansions belong to those scripts.
# shellcheck disable=SC2016

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "SKIP: daemon launchd plist test requires macOS plutil and launchd layout"
  exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SETUP="$SCRIPT_DIR/setup-corrald.sh"
WORK="$(mktemp -d)"
trap 'rm -rf -- "$WORK"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

command -v plutil >/dev/null 2>&1 \
  || fail "plutil is required on macOS"

HOME_DIR="$WORK/home"
CONFIG_DIR="$WORK/config"
RELEASE_DIR="$WORK/release"
STUB_BIN="$WORK/stub-bin"
BREW_BIN="$WORK/homebrew&bin"
LOCAL_BIN="$HOME_DIR/.local/bin"
mkdir -p \
  "$HOME_DIR/Library/LaunchAgents" \
  "$CONFIG_DIR" \
  "$RELEASE_DIR/scripts" \
  "$STUB_BIN" \
  "$BREW_BIN" \
  "$LOCAL_BIN"

printf '#!/usr/bin/env bash\nexit 0\n' > "$BREW_BIN/brew"
# shellcheck disable=SC2016
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'if [[ "$1 ${2:-}" == "auth token" ]]; then' \
  '  printf "resolved-by-launchd-path\\n"' \
  '  exit 0' \
  'fi' \
  'exit 1' > "$BREW_BIN/gh"
chmod +x "$BREW_BIN/brew" "$BREW_BIN/gh"

printf '#!/usr/bin/env bash\nexit 0\n' > "$STUB_BIN/curl"
# shellcheck disable=SC2016
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'case "${1:-}" in' \
  '  -s|-sS|-fsS) printf "Darwin\\n" ;;' \
  '  *) printf "Darwin\\n" ;;' \
  'esac' > "$STUB_BIN/uname"
# shellcheck disable=SC2016
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "%s\\n" "$*" >> "${CORRAL_TEST_LAUNCHCTL_LOG:?}"' \
  'exit 0' > "$STUB_BIN/launchctl"
chmod +x "$STUB_BIN/curl" "$STUB_BIN/uname" "$STUB_BIN/launchctl"

# Keep the release-shaped setup path: this is the same layout that
# install-corral.sh stages before invoking setup-corrald.sh --from-release.
cp "$SETUP" "$RELEASE_DIR/scripts/setup-corrald.sh"
cp "$SCRIPT_DIR/lib-corral-update-path.sh" "$RELEASE_DIR/scripts/lib-corral-update-path.sh"
cp "$SCRIPT_DIR/update-corral.sh" "$RELEASE_DIR/scripts/update-corral.sh"
cp "$SCRIPT_DIR/rotate-corral-logs.sh" "$RELEASE_DIR/scripts/rotate-corral-logs.sh"
printf '#!/usr/bin/env bash\nexit 0\n' > "$RELEASE_DIR/corrald"
chmod +x \
  "$RELEASE_DIR/scripts/setup-corrald.sh" \
  "$RELEASE_DIR/scripts/lib-corral-update-path.sh" \
  "$RELEASE_DIR/scripts/update-corral.sh" \
  "$RELEASE_DIR/scripts/rotate-corral-logs.sh" \
  "$RELEASE_DIR/corrald"

PLIST="$HOME_DIR/Library/LaunchAgents/com.corral.corrald.plist"
UPDATE_PLIST="$HOME_DIR/Library/LaunchAgents/com.corral.corrald-update.plist"
ROTATE_PLIST="$HOME_DIR/Library/LaunchAgents/com.corral.corrald-rotate.plist"
LAUNCHCTL_LOG="$WORK/launchctl.log"

run_setup() {
  HOME="$HOME_DIR" \
    CORRAL_CONFIG_DIR="$CONFIG_DIR" \
    CORRAL_TEST_LAUNCHCTL_LOG="$LAUNCHCTL_LOG" \
    GITHUB_TOKEN="must-not-appear-in-plist" \
    PATH="$BREW_BIN:$STUB_BIN:/usr/bin:/bin" \
    bash "$RELEASE_DIR/scripts/setup-corrald.sh" \
      --from-release "$RELEASE_DIR/corrald" > "$WORK/setup.log" 2>&1
}

run_setup
[[ -f "$PLIST" ]] || fail "setup did not emit the daemon plist"
[[ -f "$UPDATE_PLIST" ]] || fail "setup did not emit the update plist"
[[ -f "$ROTATE_PLIST" ]] || fail "setup did not emit the log-rotation plist"
plutil -lint "$PLIST" >/dev/null || fail "daemon plist is not valid XML"
plutil -lint "$UPDATE_PLIST" >/dev/null || fail "update plist is not valid XML"
plutil -lint "$ROTATE_PLIST" >/dev/null || fail "log-rotation plist is not valid XML"

# #555: the fresh-install bytes of every agent are the migration target, so all
# three are snapshotted before the second (idempotent) run below.
cp "$PLIST" "$WORK/daemon-first.plist"
cp "$UPDATE_PLIST" "$WORK/update-first.plist"
cp "$ROTATE_PLIST" "$WORK/rotate-first.plist"
run_setup
cmp -s "$WORK/daemon-first.plist" "$PLIST" \
  || fail "rerunning setup changed the daemon plist"
cmp -s "$WORK/update-first.plist" "$UPDATE_PLIST" \
  || fail "rerunning setup changed the update plist"
cmp -s "$WORK/rotate-first.plist" "$ROTATE_PLIST" \
  || fail "rerunning setup changed the log-rotation plist"

daemon_path="$(plutil -extract EnvironmentVariables.PATH raw -o - "$PLIST")"
update_path="$(plutil -extract EnvironmentVariables.PATH raw -o - "$UPDATE_PLIST")"
[[ -n "$daemon_path" ]] || fail "daemon plist PATH is empty"
[[ "$daemon_path" == "$update_path" ]] \
  || fail "daemon and update plists do not share the derived PATH"
[[ "$daemon_path" == *":$BREW_BIN:"* || "$daemon_path" == "$BREW_BIN:"* || "$daemon_path" == *":$BREW_BIN" ]] \
  || fail "Homebrew bin is absent from daemon PATH: $daemon_path"
[[ "$daemon_path" == *":$LOCAL_BIN:"* || "$daemon_path" == "$LOCAL_BIN:"* || "$daemon_path" == *":$LOCAL_BIN" ]] \
  || fail "user-local bin is absent from daemon PATH: $daemon_path"

# The ampersand in BREW_BIN proves that the existing plist escaping remains in
# force; plutil's raw extraction proves launchd receives the original path.
grep -Fq '&amp;' "$PLIST" \
  || fail "daemon plist did not XML-escape the Homebrew path"
[[ "$daemon_path" == *"$BREW_BIN"* ]] \
  || fail "plutil did not recover the original Homebrew path"

resolved_gh="$(/usr/bin/env -i HOME="$HOME_DIR" PATH="$daemon_path" /bin/bash -c 'command -v gh')"
[[ "$resolved_gh" == "$BREW_BIN/gh" ]] \
  || fail "daemon PATH does not resolve gh: '$resolved_gh'"
token="$(/usr/bin/env -i HOME="$HOME_DIR" PATH="$daemon_path" /bin/bash -c 'gh auth token')"
[[ "$token" == "resolved-by-launchd-path" ]] \
  || fail "gh auth token did not execute through daemon PATH"

! grep -Fq 'GITHUB_TOKEN' "$PLIST" \
  || fail "daemon plist serialized GITHUB_TOKEN"
! grep -Fq 'must-not-appear-in-plist' "$PLIST" \
  || fail "daemon plist serialized the token value"

# ---- #555: scheduling tier (ProcessType) ------------------------------------
# The daemon's only job is to answer the phone quickly, so every emitted agent
# runs at launchd's Interactive tier. A pre-#555 install (Background) must be
# migrated by a re-run, idempotently, and the re-run must say what changed.
assert_process_type() { # $1=plist, $2=expected value
  local got
  got="$(plutil -extract ProcessType raw -o - "$1" 2>/dev/null || true)"
  [[ "$got" == "$2" ]] || fail "$(basename "$1") ProcessType is '$got' (want '$2')"
}
assert_single_process_type() { # $1=plist -> exactly one ProcessType key
  local n
  n="$(grep -c '<key>ProcessType</key>' "$1" || true)"
  [[ "$n" == "1" ]] || fail "$(basename "$1") has $n ProcessType keys (want 1)"
}

for pair in "daemon:$PLIST" "update:$UPDATE_PLIST" "rotate:$ROTATE_PLIST"; do
  name="${pair%%:*}"; plist="${pair#*:}"
  assert_process_type "$plist" Interactive
  assert_single_process_type "$plist"
done
# The second run above re-emitted an already-Interactive install: that is not a
# migration, so it must not report one ($WORK/setup.log is that run's log until
# the migration leg below overwrites it).
if grep -Fq 'migrated' "$WORK/setup.log"; then
  fail "re-running setup over an already-Interactive install reported a migration"
fi

# Migration leg: recreate every plist exactly as the pre-#555 emitter wrote it
# (the ProcessType value is its only difference) and re-run the installer.
for pair in "daemon:$PLIST" "update:$UPDATE_PLIST" "rotate:$ROTATE_PLIST"; do
  name="${pair%%:*}"; plist="${pair#*:}"
  sed 's|<key>ProcessType</key><string>Interactive</string>|<key>ProcessType</key><string>Background</string>|' \
    "$plist" > "$WORK/pre555-$name.plist"
  cp "$WORK/pre555-$name.plist" "$plist"
done
run_setup
for pair in "daemon:$PLIST" "update:$UPDATE_PLIST" "rotate:$ROTATE_PLIST"; do
  name="${pair%%:*}"; plist="${pair#*:}"
  assert_process_type "$plist" Interactive
  assert_single_process_type "$plist"
  # The migration must rewrite the ProcessType value and touch nothing else:
  # exactly two changed lines (< old, > new), both carrying that one key.
  diff_out="$(diff "$WORK/pre555-$name.plist" "$plist" || true)"
  [[ "$(printf '%s\n' "$diff_out" | grep -c '^[<>] ')" == "2" ]] \
    || fail "migrating the $name plist changed more than ProcessType:
$diff_out"
  [[ "$(printf '%s\n' "$diff_out" | grep -c '^[<>] .*<key>ProcessType</key>')" == "2" ]] \
    || fail "migrating the $name plist changed a non-ProcessType line:
$diff_out"
  # A migrated install must end up byte-identical to a fresh one.
  cmp -s "$WORK/${name}-first.plist" "$plist" \
    || fail "migrated $name plist differs from a fresh install"
done
for label in com.corral.corrald com.corral.corrald-update com.corral.corrald-rotate; do
  grep -Fq "migrated $label ProcessType: Background -> Interactive" "$WORK/setup.log" \
    || fail "the re-run did not report the $label ProcessType migration"
done

# Idempotence after the migration: a further run must not rewrite any plist
# (no duplicate keys, no growth).
run_setup
for pair in "daemon:$PLIST" "update:$UPDATE_PLIST" "rotate:$ROTATE_PLIST"; do
  name="${pair%%:*}"
  cmp -s "$WORK/${name}-first.plist" "${pair#*:}" \
    || fail "a second run over the migrated $name plist changed the file"
done

echo "OK: daemon launchd plist (shared PATH, ~/.local/bin, gh resolution, XML escaping, no token, idempotence, #555 Interactive tier + migration)"
