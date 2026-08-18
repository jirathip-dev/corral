#!/usr/bin/env bash
# corrald-grant.sh — grant/revoke drive capabilities for a device key
#
# The host promotes a registered device from read-only to specific
# capabilities via the admin token (never hands the token to the device).
#
# Usage:
#   scripts/corrald-grant.sh --key <key_id> --caps read_tail,prompt
#   scripts/corrald-grant.sh --key <key_id> --revoke
#   scripts/corrald-grant.sh --list
#
# Requires the daemon to be running and CORRAL_CONFIG_DIR (default
# ~/.config/corral) to hold admin-token.
set -euo pipefail

CONFIG_DIR="${CORRAL_CONFIG_DIR:-$HOME/.config/corral}"
ADMIN="$(cat "$CONFIG_DIR/admin-token" 2>/dev/null || { echo "no admin-token in $CONFIG_DIR — is corrald set up?" >&2; exit 1; })"
BASE="${CORRAL_BASE:-http://127.0.0.1:8474}"

KEY=""; CAPS=""; REVOKE=0; LIST=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --key) KEY="$2"; shift 2 ;;
    --caps) CAPS="$2"; shift 2 ;;
    --revoke) REVOKE=1; shift ;;
    --list) LIST=1; shift ;;
    --base) BASE="$2"; shift 2 ;;
    *) echo "unknown: $1" >&2; exit 2 ;;
  esac
done

if [[ "$LIST" == "1" ]]; then
  echo ">> devices:"
  # registry.json holds the device keys + grants
  python3 - "$CONFIG_DIR/registry.json" <<'PY'
import json,sys
try:
    reg=json.load(open(sys.argv[1]))
except Exception as e:
    print(f"  (cannot read registry: {e})"); sys.exit(0)
for k,v in reg.get("devices",{}).items():
    print(f"  {v.get('key_id')}  grants={v.get('grants')}  revoked={v.get('revoked')}  expires={v.get('expiry_ts')}")
PY
  exit 0
fi

[[ -n "$KEY" ]] || { echo "need --key (and --caps to grant, or --revoke)" >&2; exit 2; }
if [[ "$REVOKE" != "1" && -z "$CAPS" ]]; then
  echo "need --caps <list> to grant, or --revoke" >&2; exit 2
fi

# Build the whole JSON body in python3 — safe for both --key and --caps
# (no shell-side splicing; a malicious value is inert data, the daemon's
# grant validation is the real gate).
if [[ "$REVOKE" == "1" ]]; then
  BODY="$(python3 -c 'import json,sys; print(json.dumps({"action":"revoke","key_id":sys.argv[1],"revoked":True}))' "$KEY")"
  echo ">> revoking $KEY"
else
  BODY="$(python3 -c 'import json,sys; k,c=sys.argv[1],sys.argv[2]; print(json.dumps({"action":"set_grants","key_id":k,"grants":[x.strip() for x in c.split(",") if x.strip()]}))' "$KEY" "$CAPS")"
  echo ">> granting $KEY: $CAPS"
fi

curl -fsS -X POST "$BASE/grants" \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer $ADMIN" \
  -d "$BODY" || { echo "!! grants request failed (HTTP error)" >&2; exit 1; }
echo
