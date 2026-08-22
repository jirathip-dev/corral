#!/usr/bin/env bash
# Hermetic transactional-install tests. They use only temporary destinations
# and stub the platform converters so injected failures are deterministic.
# A separate macOS smoke check with real sips/iconutil validates the .icns.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALLER="$SCRIPT_DIR/install-corral-ui.sh"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/corral-icon-packaging.XXXXXX")"
trap 'rm -rf -- "$TEST_ROOT"' EXIT

REAL_PATH="$PATH"
REAL_CP="$(command -v cp)"
REAL_MV="$(command -v mv)"
REAL_RM="$(command -v rm)"
REAL_STAT="$(command -v stat)"
STUB_DIR="$TEST_ROOT/stubs"
mkdir -p "$STUB_DIR" "$TEST_ROOT/home"

write_stub() {
  local name="$1"
  local body="$2"
  printf '%b\n' "$body" > "$STUB_DIR/$name"
  chmod +x "$STUB_DIR/$name"
}

write_stub cp '#!/usr/bin/env bash\nset -euo pipefail\nif [[ "${CORRAL_TEST_FAIL_CP:-0}" == "1" ]]; then exit 91; fi\nexec "$CORRAL_TEST_REAL_CP" "$@"'
write_stub mv '#!/usr/bin/env bash\nset -euo pipefail\ncount_file="${CORRAL_TEST_MV_COUNT:?}"\ncount=0\nif [[ -f "$count_file" ]]; then count="$(<"$count_file")"; fi\ncount=$((count + 1))\nprintf "%s" "$count" > "$count_file"\ncase ",${CORRAL_TEST_FAIL_MV_AT:-}," in *,"$count",*) exit 92 ;; esac\nexec "$CORRAL_TEST_REAL_MV" "$@"'
write_stub sips '#!/usr/bin/env bash\nset -euo pipefail\nif [[ "${CORRAL_TEST_FAIL_SIPS:-0}" == "1" ]]; then exit 93; fi\noutput=""\nfor ((index = 1; index <= $#; index++)); do\n  if [[ "${!index}" == "--out" ]]; then\n    next=$((index + 1))\n    output="${!next}"\n  fi\ndone\n[[ -n "$output" ]]\nprintf "stub-sips" > "$output"'
write_stub iconutil '#!/usr/bin/env bash\nset -euo pipefail\nif [[ "${CORRAL_TEST_FAIL_ICONUTIL:-0}" == "1" ]]; then exit 94; fi\nmode=""\noutput=""\nfor ((index = 1; index <= $#; index++)); do\n  if [[ "${!index}" == "-c" ]]; then\n    next=$((index + 1))\n    mode="${!next}"\n  elif [[ "${!index}" == "-o" ]]; then\n    next=$((index + 1))\n    output="${!next}"\n  fi\ndone\n[[ -n "$mode" && -n "$output" ]]\nif [[ "$mode" == "icns" ]]; then\n  printf "stub-icns" > "$output"\nelif [[ "$mode" == "iconset" ]]; then\n  mkdir -p "$output"\n  printf "stub-png" > "$output/icon_16x16.png"\nelse\n  exit 95\nfi'
write_stub plutil '#!/usr/bin/env bash\nset -euo pipefail\nif [[ "${CORRAL_TEST_FAIL_PLUTIL:-0}" == "1" ]]; then exit 96; fi\n[[ "${1:-}" == "-lint" && -s "${2:-}" ]]'
write_stub rm '#!/usr/bin/env bash\nset -euo pipefail\nif [[ "${CORRAL_TEST_TOTAL_RM_ROLLBACK:-0}" == "1" ]]; then\n  for argument in "$@"; do\n    if [[ "$argument" == */.corral-ui.rollback.* ]]; then\n      "$CORRAL_TEST_REAL_RM" -rf -- "$argument"\n      exit 97\n    fi\n  done\nfi\nif [[ "${CORRAL_TEST_INDETERMINATE_RM_ROLLBACK:-0}" == "1" ]]; then\n  for argument in "$@"; do\n    if [[ "$argument" == */.corral-ui.rollback.* ]]; then\n      "$CORRAL_TEST_REAL_RM" -rf -- "$argument"\n      printf "indeterminate rollback state" > "$argument"\n      exit 97\n    fi\n  done\nfi\nif [[ "${CORRAL_TEST_PARTIAL_RM_ROLLBACK:-0}" == "1" ]]; then\n  for argument in "$@"; do\n    if [[ "$argument" == */.corral-ui.rollback.* ]]; then\n      for candidate in "$argument"/*; do\n        if [[ -e "$candidate" || -L "$candidate" ]]; then\n          "$CORRAL_TEST_REAL_RM" -rf -- "$candidate"\n          break\n        fi\n      done\n      exit 97\n    fi\n  done\nfi\nif [[ "${CORRAL_TEST_FAIL_RM_ROLLBACK:-0}" == "1" ]]; then\n  for argument in "$@"; do\n    if [[ "$argument" == */.corral-ui.rollback.* ]]; then exit 97; fi\n  done\nfi\nexec "$CORRAL_TEST_REAL_RM" "$@"'
write_stub stat '#!/usr/bin/env bash\nset -euo pipefail\npath=""\nfor argument in "$@"; do path="$argument"; done\nif [[ "${CORRAL_TEST_STAT_MISMATCH:-0}" == "1" && "$path" == "$CORRAL_TEST_STAT_MISMATCH_PATH" ]]; then\n  printf "device-mismatch"\n  exit 0\nfi\nexec "$CORRAL_TEST_REAL_STAT" "$@"'

TEST_PATH="$STUB_DIR:$REAL_PATH"
UI_BIN="$TEST_ROOT/corrald-ui"
printf '#!/bin/sh\nprintf corral-ui\n' > "$UI_BIN"
chmod +x "$UI_BIN"

run_installer() {
  local platform="$1"
  local mac_dest="${CORRAL_TEST_MAC_DEST:-$MAC_DEST}"
  local linux_prefix="${CORRAL_TEST_LINUX_PREFIX:-$LINUX_PREFIX}"
  local other_prefix="${CORRAL_TEST_OTHER_PREFIX:-$OTHER_PREFIX}"
  shift
  env \
    PATH="$TEST_PATH" \
    HOME="$TEST_ROOT/home" \
    CORRAL_TEST_REAL_CP="$REAL_CP" \
    CORRAL_TEST_REAL_MV="$REAL_MV" \
    CORRAL_TEST_REAL_RM="$REAL_RM" \
    CORRAL_TEST_REAL_STAT="$REAL_STAT" \
    CORRAL_TEST_MV_COUNT="$TEST_ROOT/mv-count" \
    CORRAL_TEST_STAT_MISMATCH_PATH="${CORRAL_TEST_STAT_MISMATCH_PATH:-}" \
    CORRAL_INSTALL_PLATFORM="$platform" \
    CORRAL_SKIP_CODESIGN=1 \
    CORRAL_MACOS_APP_DEST="$mac_dest" \
    CORRAL_LINUX_PREFIX="$linux_prefix" \
    CORRAL_OTHER_PREFIX="$other_prefix" \
    bash "$INSTALLER" --binary "$UI_BIN"
}

clear_failures() {
  unset CORRAL_TEST_FAIL_CP CORRAL_TEST_FAIL_SIPS CORRAL_TEST_FAIL_ICONUTIL
  unset CORRAL_TEST_FAIL_PLUTIL CORRAL_TEST_FAIL_MV_AT CORRAL_TEST_FAIL_RM_ROLLBACK
  unset CORRAL_TEST_TOTAL_RM_ROLLBACK CORRAL_TEST_INDETERMINATE_RM_ROLLBACK
  unset CORRAL_TEST_PARTIAL_RM_ROLLBACK
  unset CORRAL_TEST_STAT_MISMATCH CORRAL_TEST_STAT_MISMATCH_PATH
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

assert_no_staging_entries() {
  local parent="$1"
  local candidate
  for candidate in \
    "$parent"/.corral-ui.stage.* \
    "$parent"/.corral-ui.icns-check.*; do
    [[ ! -e "$candidate" ]] || fail "temporary staging directory remains: $candidate"
  done
}

rollback_path_from_log() {
  local log="$1"
  local line
  line="$(grep -F 'rollback directory:' "$log" | tail -n 1)" || fail "rollback path was not reported in $log"
  local path="${line##*: }"
  [[ -d "$path" ]] || fail "reported rollback directory is not recoverable: $path"
  printf '%s' "$path"
}

rollback_path_from_log_marker() {
  local log="$1"
  local marker="$2"
  local line
  line="$(grep -F "$marker" "$log" | tail -n 1)" || fail "rollback path marker was not reported in $log"
  printf '%s' "${line##*: }"
}

assert_no_false_restore_message() {
  local log="$1"
  if grep -Eq 'restored|preserved' "$log"; then
    fail "false preservation message in $log"
  fi
}

assert_rollback_label() {
  local log="$1"
  grep -Fq 'payload rollback failed; prior desktop payload is retained in rollback directory:' "$log" \
    || fail "rollback log label is not platform-neutral: $log"
}

assert_cleanup_label() {
  local log="$1"
  grep -Fq 'prior desktop payload restored; rollback directory cleanup failed; inspect rollback directory:' "$log" \
    || fail "rollback cleanup log label is not truthful: $log"
  if grep -Fq 'prior desktop payload is retained in rollback directory:' "$log"; then
    fail "cleanup failure was reported as payload retention: $log"
  fi
}

assert_restored_missing_cleanup_label() {
  local log="$1"
  grep -Fq 'prior desktop payload restored; rollback directory cleanup failed; rollback directory is missing' "$log" \
    || fail "restored-payload missing cleanup label is not truthful: $log"
  if grep -Eq 'inspect|retained|remain|recover|empty|partial|rollback directory:' "$log"; then
    fail "restored-payload missing cleanup claimed an inspectable or recoverable state: $log"
  fi
}

assert_restored_indeterminate_cleanup_label() {
  local log="$1"
  grep -Fq 'prior desktop payload restored; rollback directory cleanup failed; rollback cleanup state is indeterminate; inspect rollback path:' "$log" \
    || fail "restored-payload indeterminate cleanup label is not truthful: $log"
  if grep -Eq 'rollback directory is empty|partially cleaned|prior rollback payload|retained|recover' "$log"; then
    fail "restored-payload indeterminate cleanup claimed a definite state: $log"
  fi
}

assert_empty_cleanup_label() {
  local log="$1"
  grep -Fq 'installed desktop payload; rollback directory is empty; inspect rollback directory:' "$log" \
    || fail "empty rollback cleanup log label is not truthful: $log"
  if grep -Eq 'rollback cop(y|ies) retained|prior desktop payload is retained' "$log"; then
    fail "empty rollback cleanup failure was reported as payload retention: $log"
  fi
}

assert_missing_cleanup_label() {
  local log="$1"
  grep -Fq 'installed desktop payload; rollback directory is missing' "$log" \
    || fail "missing rollback cleanup log label is not truthful: $log"
  if grep -Eq 'inspect|retained|remain|recover|empty|partial|rollback directory:' "$log"; then
    fail "missing rollback cleanup claimed an inspectable or recoverable state: $log"
  fi
}

assert_indeterminate_cleanup_label() {
  local log="$1"
  grep -Fq 'rollback cleanup state is indeterminate; inspect rollback path:' "$log" \
    || fail "indeterminate rollback cleanup log label is not truthful: $log"
  if grep -Eq 'rollback directory is empty|partially cleaned|prior rollback payload|retained|recover' "$log"; then
    fail "indeterminate rollback cleanup claimed a definite state: $log"
  fi
}

assert_fresh_cleanup_label() {
  assert_empty_cleanup_label "$1"
}

assert_partial_cleanup_label() {
  local log="$1"
  grep -Fq 'installed desktop payload; some prior rollback payload paths remain; inspect rollback directory:' "$log" \
    || fail "partial rollback cleanup log label is not truthful: $log"
  if grep -Eq 'rollback cop(y|ies) retained|rollback directory is empty' "$log"; then
    fail "partial rollback cleanup failure was misclassified: $log"
  fi
}

assert_empty_rollback_directory() {
  local rollback_dir="$1"
  [[ -d "$rollback_dir" ]] || fail "reported fresh-install rollback directory is missing: $rollback_dir"
  local candidate
  for candidate in "$rollback_dir"/* "$rollback_dir"/.[!.]* "$rollback_dir"/..?*; do
    [[ ! -e "$candidate" && ! -L "$candidate" ]] || fail "fresh-install rollback directory is not empty: $candidate"
  done
}

assert_retained_file() {
  local rollback_dir="$1"
  local relative="$2"
  local expected="$3"
  local path="$rollback_dir/$relative"
  assert_file "$path"
  [[ "$(<"$path")" == "$expected" ]] || fail "rollback copy changed: $path"
}

assert_file_or_rollback() {
  local destination="$1"
  local rollback_dir="$2"
  local relative="$3"
  local expected="$4"
  local path="$destination/$relative"
  if [[ -f "$path" && "$(<"$path")" == "$expected" ]]; then
    return 0
  fi
  assert_retained_file "$rollback_dir" "$relative" "$expected"
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
SPECIAL_SUFFIX=' path % "quoted" \slash $dollar `tick`'
LINUX_PARENT="$TEST_ROOT/linux$SPECIAL_SUFFIX"
LINUX_PREFIX="$LINUX_PARENT/.local"
OTHER_PARENT="$TEST_ROOT/other$SPECIAL_SUFFIX"
OTHER_PREFIX="$OTHER_PARENT/.local"
mkdir -p "$MAC_PARENT"
clear_failures
run_installer Darwin > "$TEST_ROOT/macos-success.log" 2>&1
assert_executable "$MAC_DEST/Contents/MacOS/corrald-ui"
assert_file "$MAC_DEST/Contents/Resources/Corral.icns"
grep -Fq '<key>CFBundleIconFile</key><string>Corral</string>' "$MAC_DEST/Contents/Info.plist"
assert_no_temporary_entries "$MAC_PARENT"

rm -rf -- "$MAC_DEST"
clear_failures
export CORRAL_TEST_TOTAL_RM_ROLLBACK=1
if ! run_installer Darwin > "$TEST_ROOT/macos-fresh-rm.log" 2>&1; then
  fail "macOS fresh rollback-directory rm injection unexpectedly failed"
fi
assert_executable "$MAC_DEST/Contents/MacOS/corrald-ui"
assert_file "$MAC_DEST/Contents/Resources/Corral.icns"
assert_missing_cleanup_label "$TEST_ROOT/macos-fresh-rm.log"
assert_no_temporary_entries "$MAC_PARENT"
clear_failures

seed_macos_old
clear_failures
export CORRAL_TEST_PARTIAL_RM_ROLLBACK=1
if ! run_installer Darwin > "$TEST_ROOT/macos-partial-rm.log" 2>&1; then
  fail "macOS replacement partial rollback-directory rm injection unexpectedly failed"
fi
assert_executable "$MAC_DEST/Contents/MacOS/corrald-ui"
assert_file "$MAC_DEST/Contents/Resources/Corral.icns"
assert_empty_cleanup_label "$TEST_ROOT/macos-partial-rm.log"
mac_partial_rollback="$(rollback_path_from_log "$TEST_ROOT/macos-partial-rm.log")"
[[ ! -e "$mac_partial_rollback/previous" ]] || fail "macOS partial cleanup retained a removed rollback payload"
assert_empty_rollback_directory "$mac_partial_rollback"
rm -rf -- "$mac_partial_rollback"
clear_failures
assert_no_temporary_entries "$MAC_PARENT"

seed_macos_old
clear_failures
export CORRAL_TEST_TOTAL_RM_ROLLBACK=1
if ! run_installer Darwin > "$TEST_ROOT/macos-total-rm.log" 2>&1; then
  fail "macOS replacement total rollback-directory rm injection unexpectedly failed"
fi
assert_executable "$MAC_DEST/Contents/MacOS/corrald-ui"
assert_file "$MAC_DEST/Contents/Resources/Corral.icns"
assert_missing_cleanup_label "$TEST_ROOT/macos-total-rm.log"
assert_no_temporary_entries "$MAC_PARENT"
clear_failures

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

seed_macos_old
clear_failures
export CORRAL_TEST_FAIL_MV_AT=2,3
if run_installer Darwin > "$TEST_ROOT/macos-double-mv.log" 2>&1; then
  fail "macOS double commit mv injection unexpectedly succeeded"
fi
assert_no_false_restore_message "$TEST_ROOT/macos-double-mv.log"
assert_rollback_label "$TEST_ROOT/macos-double-mv.log"
assert_no_staging_entries "$MAC_PARENT"
mac_rollback="$(rollback_path_from_log "$TEST_ROOT/macos-double-mv.log")"
assert_retained_file "$mac_rollback/previous" Contents/old-marker old-macos-install
rm -rf -- "$mac_rollback"

seed_macos_old
clear_failures
export CORRAL_TEST_FAIL_MV_AT=2
export CORRAL_TEST_FAIL_RM_ROLLBACK=1
if run_installer Darwin > "$TEST_ROOT/macos-rm.log" 2>&1; then
  fail "macOS rollback-directory rm injection unexpectedly succeeded"
fi
assert_macos_old
assert_cleanup_label "$TEST_ROOT/macos-rm.log"
mac_cleanup_rollback="$(rollback_path_from_log "$TEST_ROOT/macos-rm.log")"
[[ ! -e "$mac_cleanup_rollback/previous" ]] || fail "macOS cleanup-failure rollback retained restored payload"
assert_no_staging_entries "$MAC_PARENT"
rm -rf -- "$mac_cleanup_rollback"

seed_macos_old
clear_failures
export CORRAL_TEST_FAIL_MV_AT=2
export CORRAL_TEST_TOTAL_RM_ROLLBACK=1
if run_installer Darwin > "$TEST_ROOT/macos-rollback-total-rm.log" 2>&1; then
  fail "macOS rollback total-delete injection unexpectedly succeeded"
fi
assert_macos_old
assert_restored_missing_cleanup_label "$TEST_ROOT/macos-rollback-total-rm.log"
assert_no_temporary_entries "$MAC_PARENT"

seed_macos_old
clear_failures
export CORRAL_TEST_FAIL_MV_AT=2
export CORRAL_TEST_INDETERMINATE_RM_ROLLBACK=1
if run_installer Darwin > "$TEST_ROOT/macos-rollback-indeterminate-rm.log" 2>&1; then
  fail "macOS rollback indeterminate injection unexpectedly succeeded"
fi
assert_macos_old
assert_restored_indeterminate_cleanup_label "$TEST_ROOT/macos-rollback-indeterminate-rm.log"
mac_rollback_indeterminate_path="$(rollback_path_from_log_marker "$TEST_ROOT/macos-rollback-indeterminate-rm.log" 'inspect rollback path:')"
[[ -f "$mac_rollback_indeterminate_path" ]] || fail "macOS rollback indeterminate state is not a retained non-directory path"
rm -f -- "$mac_rollback_indeterminate_path"
assert_no_temporary_entries "$MAC_PARENT"

clear_failures
run_installer Linux > "$TEST_ROOT/linux-success.log" 2>&1
assert_executable "$LINUX_PREFIX/bin/corrald-ui"
assert_file "$LINUX_PREFIX/share/icons/hicolor/256x256/apps/corral.png"
assert_file "$LINUX_PREFIX/share/applications/corral.desktop"
linux_prefix_canonical="$(cd -P -- "$LINUX_PREFIX" && pwd -P)"
mise exec -- python "$REPO_DIR/tools/icon/check-desktop-entry.py" \
  "$LINUX_PREFIX/share/applications/corral.desktop" "$linux_prefix_canonical/bin/corrald-ui"
mise exec -- python - "$LINUX_PREFIX/share/applications/corral.desktop" \
  "$linux_prefix_canonical/bin/corrald-ui" \
  "$REPO_DIR/tools/icon/check-desktop-entry.py" <<'PY'
import importlib.util
from pathlib import Path
import sys

spec = importlib.util.spec_from_file_location("check_desktop_entry", sys.argv[3])
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)
arguments, _ = module.parse_desktop_entry(Path(sys.argv[1]))
assert arguments == [sys.argv[2]]
raw_exec = next(
    line.removeprefix("Exec=")
    for line in Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()
    if line.startswith("Exec=")
)

def count_backslashes_before(index):
    count = 0
    index -= 1
    while index >= 0 and raw_exec[index] == "\\":
        count += 1
        index -= 1
    return count

assert raw_exec.count("%%") == 1
quoted = raw_exec.index("quoted")
assert count_backslashes_before(quoted - 1) == 3
assert count_backslashes_before(raw_exec.index('"', quoted + len("quoted"))) == 3
dollar = raw_exec.index("$dollar")
assert count_backslashes_before(dollar) == 2
tick = raw_exec.index("`tick")
assert count_backslashes_before(tick) == 2
assert count_backslashes_before(raw_exec.index("`", tick + len("`tick"))) == 2
slash = raw_exec.index("slash")
assert count_backslashes_before(slash) == 4

for invalid in (
    r'"/tmp/path\$dollar"',
    r'"/tmp/path`tick`"',
    r'"/tmp/path\"quoted\""',
    r'"/tmp/path\\slash"',
    r'"/tmp/path=reserved"',
):
    try:
        module.tokenize_exec(module.decode_general_string(invalid))
    except SystemExit:
        pass
    else:
        raise AssertionError(f"one-layer Exec escape was accepted: {invalid!r}")
PY
if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$LINUX_PREFIX/share/applications/corral.desktop"
fi
grep -Fqx 'Icon=corral' "$LINUX_PREFIX/share/applications/corral.desktop"
assert_no_temporary_entries "$LINUX_PREFIX"

rm -rf -- "$LINUX_PREFIX"
clear_failures
export CORRAL_TEST_TOTAL_RM_ROLLBACK=1
if ! run_installer Linux > "$TEST_ROOT/linux-fresh-rm.log" 2>&1; then
  fail "Linux fresh rollback-directory rm injection unexpectedly failed"
fi
assert_executable "$LINUX_PREFIX/bin/corrald-ui"
assert_file "$LINUX_PREFIX/share/icons/hicolor/256x256/apps/corral.png"
assert_file "$LINUX_PREFIX/share/applications/corral.desktop"
assert_missing_cleanup_label "$TEST_ROOT/linux-fresh-rm.log"
assert_no_temporary_entries "$LINUX_PREFIX"
clear_failures

DEVICE_LINUX_PREFIX="$TEST_ROOT/device-linux/.local"
mkdir -p \
  "$DEVICE_LINUX_PREFIX/bin" \
  "$DEVICE_LINUX_PREFIX/share/applications" \
  "$DEVICE_LINUX_PREFIX/share/icons/hicolor/256x256/apps"
printf 'device-linux-binary' > "$DEVICE_LINUX_PREFIX/bin/corrald-ui"
printf 'device-linux-icon' > "$DEVICE_LINUX_PREFIX/share/icons/hicolor/256x256/apps/corral.png"
printf 'device-linux-desktop' > "$DEVICE_LINUX_PREFIX/share/applications/corral.desktop"
device_linux_canonical="$(cd -P -- "$DEVICE_LINUX_PREFIX" && pwd -P)"
clear_failures
export CORRAL_TEST_LINUX_PREFIX="$DEVICE_LINUX_PREFIX"
export CORRAL_TEST_STAT_MISMATCH=1
export CORRAL_TEST_STAT_MISMATCH_PATH="$device_linux_canonical/share/applications"
if run_installer Linux > "$TEST_ROOT/linux-device-mismatch.log" 2>&1; then
  fail "Linux device mismatch unexpectedly succeeded"
fi
unset CORRAL_TEST_LINUX_PREFIX CORRAL_TEST_STAT_MISMATCH CORRAL_TEST_STAT_MISMATCH_PATH
grep -Fq 'across filesystems' "$TEST_ROOT/linux-device-mismatch.log"
[[ "$(<"$DEVICE_LINUX_PREFIX/bin/corrald-ui")" == "device-linux-binary" ]] || fail "Linux device mismatch changed the old binary"
[[ "$(<"$DEVICE_LINUX_PREFIX/share/icons/hicolor/256x256/apps/corral.png")" == "device-linux-icon" ]] || fail "Linux device mismatch changed the old icon"
assert_no_temporary_entries "$DEVICE_LINUX_PREFIX"

DEVICE_OTHER_PREFIX="$TEST_ROOT/device-other/.local"
mkdir -p "$DEVICE_OTHER_PREFIX/bin"
printf 'device-other-binary' > "$DEVICE_OTHER_PREFIX/bin/corrald-ui"
device_other_canonical="$(cd -P -- "$DEVICE_OTHER_PREFIX" && pwd -P)"
clear_failures
export CORRAL_TEST_OTHER_PREFIX="$DEVICE_OTHER_PREFIX"
export CORRAL_TEST_STAT_MISMATCH=1
export CORRAL_TEST_STAT_MISMATCH_PATH="$device_other_canonical/bin"
if run_installer Other > "$TEST_ROOT/other-device-mismatch.log" 2>&1; then
  fail "Other device mismatch unexpectedly succeeded"
fi
unset CORRAL_TEST_OTHER_PREFIX CORRAL_TEST_STAT_MISMATCH CORRAL_TEST_STAT_MISMATCH_PATH
grep -Fq 'across filesystems' "$TEST_ROOT/other-device-mismatch.log"
[[ "$(<"$DEVICE_OTHER_PREFIX/bin/corrald-ui")" == "device-other-binary" ]] || fail "Other device mismatch changed the old binary"
assert_no_temporary_entries "$DEVICE_OTHER_PREFIX"

assert_rejected_control_prefix() {
  local platform="$1"
  local position="$2"
  local control_name="$3"
  local control_char="$4"
  local safe_prefix="$TEST_ROOT/control-$platform-$control_name-$position"
  local parent="$(dirname "$safe_prefix")"
  local base="$(basename "$safe_prefix")"
  local malformed
  case "$position" in
    beginning) malformed="$parent/$control_char$base" ;;
    middle) malformed="$parent/${base:0:7}$control_char${base:7}" ;;
    end) malformed="$safe_prefix$control_char" ;;
    *) fail "unknown control position: $position" ;;
  esac
  clear_failures
  case "$platform" in
    Linux) export CORRAL_TEST_LINUX_PREFIX="$malformed" ;;
    Other) export CORRAL_TEST_OTHER_PREFIX="$malformed" ;;
    *) fail "unknown control platform: $platform" ;;
  esac
  local log="$TEST_ROOT/control-$platform-$control_name-$position.log"
  if run_installer "$platform" > "$log" 2>&1; then
    fail "$platform $control_name $position destination unexpectedly succeeded"
  fi
  unset CORRAL_TEST_LINUX_PREFIX CORRAL_TEST_OTHER_PREFIX
  grep -Fq 'newline or carriage return' "$log"
  [[ ! -e "$safe_prefix" ]] || fail "control path retargeted a safe prefix: $safe_prefix"
  [[ ! -e "$malformed" ]] || fail "control path created a malformed destination"
  assert_no_staging_entries "$parent"
}

for control_name in newline carriage-return; do
  if [[ "$control_name" == "newline" ]]; then
    control_char=$'\n'
  else
    control_char=$'\r'
  fi
  for position in beginning middle end; do
    assert_rejected_control_prefix Linux "$position" "$control_name" "$control_char"
    assert_rejected_control_prefix Other "$position" "$control_name" "$control_char"
  done
done

EQUAL_PREFIX="$TEST_ROOT/linux=reserved"
clear_failures
export CORRAL_TEST_LINUX_PREFIX="$EQUAL_PREFIX"
if run_installer Linux > "$TEST_ROOT/linux-equal.log" 2>&1; then
  fail "Linux equals-sign path unexpectedly succeeded"
fi
unset CORRAL_TEST_LINUX_PREFIX
grep -Fq "Linux executable path contains Desktop Entry '='" "$TEST_ROOT/linux-equal.log"
[[ ! -e "$EQUAL_PREFIX" ]] || fail "equals-sign path created a destination"
assert_no_staging_entries "$TEST_ROOT"

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
export CORRAL_TEST_PARTIAL_RM_ROLLBACK=1
if ! run_installer Linux > "$TEST_ROOT/linux-partial-rm.log" 2>&1; then
  fail "Linux replacement partial rollback-directory rm injection unexpectedly failed"
fi
assert_executable "$LINUX_PREFIX/bin/corrald-ui"
assert_file "$LINUX_PREFIX/share/icons/hicolor/256x256/apps/corral.png"
assert_file "$LINUX_PREFIX/share/applications/corral.desktop"
assert_partial_cleanup_label "$TEST_ROOT/linux-partial-rm.log"
linux_partial_rollback="$(rollback_path_from_log "$TEST_ROOT/linux-partial-rm.log")"
[[ ! -e "$linux_partial_rollback/bin/corrald-ui" ]] || fail "Linux partial cleanup retained the removed binary rollback"
assert_retained_file "$linux_partial_rollback" share/icons/hicolor/256x256/apps/corral.png old-linux-icon
assert_retained_file "$linux_partial_rollback" share/applications/corral.desktop old-linux-desktop
rm -rf -- "$linux_partial_rollback"
clear_failures
assert_no_temporary_entries "$LINUX_PREFIX"

seed_linux_old
clear_failures
export CORRAL_TEST_TOTAL_RM_ROLLBACK=1
if ! run_installer Linux > "$TEST_ROOT/linux-total-rm.log" 2>&1; then
  fail "Linux replacement total rollback-directory rm injection unexpectedly failed"
fi
assert_executable "$LINUX_PREFIX/bin/corrald-ui"
assert_file "$LINUX_PREFIX/share/icons/hicolor/256x256/apps/corral.png"
assert_file "$LINUX_PREFIX/share/applications/corral.desktop"
assert_missing_cleanup_label "$TEST_ROOT/linux-total-rm.log"
assert_no_temporary_entries "$LINUX_PREFIX"
clear_failures

seed_linux_old
clear_failures
export CORRAL_TEST_INDETERMINATE_RM_ROLLBACK=1
if ! run_installer Linux > "$TEST_ROOT/linux-indeterminate-rm.log" 2>&1; then
  fail "Linux replacement indeterminate rollback-directory rm injection unexpectedly failed"
fi
assert_executable "$LINUX_PREFIX/bin/corrald-ui"
assert_file "$LINUX_PREFIX/share/icons/hicolor/256x256/apps/corral.png"
assert_file "$LINUX_PREFIX/share/applications/corral.desktop"
assert_indeterminate_cleanup_label "$TEST_ROOT/linux-indeterminate-rm.log"
linux_indeterminate_path="$(rollback_path_from_log_marker "$TEST_ROOT/linux-indeterminate-rm.log" 'inspect rollback path:')"
[[ -f "$linux_indeterminate_path" ]] || fail "Linux indeterminate rollback state is not a retained non-directory path"
rm -f -- "$linux_indeterminate_path"
clear_failures
assert_no_temporary_entries "$LINUX_PREFIX"

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

seed_linux_old
clear_failures
export CORRAL_TEST_FAIL_MV_AT=5,6
if run_installer Linux > "$TEST_ROOT/linux-double-mv.log" 2>&1; then
  fail "Linux double commit mv injection unexpectedly succeeded"
fi
assert_no_false_restore_message "$TEST_ROOT/linux-double-mv.log"
assert_rollback_label "$TEST_ROOT/linux-double-mv.log"
assert_no_staging_entries "$LINUX_PREFIX"
linux_rollback="$(rollback_path_from_log "$TEST_ROOT/linux-double-mv.log")"
assert_file_or_rollback "$LINUX_PREFIX" "$linux_rollback" bin/corrald-ui old-linux-binary
assert_file_or_rollback "$LINUX_PREFIX" "$linux_rollback" share/icons/hicolor/256x256/apps/corral.png old-linux-icon
assert_file_or_rollback "$LINUX_PREFIX" "$linux_rollback" share/applications/corral.desktop old-linux-desktop

seed_linux_old
clear_failures
export CORRAL_TEST_FAIL_MV_AT=5
export CORRAL_TEST_FAIL_RM_ROLLBACK=1
if run_installer Linux > "$TEST_ROOT/linux-rm.log" 2>&1; then
  fail "Linux rollback-directory rm injection unexpectedly succeeded"
fi
assert_linux_old
assert_cleanup_label "$TEST_ROOT/linux-rm.log"
linux_cleanup_rollback="$(rollback_path_from_log "$TEST_ROOT/linux-rm.log")"
[[ ! -e "$linux_cleanup_rollback/bin/corrald-ui" ]] || fail "Linux cleanup-failure rollback retained restored payload"
assert_no_staging_entries "$LINUX_PREFIX"

seed_linux_old
clear_failures
export CORRAL_TEST_FAIL_MV_AT=5
export CORRAL_TEST_TOTAL_RM_ROLLBACK=1
if run_installer Linux > "$TEST_ROOT/linux-rollback-total-rm.log" 2>&1; then
  fail "Linux rollback total-delete injection unexpectedly succeeded"
fi
assert_linux_old
assert_restored_missing_cleanup_label "$TEST_ROOT/linux-rollback-total-rm.log"
assert_no_temporary_entries "$LINUX_PREFIX"

seed_linux_old
clear_failures
export CORRAL_TEST_FAIL_MV_AT=5
export CORRAL_TEST_INDETERMINATE_RM_ROLLBACK=1
if run_installer Linux > "$TEST_ROOT/linux-rollback-indeterminate-rm.log" 2>&1; then
  fail "Linux rollback indeterminate injection unexpectedly succeeded"
fi
assert_linux_old
assert_restored_indeterminate_cleanup_label "$TEST_ROOT/linux-rollback-indeterminate-rm.log"
linux_rollback_indeterminate_path="$(rollback_path_from_log_marker "$TEST_ROOT/linux-rollback-indeterminate-rm.log" 'inspect rollback path:')"
[[ -f "$linux_rollback_indeterminate_path" ]] || fail "Linux rollback indeterminate state is not a retained non-directory path"
rm -f -- "$linux_rollback_indeterminate_path"
assert_no_temporary_entries "$LINUX_PREFIX"

mkdir -p "$OTHER_PARENT"
clear_failures
run_installer Other > "$TEST_ROOT/other-success.log" 2>&1
assert_executable "$OTHER_PREFIX/bin/corrald-ui"
assert_no_temporary_entries "$OTHER_PREFIX"

rm -rf -- "$OTHER_PREFIX"
mkdir -p "$OTHER_PREFIX/bin"
printf 'old-other-binary' > "$OTHER_PREFIX/bin/corrald-ui"
clear_failures
export CORRAL_TEST_PARTIAL_RM_ROLLBACK=1
if ! run_installer Other > "$TEST_ROOT/other-partial-rm.log" 2>&1; then
  fail "Other replacement partial rollback-directory rm injection unexpectedly failed"
fi
assert_executable "$OTHER_PREFIX/bin/corrald-ui"
assert_empty_cleanup_label "$TEST_ROOT/other-partial-rm.log"
other_partial_rollback="$(rollback_path_from_log "$TEST_ROOT/other-partial-rm.log")"
[[ ! -e "$other_partial_rollback/bin/corrald-ui" ]] || fail "Other partial cleanup retained the removed binary rollback"
assert_empty_rollback_directory "$other_partial_rollback"
rm -rf -- "$other_partial_rollback"
clear_failures
assert_no_temporary_entries "$OTHER_PREFIX"

rm -rf -- "$OTHER_PREFIX"
mkdir -p "$OTHER_PREFIX/bin"
printf 'old-other-binary' > "$OTHER_PREFIX/bin/corrald-ui"
clear_failures
export CORRAL_TEST_TOTAL_RM_ROLLBACK=1
if ! run_installer Other > "$TEST_ROOT/other-total-rm.log" 2>&1; then
  fail "Other replacement total rollback-directory rm injection unexpectedly failed"
fi
assert_executable "$OTHER_PREFIX/bin/corrald-ui"
assert_missing_cleanup_label "$TEST_ROOT/other-total-rm.log"
assert_no_temporary_entries "$OTHER_PREFIX"
clear_failures

rm -rf -- "$OTHER_PREFIX"
mkdir -p "$OTHER_PREFIX"
clear_failures
export CORRAL_TEST_TOTAL_RM_ROLLBACK=1
if ! run_installer Other > "$TEST_ROOT/other-fresh-rm.log" 2>&1; then
  fail "Other fresh rollback-directory rm injection unexpectedly failed"
fi
assert_executable "$OTHER_PREFIX/bin/corrald-ui"
assert_missing_cleanup_label "$TEST_ROOT/other-fresh-rm.log"
assert_no_temporary_entries "$OTHER_PREFIX"
clear_failures

rm -rf -- "$OTHER_PREFIX"
mkdir -p "$OTHER_PREFIX/bin"
printf 'old-other-binary' > "$OTHER_PREFIX/bin/corrald-ui"
clear_failures
export CORRAL_TEST_FAIL_MV_AT=2,3
if run_installer Other > "$TEST_ROOT/other-double-mv.log" 2>&1; then
  fail "Other double commit mv injection unexpectedly succeeded"
fi
assert_no_false_restore_message "$TEST_ROOT/other-double-mv.log"
assert_rollback_label "$TEST_ROOT/other-double-mv.log"
assert_no_staging_entries "$OTHER_PREFIX"
other_rollback="$(rollback_path_from_log "$TEST_ROOT/other-double-mv.log")"
assert_retained_file "$other_rollback" bin/corrald-ui old-other-binary

rm -rf -- "$OTHER_PREFIX"
mkdir -p "$OTHER_PREFIX/bin"
printf 'old-other-binary' > "$OTHER_PREFIX/bin/corrald-ui"
clear_failures
export CORRAL_TEST_FAIL_MV_AT=2
export CORRAL_TEST_FAIL_RM_ROLLBACK=1
if run_installer Other > "$TEST_ROOT/other-rm.log" 2>&1; then
  fail "Other rollback-directory rm injection unexpectedly succeeded"
fi
[[ -f "$OTHER_PREFIX/bin/corrald-ui" ]] || fail "Other cleanup failure removed the old payload"
[[ "$(<"$OTHER_PREFIX/bin/corrald-ui")" == "old-other-binary" ]] || fail "Other cleanup failure changed the old payload"
assert_cleanup_label "$TEST_ROOT/other-rm.log"
other_cleanup_rollback="$(rollback_path_from_log "$TEST_ROOT/other-rm.log")"
[[ ! -e "$other_cleanup_rollback/bin/corrald-ui" ]] || fail "Other cleanup-failure rollback retained restored payload"
assert_no_staging_entries "$OTHER_PREFIX"

rm -rf -- "$OTHER_PREFIX"
mkdir -p "$OTHER_PREFIX/bin"
printf 'old-other-binary' > "$OTHER_PREFIX/bin/corrald-ui"
clear_failures
export CORRAL_TEST_FAIL_MV_AT=2
export CORRAL_TEST_TOTAL_RM_ROLLBACK=1
if run_installer Other > "$TEST_ROOT/other-rollback-total-rm.log" 2>&1; then
  fail "Other rollback total-delete injection unexpectedly succeeded"
fi
[[ -f "$OTHER_PREFIX/bin/corrald-ui" ]] || fail "Other total-delete rollback removed the old payload"
[[ "$(<"$OTHER_PREFIX/bin/corrald-ui")" == "old-other-binary" ]] || fail "Other total-delete rollback changed the old payload"
assert_restored_missing_cleanup_label "$TEST_ROOT/other-rollback-total-rm.log"
assert_no_temporary_entries "$OTHER_PREFIX"

rm -rf -- "$OTHER_PREFIX"
mkdir -p "$OTHER_PREFIX/bin"
printf 'old-other-binary' > "$OTHER_PREFIX/bin/corrald-ui"
clear_failures
export CORRAL_TEST_FAIL_MV_AT=2
export CORRAL_TEST_INDETERMINATE_RM_ROLLBACK=1
if run_installer Other > "$TEST_ROOT/other-rollback-indeterminate-rm.log" 2>&1; then
  fail "Other rollback indeterminate injection unexpectedly succeeded"
fi
[[ -f "$OTHER_PREFIX/bin/corrald-ui" ]] || fail "Other indeterminate rollback removed the old payload"
[[ "$(<"$OTHER_PREFIX/bin/corrald-ui")" == "old-other-binary" ]] || fail "Other indeterminate rollback changed the old payload"
assert_restored_indeterminate_cleanup_label "$TEST_ROOT/other-rollback-indeterminate-rm.log"
other_rollback_indeterminate_path="$(rollback_path_from_log_marker "$TEST_ROOT/other-rollback-indeterminate-rm.log" 'inspect rollback path:')"
[[ -f "$other_rollback_indeterminate_path" ]] || fail "Other rollback indeterminate state is not a retained non-directory path"
rm -f -- "$other_rollback_indeterminate_path"
assert_no_temporary_entries "$OTHER_PREFIX"

TRAVERSAL_PREFIX="/tmp/x/../.."
clear_failures
export CORRAL_TEST_OTHER_PREFIX="$TRAVERSAL_PREFIX"
if run_installer Other > "$TEST_ROOT/other-traversal.log" 2>&1; then
  fail "root-resolving traversal unexpectedly succeeded"
fi
unset CORRAL_TEST_OTHER_PREFIX
grep -Fq 'containing ..' "$TEST_ROOT/other-traversal.log"

ROOT_LINK_PREFIX="$TEST_ROOT/root-prefix-link"
ln -s / "$ROOT_LINK_PREFIX"
clear_failures
export CORRAL_TEST_OTHER_PREFIX="$ROOT_LINK_PREFIX"
if run_installer Other > "$TEST_ROOT/other-root-symlink.log" 2>&1; then
  fail "prefix symlink to root unexpectedly succeeded"
fi
unset CORRAL_TEST_OTHER_PREFIX
grep -Fq 'refusing root Other install prefix' "$TEST_ROOT/other-root-symlink.log"

BIN_LINK_PREFIX="$TEST_ROOT/bin-link-prefix"
BIN_ESCAPE="$TEST_ROOT/bin-link-escape"
mkdir -p "$BIN_LINK_PREFIX"
ln -s "$BIN_ESCAPE" "$BIN_LINK_PREFIX/bin"
clear_failures
export CORRAL_TEST_OTHER_PREFIX="$BIN_LINK_PREFIX"
if run_installer Other > "$TEST_ROOT/other-bin-symlink.log" 2>&1; then
  fail "bin symlink unexpectedly succeeded"
fi
unset CORRAL_TEST_OTHER_PREFIX
grep -Fq 'refusing symlink in Other payload parent' "$TEST_ROOT/other-bin-symlink.log"
[[ ! -e "$BIN_ESCAPE" ]] || fail "bin symlink test wrote outside the prefix"

SHARE_LINK_PREFIX="$TEST_ROOT/share-link-prefix"
SHARE_ESCAPE="$TEST_ROOT/share-link-escape"
mkdir -p "$SHARE_LINK_PREFIX"
ln -s "$SHARE_ESCAPE" "$SHARE_LINK_PREFIX/share"
clear_failures
export CORRAL_TEST_LINUX_PREFIX="$SHARE_LINK_PREFIX"
if run_installer Linux > "$TEST_ROOT/linux-share-symlink.log" 2>&1; then
  fail "share symlink unexpectedly succeeded"
fi
unset CORRAL_TEST_LINUX_PREFIX
grep -Fq 'refusing symlink in Linux payload parent' "$TEST_ROOT/linux-share-symlink.log"
[[ ! -e "$SHARE_ESCAPE" ]] || fail "share symlink test wrote outside the prefix"

SAFE_REAL_PREFIX="$TEST_ROOT/safe-canonical-prefix"
SAFE_LINK_PREFIX="$TEST_ROOT/safe-canonical-link"
mkdir -p "$SAFE_REAL_PREFIX"
ln -s "$SAFE_REAL_PREFIX" "$SAFE_LINK_PREFIX"
clear_failures
export CORRAL_TEST_OTHER_PREFIX="$SAFE_LINK_PREFIX"
run_installer Other > "$TEST_ROOT/other-safe-symlink.log" 2>&1
unset CORRAL_TEST_OTHER_PREFIX
assert_executable "$SAFE_REAL_PREFIX/bin/corrald-ui"
assert_no_temporary_entries "$SAFE_REAL_PREFIX"

echo "icon packaging transactional tests: ok (staging, rollback, Exec/path/device safety, cleanup diagnostics)"
