#!/usr/bin/env bash
# docs/evidence/issue-555/probes.sh — #555 installer-half mutation battery.
#
# Each probe mutates one delivered byte range, runs the suite that owns it, and
# asserts the suite goes RED (raw non-zero exit). The file is then restored and
# the restore is proven byte-identical two ways: `shasum -a 256` before == after,
# and `git diff --exit-code -- <file>` clean (the file must be committed, so run
# this after the implementation commit with a clean tree for these paths).
#
# Usage (from the repository root):
#   bash docs/evidence/issue-555/probes.sh
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$ROOT" || exit 2
SETUP="scripts/setup-corrald.sh"
LINUX="scripts/setup-corrald-linux.sh"
LOG_DIR="${G555_PROBE_LOG_DIR:-/tmp}"
BACKUP_DIR="$(mktemp -d /tmp/g555-probe-backup.XXXXXX)"
trap 'rm -rf -- "$BACKUP_DIR"' EXIT

failures=0
sha256() { shasum -a 256 "$1" | awk '{print $1}'; }

mutate() { # $1=file $2=exact old text $3=exact new text $4=all|first
  python3 - "$1" "$2" "$3" "${4:-all}" <<'PY'
import sys
path, old, new, mode = sys.argv[1:5]
src = open(path, encoding="utf-8").read()
if old not in src:
    sys.exit("mutation target not found in %s: %r" % (path, old))
out = src.replace(old, new) if mode == "all" else src.replace(old, new, 1)
if out == src:
    sys.exit("mutation was a no-op in %s" % path)
open(path, "w", encoding="utf-8").write(out)
PY
}

snapshot() { # $1=probe name $2=file
  BEFORE_SHA="$(sha256 "$2")"
  cp "$2" "$BACKUP_DIR/$1"
}

run_probe() { # $1=probe name $2=file $3+=suite command
  local name="$1" file="$2"; shift 2
  local suite="$*"
  local log="$LOG_DIR/g555-probe-$name.log" rc restore="sha-equal"
  "$@" > "$log" 2>&1
  rc=$?
  cp "$BACKUP_DIR/$name" "$file"
  [[ "$(sha256 "$file")" == "$BEFORE_SHA" ]] || restore="SHA-MISMATCH"
  git diff --exit-code -- "$file" >/dev/null 2>&1 || restore="$restore,git-diff-dirty"
  local verdict="GREEN(not-biting)"
  [[ "$rc" -ne 0 ]] && verdict="RED(ok)"
  printf '%-32s exit=%-3s %-18s restore=%s\n  suite: %s\n  log:   %s\n' \
    "$name" "$rc" "$verdict" "$restore" "$suite" "$log"
  [[ "$rc" -ne 0 && "$restore" == "sha-equal" ]] || failures=$((failures + 1))
}

echo "== #555 mutation battery (delivered head: $(git rev-parse --short HEAD)) =="

# P1 — the whole pre-#555 emitter state: every agent back to Background.
snapshot p1-process-type-background "$SETUP"
mutate "$SETUP" \
  '<key>ProcessType</key><string>Interactive</string>' \
  '<key>ProcessType</key><string>Background</string>' all
run_probe p1-process-type-background "$SETUP" bash scripts/test-daemon-launchd-env.sh

# P2 — only the corrald agent back to Background (the daemon's own tier).
snapshot p2-corrald-process-type "$SETUP"
mutate "$SETUP" \
  '<key>ProcessType</key><string>Interactive</string>' \
  '<key>ProcessType</key><string>Background</string>' first
run_probe p2-corrald-process-type "$SETUP" bash scripts/test-daemon-launchd-env.sh

# P3 — a duplicate ProcessType key (what an append-instead-of-replace migration
# leaves behind): the value still reads Interactive for a plist library, so only
# the duplicate-key assertion can catch it.
snapshot p3-duplicate-process-type "$SETUP"
mutate "$SETUP" \
  '  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Interactive</string>' \
  '  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Interactive</string>
  <key>ProcessType</key><string>Interactive</string>' first
run_probe p3-duplicate-process-type "$SETUP" bash scripts/test-daemon-launchd-env.sh

# P4 — non-idempotent migration: append instead of replace (the brief's example).
snapshot p4-append-instead-of-replace "$SETUP"
mutate "$SETUP" \
  'cat > "$PLIST" <<PLIST_EOF' \
  'cat >> "$PLIST" <<PLIST_EOF' first
run_probe p4-append-instead-of-replace "$SETUP" bash scripts/test-daemon-launchd-env.sh

# P5 — the Linux unit loses the two scheduling directives.
snapshot p5-drop-nice-cpuweight "$LINUX"
mutate "$LINUX" \
  'Nice=$NICE_VALUE
CPUWeight=$CPU_WEIGHT_VALUE
' '' first
run_probe p5-drop-nice-cpuweight "$LINUX" bash scripts/test-install-corral-linux.sh

# P6 — the Linux migration loses its report (silent migration).
snapshot p6-silent-migration "$LINUX"
mutate "$LINUX" \
  '  report_scheduling_migration "$old_nice" "$old_cpu_weight"
' '' first
run_probe p6-silent-migration "$LINUX" bash scripts/test-install-corral-linux.sh

echo
if [[ "$failures" == "0" ]]; then
  echo "PASS: every mutation went RED and every restore is byte-identical"
else
  echo "FAIL: $failures probe(s) did not bite or did not restore" >&2
fi
exit "$failures"
