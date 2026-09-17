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
# Evidence is opt-in; only logs leave the disposable sandbox.
cleanup() {
  if [[ -n "${CORRAL_TEST_EVIDENCE_DIR:-}" ]]; then
    mkdir -p "$CORRAL_TEST_EVIDENCE_DIR"
    cp -R "$WORK/log/." "$CORRAL_TEST_EVIDENCE_DIR/"
  fi
  rm -rf -- "$WORK"
}
trap cleanup EXIT

ASSERTIONS=0
assertion_passed() { ASSERTIONS=$((ASSERTIONS + 1)); }
check() { "$@" || fail "assertion failed: $*"; assertion_passed; }
fail() { echo "FAIL: $* (completed assertions: $ASSERTIONS)" >&2; exit 1; }
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

# HTTP is entirely stubbed. Only file:// fixtures reach the real curl.
cat > "$STUB_BIN/curl" <<'CURL'
#!/usr/bin/env bash
set -eu
log="${CORRAL_TEST_CURL_LOG:-${0%/*}/../log/other-curl.log}"
printf 'curl' >> "$log"; printf ' %q' "$@" >> "$log"; printf '\n' >> "$log"
if [[ "$*" == *"/healthz"* ]]; then
  if [[ "${CORRAL_TEST_HEALTH_FAIL:-0}" == "1" ]]; then
    echo "curl: healthz connection refused (fixture)" >&2
    exit 7
  fi
  printf 'healthz probe\n' >> "$log"
  echo ok
  exit 0
fi
args=("$@")
url="" output="" headers=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --output|-o) output="$2"; shift ;;
    --dump-header|-D) headers="$2"; shift ;;
    --write-out|--connect-timeout|--max-time|--max-redirs) shift ;;
    https://*|file://*) url="$1" ;;
  esac
  shift
done
case "$url" in
  file://*) exec /usr/bin/curl "${args[@]}" ;;
  https://github.com/*/releases/latest) stage=latest ;;
  https://github.com/*/releases/download/*.sha256) stage=checksum ;;
  https://github.com/*/releases/download/*) stage=bundle ;;
  *) echo "unexpected network request: $url" >&2; exit 99 ;;
esac
status=200
if [[ "$stage" == latest ]]; then status=302; fi
if [[ "$stage" == "${CORRAL_TEST_FAIL_STAGE:-latest}" ]]; then
  if [[ "${CORRAL_TEST_CURL_EXIT:-0}" != 0 ]]; then
    echo 'curl: (6) Could not resolve host: github.com (fixture)' >&2
    exit "$CORRAL_TEST_CURL_EXIT"
  fi
  status="${CORRAL_TEST_HTTP_STATUS:-$status}"
fi
{
  # A proxy/redirect's headers must not leak into the final response.
  printf 'HTTP/1.1 200 Connection established\r\nRetry-After: stale-proxy\r\n\r\n'
  printf 'HTTP/2 %s\r\n' "$status"
  if [[ "$stage" == latest && "$status" == 302 ]]; then
    printf 'lOcAtIoN: %s\r\n' "${CORRAL_TEST_LOCATION:-https://github.com/jirathip-dev/corral/releases/tag/v0.1.0}"
  fi
  if [[ -n "${CORRAL_TEST_RETRY_AFTER:-}" ]]; then printf 'rEtRy-AfTeR: %s\r\n' "$CORRAL_TEST_RETRY_AFTER"; fi
  if [[ -n "${CORRAL_TEST_RESET:-}" ]]; then printf 'X-RateLimit-Reset: %s\r\n' "$CORRAL_TEST_RESET"; fi
  printf '\r\n'
} > "$headers"
if [[ "$status" == 200 && "$stage" != latest ]]; then
  source_file="${CORRAL_TEST_HTTP_BUNDLE:?}"
  if [[ "$stage" == checksum ]]; then source_file="$source_file.sha256"; fi
  cp "$source_file" "$output"
  if [[ "$stage" == checksum && "${CORRAL_TEST_BAD_CHECKSUM:-0}" == 1 ]]; then
    printf '%064d\n' 0 > "$output"
  fi
fi
printf '%s' "$status"
CURL
# Never reach the host's launchd, or source tooling, even on a regression.
cat > "$STUB_BIN/launchctl" <<'LAUNCHCTL'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${CORRAL_TEST_SYSTEMCTL_LOG:?}"
exit 0
LAUNCHCTL
POISON_BIN="$WORK/poison-bin"
mkdir -p "$POISON_BIN"
for tool in gh cargo rustc git; do
  cat > "$POISON_BIN/$tool" <<'POISON'
#!/usr/bin/env bash
printf '%s %s\n' "${0##*/}" "$*" >> "${0%/*}/../log/forbidden.log"
exit 97
POISON
  chmod +x "$POISON_BIN/$tool"
done
# Cargo/git stay poisoned even in the genuinely gh-absent leg.
for tool in cargo rustc git; do cp "$POISON_BIN/$tool" "$STUB_BIN/$tool"; done
# Keep setup's PATH derivation inside the fixture rather than host Homebrew.
printf '#!/usr/bin/env bash\nexit 0\n' > "$STUB_BIN/brew"
chmod +x "$STUB_BIN/"*
: > "$LOG_DIR/forbidden.log"

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
  run_logged "$name" env -i \
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
    bash "$INSTALLER" "$@"
  return $?
}

assert_log_has() { # $1=log file, $2=substring
  grep -Fq -- "$2" "$1" || fail "log $1 missing: '$2'"
  assertion_passed
}
refute_log_has() {
  if grep -Fq -- "$2" "$1"; then fail "log $1 unexpectedly has: '$2'"; fi
  assertion_passed
}
# $2 defaults to the fresh-install unit; the #555 migration scenario below
# passes its own unit file explicitly.
assert_unit_has() { # $1=substring, $2=unit file (default $UNIT_FILE)
  grep -Fq -- "$1" "${2:-$UNIT_FILE}" || fail "unit file ${2:-$UNIT_FILE} missing: '$1'"
  assertion_passed
}
refute_unit_has() {
  if grep -Fq -- "$1" "${2:-$UNIT_FILE}"; then fail "unit file ${2:-$UNIT_FILE} unexpectedly has: '$1'"; fi
  assertion_passed
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
  assertion_passed
}

# ---- fixture release bundles ------------------------------------------------
BUNDLE_URL_BASE="file://$FIXTURES/corral-v0.1.0-linux-x86_64.tar.gz"
BUNDLE="$FIXTURES/corral-v0.1.0-linux-x86_64.tar.gz"
make_bundle "v1.0" "$BUNDLE"
V1_HASH="$(sha256_hex "$BUNDLE.bin")"

# ---- #539: actual resolution, absent gh AND poison-gh legs -------------------
# These are separate environments: an executable poison shim necessarily makes
# `command -v gh` nonempty. Never mislabel the shim leg as literal PATH absence.
run_logged() {
  local name="$1" rc; shift
  printf 'COMMAND:'; printf ' %q' "$@"; printf '\n'
  if "$@" > "$LOG_DIR/$name.log" 2>&1; then rc=0; else rc=$?; fi
  printf 'RAW_EXIT %s=%s\n' "$name" "$rc"
  cat "$LOG_DIR/$name.log"
  printf '%s\t%s\n' "$name" "$rc" >> "$LOG_DIR/exits.tsv"
  return "$rc"
}

resolution_case() { # name, expected exit, message, expected downloads, env...
  local name="$1" expected_rc="$2" message="$3" requests="$4"; shift 4
  local home="$WORK/$RESOLVE_OS-$GH_MODE-$name" path rc gh_path lookup_rc
  local before_assertions="$ASSERTIONS"
  mkdir -p "$home/tmp" "$home/config" "$home/Library/LaunchAgents"
  printf 'fixture private key bytes\n' > "$home/config/key"
  chmod 600 "$home/config/key"
  path="$STUB_BIN:/usr/bin:/bin"
  if [[ "$GH_MODE" == poison ]]; then path="$POISON_BIN:$path"; fi
  if gh_path="$(env -i PATH="$path" /bin/bash --noprofile --norc -c 'command -v gh')"; then lookup_rc=0; else lookup_rc=$?; fi
  printf 'PATH=%s; command -v gh output=<%s>; RAW_EXIT=%s\n' "$path" "$gh_path" "$lookup_rc"
  if [[ "$GH_MODE" == absent ]]; then
    check test "$lookup_rc" -eq 1
    check test -z "$gh_path"
  else
    check test "$gh_path" = "$POISON_BIN/gh"
  fi
  local label="$RESOLVE_OS-$GH_MODE-$name"
  : > "$LOG_DIR/$label-curl.log"
  : > "$LOG_DIR/$label-service.log"
  if run_logged "$label" env -i HOME="$home" PATH="$path" TMPDIR="$home/tmp" \
    CORRAL_INSTALL_DIR="$home/install" CORRAL_CONFIG_DIR="$home/config" \
    CORRAL_FAKE_UNAME_S="$RESOLVE_OS" CORRAL_FAKE_UNAME_M=x86_64 \
    CORRAL_TEST_CURL_LOG="$LOG_DIR/$label-curl.log" \
    CORRAL_TEST_SYSTEMCTL_LOG="$LOG_DIR/$label-service.log" \
    CORRAL_TEST_HTTP_BUNDLE="$BUNDLE" "$@" \
    /bin/bash "$INSTALLER"; then rc=0; else rc=$?; fi
  check test "$rc" -eq "$expected_rc"
  assert_log_has "$LOG_DIR/$label.log" "$message"
  refute_log_has "$LOG_DIR/$label.log" 'stale-proxy'
  check test "$(grep -c 'github.com\|file://' "$LOG_DIR/$label-curl.log")" -eq "$requests"
  check test ! -s "$LOG_DIR/forbidden.log"
  check test "$(cat "$home/config/key")" = 'fixture private key bytes'
  check test -z "$(ls -A "$home/tmp")"
  if [[ "$rc" -eq 0 ]]; then
    check test -x "$home/install/release/corrald"
    assert_log_has "$LOG_DIR/$label.log" 'SHA-256 verified'
    if [[ "$RESOLVE_OS" == Linux ]]; then
      assert_log_has "$LOG_DIR/$label-service.log" 'enable --now corrald.service'
    else
      assert_log_has "$LOG_DIR/$label-service.log" 'bootstrap'
      check plutil -lint "$home/Library/LaunchAgents/com.corral.corrald.plist"
    fi
  else
    check test ! -e "$home/install"
    check test ! -s "$LOG_DIR/$label-service.log"
    refute_log_has "$LOG_DIR/$label.log" 'Installing prebuilt'
  fi
  printf 'SCENARIO_ASSERTIONS %s=%s\n' "$label" "$((ASSERTIONS - before_assertions))"
  printf 'HTTP_TRAFFIC %s\n' "$label"
  cat "$LOG_DIR/$label-curl.log"
  LAST_RESOLVE_LOG="$LOG_DIR/$label.log"
  LAST_RESOLVE_CURL="$LOG_DIR/$label-curl.log"
}

RESOLVE_PLATFORMS=Linux
if [[ "$(uname -s)" == Darwin ]]; then RESOLVE_PLATFORMS='Linux Darwin'; fi
for RESOLVE_OS in $RESOLVE_PLATFORMS; do
  for GH_MODE in poison absent; do
    resolution_case latest 0 'Using release v0.1.0' 3
    assert_log_has "$LAST_RESOLVE_CURL" '/releases/latest'
    if [[ "$RESOLVE_OS" == Darwin ]]; then platform=macos; else platform=linux-x86_64; fi
    assert_log_has "$LAST_RESOLVE_CURL" "/releases/download/v0.1.0/corral-v0.1.0-$platform.tar.gz"
    resolution_case pinned-tag 0 'Using release v0.1.0' 2 RELEASE_TAG=v0.1.0
    refute_log_has "$LAST_RESOLVE_CURL" '/releases/latest'
    resolution_case pinned-url 0 'Installed Corral' 2 RELEASE_URL="$BUNDLE_URL_BASE"
    refute_log_has "$LAST_RESOLVE_CURL" 'github.com'
    resolution_case latest-env 0 'Using release v0.1.0' 3 RELEASE_TAG=latest
    resolution_case relative-location 0 'Using release v0.1.0' 3 CORRAL_TEST_LOCATION=/jirathip-dev/corral/releases/tag/v0.1.0
    resolution_case slash-tag 0 'Using release test/candidate' 2 RELEASE_TAG=test/candidate
    assert_log_has "$LAST_RESOLVE_CURL" "/releases/download/test/candidate/corral-test_candidate-$platform.tar.gz"
    resolution_case repo-override 0 'Using release v0.1.0' 2 RELEASE_TAG=v0.1.0 CORRAL_RELEASE_REPO=fixture/public
    assert_log_has "$LAST_RESOLVE_CURL" 'https://github.com/fixture/public/releases/download/'
    for status in 403 429; do
      resolution_case "rate-$status-bare" 3 "HTTP $status" 1 CORRAL_TEST_HTTP_STATUS="$status"
      refute_log_has "$LAST_RESOLVE_LOG" 'Retry-After:'
      resolution_case "rate-$status-delay" 3 'Retry-After: 120' 1 CORRAL_TEST_HTTP_STATUS="$status" CORRAL_TEST_RETRY_AFTER=120
      resolution_case "rate-$status-date" 3 'Retry-After: Thu, 17 Sep 2026 12:00:00 GMT' 1 CORRAL_TEST_HTTP_STATUS="$status" 'CORRAL_TEST_RETRY_AFTER=Thu, 17 Sep 2026 12:00:00 GMT'
      resolution_case "rate-$status-reset" 3 'X-RateLimit-Reset: 1790000000' 1 CORRAL_TEST_HTTP_STATUS="$status" CORRAL_TEST_RESET=1790000000
    done
    resolution_case dns 4 'network/TLS failure (curl exit 6)' 1 CORRAL_TEST_CURL_EXIT=6
    resolution_case offline 4 'network/TLS failure (curl exit 7)' 1 CORRAL_TEST_CURL_EXIT=7
    resolution_case missing-latest 5 'release/tag or asset missing (HTTP 404)' 1 CORRAL_TEST_HTTP_STATUS=404
    resolution_case missing-redirect 5 'missing release tag redirect' 1 CORRAL_TEST_HTTP_STATUS=200
    resolution_case missing-tag 5 'release/tag or asset missing (HTTP 404)' 1 RELEASE_TAG=missing CORRAL_TEST_FAIL_STAGE=bundle CORRAL_TEST_HTTP_STATUS=404
    resolution_case missing-checksum 5 'release/tag or asset missing (HTTP 404)' 3 CORRAL_TEST_FAIL_STAGE=checksum CORRAL_TEST_HTTP_STATUS=404
    resolution_case download-rate 3 'Retry-After: 120' 2 CORRAL_TEST_FAIL_STAGE=bundle CORRAL_TEST_HTTP_STATUS=429 CORRAL_TEST_RETRY_AFTER=120
    resolution_case download-offline 4 'network/TLS failure (curl exit 7)' 2 CORRAL_TEST_FAIL_STAGE=bundle CORRAL_TEST_CURL_EXIT=7
    resolution_case server-error 6 'unexpected release HTTP status 503' 1 CORRAL_TEST_HTTP_STATUS=503
    resolution_case checksum-mismatch 1 'SHA-256 mismatch — refusing to install' 3 CORRAL_TEST_BAD_CHECKSUM=1
  done
done

# =============================================================================
echo "== scenario: fresh install (rootless, under \$HOME) =="
printf 'fixture existing key bytes\n' > "$CONFIG_DIR/key"
chmod 600 "$CONFIG_DIR/key"
TEST_UNIT_ACTIVE=0 TEST_UNIT_ENABLED=0 TEST_HEALTH_FAIL=0
run_install fresh --url "$BUNDLE_URL_BASE" || fail "fresh install exited $?"
assertion_passed
assert_log_has "$SCEN_LOG" "SHA-256 verified"
assert_log_has "$SCEN_LOG" "corrald is UP"
[[ -x "$INSTALL_ROOT/release/corrald" ]] || fail "fresh install: release binary missing"
assertion_passed
[[ "$(sha256_hex "$INSTALL_ROOT/release/corrald")" == "$V1_HASH" ]] \
  || fail "fresh install: installed binary hash mismatch"
assertion_passed
[[ -f "$UNIT_FILE" ]] || fail "fresh install: unit file not written"
assertion_passed
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
assertion_passed
ok "fresh install: unit content + enable --now + health probe + v1 binary"

if command -v systemd-analyze >/dev/null 2>&1; then
  run_logged systemd-analyze systemd-analyze verify "$UNIT_FILE" \
    || fail "systemd-analyze verify failed"
  assertion_passed
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
run_logged migrate env -i \
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
  bash "$INSTALLER" --url "$BUNDLE_URL_BASE"
rc=$?
set -e
check test "$rc" -eq 0
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
assertion_passed
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
assertion_passed
# A migrated install must end up byte-identical to a fresh one (only $HOME, and
# therefore the ExecStart path, differ between the two scenarios).
sed "s|$FAKE_HOME|@HOME@|g" "$UNIT_FILE" > "$WORK/fresh-unit.norm"
sed "s|$MIGRATE_HOME|@HOME@|g" "$MIGRATE_UNIT" > "$WORK/migrated-unit.norm"
cmp -s "$WORK/fresh-unit.norm" "$WORK/migrated-unit.norm" \
  || fail "migrated unit is not byte-identical to the fresh-install unit"
assertion_passed
ok "pre-#555 migration: reported, scheduling-only diff, byte-identical to fresh"

# =============================================================================
echo "== scenario: idempotent reinstall (same bundle, service active) =="
TEST_UNIT_ACTIVE=1 TEST_UNIT_ENABLED=1 TEST_HEALTH_FAIL=0
run_install reinstall --url "$BUNDLE_URL_BASE" || fail "reinstall exited $?"
assertion_passed
[[ "$(sha256_hex "$INSTALL_ROOT/release/corrald")" == "$V1_HASH" ]] \
  || fail "reinstall changed the binary"
assertion_passed
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
assertion_passed
[[ "$(sha256_hex "$INSTALL_ROOT/release/corrald")" == "$V2_HASH" ]] \
  || fail "update did not install the v2 binary"
assertion_passed
assert_log_has "$SYSTEMCTL_LOG" "restart corrald.service"
assert_log_has "$SCEN_LOG" "Restarting corrald.service (installed binary changed)"
[[ ! -e "$INSTALL_ROOT/release.previous" ]] || fail "update left release.previous behind"
assertion_passed
ok "update: v2 binary installed, service restarted, .previous cleaned"

# A failed update must restore an EXISTING release, not merely delete a fresh one.
TEST_HEALTH_FAIL=1
if run_install rollback-existing --url "$BUNDLE_URL_BASE"; then rc=0; else rc=$?; fi
check test "$rc" -eq 1
assert_log_has "$SCEN_LOG" 'setup failed; restoring previous release'
assert_log_has "$SYSTEMCTL_LOG" 'stop corrald.service'
check test "$(sha256_hex "$INSTALL_ROOT/release/corrald")" = "$V2_HASH"
check test ! -e "$INSTALL_ROOT/release.previous"
check test "$(cat "$CONFIG_DIR/key")" = 'fixture existing key bytes'
TEST_HEALTH_FAIL=0

# =============================================================================
echo "== scenario: checksum failure exits non-zero, no half-install =="
BAD_HOME="$WORK/home-bad"
mkdir -p "$BAD_HOME"
BAD_BUNDLE="$FIXTURES/corral-v0.1.0-linux-x86_64-bad.tar.gz"
make_bundle "v1.0-bad" "$BAD_BUNDLE"
printf '0000000000000000000000000000000000000000000000000000000000000000\n' > "$BAD_BUNDLE.sha256"
set +e
run_logged checksum env -i \
  HOME="$BAD_HOME" \
  PATH="$STUB_BIN:/usr/bin:/bin" \
  CORRAL_CONFIG_DIR="$WORK/config-bad" \
  CORRAL_INSTALL_DIR="$BAD_HOME/.local/share/corral" \
  CORRAL_FAKE_UNAME_S="Linux" CORRAL_FAKE_UNAME_M="x86_64" \
  bash "$INSTALLER" --url "file://$BAD_BUNDLE"
rc=$?
set -e
[[ "$rc" -ne 0 ]] || fail "checksum failure exited 0"
assertion_passed
assert_log_has "$LOG_DIR/checksum.log" "SHA-256 mismatch — refusing to install"
[[ ! -e "$BAD_HOME/.local/share/corral" ]] \
  || fail "checksum failure left an install root behind (half-install)"
assertion_passed
ok "checksum failure: exit $rc, no install root created"

# =============================================================================
echo "== scenario: unhealthy service -> installer exits non-zero, rolls back =="
UNHEALTHY_HOME="$WORK/home-unhealthy"
mkdir -p "$UNHEALTHY_HOME"
TEST_UNIT_ACTIVE=0 TEST_UNIT_ENABLED=0 TEST_HEALTH_FAIL=1
set +e
run_logged unhealthy env -i \
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
  bash "$INSTALLER" --url "$BUNDLE_URL_BASE"
rc=$?
set -e
[[ "$rc" -ne 0 ]] || fail "unhealthy-service install exited 0"
assertion_passed
assert_log_has "$LOG_DIR/unhealthy.log" "could not reach"
assert_log_has "$LOG_DIR/unhealthy-systemctl.log" "stop corrald.service"
[[ ! -e "$UNHEALTHY_HOME/.local/share/corral/release" ]] \
  || fail "unhealthy-service install left a release behind (no rollback)"
assertion_passed
ok "unhealthy service: exit $rc, service stopped, release rolled back"

echo "== scenario: recovery after unhealthy install (health ok) =="
TEST_UNIT_ACTIVE=0 TEST_UNIT_ENABLED=1 TEST_HEALTH_FAIL=0
set +e
run_logged recover env -i \
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
  bash "$INSTALLER" --url "$BUNDLE_URL_BASE"
rc=$?
set -e
check test "$rc" -eq 0
[[ -x "$UNHEALTHY_HOME/.local/share/corral/release/corrald" ]] \
  || fail "recovery install left no release"
assertion_passed
assert_log_has "$LOG_DIR/recover-systemctl.log" "start corrald.service"
ok "recovery: re-run installs cleanly and starts the enabled unit"

# =============================================================================
echo "== scenario: uninstall removes service + release, preserves config =="
MARKER="$CONFIG_DIR/keep-me.txt"
printf 'registry/keys live here\n' > "$MARKER"
TEST_UNIT_ACTIVE=1 TEST_UNIT_ENABLED=1 TEST_HEALTH_FAIL=0
run_install uninstall --uninstall || fail "uninstall exited $?"
assertion_passed
[[ ! -f "$UNIT_FILE" ]] || fail "uninstall left the unit file behind"
assertion_passed
[[ ! -e "$INSTALL_ROOT/release" ]] || fail "uninstall left the release behind"
assertion_passed
[[ ! -e "$INSTALL_ROOT/release.previous" ]] || fail "uninstall left release.previous behind"
assertion_passed
[[ -f "$MARKER" ]] || fail "uninstall removed $CONFIG_DIR (config must be preserved)"
assertion_passed
assert_log_has "$SYSTEMCTL_LOG" "disable --now corrald.service"
assert_log_has "$SCEN_LOG" "Uninstall complete. Config/keys kept"
ok "uninstall: unit+release removed, config/keys preserved, disable --now logged"

# =============================================================================
echo "== scenario: no-root/home-path behavior =="
# (a) install root outside $HOME is refused before anything is downloaded.
OUT_HOME="$WORK/outside-home/corral"
TEST_UNIT_ACTIVE=0 TEST_UNIT_ENABLED=0 TEST_HEALTH_FAIL=0
set +e
run_logged nohome env -i \
  HOME="$FAKE_HOME" \
  PATH="$STUB_BIN:/usr/bin:/bin" \
  CORRAL_CONFIG_DIR="$CONFIG_DIR" \
  CORRAL_INSTALL_DIR="$OUT_HOME" \
  CORRAL_FAKE_UNAME_S="Linux" CORRAL_FAKE_UNAME_M="x86_64" \
  bash "$INSTALLER" --url "$BUNDLE_URL_BASE"
rc=$?
set -e
[[ "$rc" -eq 2 ]] || fail "install root outside \$HOME exited $rc (want 2)"
assertion_passed
assert_log_has "$LOG_DIR/nohome.log" "must live under \$HOME"
[[ ! -e "$OUT_HOME" ]] || fail "install root outside \$HOME was created"
assertion_passed

# (b) non-x86_64 Linux is refused.
set +e
run_logged noarch env -i \
  HOME="$FAKE_HOME" \
  PATH="$STUB_BIN:/usr/bin:/bin" \
  CORRAL_CONFIG_DIR="$CONFIG_DIR" \
  CORRAL_INSTALL_DIR="$INSTALL_ROOT" \
  CORRAL_FAKE_UNAME_S="Linux" CORRAL_FAKE_UNAME_M="aarch64" \
  bash "$INSTALLER" --url "$BUNDLE_URL_BASE"
rc=$?
set -e
[[ "$rc" -eq 2 ]] || fail "non-x86_64 Linux exited $rc (want 2)"
assertion_passed
assert_log_has "$LOG_DIR/noarch.log" "x86_64 only"

# (c) unsupported OS is refused (self-test stays platform-neutral).
set +e
run_logged freebsd-selftest env -i \
  HOME="$FAKE_HOME" \
  PATH="$STUB_BIN:/usr/bin:/bin" \
  CORRAL_FAKE_UNAME_S="FreeBSD" CORRAL_FAKE_UNAME_M="x86_64" \
  bash "$INSTALLER" --self-test
rc=$?
set -e
[[ "$rc" -eq 0 ]] || fail "--self-test should stay platform-neutral (exit $rc)"
assertion_passed
set +e
run_logged noplat env -i \
  HOME="$FAKE_HOME" \
  PATH="$STUB_BIN:/usr/bin:/bin" \
  CORRAL_FAKE_UNAME_S="FreeBSD" CORRAL_FAKE_UNAME_M="x86_64" \
  bash "$INSTALLER" --url "$BUNDLE_URL_BASE"
rc=$?
set -e
[[ "$rc" -eq 2 ]] || fail "unsupported platform exited $rc (want 2)"
assertion_passed
assert_log_has "$LOG_DIR/noplat.log" "unsupported platform"
ok "no-root/home-path: outside-\$HOME refused, non-x86_64 refused, unsupported OS refused"

echo
check test ! -s "$LOG_DIR/forbidden.log"
check test "$(cat "$CONFIG_DIR/key")" = 'fixture existing key bytes'
echo "OK: install-corral scenarios passed; executed assertions: $ASSERTIONS; forbidden invocations: 0"
