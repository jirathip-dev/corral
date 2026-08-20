#!/usr/bin/env bash
# Hermetic transactional-install tests. They use only temporary destinations
# and stub the platform converters so injected failures are deterministic.
# A separate macOS smoke check with real sips/iconutil validates the .icns.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALLER="$SCRIPT_DIR/install-corral-ui.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/corral-icon-packaging.XXXXXX")"
trap 'rm -rf -- "$TEST_ROOT"' EXIT

REAL_PATH="$PATH"
REAL_CP="$(command -v cp)"
REAL_MV="$(command -v mv)"
STUB_DIR="$TEST_ROOT/stubs"
mkdir -p "$STUB_DIR" "$TEST_ROOT/home"

write_stub() {
  local name="$1"
  local body="$2"
  printf '%b\n' "$body" > "$STUB_DIR/$name"
  chmod +x "$STUB_DIR/$name"
}

write_stub cp '#!/usr/bin/env bash\nset -euo pipefail\nif [[ "${CORRAL_TEST_FAIL_CP:-0}" == "1" ]]; then exit 91; fi\nexec "$CORRAL_TEST_REAL_CP" "$@"'
write_stub mv '#!/usr/bin/env bash\nset -euo pipefail\ncount_file="${CORRAL_TEST_MV_COUNT:?}"\ncount=0\nif [[ -f "$count_file" ]]; then count="$(<"$count_file")"; fi\ncount=$((count + 1))\nprintf "%s" "$count" > "$count_file"\nif [[ "${CORRAL_TEST_FAIL_MV_AT:-0}" == "$count" ]]; then exit 92; fi\nexec "$CORRAL_TEST_REAL_MV" "$@"'
write_stub sips '#!/usr/bin/env bash\nset -euo pipefail\nif [[ "${CORRAL_TEST_FAIL_SIPS:-0}" == "1" ]]; then exit 93; fi\noutput=""\nfor ((index = 1; index <= $#; index++)); do\n  if [[ "${!index}" == "--out" ]]; then\n    next=$((index + 1))\n    output="${!next}"\n  fi\ndone\n[[ -n "$output" ]]\nprintf "stub-sips" > "$output"'
write_stub iconutil '#!/usr/bin/env bash\nset -euo pipefail\nif [[ "${CORRAL_TEST_FAIL_ICONUTIL:-0}" == "1" ]]; then exit 94; fi\nmode=""\noutput=""\nfor ((index = 1; index <= $#; index++)); do\n  if [[ "${!index}" == "-c" ]]; then\n    next=$((index + 1))\n    mode="${!next}"\n  elif [[ "${!index}" == "-o" ]]; then\n    next=$((index + 1))\n    output="${!next}"\n  fi\ndone\n[[ -n "$mode" && -n "$output" ]]\nif [[ "$mode" == "icns" ]]; then\n  printf "stub-icns" > "$output"\nelif [[ "$mode" == "iconset" ]]; then\n  mkdir -p "$output"\n  printf "stub-png" > "$output/icon_16x16.png"\nelse\n  exit 95\nfi'
write_stub plutil '#!/usr/bin/env bash\nset -euo pipefail\nif [[ "${CORRAL_TEST_FAIL_PLUTIL:-0}" == "1" ]]; then exit 96; fi\n[[ "${1:-}" == "-lint" && -s "${2:-}" ]]'

TEST_PATH="$STUB_DIR:$REAL_PATH"
UI_BIN="$TEST_ROOT/corrald-ui"
printf '#!/bin/sh\nprintf corral-ui\n' > "$UI_BIN"
chmod +x "$UI_BIN"

run_installer() {
  local platform="$1"
  shift
  env \
    PATH="$TEST_PATH" \
    HOME="$TEST_ROOT/home" \
    CORRAL_TEST_REAL_CP="$REAL_CP" \
    CORRAL_TEST_REAL_MV="$REAL_MV" \
    CORRAL_TEST_MV_COUNT="$TEST_ROOT/mv-count" \
    CORRAL_INSTALL_PLATFORM="$platform" \
    CORRAL_SKIP_CODESIGN=1 \
    CORRAL_MACOS_APP_DEST="$MAC_DEST" \
    CORRAL_LINUX_PREFIX="$LINUX_PREFIX" \
    bash "$INSTALLER" --binary "$UI_BIN"
}

clear_failures() {
  unset CORRAL_TEST_FAIL_CP CORRAL_TEST_FAIL_SIPS CORRAL_TEST_FAIL_ICONUTIL
  unset CORRAL_TEST_FAIL_PLUTIL CORRAL_TEST_FAIL_MV_AT
  rm -f -- "$TEST_ROOT/mv-count"
}

fail() {
  echo "icon packaging test failed: $*" >&2
  exit 1
}

assert_file() {
  [[ -f "$1" ]] || fail "missing file: $1"
}

assert_executable() {
  [[ -x "$1" ]] || fail "missing executable: $1"
}

assert_no_temporary_entries() {
  local parent="$1"
  local candidate
  for candidate in \
    "$parent"/.corral-ui.stage.* \
    "$parent"/.corral-ui.rollback.* \
    "$parent"/.corral-ui.icns-check.*; do
    [[ ! -e "$candidate" ]] || fail "temporary packaging directory remains: $candidate"
  done
}

seed_macos_old() {
  rm -rf -- "$MAC_DEST"
  mkdir -p "$MAC_DEST/Contents"
  printf 'old-macos-install' > "$MAC_DEST/Contents/old-marker"
}

assert_macos_old() {
  assert_file "$MAC_DEST/Contents/old-marker"
  [[ "$(<"$MAC_DEST/Contents/old-marker")" == "old-macos-install" ]] || fail "macOS install changed the old payload"
  [[ ! -e "$MAC_DEST/Contents/Resources/Corral.icns" ]] || fail "failed macOS install left a new icon"
}

MAC_PARENT="$TEST_ROOT/macos"
MAC_DEST="$MAC_PARENT/Corral.app"
LINUX_PARENT="$TEST_ROOT/linux"
LINUX_PREFIX="$LINUX_PARENT/.local"
mkdir -p "$MAC_PARENT"
clear_failures
run_installer Darwin > "$TEST_ROOT/macos-success.log" 2>&1
assert_executable "$MAC_DEST/Contents/MacOS/corrald-ui"
assert_file "$MAC_DEST/Contents/Resources/Corral.icns"
grep -Fq '<key>CFBundleIconFile</key><string>Corral</string>' "$MAC_DEST/Contents/Info.plist"
assert_no_temporary_entries "$MAC_PARENT"

for failure in cp sips iconutil plutil; do
  seed_macos_old
  clear_failures
  case "$failure" in
    cp) export CORRAL_TEST_FAIL_CP=1 ;;
    sips) export CORRAL_TEST_FAIL_SIPS=1 ;;
    iconutil) export CORRAL_TEST_FAIL_ICONUTIL=1 ;;
    plutil) export CORRAL_TEST_FAIL_PLUTIL=1 ;;
  esac
  if run_installer Darwin > "$TEST_ROOT/macos-$failure.log" 2>&1; then
    fail "macOS $failure injection unexpectedly succeeded"
  fi
  assert_macos_old
  assert_no_temporary_entries "$MAC_PARENT"
done

seed_macos_old
clear_failures
export CORRAL_TEST_FAIL_MV_AT=2
if run_installer Darwin > "$TEST_ROOT/macos-mv.log" 2>&1; then
  fail "macOS commit mv injection unexpectedly succeeded"
fi
assert_macos_old
assert_no_temporary_entries "$MAC_PARENT"

clear_failures
run_installer Linux > "$TEST_ROOT/linux-success.log" 2>&1
assert_executable "$LINUX_PREFIX/bin/corrald-ui"
assert_file "$LINUX_PREFIX/share/icons/hicolor/256x256/apps/corral.png"
assert_file "$LINUX_PREFIX/share/applications/corral.desktop"
grep -Fqx "Exec=$LINUX_PREFIX/bin/corrald-ui" "$LINUX_PREFIX/share/applications/corral.desktop"
grep -Fqx 'Icon=corral' "$LINUX_PREFIX/share/applications/corral.desktop"
assert_no_temporary_entries "$LINUX_PREFIX"

seed_linux_old() {
  rm -rf -- "$LINUX_PREFIX"
  mkdir -p \
    "$LINUX_PREFIX/bin" \
    "$LINUX_PREFIX/share/applications" \
    "$LINUX_PREFIX/share/icons/hicolor/256x256/apps"
  printf 'old-linux-binary' > "$LINUX_PREFIX/bin/corrald-ui"
  printf 'old-linux-icon' > "$LINUX_PREFIX/share/icons/hicolor/256x256/apps/corral.png"
  printf 'old-linux-desktop' > "$LINUX_PREFIX/share/applications/corral.desktop"
}

assert_linux_old() {
  [[ "$(<"$LINUX_PREFIX/bin/corrald-ui")" == "old-linux-binary" ]] || fail "Linux install changed the old binary"
  [[ "$(<"$LINUX_PREFIX/share/icons/hicolor/256x256/apps/corral.png")" == "old-linux-icon" ]] || fail "Linux install changed the old icon"
  [[ "$(<"$LINUX_PREFIX/share/applications/corral.desktop")" == "old-linux-desktop" ]] || fail "Linux install changed the old desktop entry"
}

seed_linux_old
clear_failures
export CORRAL_TEST_FAIL_CP=1
if run_installer Linux > "$TEST_ROOT/linux-cp.log" 2>&1; then
  fail "Linux cp injection unexpectedly succeeded"
fi
assert_linux_old
assert_no_temporary_entries "$LINUX_PREFIX"

seed_linux_old
clear_failures
export CORRAL_TEST_FAIL_MV_AT=5
if run_installer Linux > "$TEST_ROOT/linux-mv.log" 2>&1; then
  fail "Linux commit mv injection unexpectedly succeeded"
fi
assert_linux_old
assert_no_temporary_entries "$LINUX_PREFIX"

echo "icon packaging transactional tests: ok (macOS staging, rollback, Linux payload)"
