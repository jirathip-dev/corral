#!/usr/bin/env bash
# Hermetic #327 compatibility regression suite.
#
# It never touches the live Corral installation. The updater runs against a
# disposable dirty feature checkout and a local bare origin; the installer
# consumes a file:// release into disposable paths; the promotion gate is
# checked against a temporary invalid declaration as well as the repository
# declaration.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/corral-327.XXXXXX")"
cleanup() { [[ "${KEEP_WORK:-}" == 1 ]] || rm -rf -- "$WORK"; }
trap cleanup EXIT

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

# --- dirty source checkout + advanced origin/main ----------------------------
mkdir -p "$WORK/seed" "$WORK/primary" "$WORK/bin" "$WORK/home"
git init -q --bare "$WORK/origin.git"
git init -q -b main "$WORK/seed"
git -C "$WORK/seed" config user.email corral-327@example.invalid
git -C "$WORK/seed" config user.name corral-327
printf 'old-host\n' > "$WORK/seed/origin-marker"
git -C "$WORK/seed" add origin-marker
git -C "$WORK/seed" commit -qm initial
git -C "$WORK/seed" remote add origin "$WORK/origin.git"
git -C "$WORK/seed" push -q -u origin main

git clone -q "$WORK/origin.git" "$WORK/primary"
git -C "$WORK/primary" config user.email corral-327@example.invalid
git -C "$WORK/primary" config user.name corral-327
git -C "$WORK/primary" checkout -qb feature/local-work
printf 'developer-only-edit\n' >> "$WORK/primary/origin-marker"
printf 'dirty-feature-checkout\n' > "$WORK/primary/developer-note"
primary_diff="$WORK/primary.diff"
git -C "$WORK/primary" diff --binary > "$primary_diff"
primary_branch="$(git -C "$WORK/primary" branch --show-current)"

# Advance only origin/main. The updater must build this marker from the
# isolated origin/main source, not the dirty feature checkout.
printf 'new-host\n' > "$WORK/seed/origin-marker"
git -C "$WORK/seed" add origin-marker
git -C "$WORK/seed" commit -qm 'new host artifact'
git -C "$WORK/seed" push -q origin main

cat > "$WORK/bin/cargo" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
mkdir -p target/release
printf '%s\n' "$PWD" > "${CARGO_PWD_FILE:?}"
cp origin-marker target/release/corrald
cp origin-marker target/release/corrald-ui
chmod +x target/release/corrald target/release/corrald-ui
STUB

cat > "$WORK/bin/launchctl" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  print)
    printf '    program = %s\n' "${LAUNCHCTL_PROGRAM:?}"
    ;;
  kickstart)
    printf '%s\n' "$*" >> "${LAUNCHCTL_KICKS:?}"
    ;;
  *) exit 0 ;;
esac
STUB
chmod +x "$WORK/bin/cargo" "$WORK/bin/launchctl"

mkdir -p "$WORK/update-config"
mkdir -p "$WORK/installed"
CARGO_PWD_FILE="$WORK/cargo.pwd" \
LAUNCHCTL_PROGRAM="$WORK/installed/corrald" \
LAUNCHCTL_KICKS="$WORK/kickstarts" \
CORRAL_REPO_DIR="$WORK/primary" \
CORRAL_CONFIG_DIR="$WORK/update-config" \
HOME="$WORK/home" \
PATH="$WORK/bin:$PATH" \
bash "$SCRIPT_DIR/update-corral.sh"

update_log="$WORK/update-config/corral-update.log"
[[ -f "$update_log" ]] || fail "updater did not create a log"
! grep -Fq 'skip: not on main' "$update_log" \
  || fail "dirty feature checkout was silently skipped"
grep -Fq 'building origin/main' "$update_log" \
  || fail "updater did not record the isolated origin/main build"
[[ "$(cat "$WORK/installed/corrald")" == "new-host" ]] \
  || fail "new origin/main artifact was not installed"
[[ "$(cat "$WORK/cargo.pwd")" != "$WORK/primary"* ]] \
  || fail "cargo built inside the developer checkout"
[[ "$(git -C "$WORK/primary" branch --show-current)" == "$primary_branch" ]] \
  || fail "updater changed the developer branch"
git -C "$WORK/primary" diff --binary > "$WORK/primary.after.diff"
cmp -s "$primary_diff" "$WORK/primary.after.diff" \
  || fail "updater changed the developer worktree"
[[ "$(cat "$WORK/primary/developer-note")" == "dirty-feature-checkout" ]] \
  || fail "updater removed the developer's untracked file"
[[ "$(wc -l < "$WORK/kickstarts" | tr -d ' ')" == 1 ]] \
  || fail "updated daemon was not restarted exactly once"
printf 'OK dirty feature checkout builds fetched origin/main in isolation\n'

# A release-shaped updater with no source checkout must fail explicitly. An
# old "skip: ..." success is the indefinite-silent-failure defect.
mkdir -p "$WORK/release-copy" "$WORK/release-config"
cp "$SCRIPT_DIR/update-corral.sh" "$WORK/release-copy/update-corral.sh"
if CORRAL_REPO_DIR="$WORK/release-copy" \
  CORRAL_CONFIG_DIR="$WORK/release-config" HOME="$WORK/home" \
  PATH="$WORK/bin:$PATH" \
  bash "$WORK/release-copy/update-corral.sh"; then
  fail "non-source updater returned success instead of release-required failure"
fi
grep -Fq 'release-required' "$WORK/release-config/corral-update.log" \
  || fail "non-source updater did not log an explicit release-required reason"
printf 'OK non-source updater fails explicitly for release installation\n'

# --- installer rollback and config preservation -----------------------------
mkdir -p "$WORK/bundle/scripts" "$WORK/bundle/assets/icon" \
  "$WORK/install/release" "$WORK/app/Corral.app" "$WORK/install-home/Library/LaunchAgents" \
  "$WORK/install-config"
printf 'old-release\n' > "$WORK/install/release/version"
printf 'old-app\n' > "$WORK/app/Corral.app/version"
printf 'unchanged-config\n' > "$WORK/install-config/app.json"
printf 'existing-device-key\n' > "$WORK/install-config/device-keys.json"
printf 'existing-grants\n' > "$WORK/install-config/grants.json"
printf 'old-plist\n' > "$WORK/install-home/Library/LaunchAgents/com.corral.corrald.plist"

printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf new-app > "${CORRAL_MACOS_APP_DEST:?}/version"\nprintf new-plist > "${HOME:?}/Library/LaunchAgents/com.corral.corrald.plist"\nprintf new-config > "${CORRAL_CONFIG_DIR:?}/app.json"\nprintf new-key > "${CORRAL_CONFIG_DIR:?}/device-keys.json"\nprintf new-grants > "${CORRAL_CONFIG_DIR:?}/grants.json"\nexit 1\n' > "$WORK/bundle/scripts/setup-corrald.sh"
printf '#!/usr/bin/env bash\nexit 0\n' > "$WORK/bundle/scripts/install-corral-ui.sh"
cp "$SCRIPT_DIR/update-corral.sh" "$WORK/bundle/scripts/update-corral.sh"
cp "$SCRIPT_DIR/lib-corral-update-path.sh" "$WORK/bundle/scripts/lib-corral-update-path.sh"
cp "$SCRIPT_DIR/rotate-corral-logs.sh" "$WORK/bundle/scripts/rotate-corral-logs.sh"
printf '#!/usr/bin/env bash\nexit 0\n' > "$WORK/bundle/corrald"
printf '#!/usr/bin/env bash\nexit 0\n' > "$WORK/bundle/corrald-ui"
cp "$ROOT/assets/icon/corral-icon-macos.png" "$WORK/bundle/assets/icon/corral-icon-macos.png"
cp "$ROOT/assets/icon/corral-icon-256.png" "$WORK/bundle/assets/icon/corral-icon-256.png"
chmod +x "$WORK/bundle/corrald" "$WORK/bundle/corrald-ui" "$WORK/bundle/scripts"/*.sh
bundle="$WORK/new-release.tar.gz"
tar -C "$WORK/bundle" -czf "$bundle" corrald corrald-ui scripts assets
shasum -a 256 "$bundle" | awk '{print $1}' > "$bundle.sha256"

if HOME="$WORK/install-home" \
  CORRAL_CONFIG_DIR="$WORK/install-config" \
  CORRAL_INSTALL_DIR="$WORK/install" \
  CORRAL_MACOS_APP_DEST="$WORK/app/Corral.app" \
  RELEASE_URL="file://$bundle" \
  bash "$SCRIPT_DIR/install-corral.sh"; then
  fail "failing setup unexpectedly completed installation"
fi
[[ "$(cat "$WORK/install/release/version")" == "old-release" ]] \
  || fail "installer did not restore the previous release"
[[ "$(cat "$WORK/app/Corral.app/version")" == "old-app" ]] \
  || fail "installer did not restore the previous app"
[[ "$(cat "$WORK/install-config/app.json")" == "unchanged-config" ]] \
  || fail "installer changed app/config data"
[[ "$(cat "$WORK/install-config/device-keys.json")" == "existing-device-key" ]] \
  || fail "installer changed device keys"
[[ "$(cat "$WORK/install-config/grants.json")" == "existing-grants" ]] \
  || fail "installer changed grants"
[[ "$(cat "$WORK/install-home/Library/LaunchAgents/com.corral.corrald.plist")" == "old-plist" ]] \
  || fail "installer did not restore the previous daemon plist"
printf 'OK installer rollback restores release, app, plist, and config\n'

# --- promotion gate ----------------------------------------------------------
python3 "$ROOT/scripts/check-host-compatibility.py"
invalid="$WORK/invalid-host-compatibility.json"
printf '%s\n' '{"protocol_version":1,"schema_version":5,"host_artifact":"","compatibility_declaration":""}' > "$invalid"
if python3 "$ROOT/scripts/check-host-compatibility.py" "$invalid" >/dev/null 2>&1; then
  fail "promotion gate accepted an empty host artifact/declaration"
fi
printf '%s\n' '{"protocol_version":1,"schema_version":5,"host_artifact":"unverified.tar.gz","compatibility_declaration":""}' > "$invalid"
if python3 "$ROOT/scripts/check-host-compatibility.py" "$invalid" >/dev/null 2>&1; then
  fail "promotion gate accepted an unverified host artifact name"
fi
printf 'OK promotion gate rejects missing host artifact and declaration\n'

printf 'PASS: Corral #327 host update compatibility regressions\n'
