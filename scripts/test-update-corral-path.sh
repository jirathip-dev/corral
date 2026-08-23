#!/usr/bin/env bash
# Small, hermetic test for scripts/lib-corral-update-path.sh. Verifies the
# launchd PATH derivation: a brew bin is prepended under a minimal PATH, the
# rustup cargo bin is prepended, an existing brew on PATH is resolved without
# duplication, and it is a no-op when neither exists.
#
# Run with one command:
#   bash scripts/test-update-corral-path.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="$SCRIPT_DIR/lib-corral-update-path.sh"
WORK="$(mktemp -d)"
trap 'rm -rf -- "$WORK"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

# shellcheck disable=SC1090
source "$HELPER"

# 1. A brew bin passed as a candidate is prepended to a minimal PATH.
BREW_BIN="$WORK/homebrew/bin"
mkdir -p "$BREW_BIN"
export PATH="/usr/bin:/bin"
export HOME="$WORK/home1"
corral_prepend_update_path "$BREW_BIN"
[[ "$PATH" == "$BREW_BIN":/usr/bin:/bin ]] \
  || fail "candidate brew bin not prepended; PATH='$PATH'"

# 2. The rustup cargo bin is prepended too.
HOME_BIN="$WORK/home2/.cargo/bin"
mkdir -p "$HOME_BIN"
export PATH="/usr/bin:/bin"
export HOME="$WORK/home2"
corral_prepend_update_path "$WORK/no-such-brew"
[[ "$PATH" == "$HOME_BIN":/usr/bin:/bin ]] \
  || fail "cargo bin not prepended; PATH='$PATH'"

# 3. An existing brew on PATH keeps its dir without duplication.
BREW_BIN2="$WORK/brew2/bin"
mkdir -p "$BREW_BIN2"
printf '#!/usr/bin/env bash\nexit 0\n' > "$BREW_BIN2/brew"
chmod +x "$BREW_BIN2/brew"
export PATH="$BREW_BIN2:/usr/bin:/bin"
export HOME="$WORK/home3"
corral_prepend_update_path "$WORK/ignored-candidate"
[[ "$PATH" == "$BREW_BIN2:/usr/bin:/bin" ]] \
  || fail "existing brew dir duplicated or changed; PATH='$PATH'"

# 4. No brew, no cargo -> no-op, PATH unchanged, exits 0.
export PATH="/usr/bin:/bin"
export HOME="$WORK/no-home"
corral_prepend_update_path "$WORK/absent-brew"
[[ "$PATH" == "/usr/bin:/bin" ]] \
  || fail "PATH changed without brew/cargo; PATH='$PATH'"

# 5. Missing lib: update-corral.sh must not die under set -e before the log is
#    set up. Run a copy from a dir with no lib; it should reach its guard and
#    log a skip (no silent-failure mode). The first guard is "not a source
#    checkout" because the temp dir is not a git repo.
mkdir -p "$WORK/update" "$WORK/config"
cp "$SCRIPT_DIR/update-corral.sh" "$WORK/update/update-corral.sh"
export HOME="$WORK/home5"
CORRAL_CONFIG_DIR="$WORK/config" bash "$WORK/update/update-corral.sh"
[[ -f "$WORK/config/corral-update.log" ]] \
  || fail "missing lib produced no log (silent-failure mode)"
grep -q 'skip:' "$WORK/config/corral-update.log" \
  || fail "missing lib did not log a skip; log='$(cat "$WORK/config/corral-update.log")'"

echo "OK: corral launchd PATH derivation (prepend, cargo, resolve, no-op, missing-lib guard)"
