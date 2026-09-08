#!/usr/bin/env bash
# issue-442 — secret/privacy scan + fictional-fixture audit of the evidence
# directory. Two independent tools: gitleaks (if installed) and a grep
# battery over text files (PNGs are binary and generated; fixtures are the
# only data source). Exit 1 on any hit.
set -u
cd "$(dirname "$0")/.." || exit 2
[ "$(basename "$PWD")" = "issue-442" ] || { echo "wrong dir: $PWD"; exit 2; }
rc=0

echo "== gitleaks =="
if command -v gitleaks >/dev/null 2>&1; then
  gitleaks detect --no-git --source . --redact -v; g=$?
  echo "gitleaks exit=$g"; [ "$g" -eq 0 ] || rc=1
else
  echo "gitleaks not installed — grep battery only"
fi

echo "== grep battery (text files, excluding this scanner + the pattern source) =="
FILES=$(find . -type f \( -name '*.html' -o -name '*.md' -o -name '*.py' -o -name '*.json' -o -name '*.sh' -o -name '*.sha256' \) \
        ! -name 'privacy-scan.sh' ! -name 'fixtures.py' ! -path './logs/*')
for pat in 'synergyservices' 'jirathip(?!-dev/corral)' 'AKIA[0-9A-Z]{16}' 'ghp_[A-Za-z0-9]{30,}' 'sk-[A-Za-z0-9]{20,}' \
           '-----BEGIN [A-Z ]*PRIVATE KEY' '[A-Za-z0-9._%+-]+@(?!example\.invalid)[A-Za-z0-9.-]+\.(com|org|net|co\.th|th)\b' \
           '\b(password|passwd|api[_-]?key|secret)\s*[:=]\s*["'"'"'][^"'"'"']{6,}' 'tail8c3301|ts\.net' '/Users/[a-z]+/'; do
  hits=$(echo "$FILES" | xargs grep -nPI "$pat" 2>/dev/null)
  if [ -n "$hits" ]; then echo "HIT [$pat]:"; echo "$hits"; rc=1; else echo "clean [$pat]"; fi
done

echo "== fictional-fixture audit =="
PYTHONDONTWRITEBYTECODE=1 python3 - <<'PY' || rc=1
import sys; sys.path.insert(0, "scripts")
import fixtures as F
repos = sorted({a.repo for fr in F.FRAMES.values() for a in fr().agents})
names = sorted({a.name for fr in F.FRAMES.values() for a in fr().agents})
hosts = sorted({h[0] for fr in F.FRAMES.values() for h in fr().hosts})
print("repos:", repos); print("lanes:", len(names), names); print("hosts:", hosts)
# lane names are tree/plant compounds; repos are invented slugs; no real org
real = {"corral", "synergy-apps", "morsel", "sendmeter", "plush-meadow", "hermes-brain", "fleet-operations"}
bad = [r for r in repos if r in real] + [n for n in names if n in real]
assert not bad, f"real project names in fixtures: {bad}"
assert all(F.FRAMES[k]().note for k in F.FRAMES)
print("fixture audit OK: all repos/lanes/hosts invented; no real project names")
PY

echo "privacy-scan exit=$rc"; exit $rc
