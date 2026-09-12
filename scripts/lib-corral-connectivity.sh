#!/usr/bin/env bash
# lib-corral-connectivity.sh — bounded private-connectivity checks and a
# consent-gated Serve mapping for the supported path: corrald on loopback,
# fronted by real TLS via private Tailscale Serve (docs/CONNECTIVITY.md).
#
# Source-only library: defines corral_conn_main() and helpers for
# `scripts/corral-status.sh --connectivity`. It is not runnable directly.
#
# Safety contract (enforced by scripts/test-corral-connectivity.sh):
#   * The check is READ-ONLY: it runs `tailscale status --json` and
#     `tailscale serve status --json` only. A mutating Tailscale command is
#     reachable only via `--apply --yes` (explicit consent), after a fresh
#     state inspection, and never overwrites an unrelated mapping.
#   * `tailscale`/`curl` resolve from PATH only (CORRAL_TAILSCALE_BIN /
#     CORRAL_CURL_BIN); there is no fallback to a well-known install location,
#     so a hermetic fixture PATH fully isolates the probes from live services.
#   * Supported path only: loopback corrald + private HTTPS Serve. Never
#     Funnel, never a public/raw tailnet bind, never a TLS-verification
#     bypass, never ACL/account changes.
#   * Fail closed: an unrecognized Tailscale JSON schema, a conflicting Serve
#     mapping, or a non-loopback origin is reported, never guessed, never
#     overwritten.
#   * Subprocess stdout/stderr is discarded; only allowlisted, charset-checked
#     tokens are echoed, so credentials or pairing material cannot reach the
#     report.
#
# Exit codes (first failing class wins): 0 ok, 1 herdr, 2 usage/consent/config,
# 3 corrald, 4 tailscale, 5 serve, 6 tls.
set -euo pipefail

CORRAL_CONN_EXIT_HERDR=1
CORRAL_CONN_EXIT_USAGE=2
CORRAL_CONN_EXIT_CORRALD=3
CORRAL_CONN_EXIT_TAILSCALE=4
CORRAL_CONN_EXIT_SERVE=5
CORRAL_CONN_EXIT_TLS=6

# --------------------------------------------------------------- helpers ---

corral_conn_usage() {
  printf '%s\n' \
    'usage: corral-status.sh --connectivity [--apply --yes]' \
    '' \
    '  --connectivity   read-only bounded check of the private path:' \
    '                   Herdr socket, corrald, Tailscale, Serve, TLS/HTTP.' \
    '  --apply --yes    add the missing loopback Serve HTTPS mapping.' \
    '                   Requires explicit consent via --yes; the current' \
    '                   Serve state is inspected first and a conflicting' \
    '                   mapping fails closed (never overwritten).' \
    '' \
    'exit codes: 0 ok; 1 herdr; 2 usage/consent/config; 3 corrald;' \
    '            4 tailscale; 5 serve; 6 tls (first failing class wins)'
}

# Masks Tailscale credential shapes as defense in depth. Values echoed by this
# library are already charset-allowlisted by the JSON evaluators; this pass
# exists so a future message cannot leak a key even by accident.
corral_conn_redact() {
  local value="$1"
  value="${value//tskey-*/tskey-[redacted]}"
  value="${value//login.tailscale.com\/a\/*/login.tailscale.com\/a\/[redacted]}"
  printf '%s' "$value"
}

# Strips characters that are not part of a URL-shaped value before echoing it
# back in an error message (no control characters, no shell metacharacters).
corral_conn_sanitize_url() {
  local value="$1" out="" char
  local i
  for ((i = 0; i < ${#value}; i++)); do
    char="${value:$i:1}"
    case "$char" in
      [A-Za-z0-9:/._\[\]@-]) out="$out$char" ;;
      *) out="$out?" ;;
    esac
  done
  printf '%s' "$out"
}

# ---------------------------------------------------------------- config ---

corral_conn_load_config() {
  local re url port

  CORRAL_CONN_TIMEOUT="${CORRAL_STATUS_TIMEOUT_SECONDS:-5}"
  case "$CORRAL_CONN_TIMEOUT" in
    '' | *[!0-9]* | 0)
      printf 'corral connectivity: CORRAL_STATUS_TIMEOUT_SECONDS must be a positive integer\n' >&2
      return 2
      ;;
  esac

  local https_port="${CORRAL_CONNECTIVITY_HTTPS_PORT:-443}"
  case "$https_port" in
    '' | *[!0-9]*)
      printf 'corral connectivity: CORRAL_CONNECTIVITY_HTTPS_PORT must be an integer\n' >&2
      return 2
      ;;
  esac
  CORRAL_CONN_HTTPS_PORT=$((10#$https_port))
  if [[ "$CORRAL_CONN_HTTPS_PORT" -lt 1 || "$CORRAL_CONN_HTTPS_PORT" -gt 65535 ]]; then
    printf 'corral connectivity: CORRAL_CONNECTIVITY_HTTPS_PORT must be 1-65535\n' >&2
    return 2
  fi

  # The only supported topology is the loopback daemon fronted by private
  # HTTPS; keep the origin comparison and the Serve target strictly loopback.
  url="${CORRALD_URL:-http://127.0.0.1:8474}"
  re='^http://(127\.0\.0\.1|localhost|\[::1\]):([0-9]{1,5})$'
  if [[ ! "$url" =~ $re ]]; then
    printf 'corral connectivity: CORRALD_URL must be a loopback http origin such as http://127.0.0.1:8474 (got %s)\n' \
      "$(corral_conn_sanitize_url "$url")" >&2
    return 2
  fi
  port=$((10#${BASH_REMATCH[2]}))
  if [[ "$port" -lt 1 || "$port" -gt 65535 ]]; then
    printf 'corral connectivity: CORRALD_URL port must be 1-65535\n' >&2
    return 2
  fi
  CORRAL_CONN_ORIGIN="http://127.0.0.1:$port"

  CORRAL_CONN_HERDR_SOCKET="${HERDR_SOCKET:-${HOME:-}/.config/herdr/herdr.sock}"
  CORRAL_CONN_APPLY_TIMEOUT="${CORRAL_CONNECTIVITY_APPLY_TIMEOUT_SECONDS:-60}"
  case "$CORRAL_CONN_APPLY_TIMEOUT" in
    '' | *[!0-9]* | 0)
      printf 'corral connectivity: CORRAL_CONNECTIVITY_APPLY_TIMEOUT_SECONDS must be a positive integer\n' >&2
      return 2
      ;;
  esac

  CORRAL_CONN_PYTHON="$(command -v "${CORRAL_PYTHON_BIN:-python3}" 2>/dev/null || true)"
  if [[ -z "$CORRAL_CONN_PYTHON" ]]; then
    printf 'corral connectivity: python3 not found (required for the bounded JSON probes); set CORRAL_PYTHON_BIN\n' >&2
    return 2
  fi
  CORRAL_CONN_CURL="$(command -v "${CORRAL_CURL_BIN:-curl}" 2>/dev/null || true)"
  CORRAL_CONN_TAILSCALE="$(command -v "${CORRAL_TAILSCALE_BIN:-tailscale}" 2>/dev/null || true)"
  return 0
}

# ------------------------------------------------------------- execution ---

# corral_conn_bounded <seconds> <command> [args...]
# Runs the command with a hard timeout (macOS has no coreutils `timeout`).
# stdout is forwarded; subprocess stderr is discarded so arbitrary CLI output
# cannot reach the report. Exit status is the command's, or 124 on timeout.
corral_conn_bounded() {
  "$CORRAL_CONN_PYTHON" -c '
import subprocess
import sys

try:
    seconds = float(sys.argv[1])
    proc = subprocess.run(
        sys.argv[2:],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        timeout=seconds,
    )
except subprocess.TimeoutExpired:
    sys.exit(124)
except OSError:
    sys.exit(127)
sys.stdout.buffer.write(proc.stdout)
sys.exit(proc.returncode)
' "$@"
}

# -------------------------------------------------------------- JSON eval ---

_CORRAL_CONN_EVAL_TAILSCALE='
import json
import re
import sys

def token(value, pattern):
    if isinstance(value, str) and re.match(pattern, value):
        return value
    return ""

raw = sys.stdin.read()
try:
    status = json.loads(raw) if raw.strip() else None
except ValueError:
    status = None
if not isinstance(status, dict):
    print("state=unrecognized")
    print("dns=")
    print("certdomains=0")
    raise SystemExit(0)

state = token(status.get("BackendState"), r"^[A-Za-z]{1,32}$")
dns = ""
self_value = status.get("Self")
if isinstance(self_value, dict):
    dns = token(self_value.get("DNSName"), r"^[A-Za-z0-9.-]{1,253}$")
    dns = dns.rstrip(".")
certs = status.get("CertDomains")
cert_count = len(certs) if isinstance(certs, list) else 0
print("state=%s" % (state if state else "unrecognized"))
print("dns=%s" % dns)
print("certdomains=%d" % cert_count)
'

_CORRAL_CONN_EVAL_SERVE='
import json
import re
import sys

PROXY_RE = re.compile(r"^[A-Za-z0-9:/._\[\]@-]{1,200}$")
HOSTPORT_RE = re.compile(r"^[A-Za-z0-9.-]{1,253}:[0-9]{1,5}$")
KNOWN_KEYS = {"TCP", "Web", "AllowFunnel", "Services", "ETag"}

hostport = sys.argv[1]
desired = sys.argv[2]
if not HOSTPORT_RE.match(hostport):
    print("serve=unrecognized")
    raise SystemExit(0)

raw = sys.stdin.read()
text = raw.strip()
if text in ("", "null", "{}"):
    print("serve=missing")
    raise SystemExit(0)
try:
    config = json.loads(text)
except ValueError:
    print("serve=unrecognized")
    raise SystemExit(0)
if not isinstance(config, dict) or (set(config) - KNOWN_KEYS):
    print("serve=unrecognized")
    raise SystemExit(0)

web = config.get("Web")
tcp = config.get("TCP")
funnel = config.get("AllowFunnel")
if web is None:
    web = {}
if tcp is None:
    tcp = {}
if funnel is None:
    funnel = {}
if not isinstance(web, dict) or not isinstance(tcp, dict) or not isinstance(funnel, dict):
    print("serve=unrecognized")
    raise SystemExit(0)

port = hostport.rsplit(":", 1)[1]
entry = web.get(hostport)
if entry is None:
    handler = tcp.get(port)
    if handler is not None:
        is_https = isinstance(handler, dict) and handler.get("HTTPS") is True
        if not is_https:
            print("serve=conflict")
            print("reason=port-in-use")
            raise SystemExit(0)
    print("serve=missing")
    raise SystemExit(0)

if not isinstance(entry, dict):
    print("serve=unrecognized")
    raise SystemExit(0)
handlers = entry.get("Handlers")
if not isinstance(handlers, dict):
    print("serve=unrecognized")
    raise SystemExit(0)
root = handlers.get("/")
if not isinstance(root, dict):
    print("serve=conflict")
    print("reason=no-root-handler")
    raise SystemExit(0)
proxy = root.get("Proxy")
if not isinstance(proxy, str) or not PROXY_RE.match(proxy):
    print("serve=conflict")
    print("reason=opaque-proxy")
    raise SystemExit(0)
if funnel.get(hostport) is True:
    print("serve=funnel")
    print("proxy=%s" % proxy.rstrip("/"))
    raise SystemExit(0)
print("proxy=%s" % proxy.rstrip("/"))
if proxy.rstrip("/") == desired.rstrip("/"):
    print("serve=match")
else:
    print("serve=conflict")
    print("reason=other-proxy")
'

corral_conn_eval_tailscale() {
  printf '%s' "$1" | "$CORRAL_CONN_PYTHON" -c "$_CORRAL_CONN_EVAL_TAILSCALE"
}

# Bounded, read-only transport-liveness probe for a unix socket: connect()
# once and close. Always prints one key=value line and exits 0; the caller
# classifies. A stale socket file (bind+close leftover, crashed daemon, or a
# daemon that never re-bound) passes every file check but refuses here.
_CORRAL_CONN_SOCKET_CONNECT='
import errno
import socket
import sys

path = sys.argv[1]
seconds = float(sys.argv[2])

client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
client.settimeout(seconds)
try:
    client.connect(path)
except socket.timeout:
    print("liveness=timeout")
except OSError as exc:
    code = exc.errno
    if code == errno.ECONNREFUSED:
        print("liveness=refused")
    elif code == errno.ENOENT:
        print("liveness=missing")
    elif code in (errno.EACCES, errno.EPERM):
        print("liveness=denied")
    else:
        print("liveness=error")
        print("errno=%s" % (code if code is not None else -1))
else:
    print("liveness=ok")
finally:
    client.close()
'

corral_conn_eval_serve() {
  printf '%s' "$1" | "$CORRAL_CONN_PYTHON" -c "$_CORRAL_CONN_EVAL_SERVE" "$2" "$3"
}

# Parses key=value lines into CORRAL_CONN_KV_* globals.
corral_conn_parse_kv() {
  CORRAL_CONN_KV_STATE=""
  CORRAL_CONN_KV_DNS=""
  CORRAL_CONN_KV_CERTDOMAINS=""
  CORRAL_CONN_KV_SERVE=""
  CORRAL_CONN_KV_REASON=""
  CORRAL_CONN_KV_PROXY=""
  CORRAL_CONN_KV_LIVENESS=""
  CORRAL_CONN_KV_ERRNO=""
  local line key value
  while IFS= read -r line; do
    key="${line%%=*}"
    value="${line#*=}"
    case "$key" in
      state) CORRAL_CONN_KV_STATE="$value" ;;
      dns) CORRAL_CONN_KV_DNS="$value" ;;
      certdomains) CORRAL_CONN_KV_CERTDOMAINS="$value" ;;
      serve) CORRAL_CONN_KV_SERVE="$value" ;;
      reason) CORRAL_CONN_KV_REASON="$value" ;;
      proxy) CORRAL_CONN_KV_PROXY="$value" ;;
      liveness) CORRAL_CONN_KV_LIVENESS="$value" ;;
      errno) CORRAL_CONN_KV_ERRNO="$value" ;;
    esac
  done <<< "$1"
}

# ----------------------------------------------------------- step state ----

CORRAL_CONN_STEP_RESULT=""
CORRAL_CONN_STEP_DETAIL=""
CORRAL_CONN_STEP_NEXT=""
CORRAL_CONN_STEP_CLASS=0

_corral_conn_set() { # <result> <class> <detail> [next]
  CORRAL_CONN_STEP_RESULT="$1"
  CORRAL_CONN_STEP_CLASS="$2"
  CORRAL_CONN_STEP_DETAIL="$3"
  CORRAL_CONN_STEP_NEXT="${4:-}"
}

_corral_conn_pass() { _corral_conn_set PASS 0 "$1" ""; }
_corral_conn_skip() { _corral_conn_set SKIP 0 "$1" "${2:-}"; }
_corral_conn_fail() { _corral_conn_set FAIL "$1" "$2" "$3"; }

_corral_conn_step_line() { # <name>
  printf '  %-10s %-4s %s\n' "$1" "$CORRAL_CONN_STEP_RESULT" "$CORRAL_CONN_STEP_DETAIL"
  if [[ -n "$CORRAL_CONN_STEP_NEXT" ]]; then
    printf '                   next: %s\n' "$CORRAL_CONN_STEP_NEXT"
  fi
}

# ---------------------------------------------------------------- probes ---

CORRAL_CONN_LIVENESS=""
CORRAL_CONN_LIVENESS_ERRNO=""

corral_conn_probe_socket_liveness() { # <socket path>
  # Bounded, read-only transport liveness: connect() once, then close. The
  # bound is the socket timeout inside the probe plus the shared bounded
  # runner around it, so a hung or non-listening peer cannot stall the check.
  # The verdict is transport-only: the Herdr protocol is never spoken and
  # never claimed healthy.
  local sock="$1" out rc=0
  CORRAL_CONN_LIVENESS=""
  CORRAL_CONN_LIVENESS_ERRNO=""
  out="$(corral_conn_bounded "$CORRAL_CONN_TIMEOUT" "$CORRAL_CONN_PYTHON" \
    -c "$_CORRAL_CONN_SOCKET_CONNECT" "$sock" "$CORRAL_CONN_TIMEOUT")" || rc=$?
  if [[ "$rc" == 124 ]]; then
    CORRAL_CONN_LIVENESS="timeout"
    return 0
  fi
  if [[ "$rc" != 0 ]]; then
    CORRAL_CONN_LIVENESS="probe-failed"
    CORRAL_CONN_LIVENESS_ERRNO="$rc"
    return 0
  fi
  corral_conn_parse_kv "$out"
  CORRAL_CONN_LIVENESS="$CORRAL_CONN_KV_LIVENESS"
  CORRAL_CONN_LIVENESS_ERRNO="$CORRAL_CONN_KV_ERRNO"
  if [[ -z "$CORRAL_CONN_LIVENESS" ]]; then
    CORRAL_CONN_LIVENESS="probe-failed"
  fi
  return 0
}

corral_conn_probe_herdr() {
  local sock="$CORRAL_CONN_HERDR_SOCKET"
  if [[ ! -e "$sock" && ! -L "$sock" ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_HERDR" \
      "no unix socket at $sock" \
      "start Herdr on this host (its socket appears on launch), or set HERDR_SOCKET to the real socket path"
    return 0
  fi
  if [[ ! -S "$sock" ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_HERDR" \
      "$sock exists but is not a unix socket" \
      "point HERDR_SOCKET at the herdr API socket (default ~/.config/herdr/herdr.sock)"
    return 0
  fi
  if [[ ! -r "$sock" || ! -w "$sock" ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_HERDR" \
      "socket exists but is not accessible to this user (permission denied)" \
      "fix the socket ownership/mode so the corrald user can connect to $sock"
    return 0
  fi
  # Transport liveness: a stale socket file (daemon stopped or crashed, or a
  # bind+close leftover) passes every file check above but refuses
  # connections. Connect once, bounded and read-only.
  corral_conn_probe_socket_liveness "$sock"
  case "$CORRAL_CONN_LIVENESS" in
    ok)
      _corral_conn_pass "socket $sock accepts connections (transport verified; the herdr protocol itself is not probed)"
      ;;
    refused)
      _corral_conn_fail "$CORRAL_CONN_EXIT_HERDR" \
        "socket $sock is stale: it exists but nothing is listening (connect refused)" \
        "start or restart Herdr on this host (a stale socket file means no daemon is bound to it); if Herdr is running, point HERDR_SOCKET at its real socket"
      ;;
    timeout)
      _corral_conn_fail "$CORRAL_CONN_EXIT_HERDR" \
        "connecting to $sock timed out after ${CORRAL_CONN_TIMEOUT}s" \
        "Herdr is not accepting connections (hung or overloaded); restart Herdr and re-run this check"
      ;;
    denied)
      _corral_conn_fail "$CORRAL_CONN_EXIT_HERDR" \
        "socket exists but the connection was refused (permission denied)" \
        "fix the socket ownership/mode so the corrald user can connect to $sock"
      ;;
    missing)
      _corral_conn_fail "$CORRAL_CONN_EXIT_HERDR" \
        "no unix socket at $sock (it disappeared during the check)" \
        "start or restart Herdr on this host, then re-run this check"
      ;;
    probe-failed)
      _corral_conn_fail "$CORRAL_CONN_EXIT_HERDR" \
        "could not verify that $sock accepts connections (liveness probe failed, exit ${CORRAL_CONN_LIVENESS_ERRNO})" \
        "inspect Herdr on this host; the check fails closed instead of assuming health"
      ;;
    *)
      _corral_conn_fail "$CORRAL_CONN_EXIT_HERDR" \
        "could not verify that $sock accepts connections (unrecognized probe result)" \
        "inspect Herdr on this host; the check fails closed instead of assuming health"
      ;;
  esac
  return 0
}

corral_conn_probe_corrald() {
  if [[ -z "$CORRAL_CONN_CURL" ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_CORRALD" \
      "curl binary '${CORRAL_CURL_BIN:-curl}' not found" \
      "install curl or set CORRAL_CURL_BIN to its path"
    return 0
  fi
  local url="$CORRAL_CONN_ORIGIN/healthz" body rc=0
  body="$("$CORRAL_CONN_CURL" --fail --silent --max-time "$CORRAL_CONN_TIMEOUT" "$url" 2>/dev/null)" || rc=$?
  if [[ "$rc" != 0 ]]; then
    case "$rc" in
      7)
        _corral_conn_fail "$CORRAL_CONN_EXIT_CORRALD" \
          "connection refused at $url (corrald is not running)" \
          "start corrald: scripts/setup-corrald.sh (macOS) or scripts/setup-corrald-linux.sh (Linux)"
        ;;
      28)
        _corral_conn_fail "$CORRAL_CONN_EXIT_CORRALD" \
          "no response from $url within ${CORRAL_CONN_TIMEOUT}s" \
          "inspect the corrald process/service and its log"
        ;;
      22)
        _corral_conn_fail "$CORRAL_CONN_EXIT_CORRALD" \
          "GET $url returned an HTTP error" \
          "inspect the corrald log; the loopback port answers but is not healthy"
        ;;
      *)
        _corral_conn_fail "$CORRAL_CONN_EXIT_CORRALD" \
          "GET $url failed (curl exit $rc)" \
          "check that corrald is running and listening on the loopback port"
        ;;
    esac
    return 0
  fi
  body="${body//$'\r'/}"
  body="${body//$'\n'/}"
  if [[ "$body" != "ok" ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_CORRALD" \
      "unexpected /healthz response (expected 'ok')" \
      "inspect the corrald log; a healthy corrald answers 'ok' on /healthz"
    return 0
  fi
  _corral_conn_pass "$url responded ok"
}

CORRAL_CONN_TS_STATE=""
CORRAL_CONN_TS_DNS=""
CORRAL_CONN_TS_CERTS=""

corral_conn_probe_tailscale() {
  CORRAL_CONN_TS_STATE=""
  CORRAL_CONN_TS_DNS=""
  CORRAL_CONN_TS_CERTS=""
  if [[ -z "$CORRAL_CONN_TAILSCALE" ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_TAILSCALE" \
      "tailscale CLI not found on PATH" \
      "install Tailscale on this host, or set CORRAL_TAILSCALE_BIN to the CLI path"
    return 0
  fi
  local out rc=0
  out="$(corral_conn_bounded "$CORRAL_CONN_TIMEOUT" "$CORRAL_CONN_TAILSCALE" status --json)" || rc=$?
  if [[ "$rc" == 124 ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_TAILSCALE" \
      "tailscale status timed out after ${CORRAL_CONN_TIMEOUT}s" \
      "run 'tailscale status' manually on this host and check the tailscaled service"
    return 0
  fi
  if [[ "$rc" != 0 ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_TAILSCALE" \
      "tailscale status failed (exit $rc)" \
      "check that the tailscaled service is running on this host"
    return 0
  fi
  local kv_lines
  kv_lines="$(corral_conn_eval_tailscale "$out")" || kv_lines="state=unrecognized"
  corral_conn_parse_kv "$kv_lines"
  CORRAL_CONN_TS_STATE="$CORRAL_CONN_KV_STATE"
  CORRAL_CONN_TS_DNS="$CORRAL_CONN_KV_DNS"
  CORRAL_CONN_TS_CERTS="$CORRAL_CONN_KV_CERTDOMAINS"

  if [[ "$CORRAL_CONN_TS_STATE" == "unrecognized" || -z "$CORRAL_CONN_TS_STATE" ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_TAILSCALE" \
      "unrecognized 'tailscale status --json' output" \
      "run 'tailscale status --json' manually and compare; upgrade the Tailscale CLI if it predates this schema"
    return 0
  fi
  if [[ "$CORRAL_CONN_TS_STATE" != "Running" ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_TAILSCALE" \
      "backend state \"$CORRAL_CONN_TS_STATE\" - this host is not signed in / not running" \
      "sign in on this host: run 'tailscale up' (approve the node in the admin console if prompted)"
    return 0
  fi
  if [[ -z "$CORRAL_CONN_TS_DNS" ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_TAILSCALE" \
      "no node DNS name reported (MagicDNS off?)" \
      "enable MagicDNS for the tailnet in the Tailscale admin console (DNS page)"
    return 0
  fi
  _corral_conn_pass "backend running, node DNS name $CORRAL_CONN_TS_DNS"
}

CORRAL_CONN_SERVE_STATE=""
CORRAL_CONN_SERVE_REASON=""
CORRAL_CONN_SERVE_PROXY=""
CORRAL_CONN_SERVE_CLI_RC=0

corral_conn_inspect_serve() {
  CORRAL_CONN_SERVE_STATE=""
  CORRAL_CONN_SERVE_REASON=""
  CORRAL_CONN_SERVE_PROXY=""
  CORRAL_CONN_SERVE_CLI_RC=0
  local hp out rc=0
  hp="$CORRAL_CONN_TS_DNS:$CORRAL_CONN_HTTPS_PORT"
  out="$(corral_conn_bounded "$CORRAL_CONN_TIMEOUT" "$CORRAL_CONN_TAILSCALE" serve status --json)" || rc=$?
  if [[ "$rc" == 124 ]]; then
    CORRAL_CONN_SERVE_STATE="timeout"
    return 0
  fi
  if [[ "$rc" != 0 ]]; then
    CORRAL_CONN_SERVE_STATE="failed"
    CORRAL_CONN_SERVE_CLI_RC="$rc"
    return 0
  fi
  local kv_lines
  kv_lines="$(corral_conn_eval_serve "$out" "$hp" "$CORRAL_CONN_ORIGIN")" || kv_lines="serve=unrecognized"
  corral_conn_parse_kv "$kv_lines"
  CORRAL_CONN_SERVE_STATE="$CORRAL_CONN_KV_SERVE"
  CORRAL_CONN_SERVE_REASON="$CORRAL_CONN_KV_REASON"
  CORRAL_CONN_SERVE_PROXY="$CORRAL_CONN_KV_PROXY"
  return 0
}

corral_conn_probe_serve() {
  if [[ "$CORRAL_CONN_TS_STATE" != "Running" || -z "$CORRAL_CONN_TS_DNS" ]]; then
    _corral_conn_skip "tailscale is not running" "fix the tailscale check above, then re-run"
    return 0
  fi
  if [[ "$CORRAL_CONN_TS_CERTS" == "0" || -z "$CORRAL_CONN_TS_CERTS" ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_SERVE" \
      "HTTPS certificates are not enabled for this tailnet" \
      "one-time admin step: Tailscale admin console -> DNS -> enable HTTPS Certificates, then re-run"
    return 0
  fi
  corral_conn_inspect_serve
  local hp="$CORRAL_CONN_TS_DNS:$CORRAL_CONN_HTTPS_PORT"
  case "$CORRAL_CONN_SERVE_STATE" in
    match)
      _corral_conn_pass "https://$hp -> $CORRAL_CONN_SERVE_PROXY (tailnet-only HTTPS)"
      ;;
    missing)
      _corral_conn_fail "$CORRAL_CONN_EXIT_SERVE" \
        "no https:// mapping for $hp -> $CORRAL_CONN_ORIGIN" \
        "add it with explicit consent: bash scripts/corral-status.sh --connectivity --apply --yes (unrelated mappings are never touched)"
      ;;
    funnel)
      _corral_conn_fail "$CORRAL_CONN_EXIT_SERVE" \
        "Funnel is enabled for $hp ($(corral_conn_redact "$CORRAL_CONN_SERVE_PROXY")) - public exposure is not supported" \
        "disable Funnel for this port ('tailscale funnel status' shows it) and keep Serve tailnet-only"
      ;;
    conflict)
      case "$CORRAL_CONN_SERVE_REASON" in
        port-in-use)
          _corral_conn_fail "$CORRAL_CONN_EXIT_SERVE" \
            "port $CORRAL_CONN_HTTPS_PORT is already used by a non-HTTPS mapping - refusing to overwrite" \
            "inspect 'tailscale serve status' and resolve the port manually; this tool never overwrites unrelated mappings"
          ;;
        other-proxy)
          _corral_conn_fail "$CORRAL_CONN_EXIT_SERVE" \
            "the existing mapping for $hp points elsewhere ($(corral_conn_redact "$CORRAL_CONN_SERVE_PROXY")) - refusing to overwrite" \
            "inspect 'tailscale serve status' and resolve it manually, or point CORRALD_URL at the daemon that mapping serves"
          ;;
        *)
          _corral_conn_fail "$CORRAL_CONN_EXIT_SERVE" \
            "an existing mapping for $hp cannot be interpreted safely - refusing to overwrite" \
            "inspect 'tailscale serve status' manually; Serve state is never modified from here"
          ;;
      esac
      ;;
    timeout)
      _corral_conn_fail "$CORRAL_CONN_EXIT_SERVE" \
        "tailscale serve status timed out after ${CORRAL_CONN_TIMEOUT}s" \
        "run 'tailscale serve status' manually on this host"
      ;;
    failed)
      _corral_conn_fail "$CORRAL_CONN_EXIT_SERVE" \
        "tailscale serve status failed (exit $CORRAL_CONN_SERVE_CLI_RC)" \
        "check the tailscaled service on this host"
      ;;
    *)
      _corral_conn_fail "$CORRAL_CONN_EXIT_SERVE" \
        "unrecognized 'tailscale serve status --json' output - failing closed" \
        "inspect 'tailscale serve status' manually; this tool refuses to guess the mapping schema"
      ;;
  esac
}

corral_conn_probe_tls() {
  if [[ "$CORRAL_CONN_SERVE_STATE" != "match" ]]; then
    _corral_conn_skip "no verified Serve mapping to probe" "fix the serve check above, then re-run"
    return 0
  fi
  if [[ -z "$CORRAL_CONN_CURL" ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_TLS" \
      "curl binary '${CORRAL_CURL_BIN:-curl}' not found" \
      "install curl or set CORRAL_CURL_BIN to its path"
    return 0
  fi
  local url="https://$CORRAL_CONN_TS_DNS"
  if [[ "$CORRAL_CONN_HTTPS_PORT" != "443" ]]; then
    url="$url:$CORRAL_CONN_HTTPS_PORT"
  fi
  url="$url/healthz"
  local code rc=0
  code="$("$CORRAL_CONN_CURL" --silent --max-time "$CORRAL_CONN_TIMEOUT" --output /dev/null \
    --write-out '%{http_code}' "$url" 2>/dev/null)" || rc=$?
  if [[ "$rc" == 28 || "$rc" == 124 ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_TLS" \
      "no response from $url within ${CORRAL_CONN_TIMEOUT}s" \
      "check the Serve mapping ('tailscale serve status') and the daemon behind it"
    return 0
  fi
  if [[ "$rc" != 0 ]]; then
    case "$rc" in
      60 | 51 | 58 | 35 | 83)
        _corral_conn_fail "$CORRAL_CONN_EXIT_TLS" \
          "TLS verification failed for $url (curl exit $rc)" \
          "confirm HTTPS Certificates are enabled and the cert is provisioned (tailscale cert $CORRAL_CONN_TS_DNS)"
        ;;
      6)
        _corral_conn_fail "$CORRAL_CONN_EXIT_TLS" \
          "DNS resolution failed for $CORRAL_CONN_TS_DNS (curl exit 6)" \
          "enable MagicDNS for the tailnet and confirm the node DNS name"
        ;;
      7)
        _corral_conn_fail "$CORRAL_CONN_EXIT_TLS" \
          "connection refused for $url (curl exit 7)" \
          "confirm the Serve mapping is active: tailscale serve status"
        ;;
      *)
        _corral_conn_fail "$CORRAL_CONN_EXIT_TLS" \
          "HTTPS request to $url failed (curl exit $rc)" \
          "inspect the Serve mapping and this host's network state"
        ;;
    esac
    return 0
  fi
  if [[ "$code" != "200" ]]; then
    _corral_conn_fail "$CORRAL_CONN_EXIT_TLS" \
      "HTTPS origin answered HTTP $code" \
      "the Serve mapping is up but the origin is not healthy; check corrald behind Serve"
    return 0
  fi
  _corral_conn_pass "$url verified TLS, HTTP 200"
}

# ---------------------------------------------------------------- check ----

_corral_conn_class_name() {
  case "$1" in
    "$CORRAL_CONN_EXIT_HERDR") printf 'herdr' ;;
    "$CORRAL_CONN_EXIT_CORRALD") printf 'corrald' ;;
    "$CORRAL_CONN_EXIT_TAILSCALE") printf 'tailscale' ;;
    "$CORRAL_CONN_EXIT_SERVE") printf 'serve' ;;
    "$CORRAL_CONN_EXIT_TLS") printf 'tls' ;;
    *) printf 'unknown' ;;
  esac
}

corral_conn_check() {
  local first_fail=0
  printf 'corral connectivity: bounded private-connectivity check (read-only; no changes made)\n'
  printf '  loopback origin: %s\n' "$CORRAL_CONN_ORIGIN"

  corral_conn_probe_herdr
  _corral_conn_step_line "herdr"
  if [[ "$CORRAL_CONN_STEP_RESULT" == "FAIL" && "$first_fail" == 0 ]]; then
    first_fail="$CORRAL_CONN_STEP_CLASS"
  fi

  corral_conn_probe_corrald
  _corral_conn_step_line "corrald"
  if [[ "$CORRAL_CONN_STEP_RESULT" == "FAIL" && "$first_fail" == 0 ]]; then
    first_fail="$CORRAL_CONN_STEP_CLASS"
  fi

  corral_conn_probe_tailscale
  _corral_conn_step_line "tailscale"
  if [[ "$CORRAL_CONN_STEP_RESULT" == "FAIL" && "$first_fail" == 0 ]]; then
    first_fail="$CORRAL_CONN_STEP_CLASS"
  fi

  corral_conn_probe_serve
  _corral_conn_step_line "serve"
  if [[ "$CORRAL_CONN_STEP_RESULT" == "FAIL" && "$first_fail" == 0 ]]; then
    first_fail="$CORRAL_CONN_STEP_CLASS"
  fi

  corral_conn_probe_tls
  _corral_conn_step_line "tls"
  if [[ "$CORRAL_CONN_STEP_RESULT" == "FAIL" && "$first_fail" == 0 ]]; then
    first_fail="$CORRAL_CONN_STEP_CLASS"
  fi

  if [[ "$first_fail" == 0 ]]; then
    printf 'host-local: PASS\n'
  else
    printf 'host-local: FAIL (%s)\n' "$(_corral_conn_class_name "$first_fail")"
  fi
  printf 'phone-reachability: UNKNOWN - only a check from the actual iPhone on this tailnet can confirm device access (host-local checks cannot)\n'
  return "$first_fail"
}

# ---------------------------------------------------------------- apply ----

corral_conn_apply() {
  local tailscale_cmd="${CORRAL_CONN_TAILSCALE:-${CORRAL_TAILSCALE_BIN:-tailscale}}"
  printf 'corral connectivity: apply (adds the loopback Serve HTTPS mapping)\n'

  if [[ "$CORRAL_CONN_CONSENT" != "yes" ]]; then
    printf '  consent: REQUIRED - nothing was changed and no service was contacted\n'
    printf '  planned action: add a tailnet-only HTTPS Serve mapping for this node (%s) -> %s\n' \
      "https port $CORRAL_CONN_HTTPS_PORT" "$CORRAL_CONN_ORIGIN"
    printf '  the exact Tailscale command is: %s serve --bg --https=%s %s\n' \
      "$tailscale_cmd" "$CORRAL_CONN_HTTPS_PORT" "$CORRAL_CONN_ORIGIN"
    printf '  re-run with --apply --yes to consent\n'
    return "$CORRAL_CONN_EXIT_USAGE"
  fi
  printf '  consent: --yes provided\n'

  corral_conn_probe_corrald
  _corral_conn_step_line "corrald"
  if [[ "$CORRAL_CONN_STEP_RESULT" != "PASS" ]]; then
    printf '  aborted: corrald must be healthy before touching Serve\n'
    return "$CORRAL_CONN_EXIT_CORRALD"
  fi

  corral_conn_probe_tailscale
  _corral_conn_step_line "tailscale"
  if [[ "$CORRAL_CONN_STEP_RESULT" != "PASS" ]]; then
    printf '  aborted: tailscale must be running and signed in before touching Serve\n'
    return "$CORRAL_CONN_EXIT_TAILSCALE"
  fi

  if [[ "$CORRAL_CONN_TS_CERTS" == "0" || -z "$CORRAL_CONN_TS_CERTS" ]]; then
    printf '  aborted: HTTPS certificates are not enabled for this tailnet\n'
    printf '                   next: one-time admin step: Tailscale admin console -> DNS -> enable HTTPS Certificates\n'
    return "$CORRAL_CONN_EXIT_SERVE"
  fi

  local hp="$CORRAL_CONN_TS_DNS:$CORRAL_CONN_HTTPS_PORT"
  corral_conn_inspect_serve
  case "$CORRAL_CONN_SERVE_STATE" in
    match)
      printf '  serve state: already configured (https://%s -> %s); nothing to do\n' "$hp" "$CORRAL_CONN_SERVE_PROXY"
      return 0
      ;;
    missing)
      printf '  serve state: no mapping for https://%s (checked %s)\n' "$hp" "$CORRAL_CONN_ORIGIN"
      ;;
    funnel | conflict)
      printf '  serve state: CONFLICT (%s) - nothing was changed\n' "$(corral_conn_redact "$CORRAL_CONN_SERVE_PROXY")"
      printf '  aborted: refusing to overwrite a conflicting or unrelated mapping; resolve it with tailscale serve status\n'
      return "$CORRAL_CONN_EXIT_SERVE"
      ;;
    timeout | failed)
      printf '  serve state: could not be read (%s); nothing was changed\n' "$CORRAL_CONN_SERVE_STATE"
      return "$CORRAL_CONN_EXIT_SERVE"
      ;;
    *)
      printf '  serve state: unrecognized Serve config - failing closed; nothing was changed\n'
      return "$CORRAL_CONN_EXIT_SERVE"
      ;;
  esac

  # Re-inspect immediately before the action: if the state moved under us
  # (concurrent serve change), fail closed rather than race it.
  corral_conn_inspect_serve
  case "$CORRAL_CONN_SERVE_STATE" in
    match)
      printf '  serve state changed to already-configured while checking; nothing to do\n'
      return 0
      ;;
    missing)
      : # still missing - proceed
      ;;
    *)
      printf '  aborted: Serve state changed to a conflict/unreadable state immediately before the action; nothing was changed\n'
      return "$CORRAL_CONN_EXIT_SERVE"
      ;;
  esac

  printf '  action: %s serve --bg --https=%s %s\n' "$tailscale_cmd" "$CORRAL_CONN_HTTPS_PORT" "$CORRAL_CONN_ORIGIN"
  local rc=0
  corral_conn_bounded "$CORRAL_CONN_APPLY_TIMEOUT" "$CORRAL_CONN_TAILSCALE" \
    serve --bg --https="$CORRAL_CONN_HTTPS_PORT" "$CORRAL_CONN_ORIGIN" >/dev/null || rc=$?
  if [[ "$rc" != 0 ]]; then
    printf '  FAIL: the Serve command failed (exit %s); nothing else was attempted\n' "$rc"
    return "$CORRAL_CONN_EXIT_SERVE"
  fi

  corral_conn_inspect_serve
  if [[ "$CORRAL_CONN_SERVE_STATE" == "match" && "$CORRAL_CONN_SERVE_PROXY" == "$CORRAL_CONN_ORIGIN" ]]; then
    printf '  result: mapping added and verified (https://%s -> %s)\n' "$hp" "$CORRAL_CONN_SERVE_PROXY"
    printf 'phone-reachability: still UNKNOWN - confirm from the actual iPhone on this tailnet\n'
    return 0
  fi
  printf '  FAIL: the mapping was not verified after the apply (state: %s); inspect tailscale serve status manually\n' \
    "$CORRAL_CONN_SERVE_STATE"
  return "$CORRAL_CONN_EXIT_SERVE"
}

# ----------------------------------------------------------------- main ----

corral_conn_main() {
  local apply="no" consent="no"
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --apply) apply="yes" ;;
      --yes) consent="yes" ;;
      --help | -h)
        corral_conn_usage
        return 0
        ;;
      *)
        printf 'corral connectivity: unknown option %s\n' "$1" >&2
        corral_conn_usage >&2
        return "$CORRAL_CONN_EXIT_USAGE"
        ;;
    esac
    shift
  done
  if [[ "$consent" == "yes" && "$apply" != "yes" ]]; then
    printf 'corral connectivity: --yes is only meaningful with --apply\n' >&2
    corral_conn_usage >&2
    return "$CORRAL_CONN_EXIT_USAGE"
  fi
  corral_conn_load_config || return "$CORRAL_CONN_EXIT_USAGE"
  if [[ "$apply" == "yes" ]]; then
    CORRAL_CONN_CONSENT="$consent"
    corral_conn_apply
  else
    corral_conn_check
  fi
}
