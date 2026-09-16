#!/usr/bin/env bash
# docs/evidence/issue-555/base-identity.sh — proves the installers emit exactly
# the scheduling-priority change and nothing else.
#
# It runs the BASE commit's two installers and the delivered worktree's two
# installers side by side in throwaway fixture homes (stubbed launchctl/curl/
# systemctl — no live launchd, no real systemd, no host state) and diffs the
# generated files. Expected result, asserted below:
#
#   * all three launchd plists differ by exactly ONE line: the ProcessType
#     value, Background -> Interactive (one line per agent);
#   * the systemd unit differs by exactly FOUR added lines (the two scheduling
#     directives and their comment) and nothing removed — i.e. the hardening
#     block and every other directive are byte-identical.
#
# Usage (from anywhere):
#   bash docs/evidence/issue-555/base-identity.sh [base-commit]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
BASE="${1:-97ed9e03cb4d265da02ac28bad1685000b159f80}"
W="$(mktemp -d /tmp/g555-base-identity.XXXXXX)"
trap 'rm -rf -- "$W"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

echo "repo:  $ROOT"
echo "base:  $BASE"
echo "work:  $W"

mkdir -p "$W/base/scripts" "$W/head/scripts"
for side in base head; do
  mkdir -p "$W/$side/scripts"
done
git -C "$ROOT" show "$BASE:scripts/setup-corrald.sh" > "$W/base/scripts/setup-corrald.sh"
git -C "$ROOT" show "$BASE:scripts/setup-corrald-linux.sh" > "$W/base/scripts/setup-corrald-linux.sh"
cp "$ROOT/scripts/setup-corrald.sh" "$W/head/scripts/setup-corrald.sh"
cp "$ROOT/scripts/setup-corrald-linux.sh" "$W/head/scripts/setup-corrald-linux.sh"

# ---- launchd side: three agents, one fixture per side ------------------------
build_plists() { # $1=side (also the dir name; $W/$1/scripts holds the installer)
  local side="$1" home="$W/$1/home" rel="$W/$1/release" stub="$W/$1/stub-bin"
  mkdir -p "$home/Library/LaunchAgents" "$rel/scripts" "$stub" "$W/$1/config"
  cp "$W/$side/scripts/setup-corrald.sh" "$rel/scripts/setup-corrald.sh"
  cp "$ROOT/scripts/lib-corral-update-path.sh" "$ROOT/scripts/update-corral.sh" \
     "$ROOT/scripts/rotate-corral-logs.sh" "$rel/scripts/"
  printf '#!/usr/bin/env bash\nexit 0\n' > "$rel/corrald"
  chmod +x "$rel/corrald" "$rel/scripts/"*.sh
  printf '%s\n' '#!/usr/bin/env bash' \
    'printf "%s\n" "$*" >> "${CORRAL_TEST_LAUNCHCTL_LOG:?}"' \
    'exit 0' > "$stub/launchctl"
  printf '#!/usr/bin/env bash\nexit 0\n' > "$stub/curl"
  chmod +x "$stub/launchctl" "$stub/curl"
  # env -i: the only $HOME these runs may see is the fixture (a stray
  # XDG_CONFIG_HOME would point the Linux emitter at the real user config).
  env -i HOME="$home" CORRAL_CONFIG_DIR="$W/$side/config" \
    CORRAL_TEST_LAUNCHCTL_LOG="$W/$side/launchctl.log" \
    PATH="$stub:/usr/bin:/bin" \
    bash "$rel/scripts/setup-corrald.sh" --from-release "$rel/corrald" \
    > "$W/$side/plist-setup.log" 2>&1 \
    || { cat "$W/$side/plist-setup.log" >&2; fail "plist setup failed for $side"; }
}

# ---- systemd side: one unit per side ----------------------------------------
build_unit() { # $1=side
  local side="$1" home="$W/$1/lin-home" stub="$W/$1/lin-stub" bin="$W/$1/lin-bin"
  mkdir -p "$home" "$stub" "$bin" "$W/$1/lin-config"
  printf '#!/usr/bin/env bash\nexit 0\n' > "$stub/systemctl"
  printf '#!/usr/bin/env bash\nexit 0\n' > "$stub/curl"
  printf '#!/usr/bin/env bash\nexit 0\n' > "$bin/corrald"
  chmod +x "$stub/systemctl" "$stub/curl" "$bin/corrald"
  env -i HOME="$home" PATH="$stub:/usr/bin:/bin" \
    CORRAL_CONFIG_DIR="$W/$1/lin-config" \
    bash "$W/$side/scripts/setup-corrald-linux.sh" --from-release "$bin/corrald" --changed no \
    > "$W/$side/unit-setup.log" 2>&1 \
    || { cat "$W/$side/unit-setup.log" >&2; fail "unit setup failed for $side"; }
}

for side in base head; do
  build_plists "$side"
  build_unit "$side"
done

# The two sides run from different fixture directories, so every generated path
# is normalized to @SIDE@ before diffing: what is left is the real change.
normalize() { # $1=side $2=path relative to $W/$1 -> prints the normalized copy
  local out="$W/$1/norm/$(printf '%s' "$2" | tr '/' '_')"
  mkdir -p "$(dirname "$out")"
  sed "s|$W/$1|@SIDE@|g" "$W/$1/$2" > "$out"
  printf '%s\n' "$out"
}

status=0
for agent in com.corral.corrald com.corral.corrald-update com.corral.corrald-rotate; do
  rel="home/Library/LaunchAgents/$agent.plist"
  a="$(normalize base "$rel")"
  b="$(normalize head "$rel")"
  echo
  echo "=== launchd $agent.plist: base vs head (paths normalized to @SIDE@) ==="
  diff -u "$a" "$b" || true
  removed="$(diff "$a" "$b" | grep -c '^< ' || true)"
  added="$(diff "$a" "$b" | grep -c '^> ' || true)"
  proc="$(diff "$a" "$b" | grep -c '^[<>] .*<key>ProcessType</key>' || true)"
  old_val="$(diff "$a" "$b" | grep -c '^< .*<string>Background</string>' || true)"
  new_val="$(diff "$a" "$b" | grep -c '^> .*<string>Interactive</string>' || true)"
  [[ "$removed" == "1" && "$added" == "1" && "$proc" == "2" && "$old_val" == "1" && "$new_val" == "1" ]] \
    || { echo "UNEXPECTED: $agent.plist removed=$removed added=$added processTypeLines=$proc old=$old_val new=$new_val (want 1/1/2/1/1)" >&2; status=1; }
done

rel="lin-home/.config/systemd/user/corrald.service"
ua="$(normalize base "$rel")"
ub="$(normalize head "$rel")"
echo
echo "=== systemd corrald.service: base vs head (paths normalized to @SIDE@) ==="
diff -u "$ua" "$ub" || true
unit_removed="$(diff "$ua" "$ub" | grep -c '^< ' || true)"
unit_adds="$(diff "$ua" "$ub" | grep '^> ' | sed 's/^> //' || true)"
unit_block=""
IFS= read -r -d '' unit_block <<'EOF' || true
# #555: answer under load — Nice/CPUWeight raise the daemon's scheduling weight
# and CPU share while the rest of the host is saturated (see the header).
Nice=-5
CPUWeight=500
EOF
{ [[ "$unit_removed" == "0" && "$unit_adds" == "${unit_block%$'\n'}" ]] \
    || { echo "UNEXPECTED: unit removed=$unit_removed; added lines:
$unit_adds
(want 0 removed and exactly the four #555 lines)" >&2; status=1; }; }

# The hardening block must be byte-identical, not merely present: compare the
# section from the hardening comment to the end of [Service] on both sides.
sed -n '/^# Hardening:/,/^StandardError=journal$/p' "$ua" > "$W/hardening-base.txt"
sed -n '/^# Hardening:/,/^StandardError=journal$/p' "$ub" > "$W/hardening-head.txt"
echo
echo "=== hardening block (base vs head, must be identical) ==="
cat "$W/hardening-base.txt"
cmp -s "$W/hardening-base.txt" "$W/hardening-head.txt" || { echo "UNEXPECTED: hardening block changed" >&2; status=1; }
shasum -a 256 "$W/hardening-base.txt" "$W/hardening-head.txt"

echo
if [[ "$status" == "0" ]]; then
  echo "PASS: base-vs-head generated files differ only by the #555 scheduling lines"
else
  echo "FAIL: unexpected differences above" >&2
fi
exit "$status"
