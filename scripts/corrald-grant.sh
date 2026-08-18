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

[[ -n "$KEY" && -n "$CAPS" ]] || { echo "need --key and --caps (or --list)" >&2; exit 2; }

if [[ "$REVOKE" == "1" ]]; then
  BODY="{\"action\":\"revoke\",\"key_id\":\"$KEY\",\"revoked\":true}"
  echo ">> revoking $KEY"
else
  # --caps is a comma list; build a proper JSON array (python3 is present on
  # macOS + CI runners; no jq dependency)
  ARR="$(python3 -c 'import json,sys; print(json.dumps([c.strip() for c in sys.argv[1].split(",") if c.strip()]))' "$CAPS")"
  BODY="{\"action\":\"set_grants\",\"key_id\":\"$KEY\",\"grants\":$ARR}"
  echo ">> granting $KEY: $CAPS"
fi

curl -sS -X POST "$BASE/grants" \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer $ADMIN" \
  -d "$BODY"
echo
