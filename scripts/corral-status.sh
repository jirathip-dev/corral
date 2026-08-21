#!/usr/bin/env bash
# Read-only Herdr plugin status action for Corral.
#
# The action deliberately checks the daemon's public liveness endpoint before
# parsing the public snapshot endpoint. It prints counts and cursors only;
# the snapshot itself can contain fleet metadata and is never echoed.
set -euo pipefail

BASE_URL="${CORRALD_URL:-http://127.0.0.1:8474}"
BASE_URL="${BASE_URL%/}"
CURL_BIN="${CORRAL_CURL_BIN:-curl}"
TIMEOUT="${CORRAL_STATUS_TIMEOUT_SECONDS:-5}"

if [[ ! "$TIMEOUT" =~ ^[1-9][0-9]*$ ]]; then
  echo "corral status: CORRAL_STATUS_TIMEOUT_SECONDS must be a positive integer" >&2
  exit 2
fi

CURL_ARGS=(--fail --silent --show-error --max-time "$TIMEOUT")

if ! command -v "$CURL_BIN" >/dev/null 2>&1; then
  echo "corral status: corrald is unavailable (GET /healthz failed)" >&2
  exit 1
fi

if ! health="$("$CURL_BIN" "${CURL_ARGS[@]}" "$BASE_URL/healthz")"; then
  echo "corral status: corrald is unavailable (GET /healthz failed)" >&2
  exit 1
fi
health="${health//$'\r'/}"
health="${health//$'\n'/}"
if [[ "$health" != "ok" ]]; then
  echo "corral status: corrald returned an unexpected /healthz response" >&2
  exit 1
fi

if ! snapshot="$("$CURL_BIN" "${CURL_ARGS[@]}" "$BASE_URL/snapshot")"; then
  echo "corral status: corrald is unhealthy (GET /snapshot failed)" >&2
  exit 1
fi

if ! summary="$(printf '%s' "$snapshot" | python3 -c '
import json
import sys

try:
    payload = json.load(sys.stdin)
    agents = payload["agents"]
    rev = payload["rev"]
    schema = payload["schema_version"]
    if not isinstance(agents, dict):
        raise TypeError("agents is not an object")
    if not isinstance(rev, int) or not isinstance(schema, int):
        raise TypeError("snapshot cursors are not integers")
except (KeyError, TypeError, ValueError, json.JSONDecodeError) as exc:
    print(f"invalid snapshot: {exc}", file=sys.stderr)
    raise SystemExit(1)

print(f"fleet: {len(agents)} agents (snapshot rev {rev}, schema {schema})")
')"; then
  echo "corral status: invalid /snapshot response" >&2
  exit 1
fi

printf 'corrald: healthy\n%s\n' "$summary"
