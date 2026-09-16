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
# $2 defaults to the fresh-install unit; the #555 migration scenario below
# passes its own unit file explicitly.
assert_unit_has() { # $1=substring, $2=unit file (default $UNIT_FILE)
  grep -Fq -- "$1" "${2:-$UNIT_FILE}" || fail "unit file ${2:-$UNIT_FILE} missing: '$1'"
}
refute_unit_has() {
  if grep -Fq -- "$1" "${2:-$UNIT_FILE}"; then fail "unit file ${2:-$UNIT_FILE} unexpectedly has: '$1'"; fi
}
# #555: the [Service] section must be exactly the pre-#555 text plus the two
# scheduling directives — that is what keeps the hardening block and every
# other directive byte-identical, and it is compared verbatim (not key by key).
# $1 = the expected ExecStart line, $2 = unit file.
assert_service_block() {
  local exec_start="$1" unit="$2" home="${3:-$FAKE_HOME}" expected actual block
  # A heredoc inside $(...) does not parse under bash 3.2 (macOS /bin/bash), so
  # the expected text is read into a variable directly. The emitter expands
  # $HOME and $CONFIG_DIR inside its own heredoc, hence the placeholders.
  IFS= read -r -d '' block <<'EOF' || true
[Service]
Type=simple
@EXEC_START@
# #555: answer under load — Nice/CPUWeight raise the daemon's scheduling weight
# and CPU share while the rest of the host is saturated (see the header).
Nice=-5
CPUWeight=500
Restart=on-failure
RestartSec=2
# The daemon shells out to git/gh for the repo/GitHub planes; give the user
# service a deterministic PATH instead of the manager's minimal default.
Environment=PATH=/usr/local/bin:/usr/bin:/bin
Environment=CORRAL_CONFIG_DIR=@CONFIG_DIR@
# Hardening: the daemon needs only its own files under @HOME@ (config), the
# herdr socket, loopback HTTP, and outbound HTTPS for the gh plane. It must
# keep writing @CONFIG_DIR@, so ProtectHome/ProtectSystem are deliberately not
# used; NoNewPrivileges + PrivateTmp + the seccomp-ish flags below stay valid
# for unprivileged user managers.
NoNewPrivileges=true
PrivateTmp=true
RestrictSUIDSGID=true
ProtectClock=true
UMask=0077
StandardOutput=journal
StandardError=journal
EOF
  expected="${block%$'\n'}"
  expected="${expected//@EXEC_START@/$exec_start}"
  expected="${expected//@CONFIG_DIR@/$CONFIG_DIR}"
  expected="${expected//@HOME@/$home}"
  actual="$(cat "$unit")"
  [[ "$actual" == *"$expected"* ]] \
    || fail "[Service] block of $unit is not the #555 contract (see $unit)"
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
# #555: the daemon must not sit at the bottom of the scheduling tier, and the
# hardening block must stay byte-identical — assert_service_block compares the
# whole [Service] section verbatim against the pre-#555 text plus the two new
# scheduling directives.
assert_unit_has "Nice=-5"
assert_unit_has "CPUWeight=500"
refute_unit_has "CPUSchedulingPolicy="
assert_service_block \
  "ExecStart=$INSTALL_ROOT/release/corrald --socket $FAKE_HOME/.config/herdr/herdr.sock --bind 127.0.0.1 --port 8474" \
  "$UNIT_FILE" "$FAKE_HOME"
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
echo "== scenario: migration of a pre-#555 unit (bare, no scheduling directives) =="
# The unit exactly as the emitter wrote it before #555 — i.e. what an existing
# install has on disk. A re-run must migrate it, report what changed, and reach
# the same bytes a fresh install produces. This scenario gets its own $HOME and
# install root so the fresh/reinstall scenarios above keep their state.
MIGRATE_HOME="$WORK/home-migrate"
MIGRATE_INSTALL_ROOT="$MIGRATE_HOME/.local/share/corral"
MIGRATE_UNIT_DIR="$MIGRATE_HOME/.config/systemd/user"
MIGRATE_UNIT="$MIGRATE_UNIT_DIR/corrald.service"
mkdir -p "$MIGRATE_UNIT_DIR" "$MIGRATE_HOME"
cat > "$WORK/pre555-unit.template" <<'EOF'
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
ExecStart=@EXEC_START@
Restart=on-failure
RestartSec=2
# The daemon shells out to git/gh for the repo/GitHub planes; give the user
# service a deterministic PATH instead of the manager's minimal default.
Environment=PATH=/usr/local/bin:/usr/bin:/bin
Environment=CORRAL_CONFIG_DIR=@CONFIG_DIR@
# Hardening: the daemon needs only its own files under @HOME@ (config), the
# herdr socket, loopback HTTP, and outbound HTTPS for the gh plane. It must
# keep writing @CONFIG_DIR@, so ProtectHome/ProtectSystem are deliberately not
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
EOF
sed -e "s|@CONFIG_DIR@|$CONFIG_DIR|" \
  -e "s|@HOME@|$MIGRATE_HOME|" \
  -e "s|@EXEC_START@|$MIGRATE_INSTALL_ROOT/release/corrald --socket $MIGRATE_HOME/.config/herdr/herdr.sock --bind 127.0.0.1 --port 8474|" \
  "$WORK/pre555-unit.template" > "$MIGRATE_UNIT"
cp "$MIGRATE_UNIT" "$WORK/pre555-unit.service"

TEST_UNIT_ACTIVE=0 TEST_UNIT_ENABLED=1 TEST_HEALTH_FAIL=0
set +e
env -i \
  HOME="$MIGRATE_HOME" \
  PATH="$STUB_BIN:/usr/bin:/bin" \
  CORRAL_CONFIG_DIR="$CONFIG_DIR" \
  CORRAL_INSTALL_DIR="$MIGRATE_INSTALL_ROOT" \
  CORRAL_FAKE_UNAME_S="Linux" CORRAL_FAKE_UNAME_M="x86_64" \
  CORRAL_TEST_SYSTEMCTL_LOG="$LOG_DIR/migrate-systemctl.log" \
  CORRAL_TEST_CURL_LOG="$LOG_DIR/migrate-curl.log" \
  CORRAL_TEST_UNIT_ACTIVE="$TEST_UNIT_ACTIVE" \
  CORRAL_TEST_UNIT_ENABLED="$TEST_UNIT_ENABLED" \
  CORRAL_TEST_HEALTH_FAIL="$TEST_HEALTH_FAIL" \
  bash "$INSTALLER" --url "$BUNDLE_URL_BASE" > "$LOG_DIR/migrate.log" 2>&1
rc=$?
set -e
[[ "$rc" -eq 0 ]] || { cat "$LOG_DIR/migrate.log" >&2; fail "migration install exited $rc"; }
assert_log_has "$LOG_DIR/migrate.log" \
  "migrated scheduling directives: Nice=unset -> -5, CPUWeight=unset -> 500"
assert_unit_has "Nice=-5" "$MIGRATE_UNIT"
assert_unit_has "CPUWeight=500" "$MIGRATE_UNIT"
refute_unit_has "CPUSchedulingPolicy=" "$MIGRATE_UNIT"
assert_service_block \
  "ExecStart=$MIGRATE_INSTALL_ROOT/release/corrald --socket $MIGRATE_HOME/.config/herdr/herdr.sock --bind 127.0.0.1 --port 8474" \
  "$MIGRATE_UNIT" "$MIGRATE_HOME"
# The migration touched the scheduling directives only: nothing was removed or
# replaced from the pre-#555 bytes, and exactly the expected lines were added.
diff_out="$(diff "$WORK/pre555-unit.service" "$MIGRATE_UNIT" || true)"
[[ "$(printf '%s\n' "$diff_out" | grep -c '^<')" == "0" ]] \
  || fail "migration removed or replaced a pre-#555 line:
$diff_out"
added="$(printf '%s\n' "$diff_out" | grep '^> ' || true)"
expected_block=""
IFS= read -r -d '' expected_block <<'EOF' || true
> # #555: answer under load — Nice/CPUWeight raise the daemon's scheduling weight
> # and CPU share while the rest of the host is saturated (see the header).
> Nice=-5
> CPUWeight=500
EOF
expected_added="${expected_block%$'\n'}"
[[ "$added" == "$expected_added" ]] || fail "migration added unexpected lines:
$added"
# A migrated install must end up byte-identical to a fresh one (only $HOME, and
# therefore the ExecStart path, differ between the two scenarios).
sed "s|$FAKE_HOME|@HOME@|g" "$UNIT_FILE" > "$WORK/fresh-unit.norm"
sed "s|$MIGRATE_HOME|@HOME@|g" "$MIGRATE_UNIT" > "$WORK/migrated-unit.norm"
cmp -s "$WORK/fresh-unit.norm" "$WORK/migrated-unit.norm" \
  || fail "migrated unit is not byte-identical to the fresh-install unit"
ok "pre-#555 migration: reported, scheduling-only diff, byte-identical to fresh"

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
