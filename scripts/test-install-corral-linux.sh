#!/usr/bin/env bash
# test-install-corral-linux.sh — hermetic unit tests for the Linux/systemd
# branch of scripts/install-corral.sh (G4: fresh install, idempotent
# reinstall/update, checksum failure, unhealthy service, uninstall with
# config preserved, no-root/home-path behavior).
#
# Runs the REAL installer against fixture release bundles served over
# file:// URLs, with a fake $HOME and PATH stubs for systemctl/curl/uname.
# No network, no root, no real systemd, no RPM, no container. Runs on any
# host with bash (macOS host and ubuntu CI both); when systemd-analyze is
# present the generated unit file is additionally verified.
#
# Run with one command:
#   bash scripts/test-install-corral-linux.sh
set -euo pipefail
# The printf fixtures below intentionally write literal shell scripts whose $
# expansions belong to those scripts.
# shellcheck disable=SC2016

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALLER="$SCRIPT_DIR/install-corral.sh"
WORK="$(mktemp -d)"
trap 'rm -rf -- "$WORK"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "ok: $*"; }

HOME_DIR="$WORK/home"
FAKE_HOME="$HOME_DIR/user"          # $HOME the installer sees (sandboxed)
CONFIG_DIR="$WORK/config-corral"    # $CORRAL_CONFIG_DIR (config/keys sandbox)
INSTALL_ROOT="$FAKE_HOME/.local/share/corral"
UNIT_DIR="$FAKE_HOME/.config/systemd/user"
UNIT_FILE="$UNIT_DIR/corrald.service"
STUB_BIN="$WORK/stub-bin"
FIXTURES="$WORK/fixtures"
LOG_DIR="$WORK/log"
mkdir -p "$FAKE_HOME" "$CONFIG_DIR" "$STUB_BIN" "$FIXTURES" "$LOG_DIR" "$UNIT_DIR"

# ---- PATH stubs -------------------------------------------------------------
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'case "${1:-}" in' \
  '  -s) if [[ -n "${CORRAL_FAKE_UNAME_S:-}" ]]; then echo "$CORRAL_FAKE_UNAME_S"; exit 0; fi ;;' \
  '  -m) if [[ -n "${CORRAL_FAKE_UNAME_M:-}" ]]; then echo "$CORRAL_FAKE_UNAME_M"; exit 0; fi ;;' \
  'esac' \
  'exec /usr/bin/uname "$@"' > "$STUB_BIN/uname"

# systemctl stub: logs every invocation; is-active/is-enabled answer from
# scenario knobs; every other verb (daemon-reload/enable/start/restart/stop)
# succeeds. A real systemd user manager is never required.
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "%s\n" "$*" >> "${CORRAL_TEST_SYSTEMCTL_LOG:?}"' \
  'for a in "$@"; do' \
  '  case "$a" in' \
  '    is-active)  [[ "${CORRAL_TEST_UNIT_ACTIVE:-0}" == "1" ]] && exit 0 || exit 1 ;;' \
  '    is-enabled) [[ "${CORRAL_TEST_UNIT_ENABLED:-0}" == "1" ]] && exit 0 || exit 1 ;;' \
  '  esac' \
  'done' \
  'exit 0' > "$STUB_BIN/systemctl"

# curl stub: healthz probes are answered locally (never a real loopback
# daemon); everything else (the file:// bundle + .sha256 downloads) is passed
# through to the real curl.
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'if [[ "$*" == *"/healthz"* ]]; then' \
  '  if [[ "${CORRAL_TEST_HEALTH_FAIL:-0}" == "1" ]]; then' \
  '    echo "curl: healthz connection refused (fixture)" >&2' \
  '    exit 7' \
  '  fi' \
  '  printf "healthz probe\n" >> "${CORRAL_TEST_CURL_LOG:?}"' \
  '  echo ok' \
  '  exit 0' \
  'fi' \
  'exec /usr/bin/curl "$@"' > "$STUB_BIN/curl"
chmod +x "$STUB_BIN/uname" "$STUB_BIN/systemctl" "$STUB_BIN/curl"

# ---- hash + fixture helpers -------------------------------------------------
sha256_hex() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    fail "no sha256 tool for fixtures"
  fi
}

make_bundle() { # $1=version marker text; $2=bundle path (must not exist yet)
  local marker="$1" bundle="$2" src
  src="$(mktemp -d "$WORK/bundle-src.XXXXXX")"
  mkdir -p "$src/scripts"
  # ELF magic prefix keeps the installer's never-relabel-a-macOS-artifact
  # check honest; the marker text differentiates bundle generations (hash).
  # "$bundle.bin" is kept for hash assertions against the INSTALLED binary.
  printf '\177ELF' > "$src/corrald"
  printf 'corrald fixture binary: %s\n' "$marker" >> "$src/corrald"
  chmod +x "$src/corrald"
  cp "$src/corrald" "$bundle.bin"
  cp "$SCRIPT_DIR/setup-corrald.sh" "$src/scripts/setup-corrald.sh"
  cp "$SCRIPT_DIR/setup-corrald-linux.sh" "$src/scripts/setup-corrald-linux.sh"
  cp "$SCRIPT_DIR/install-corral.sh" "$src/scripts/install-corral.sh"
  cp "$SCRIPT_DIR/update-corral.sh" "$src/scripts/update-corral.sh"
  cp "$SCRIPT_DIR/lib-corral-update-path.sh" "$src/scripts/lib-corral-update-path.sh"
  cp "$SCRIPT_DIR/rotate-corral-logs.sh" "$src/scripts/rotate-corral-logs.sh"
  tar -C "$src" -czf "$bundle" corrald scripts
  sha256_hex "$bundle" > "$bundle.sha256"
  rm -rf -- "$src"
}

# Per-scenario knobs (read by run_install through the env block).
TEST_UNIT_ACTIVE=0
TEST_UNIT_ENABLED=0
TEST_HEALTH_FAIL=0
SCEN_LOG=""
SYSTEMCTL_LOG=""
CURL_LOG=""

run_install() { # extra args go to install-corral.sh
  local name="$1"; shift
  SCEN_LOG="$LOG_DIR/$name.log"
  SYSTEMCTL_LOG="$LOG_DIR/$name-systemctl.log"
  CURL_LOG="$LOG_DIR/$name-curl.log"
  # env -i: scrub ambient XDG_*/etc. so the fake $HOME is the ONLY home the
  # installer/helper can see (a macOS host exports XDG_CONFIG_HOME=/Users/...,
  # which would make the helper write the unit into the real user config).
  env -i \
    HOME="$FAKE_HOME" \
    PATH="$STUB_BIN:/usr/bin:/bin" \
    CORRAL_CONFIG_DIR="$CONFIG_DIR" \
    CORRAL_INSTALL_DIR="$INSTALL_ROOT" \
    CORRAL_FAKE_UNAME_S="Linux" \
    CORRAL_FAKE_UNAME_M="x86_64" \
    CORRAL_TEST_SYSTEMCTL_LOG="$SYSTEMCTL_LOG" \
    CORRAL_TEST_CURL_LOG="$CURL_LOG" \
    CORRAL_TEST_UNIT_ACTIVE="$TEST_UNIT_ACTIVE" \
    CORRAL_TEST_UNIT_ENABLED="$TEST_UNIT_ENABLED" \
    CORRAL_TEST_HEALTH_FAIL="$TEST_HEALTH_FAIL" \
    bash "$INSTALLER" "$@" >"$SCEN_LOG" 2>&1
  return $?
}

assert_log_has() { # $1=log file, $2=substring
  grep -Fq -- "$2" "$1" || fail "log $1 missing: '$2'"
}
refute_log_has() {
  if grep -Fq -- "$2" "$1"; then fail "log $1 unexpectedly has: '$2'"; fi
}
assert_unit_has() {
  grep -Fq -- "$1" "$UNIT_FILE" || fail "unit file missing: '$1'"
}
refute_unit_has() {
  if grep -Fq -- "$1" "$UNIT_FILE"; then fail "unit file unexpectedly has: '$1'"; fi
}

# ---- fixture release bundles ------------------------------------------------
BUNDLE_URL_BASE="file://$FIXTURES/corral-v0.1.0-linux-x86_64.tar.gz"
BUNDLE="$FIXTURES/corral-v0.1.0-linux-x86_64.tar.gz"
make_bundle "v1.0" "$BUNDLE"
V1_HASH="$(sha256_hex "$BUNDLE.bin")"

# =============================================================================
echo "== scenario: fresh install (rootless, under \$HOME) =="
TEST_UNIT_ACTIVE=0 TEST_UNIT_ENABLED=0 TEST_HEALTH_FAIL=0
run_install fresh --url "$BUNDLE_URL_BASE" || fail "fresh install exited $?"
assert_log_has "$SCEN_LOG" "SHA-256 verified"
assert_log_has "$SCEN_LOG" "corrald is UP"
[[ -x "$INSTALL_ROOT/release/corrald" ]] || fail "fresh install: release binary missing"
[[ "$(sha256_hex "$INSTALL_ROOT/release/corrald")" == "$V1_HASH" ]] \
  || fail "fresh install: installed binary hash mismatch"
[[ -f "$UNIT_FILE" ]] || fail "fresh install: unit file not written"
assert_unit_has "ExecStart=$INSTALL_ROOT/release/corrald --socket $FAKE_HOME/.config/herdr/herdr.sock --bind 127.0.0.1 --port 8474"
assert_unit_has "Restart=on-failure"
assert_unit_has "RestartSec=2"
assert_unit_has "StartLimitIntervalSec=90"
assert_unit_has "StartLimitBurst=6"
assert_unit_has "NoNewPrivileges=true"
assert_unit_has "PrivateTmp=true"
assert_unit_has "WantedBy=default.target"
refute_unit_has "ProtectHome="
refute_unit_has "ProtectSystem="
refute_log_has "$SYSTEMCTL_LOG" "restart"
assert_log_has "$SYSTEMCTL_LOG" "enable --now corrald.service"
assert_log_has "$CURL_LOG" "healthz probe"
# install under a plain user's $HOME must never need root
[[ "$(id -u)" -ne 0 ]] || fail "this test suite must run as a non-root user"
ok "fresh install: unit content + enable --now + health probe + v1 binary"

if command -v systemd-analyze >/dev/null 2>&1; then
  systemd-analyze verify "$UNIT_FILE" >"$LOG_DIR/systemd-analyze.log" 2>&1 \
    || { cat "$LOG_DIR/systemd-analyze.log" >&2; fail "systemd-analyze verify failed"; }
  ok "fresh install: systemd-analyze verify PASS"
else
  echo "skip: systemd-analyze not present on this host (ubuntu CI runs it)"
fi

# =============================================================================
echo "== scenario: idempotent reinstall (same bundle, service active) =="
TEST_UNIT_ACTIVE=1 TEST_UNIT_ENABLED=1 TEST_HEALTH_FAIL=0
run_install reinstall --url "$BUNDLE_URL_BASE" || fail "reinstall exited $?"
[[ "$(sha256_hex "$INSTALL_ROOT/release/corrald")" == "$V1_HASH" ]] \
  || fail "reinstall changed the binary"
refute_log_has "$SYSTEMCTL_LOG" "restart corrald.service"
refute_log_has "$SYSTEMCTL_LOG" "start corrald.service"
refute_log_has "$SYSTEMCTL_LOG" "enable --now corrald.service"
refute_log_has "$SYSTEMCTL_LOG" "stop corrald.service"
assert_log_has "$SCEN_LOG" "systemd user unit unchanged"
assert_log_has "$SCEN_LOG" "corrald is UP"
ok "reinstall: no restart/rewrite when binary and unit are unchanged"

# =============================================================================
echo "== scenario: update (changed binary, service active -> restart) =="
make_bundle "v2.0" "$BUNDLE.v2"
V2_HASH="$(sha256_hex "$BUNDLE.v2.bin")"
TEST_UNIT_ACTIVE=1 TEST_UNIT_ENABLED=1 TEST_HEALTH_FAIL=0
run_install update --url "file://$BUNDLE.v2" || fail "update exited $?"
[[ "$(sha256_hex "$INSTALL_ROOT/release/corrald")" == "$V2_HASH" ]] \
  || fail "update did not install the v2 binary"
assert_log_has "$SYSTEMCTL_LOG" "restart corrald.service"
assert_log_has "$SCEN_LOG" "Restarting corrald.service (installed binary changed)"
[[ ! -e "$INSTALL_ROOT/release.previous" ]] || fail "update left release.previous behind"
ok "update: v2 binary installed, service restarted, .previous cleaned"

# =============================================================================
echo "== scenario: checksum failure exits non-zero, no half-install =="
BAD_HOME="$WORK/home-bad"
mkdir -p "$BAD_HOME"
BAD_BUNDLE="$FIXTURES/corral-v0.1.0-linux-x86_64-bad.tar.gz"
make_bundle "v1.0-bad" "$BAD_BUNDLE"
printf '0000000000000000000000000000000000000000000000000000000000000000\n' > "$BAD_BUNDLE.sha256"
set +e
env -i \
  HOME="$BAD_HOME" \
  PATH="$STUB_BIN:/usr/bin:/bin" \
  CORRAL_CONFIG_DIR="$WORK/config-bad" \
  CORRAL_INSTALL_DIR="$BAD_HOME/.local/share/corral" \
  CORRAL_FAKE_UNAME_S="Linux" CORRAL_FAKE_UNAME_M="x86_64" \
  bash "$INSTALLER" --url "file://$BAD_BUNDLE" >"$LOG_DIR/checksum.log" 2>&1
rc=$?
set -e
[[ "$rc" -ne 0 ]] || fail "checksum failure exited 0"
assert_log_has "$LOG_DIR/checksum.log" "SHA-256 mismatch — refusing to install"
[[ ! -e "$BAD_HOME/.local/share/corral" ]] \
  || fail "checksum failure left an install root behind (half-install)"
ok "checksum failure: exit $rc, no install root created"

# =============================================================================
echo "== scenario: unhealthy service -> installer exits non-zero, rolls back =="
UNHEALTHY_HOME="$WORK/home-unhealthy"
mkdir -p "$UNHEALTHY_HOME"
TEST_UNIT_ACTIVE=0 TEST_UNIT_ENABLED=0 TEST_HEALTH_FAIL=1
set +e
env -i \
  HOME="$UNHEALTHY_HOME" \
  PATH="$STUB_BIN:/usr/bin:/bin" \
  CORRAL_CONFIG_DIR="$WORK/config-unhealthy" \
  CORRAL_INSTALL_DIR="$UNHEALTHY_HOME/.local/share/corral" \
  CORRAL_FAKE_UNAME_S="Linux" CORRAL_FAKE_UNAME_M="x86_64" \
  CORRAL_TEST_SYSTEMCTL_LOG="$LOG_DIR/unhealthy-systemctl.log" \
  CORRAL_TEST_CURL_LOG="$LOG_DIR/unhealthy-curl.log" \
  CORRAL_TEST_UNIT_ACTIVE="$TEST_UNIT_ACTIVE" \
  CORRAL_TEST_UNIT_ENABLED="$TEST_UNIT_ENABLED" \
  CORRAL_TEST_HEALTH_FAIL="$TEST_HEALTH_FAIL" \
  bash "$INSTALLER" --url "$BUNDLE_URL_BASE" >"$LOG_DIR/unhealthy.log" 2>&1
rc=$?
set -e
[[ "$rc" -ne 0 ]] || fail "unhealthy-service install exited 0"
assert_log_has "$LOG_DIR/unhealthy.log" "could not reach"
assert_log_has "$LOG_DIR/unhealthy-systemctl.log" "stop corrald.service"
[[ ! -e "$UNHEALTHY_HOME/.local/share/corral/release" ]] \
  || fail "unhealthy-service install left a release behind (no rollback)"
ok "unhealthy service: exit $rc, service stopped, release rolled back"

echo "== scenario: recovery after unhealthy install (health ok) =="
TEST_UNIT_ACTIVE=0 TEST_UNIT_ENABLED=1 TEST_HEALTH_FAIL=0
set +e
env -i \
  HOME="$UNHEALTHY_HOME" \
  PATH="$STUB_BIN:/usr/bin:/bin" \
  CORRAL_CONFIG_DIR="$WORK/config-unhealthy" \
  CORRAL_INSTALL_DIR="$UNHEALTHY_HOME/.local/share/corral" \
  CORRAL_FAKE_UNAME_S="Linux" CORRAL_FAKE_UNAME_M="x86_64" \
  CORRAL_TEST_SYSTEMCTL_LOG="$LOG_DIR/recover-systemctl.log" \
  CORRAL_TEST_CURL_LOG="$LOG_DIR/recover-curl.log" \
  CORRAL_TEST_UNIT_ACTIVE="$TEST_UNIT_ACTIVE" \
  CORRAL_TEST_UNIT_ENABLED="$TEST_UNIT_ENABLED" \
  CORRAL_TEST_HEALTH_FAIL="$TEST_HEALTH_FAIL" \
  bash "$INSTALLER" --url "$BUNDLE_URL_BASE" >"$LOG_DIR/recover.log" 2>&1
rc=$?
set -e
[[ "$rc" -eq 0 ]] || { cat "$LOG_DIR/recover.log" >&2; fail "recovery install exited $rc"; }
[[ -x "$UNHEALTHY_HOME/.local/share/corral/release/corrald" ]] \
  || fail "recovery install left no release"
assert_log_has "$LOG_DIR/recover-systemctl.log" "start corrald.service"
ok "recovery: re-run installs cleanly and starts the enabled unit"

# =============================================================================
echo "== scenario: uninstall removes service + release, preserves config =="
MARKER="$CONFIG_DIR/keep-me.txt"
printf 'registry/keys live here\n' > "$MARKER"
TEST_UNIT_ACTIVE=1 TEST_UNIT_ENABLED=1 TEST_HEALTH_FAIL=0
run_install uninstall --uninstall || fail "uninstall exited $?"
[[ ! -f "$UNIT_FILE" ]] || fail "uninstall left the unit file behind"
[[ ! -e "$INSTALL_ROOT/release" ]] || fail "uninstall left the release behind"
[[ ! -e "$INSTALL_ROOT/release.previous" ]] || fail "uninstall left release.previous behind"
[[ -f "$MARKER" ]] || fail "uninstall removed $CONFIG_DIR (config must be preserved)"
assert_log_has "$SYSTEMCTL_LOG" "disable --now corrald.service"
assert_log_has "$SCEN_LOG" "Uninstall complete. Config/keys kept"
ok "uninstall: unit+release removed, config/keys preserved, disable --now logged"

# =============================================================================
echo "== scenario: no-root/home-path behavior =="
# (a) install root outside $HOME is refused before anything is downloaded.
OUT_HOME="$WORK/outside-home/corral"
TEST_UNIT_ACTIVE=0 TEST_UNIT_ENABLED=0 TEST_HEALTH_FAIL=0
set +e
env -i \
  HOME="$FAKE_HOME" \
  PATH="$STUB_BIN:/usr/bin:/bin" \
  CORRAL_CONFIG_DIR="$CONFIG_DIR" \
  CORRAL_INSTALL_DIR="$OUT_HOME" \
  CORRAL_FAKE_UNAME_S="Linux" CORRAL_FAKE_UNAME_M="x86_64" \
  bash "$INSTALLER" --url "$BUNDLE_URL_BASE" >"$LOG_DIR/nohome.log" 2>&1
rc=$?
set -e
[[ "$rc" -eq 2 ]] || fail "install root outside \$HOME exited $rc (want 2)"
assert_log_has "$LOG_DIR/nohome.log" "must live under \$HOME"
[[ ! -e "$OUT_HOME" ]] || fail "install root outside \$HOME was created"

# (b) non-x86_64 Linux is refused.
set +e
env -i \
  HOME="$FAKE_HOME" \
  PATH="$STUB_BIN:/usr/bin:/bin" \
  CORRAL_CONFIG_DIR="$CONFIG_DIR" \
  CORRAL_INSTALL_DIR="$INSTALL_ROOT" \
  CORRAL_FAKE_UNAME_S="Linux" CORRAL_FAKE_UNAME_M="aarch64" \
  bash "$INSTALLER" --url "$BUNDLE_URL_BASE" >"$LOG_DIR/noarch.log" 2>&1
rc=$?
set -e
[[ "$rc" -eq 2 ]] || fail "non-x86_64 Linux exited $rc (want 2)"
assert_log_has "$LOG_DIR/noarch.log" "x86_64 only"

# (c) unsupported OS is refused (self-test stays platform-neutral).
set +e
env -i \
  HOME="$FAKE_HOME" \
  PATH="$STUB_BIN:/usr/bin:/bin" \
  CORRAL_FAKE_UNAME_S="FreeBSD" CORRAL_FAKE_UNAME_M="x86_64" \
  bash "$INSTALLER" --self-test >"$LOG_DIR/freebsd-selftest.log" 2>&1
rc=$?
set -e
[[ "$rc" -eq 0 ]] || fail "--self-test should stay platform-neutral (exit $rc)"
set +e
env -i \
  HOME="$FAKE_HOME" \
  PATH="$STUB_BIN:/usr/bin:/bin" \
  CORRAL_FAKE_UNAME_S="FreeBSD" CORRAL_FAKE_UNAME_M="x86_64" \
  bash "$INSTALLER" --url "$BUNDLE_URL_BASE" >"$LOG_DIR/noplat.log" 2>&1
rc=$?
set -e
[[ "$rc" -eq 2 ]] || fail "unsupported platform exited $rc (want 2)"
assert_log_has "$LOG_DIR/noplat.log" "unsupported platform"
ok "no-root/home-path: outside-\$HOME refused, non-x86_64 refused, unsupported OS refused"

echo
echo "OK: install-corral Linux/systemd scenarios passed ($(ls "$LOG_DIR"/*.log | wc -l | tr -d ' ') logs in $LOG_DIR)"
