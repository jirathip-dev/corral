#!/usr/bin/env bash
# #563 RED/GREEN evidence driver — re-runnable from anywhere in this worktree.
#
# The two integration-surface tests below are byte-identical in every phase;
# only the PRODUCTION bytes of the fence files change, so the same test both
# bites at the pre-fix head and passes at the fixed head.
#
# Phases
#   1  RED (redaction): fence files reverted to BASE, tests kept at the fixed
#      head. Both tests must FAIL with the raw credential-shaped value
#      observed (gh_plane e2e) / the binding lost (integration).
#   2  GREEN: the committed fixed sources; the same tests must pass, plus the
#      in-crate unit tests (new #563 test and the updated #556 identity test).
#   3  RED (binding, masking mutation): the gh wire's identity-preserving
#      redaction is replaced by a naive mask (`[REDACTED]`, no hashed
#      identity) while the local side keeps hashing — the pipeline binding
#      test must FAIL (the naive mask costs the binding).
#   4  RESTORED: the mutated file is restored from the committed head; its
#      SHA-256 must equal the pre-mutation value and the same tests must pass
#      again (byte-identity proven before/after).
#
# Outputs: docs/evidence/issue-563/logs/*.log (raw stdout+stderr per run) and
# docs/evidence/issue-563/redgreen.json (raw exit codes + the observed lines).
set -u
cd "$(dirname "$0")/../../.."

BASE="${BASE:-e0fe2e6c46d8dc3ecec36680850c29dd6778323e}"
GH=src/adapters/gh_plane.rs
INT=src/integrate/mod.rs
LOGS=docs/evidence/issue-563/logs
OUT=docs/evidence/issue-563/redgreen.json
mkdir -p "$LOGS"

sha() { shasum -a 256 "$1" | awk '{print $1}'; }
run() { LOG="$1"; shift; "$@" >"$LOG" 2>&1; RC=$?; }
fail() { echo "DRIVER ABORT: $*" >&2; exit 2; }

HEAD_SHA=$(git rev-parse HEAD)
GH_FIXED=$(sha "$GH")
INT_FIXED=$(sha "$INT")
echo "driver HEAD=$HEAD_SHA BASE=$BASE"
echo "  $GH  sha256=$GH_FIXED"
echo "  $INT sha256=$INT_FIXED"

# ---------------------------------------------------------------- phase 1
git checkout "$BASE" -- "$GH" "$INT"
[ "$(sha "$GH")" != "$GH_FIXED" ] || fail "base checkout did not change $GH"
run "$LOGS/red-base-gh-plane.log" cargo test --test gh_plane g563_
RED_GH=$RC
run "$LOGS/red-base-integration.log" cargo test --test integration credential_shaped
RED_INT=$RC
git checkout "$HEAD_SHA" -- "$GH" "$INT"
[ "$(sha "$GH")" = "$GH_FIXED" ] || fail "phase-1 restore changed $GH"
[ "$(sha "$INT")" = "$INT_FIXED" ] || fail "phase-1 restore changed $INT"

# ---------------------------------------------------------------- phase 2
run "$LOGS/green-gh-plane.log" cargo test --test gh_plane g563_
GREEN_GH=$RC
run "$LOGS/green-integration.log" cargo test --test integration credential_shaped
GREEN_INT=$RC
run "$LOGS/green-unit-g563.log" cargo test --lib g563_
GREEN_UNIT=$RC
run "$LOGS/green-unit-g556.log" cargo test --lib g556_redaction_preserves_binding_identity
GREEN_G556=$RC

# ---------------------------------------------------------------- phase 3
# The naive-mask mutation: the ref keys are simply masked, with no hashed
# identity to compare against the LOCAL side's projection.
python3 - "$GH" <<'PY' || fail "mutation anchor drift"
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
source = path.read_text()
old = """                    if !token.is_empty() && identity.contains(token) {
                        *identity = ref_identity_placeholder(identity);
                    } else {
                        *identity = ref_identity(identity);
                    }
"""
new = """                    // #563 EVIDENCE MUTATION: naive mask, no hashed identity.
                    let _ = token;
                    *identity = crate::core::redact::REDACTED.to_string();
"""
assert source.count(old) == 1, "anchor drift: ref identity block not found exactly once"
path.write_text(source.replace(old, new))
PY
[ "$(sha "$GH")" != "$GH_FIXED" ] || fail "mutation did not change $GH"
run "$LOGS/red-mutation-binding.log" cargo test --test gh_plane g563_
MUT_GH=$RC
run "$LOGS/red-mutation-integration.log" cargo test --test integration credential_shaped
MUT_INT=$RC
git checkout "$HEAD_SHA" -- "$GH"
[ "$(sha "$GH")" = "$GH_FIXED" ] || fail "mutation restore is NOT byte-identical"

# ---------------------------------------------------------------- phase 4
run "$LOGS/restored-gh-plane.log" cargo test --test gh_plane g563_
REST_GH=$RC
run "$LOGS/restored-integration.log" cargo test --test integration credential_shaped
REST_INT=$RC

# ---------------------------------------------------------------- summary
observed() { grep -m3 -E "panicked at|left:|right:|never bound PR|test result:" "$1" | sed 's/^/    /'; }
echo "observed in $LOGS/red-base-gh-plane.log:";      observed "$LOGS/red-base-gh-plane.log"
echo "observed in $LOGS/red-base-integration.log:";   observed "$LOGS/red-base-integration.log"
echo "observed in $LOGS/red-mutation-binding.log:";   observed "$LOGS/red-mutation-binding.log"

cat >"$OUT" <<JSON
{
  "issue": 563,
  "driver": "docs/evidence/issue-563/redgreen.sh",
  "head": "$HEAD_SHA",
  "base": "$BASE",
  "fence_sha256": {"$GH": "$GH_FIXED", "$INT": "$INT_FIXED"},
  "phases": [
    {"phase": "red-base", "file": "$GH", "test": "cargo test --test gh_plane g563_", "exit": $RED_GH},
    {"phase": "red-base", "file": "$INT", "test": "cargo test --test integration credential_shaped", "exit": $RED_INT},
    {"phase": "green-fixed", "test": "cargo test --test gh_plane g563_", "exit": $GREEN_GH},
    {"phase": "green-fixed", "test": "cargo test --test integration credential_shaped", "exit": $GREEN_INT},
    {"phase": "green-fixed", "test": "cargo test --lib g563_", "exit": $GREEN_UNIT},
    {"phase": "green-fixed", "test": "cargo test --lib g556_redaction_preserves_binding_identity", "exit": $GREEN_G556},
    {"phase": "red-mutation-naive-mask", "test": "cargo test --test gh_plane g563_", "exit": $MUT_GH},
    {"phase": "red-mutation-naive-mask", "test": "cargo test --test integration credential_shaped", "exit": $MUT_INT},
    {"phase": "restored", "test": "cargo test --test gh_plane g563_", "exit": $REST_GH},
    {"phase": "restored", "test": "cargo test --test integration credential_shaped", "exit": $REST_INT}
  ]
}
JSON

echo "summary:"
echo "  RED  base   gh_plane=$RED_GH integration=$RED_INT"
echo "  GREEN fixed gh_plane=$GREEN_GH integration=$GREEN_INT unit563=$GREEN_UNIT unit556=$GREEN_G556"
echo "  RED  mutation gh_plane=$MUT_GH integration=$MUT_INT"
echo "  RESTORED gh_plane=$REST_GH integration=$REST_INT (byte-identity verified)"
echo "wrote $OUT"

# Expectations: RED phases must be nonzero (tests failed); GREEN/RESTORED
# phases must be zero.
[ "$RED_GH" -ne 0 ] && [ "$RED_INT" -ne 0 ] && [ "$MUT_GH" -ne 0 ] \
  && [ "$GREEN_GH" -eq 0 ] && [ "$GREEN_INT" -eq 0 ] && [ "$GREEN_UNIT" -eq 0 ] \
  && [ "$GREEN_G556" -eq 0 ] && [ "$REST_GH" -eq 0 ] && [ "$REST_INT" -eq 0 ] \
  || fail "phase exits do not match the expected RED/GREEN pattern"
echo "DRIVER OK"
