#!/usr/bin/env bash
# Small, offline proof for the status action's endpoint and summary contract.
set -euo pipefail

cd "$(dirname "$0")/.."
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$WORK/bin"
cat > "$WORK/bin/curl" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
url="${!#}"
printf '%s\n' "$url" >> "$CURL_LOG"
case "$url" in
  */healthz) printf 'ok\n' ;;
  */snapshot) printf '%s\n' '{"schema_version":4,"rev":17,"generated_at":1,"agents":{"herdr:a":{"agent_id":"herdr:a"}}}' ;;
  *) exit 22 ;;
esac
STUB
chmod +x "$WORK/bin/curl"

export PATH="$WORK/bin:$PATH"
export CURL_LOG="$WORK/curl.log"
export CORRALD_URL="http://127.0.0.1:18474/"

output="$(scripts/corral-status.sh)"
printf '%s\n' "$output" | grep -Fx 'corrald: healthy' >/dev/null
printf '%s\n' "$output" | grep -Fx 'fleet: 1 agents (snapshot rev 17, schema 4)' >/dev/null

printf '%s\n' \
  'http://127.0.0.1:18474/healthz' \
  'http://127.0.0.1:18474/snapshot' > "$WORK/expected.log"
diff -u "$WORK/expected.log" "$CURL_LOG"

echo "OK: plugin status checks /healthz and summarizes /snapshot"
