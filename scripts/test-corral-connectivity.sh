#!/usr/bin/env bash
# Hermetic tests for the bounded private-connectivity check + consent-gated
# Tailscale Serve setup in `scripts/corral-status.sh --connectivity`.
#
# What this suite guarantees:
#   * NO live service is ever touched. Every scenario runs the REAL script
#     under test with a fixture PATH (stub `curl` + stub `tailscale`), a
#     sandbox HOME and fixture state files. The stubs are the only binaries
#     those names can resolve to; the script has no fallback to a live
#     tailscale/curl install, and the suite asserts the host's real
#     `tailscale` CLI is NOT reachable from the fixture PATH.
#   * Read-only by default: for every check-mode scenario the suite asserts the
#     stub recorded ONLY `status --json` / `serve status --json` invocations and
#     that no mutator (`serve --bg`, `funnel`, `serve reset`) ever ran.
#   * No TLS bypass: every recorded curl invocation is asserted to be free of
#     `-k` / `--insecure`.
#   * Preserve-by-default: fixtures, per-scenario stdout/stderr and RAW exit
#     codes stay under `.logs-484/<run-id>/` (override with
#     CORRAL_CONNECTIVITY_LOG_DIR). This suite NEVER deletes anything — there
#     is no `rm` and no EXIT trap.
#
# Usage:
#   bash scripts/test-corral-connectivity.sh [--only <scenario>]
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SCRIPT_UNDER_TEST="$SCRIPT_DIR/corral-status.sh"

ONLY=""
if [[ "${1:-}" == "--only" ]]; then
  ONLY="${2:-}"
fi

BASH_BIN="$(command -v bash || true)"
PYTHON_BIN="$(command -v python3 || true)"
if [[ -z "$BASH_BIN" || -z "$PYTHON_BIN" ]]; then
  echo "FAIL: test prerequisites missing (bash or python3 not found)" >&2
  exit 1
fi

RUN_ROOT="${CORRAL_CONNECTIVITY_LOG_DIR:-$REPO_ROOT/.logs-484}"
RUN_DIR="$RUN_ROOT/run-$(date -u +%Y%m%dT%H%M%SZ)-$$"
# macOS caps AF_UNIX paths at ~104 bytes; the worktree log path alone is longer,
# so fixture sockets live under a short /tmp root (still never cleaned up).
SOCK_ROOT="/tmp/corral484-$$"
mkdir -p "$RUN_DIR/scenarios"
printf 'corral connectivity test run: %s\n' "$RUN_DIR" > "$RUN_DIR/summary.txt"

PASSED=0
FAILED_SCENARIOS=0
TOTAL_SCENARIOS=0

SCEN=""
SCEN_DIR=""
SCEN_BIN=""
SCEN_HOME=""
SCEN_STATE=""
SCEN_PREFIX=""
SCEN_FAILED=0
SCEN_TIMEOUT=2
SCEN_CORRALD_URL="http://127.0.0.1:8474"
SCEN_CURL_BIN="curl"
SCEN_TAILSCALE_BIN="tailscale"
SCEN_PYTHON_BIN="$PYTHON_BIN"

STUB_DNS="corral-test-node.tailnet-fixture.ts.net"
STUB_HTTPS_PORT=443
STUB_ORIGIN="http://127.0.0.1:8474"

TS_RUNNING_JSON='{"BackendState":"Running","Self":{"DNSName":"corral-test-node.tailnet-fixture.ts.net."},"CertDomains":["corral-test-node.tailnet-fixture.ts.net"]}'
SERVE_MATCH_JSON='{"TCP":{"443":{"HTTPS":true}},"Web":{"corral-test-node.tailnet-fixture.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:8474"}}}}}'

# ---------------------------------------------------------------- fixtures ---

# A real unix socket fixture inside the sandbox HOME (never a live service).
create_unix_socket() {
  "$PYTHON_BIN" -c 'import socket, sys; s = socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); s.close()' "$1"
}

write_stub_curl() {
  cat > "$SCEN_BIN/curl" <<'SH'
#!/bin/bash
# Fixture stub for curl. Never touches the network. Behavior is driven by
# STUB_STATE_DIR/curl-mode-http and STUB_STATE_DIR/curl-mode-https (falling
# back to STUB_STATE_DIR/curl-mode, then "ok").
state="${STUB_STATE_DIR:?}"
printf 'curl %s\n' "$*" >> "$state/curl-calls.log"
url=""
write_out=no
for arg in "$@"; do
  case "$arg" in
    http://*|https://*) url="$arg" ;;
    *http_code*) write_out=yes ;;
  esac
done
kind=http
case "$url" in https://*) kind=https ;; esac
mode="$(cat "$state/curl-mode-$kind" 2>/dev/null || true)"
if [[ -z "$mode" ]]; then mode="$(cat "$state/curl-mode" 2>/dev/null || true)"; fi
if [[ -z "$mode" ]]; then mode="ok"; fi
body_file="$state/curl-body"
case "$url" in
  */snapshot) body_file="$state/curl-body-snapshot" ;;
esac
if [[ -f "$body_file" ]]; then
  mode="file"
fi
case "$mode" in
  file)      rc=0;  code=200; body="$(cat "$body_file")" ;;
  ok)        rc=0;  code=200; body=ok ;;
  unhealthy) rc=0;  code=200; body=degraded ;;
  http-error) rc=22; code=500; body=oops ;;
  http-502)  rc=0;  code=502; body= ;;
  refused)   rc=7;  code=000; body= ;;
  timeout)   rc=28; code=000; body= ;;
  tls-fail)  rc=60; code=000; body= ;;
  dns-fail)  rc=6;  code=000; body= ;;
  *)         rc=99; code=000; body= ;;
esac
if [[ "$write_out" == yes ]]; then printf '%s' "$code"; else printf '%s' "$body"; fi
exit "$rc"
SH
  chmod +x "$SCEN_BIN/curl"
}

write_stub_tailscale() {
  cat > "$SCEN_BIN/tailscale" <<'SH'
#!/bin/bash
# Fixture stub for the Tailscale CLI. Never touches a tailscaled. Reads fixture
# files from STUB_STATE_DIR; records every invocation; the only mutating branch
# is `serve --bg ...`, which appends to mutations.log and rewrites the fixture
# serve config so a post-apply verification sees the new mapping.
state="${STUB_STATE_DIR:?}"
printf 'tailscale %s\n' "$*" >> "$state/tailscale-calls.log"
case "${1:-}" in
  status)
    [[ "${2:-}" == "--json" ]] || exit 2
    if [[ -f "$state/ts-sleep" ]]; then sleep 20 >/dev/null 2>&1; fi
    if [[ -f "$state/ts-status.json" ]]; then cat "$state/ts-status.json"; else printf '{}\n'; fi
    if [[ -f "$state/ts-exit" ]]; then exit "$(cat "$state/ts-exit")"; fi
    exit 0
    ;;
  serve)
    case "${2:-}" in
      status)
        [[ "${3:-}" == "--json" ]] || exit 2
        n=0
        [[ -f "$state/serve-calls.count" ]] && n="$(cat "$state/serve-calls.count")"
        n=$((n + 1))
        printf '%s\n' "$n" > "$state/serve-calls.count"
        if [[ -f "$state/serve2.json" && "$n" -ge 2 ]]; then
          cat "$state/serve2.json"
        else
          cat "$state/serve.json"
        fi
        exit 0
        ;;
      --bg)
        printf '%s\n' "$*" >> "$state/mutations.log"
        printf '{"TCP":{"%s":{"HTTPS":true}},"Web":{"%s:%s":{"Handlers":{"/":{"Proxy":"%s"}}}}}\n' \
          "$STUB_HTTPS_PORT" "$STUB_DNS" "$STUB_HTTPS_PORT" "$STUB_ORIGIN" > "$state/serve.json"
        exit 0
        ;;
    esac
    ;;
esac
exit 2
SH
  chmod +x "$SCEN_BIN/tailscale"
}

# -------------------------------------------------------------- assertions ---

scenario_fail() { SCEN_FAILED=1; printf '  FAIL: %s\n' "$*"; }

assert_exit() {
  local want="$1" got
  got="$(cat "$SCEN_PREFIX.exit")"
  [[ "$got" == "$want" ]] || scenario_fail "exit expected $want, got $got"
}

assert_out_has()   { grep -qF -- "$1" "$SCEN_PREFIX.out" || scenario_fail "stdout missing: $1"; return 0; }
assert_out_lacks() { if grep -qF -- "$1" "$SCEN_PREFIX.out"; then scenario_fail "stdout unexpectedly contains: $1"; fi; return 0; }
assert_err_has()   { grep -qF -- "$1" "$SCEN_PREFIX.err" || scenario_fail "stderr missing: $1"; return 0; }
assert_out_matches() { grep -qE -- "$1" "$SCEN_PREFIX.out" || scenario_fail "stdout does not match: $1"; return 0; }

assert_no_mutator() {
  if [[ -s "$SCEN_STATE/mutations.log" ]]; then
    scenario_fail "mutator ran: $(cat "$SCEN_STATE/mutations.log")"
  fi
  return 0
}

assert_only_readonly_tailscale_calls() {
  if [[ -f "$SCEN_STATE/tailscale-calls.log" ]]; then
    while IFS= read -r line; do
      case "$line" in
        "tailscale status --json"|"tailscale serve status --json") ;;
        *) scenario_fail "unexpected tailscale invocation: $line" ;;
      esac
    done < "$SCEN_STATE/tailscale-calls.log"
  fi
  return 0
}

assert_no_tls_bypass() {
  if [[ -f "$SCEN_STATE/curl-calls.log" ]]; then
    while IFS= read -r line; do
      case "$line" in
        *" -k "*|*--insecure*) scenario_fail "curl TLS bypass flag used: $line" ;;
      esac
    done < "$SCEN_STATE/curl-calls.log"
  fi
  return 0
}

# ------------------------------------------------------------- scenario ops ---

should_run() { [[ -z "$ONLY" || "$ONLY" == "$1" ]]; }

begin_scenario() {
  SCEN="$1"
  SCEN_DIR="$RUN_DIR/scenarios/$SCEN"
  SCEN_BIN="$SCEN_DIR/bin"
  SCEN_HOME="$SCEN_DIR/home"
  SCEN_STATE="$SCEN_DIR/state"
  SCEN_PREFIX="$SCEN_DIR/result"
  SCEN_SOCKET="$SOCK_ROOT/$SCEN/herdr.sock"
  SCEN_FAILED=0
  SCEN_TIMEOUT=2
  SCEN_CORRALD_URL="http://127.0.0.1:8474"
  SCEN_CURL_BIN="curl"
  SCEN_TAILSCALE_BIN="tailscale"
  SCEN_PYTHON_BIN="$PYTHON_BIN"
  mkdir -p "$SCEN_BIN" "$SCEN_HOME/.config/herdr" "$SCEN_STATE" "$SOCK_ROOT/$SCEN"
  write_stub_curl
  TOTAL_SCENARIOS=$((TOTAL_SCENARIOS + 1))
  printf '== %s\n' "$SCEN"
}

end_scenario() {
  if [[ "$SCEN_FAILED" == 0 ]]; then
    printf '  PASS (exit %s)\n' "$(cat "$SCEN_PREFIX.exit")"
    PASSED=$((PASSED + 1))
  else
    printf '  FAILED (exit %s) - artifacts: %s\n' "$(cat "$SCEN_PREFIX.exit")" "$SCEN_DIR"
    FAILED_SCENARIOS=$((FAILED_SCENARIOS + 1))
  fi
  printf '%s: exit=%s failed=%s\n' "$SCEN" "$(cat "$SCEN_PREFIX.exit")" "$SCEN_FAILED" >> "$RUN_DIR/summary.txt"
}

run_under_test() {
  local out="$1"; shift
  local rc=0
  (
    cd "$SCEN_DIR" && exec env -i \
      PATH="$SCEN_BIN:/usr/bin:/bin" \
      HOME="$SCEN_HOME" \
      STUB_STATE_DIR="$SCEN_STATE" \
      STUB_DNS="$STUB_DNS" \
      STUB_HTTPS_PORT="$STUB_HTTPS_PORT" \
      STUB_ORIGIN="$STUB_ORIGIN" \
      CORRAL_PYTHON_BIN="$SCEN_PYTHON_BIN" \
      CORRAL_CURL_BIN="$SCEN_CURL_BIN" \
      CORRAL_TAILSCALE_BIN="$SCEN_TAILSCALE_BIN" \
      CORRAL_STATUS_TIMEOUT_SECONDS="$SCEN_TIMEOUT" \
      HERDR_SOCKET="$SCEN_SOCKET" \
      CORRALD_URL="$SCEN_CORRALD_URL" \
      "$BASH_BIN" "$SCRIPT_UNDER_TEST" "$@"
  ) > "$out.out" 2> "$out.err"
  rc=$?
  printf '%s\n' "$rc" > "$out.exit"
  return 0
}

run_check()      { run_under_test "$SCEN_PREFIX" --connectivity "$@"; }
run_default()    { run_under_test "$SCEN_PREFIX"; }
run_apply()      { run_under_test "$SCEN_PREFIX" --connectivity --apply "$@"; }

fixture_corrald_ok()      { printf 'ok' > "$SCEN_STATE/curl-mode"; }
fixture_herdr_socket()    { create_unix_socket "$SCEN_SOCKET"; }
fixture_tailscale_up()    { write_stub_tailscale; printf '%s\n' "$TS_RUNNING_JSON" > "$SCEN_STATE/ts-status.json"; }
fixture_serve_match()     { printf '%s\n' "$SERVE_MATCH_JSON" > "$SCEN_STATE/serve.json"; }
fixture_full_healthy()    { fixture_herdr_socket; fixture_corrald_ok; fixture_tailscale_up; fixture_serve_match; }

# ============================================================== scenarios ====

# ---- herdr classes ----------------------------------------------------------

if should_run herdr-missing; then
  begin_scenario herdr-missing
  fixture_corrald_ok; fixture_tailscale_up; fixture_serve_match
  run_check
  assert_exit 1
  assert_out_matches '^  herdr +FAIL'
  assert_out_has "next:"
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run herdr-not-socket; then
  begin_scenario herdr-not-socket
  printf 'not a socket\n' > "$SCEN_SOCKET"
  fixture_corrald_ok; fixture_tailscale_up; fixture_serve_match
  run_check
  assert_exit 1
  assert_out_matches '^  herdr +FAIL'
  assert_out_has "not a unix socket"
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run herdr-permission; then
  begin_scenario herdr-permission
  fixture_herdr_socket
  chmod 000 "$SCEN_SOCKET"
  fixture_corrald_ok; fixture_tailscale_up; fixture_serve_match
  run_check
  assert_exit 1
  assert_out_matches '^  herdr +FAIL'
  assert_out_matches '^  herdr +FAIL .*permission'
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

# ---- corrald classes --------------------------------------------------------

if should_run corrald-stopped; then
  begin_scenario corrald-stopped
  fixture_herdr_socket; fixture_tailscale_up; fixture_serve_match
  printf 'refused' > "$SCEN_STATE/curl-mode-http"
  run_check
  assert_exit 3
  assert_out_matches '^  corrald +FAIL'
  assert_out_matches 'refused|not running'
  assert_out_has "next:"
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run corrald-timeout; then
  begin_scenario corrald-timeout
  fixture_herdr_socket; fixture_tailscale_up; fixture_serve_match
  printf 'timeout' > "$SCEN_STATE/curl-mode-http"
  run_check
  assert_exit 3
  assert_out_matches '^  corrald +FAIL'
  assert_out_matches 'no response|timed out'
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run corrald-unhealthy; then
  begin_scenario corrald-unhealthy
  fixture_herdr_socket; fixture_tailscale_up; fixture_serve_match
  printf 'unhealthy' > "$SCEN_STATE/curl-mode-http"
  run_check
  assert_exit 3
  assert_out_matches '^  corrald +FAIL'
  assert_out_matches 'unexpected|unhealthy'
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run corrald-http-error; then
  begin_scenario corrald-http-error
  fixture_herdr_socket; fixture_tailscale_up; fixture_serve_match
  printf 'http-error' > "$SCEN_STATE/curl-mode-http"
  run_check
  assert_exit 3
  assert_out_matches '^  corrald +FAIL'
  assert_out_matches 'HTTP'
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run curl-missing; then
  begin_scenario curl-missing
  fixture_herdr_socket; fixture_tailscale_up; fixture_serve_match
  SCEN_CURL_BIN="corral-absent-curl"
  run_check
  assert_exit 3
  assert_out_matches '^  corrald +FAIL'
  assert_out_has "not found"
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

# ---- tailscale classes ------------------------------------------------------

if should_run tailscale-missing; then
  begin_scenario tailscale-missing
  fixture_herdr_socket; fixture_corrald_ok; fixture_serve_match
  # No tailscale stub: the CLI is absent from the fixture PATH.
  if PATH="$SCEN_BIN:/usr/bin:/bin" command -v tailscale >/dev/null 2>&1; then
    scenario_fail "fixture PATH unexpectedly resolves a tailscale binary"
  fi
  if [[ -x /usr/local/bin/tailscale ]]; then
    printf 'note: host has /usr/local/bin/tailscale but the fixture PATH must not reach it\n' >> "$SCEN_DIR/host-tailscale.note"
  fi
  run_check
  assert_exit 4
  assert_out_matches '^  tailscale +FAIL'
  assert_out_has "CORRAL_TAILSCALE_BIN"
  assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run tailscale-signed-out; then
  begin_scenario tailscale-signed-out
  fixture_herdr_socket; fixture_corrald_ok; fixture_serve_match
  write_stub_tailscale
  printf '%s\n' '{"BackendState":"NeedsLogin","Self":{"DNSName":"corral-test-node.tailnet-fixture.ts.net."}}' > "$SCEN_STATE/ts-status.json"
  run_check
  assert_exit 4
  assert_out_matches '^  tailscale +FAIL'
  assert_out_has "NeedsLogin"
  assert_out_matches '^  serve +SKIP'
  assert_out_matches '^  tls +SKIP'
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run tailscale-malformed; then
  begin_scenario tailscale-malformed
  fixture_herdr_socket; fixture_corrald_ok; fixture_serve_match
  write_stub_tailscale
  printf 'this is not json\n' > "$SCEN_STATE/ts-status.json"
  run_check
  assert_exit 4
  assert_out_matches '^  tailscale +FAIL'
  assert_out_has "unrecognized"
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run tailscale-timeout; then
  begin_scenario tailscale-timeout
  fixture_herdr_socket; fixture_corrald_ok; fixture_serve_match
  write_stub_tailscale
  printf '%s\n' "$TS_RUNNING_JSON" > "$SCEN_STATE/ts-status.json"
  : > "$SCEN_STATE/ts-sleep"
  run_check
  assert_exit 4
  assert_out_matches '^  tailscale +FAIL'
  assert_out_matches 'timed out'
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

# ---- serve classes ----------------------------------------------------------

if should_run serve-missing; then
  begin_scenario serve-missing
  fixture_herdr_socket; fixture_corrald_ok; fixture_tailscale_up
  printf '%s\n' 'null' > "$SCEN_STATE/serve.json"
  run_check
  assert_exit 5
  assert_out_matches '^  serve +FAIL'
  assert_out_has "no https:// mapping"
  assert_out_has "--apply --yes"
  assert_out_matches '^  tls +SKIP'
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run serve-conflict; then
  begin_scenario serve-conflict
  fixture_herdr_socket; fixture_corrald_ok; fixture_tailscale_up
  printf '%s\n' '{"TCP":{"443":{"HTTPS":true}},"Web":{"corral-test-node.tailnet-fixture.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:9999"}}}}}' > "$SCEN_STATE/serve.json"
  run_check
  assert_exit 5
  assert_out_matches '^  serve +FAIL'
  assert_out_matches 'refus|overwrite|elsewhere'
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run serve-port-in-use; then
  begin_scenario serve-port-in-use
  fixture_herdr_socket; fixture_corrald_ok; fixture_tailscale_up
  printf '%s\n' '{"TCP":{"443":{"TCPForward":"127.0.0.1:22"}}}' > "$SCEN_STATE/serve.json"
  run_check
  assert_exit 5
  assert_out_matches '^  serve +FAIL'
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run serve-funnel; then
  begin_scenario serve-funnel
  fixture_herdr_socket; fixture_corrald_ok; fixture_tailscale_up
  printf '%s\n' '{"TCP":{"443":{"HTTPS":true}},"Web":{"corral-test-node.tailnet-fixture.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:8474"}}}},"AllowFunnel":{"corral-test-node.tailnet-fixture.ts.net:443":true}}' > "$SCEN_STATE/serve.json"
  run_check
  assert_exit 5
  assert_out_matches '^  serve +FAIL'
  assert_out_has "Funnel"
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run serve-unrecognized; then
  begin_scenario serve-unrecognized
  fixture_herdr_socket; fixture_corrald_ok; fixture_tailscale_up
  printf '%s\n' '{"UnexpectedTopLevel":{"a":1}}' > "$SCEN_STATE/serve.json"
  run_check
  assert_exit 5
  assert_out_matches '^  serve +FAIL'
  assert_out_matches '^  serve +FAIL .*unrecognized.*fail(ing)? closed'
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run serve-certificates-not-enabled; then
  begin_scenario serve-certificates-not-enabled
  fixture_herdr_socket; fixture_corrald_ok; fixture_serve_match
  write_stub_tailscale
  printf '%s\n' '{"BackendState":"Running","Self":{"DNSName":"corral-test-node.tailnet-fixture.ts.net."},"CertDomains":[]}' > "$SCEN_STATE/ts-status.json"
  run_check
  assert_exit 5
  assert_out_matches '^  serve +FAIL'
  assert_out_has "HTTPS Certificates"
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

# ---- TLS classes ------------------------------------------------------------

if should_run tls-verify-failure; then
  begin_scenario tls-verify-failure
  fixture_full_healthy
  printf 'tls-fail' > "$SCEN_STATE/curl-mode-https"
  run_check
  assert_exit 6
  assert_out_matches '^  tls +FAIL'
  assert_out_matches 'TLS'
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run tls-dns-failure; then
  begin_scenario tls-dns-failure
  fixture_full_healthy
  printf 'dns-fail' > "$SCEN_STATE/curl-mode-https"
  run_check
  assert_exit 6
  assert_out_matches '^  tls +FAIL'
  assert_out_matches 'DNS'
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run tls-http-error; then
  begin_scenario tls-http-error
  fixture_full_healthy
  printf 'http-502' > "$SCEN_STATE/curl-mode-https"
  run_check
  assert_exit 6
  assert_out_matches '^  tls +FAIL'
  assert_out_matches 'HTTP 502'
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

# ---- healthy / redaction / config ------------------------------------------

if should_run healthy; then
  begin_scenario healthy
  fixture_full_healthy
  run_check
  assert_exit 0
  assert_out_matches '^  herdr +PASS'
  assert_out_matches '^  corrald +PASS'
  assert_out_matches '^  tailscale +PASS'
  assert_out_matches '^  serve +PASS'
  assert_out_matches '^  tls +PASS'
  assert_out_has "host-local: PASS"
  assert_out_has "phone-reachability: UNKNOWN"
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run redaction; then
  begin_scenario redaction
  fixture_herdr_socket; fixture_corrald_ok; fixture_serve_match
  write_stub_tailscale
  # A credential-shaped token in the fake CLI output must never reach our output.
  printf '%s\n' '{"BackendState":"NeedsLogin tskey-auth-kFIXTUREONLY0000000000","Self":{"DNSName":"corral-test-node.tailnet-fixture.ts.net."}}' > "$SCEN_STATE/ts-status.json"
  run_check
  assert_exit 4
  assert_out_lacks "tskey"
  assert_out_has "unrecognized"
  assert_only_readonly_tailscale_calls; assert_no_mutator; assert_no_tls_bypass
  end_scenario
fi

if should_run python-missing; then
  begin_scenario python-missing
  fixture_full_healthy
  SCEN_PYTHON_BIN="corral-absent-python"
  run_check
  assert_exit 2
  assert_err_has "python3"
  if [[ -f "$SCEN_STATE/tailscale-calls.log" ]]; then
    scenario_fail "probes ran despite the missing interpreter"
  fi
  end_scenario
fi

if should_run non-loopback-url; then
  begin_scenario non-loopback-url
  fixture_full_healthy
  SCEN_CORRALD_URL="http://100.64.1.5:8474"
  run_check
  assert_exit 2
  assert_err_has "loopback"
  if [[ -f "$SCEN_STATE/tailscale-calls.log" ]]; then
    scenario_fail "probes ran despite the invalid CORRALD_URL"
  fi
  end_scenario
fi

if should_run unknown-option; then
  begin_scenario unknown-option
  fixture_full_healthy
  run_check --bogus
  assert_exit 2
  assert_err_has "--bogus"
  assert_no_mutator
  end_scenario
fi

# ---- apply classes ----------------------------------------------------------

if should_run apply-without-consent; then
  begin_scenario apply-without-consent
  fixture_herdr_socket; fixture_corrald_ok; fixture_tailscale_up
  printf '%s\n' 'null' > "$SCEN_STATE/serve.json"
  run_apply
  assert_exit 2
  assert_out_matches '^  consent: REQUIRED'
  assert_out_has "--yes"
  assert_no_mutator
  end_scenario
fi

if should_run apply-replay; then
  begin_scenario apply-replay
  fixture_full_healthy
  run_apply --yes
  assert_exit 0
  assert_out_has "already configured"
  assert_no_mutator
  assert_only_readonly_tailscale_calls; assert_no_tls_bypass
  end_scenario
fi

if should_run apply-success; then
  begin_scenario apply-success
  fixture_herdr_socket; fixture_corrald_ok; fixture_tailscale_up
  printf '%s\n' 'null' > "$SCEN_STATE/serve.json"
  run_apply --yes
  assert_exit 0
  if [[ -f "$SCEN_STATE/mutations.log" ]]; then
    mut_count="$(wc -l < "$SCEN_STATE/mutations.log" | tr -d ' ')"
  else
    mut_count=0
  fi
  [[ "$mut_count" == 1 ]] || scenario_fail "expected exactly one mutator invocation, got $mut_count"
  grep -qxF 'serve --bg --https=443 http://127.0.0.1:8474' "$SCEN_STATE/mutations.log" \
    || scenario_fail "mutator argv mismatch: $(cat "$SCEN_STATE/mutations.log" 2>/dev/null)"
  assert_out_has "verified"
  # every tailscale invocation is either read-only or the single recorded mutation
  while IFS= read -r line; do
    case "$line" in
      "tailscale status --json"|"tailscale serve status --json"|"tailscale serve --bg --https=443 http://127.0.0.1:8474") ;;
      *) scenario_fail "unexpected tailscale invocation: $line" ;;
    esac
  done < "$SCEN_STATE/tailscale-calls.log"
  assert_no_tls_bypass
  end_scenario
fi

if should_run apply-conflict; then
  begin_scenario apply-conflict
  fixture_herdr_socket; fixture_corrald_ok; fixture_tailscale_up
  printf '%s\n' '{"TCP":{"443":{"HTTPS":true}},"Web":{"corral-test-node.tailnet-fixture.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:9999"}}}}}' > "$SCEN_STATE/serve.json"
  run_apply --yes
  assert_exit 5
  assert_no_mutator
  assert_out_matches 'refus|overwrite|elsewhere'
  end_scenario
fi

if should_run apply-concurrent-conflict; then
  begin_scenario apply-concurrent-conflict
  fixture_herdr_socket; fixture_corrald_ok; fixture_tailscale_up
  # First inspection: nothing configured. Second (immediately before acting):
  # a conflicting mapping appeared. The apply must fail closed, never mutate.
  printf '%s\n' 'null' > "$SCEN_STATE/serve.json"
  printf '%s\n' '{"TCP":{"443":{"HTTPS":true}},"Web":{"corral-test-node.tailnet-fixture.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:9999"}}}}}' > "$SCEN_STATE/serve2.json"
  run_apply --yes
  assert_exit 5
  assert_no_mutator
  end_scenario
fi

if should_run apply-non-loopback; then
  begin_scenario apply-non-loopback
  fixture_full_healthy
  SCEN_CORRALD_URL="http://100.64.1.5:8474"
  run_apply --yes
  assert_exit 2
  assert_err_has "loopback"
  if [[ -f "$SCEN_STATE/tailscale-calls.log" ]]; then
    scenario_fail "apply ran probes despite the non-loopback origin"
  fi
  assert_no_mutator
  end_scenario
fi

# ---- default-mode compatibility --------------------------------------------

if should_run default-healthy; then
  begin_scenario default-healthy
  printf 'ok' > "$SCEN_STATE/curl-mode"
  printf '%s\n' '{"schema_version":5,"rev":1,"agents":{}}' > "$SCEN_STATE/curl-body-snapshot"
  run_default
  assert_exit 0
  assert_out_matches '^corrald: healthy'
  assert_out_matches '^fleet: 0 agents \(snapshot rev 1, schema 5\)'
  if [[ -f "$SCEN_STATE/tailscale-calls.log" ]]; then
    scenario_fail "default mode invoked tailscale"
  fi
  end_scenario
fi

if should_run default-unavailable; then
  begin_scenario default-unavailable
  printf 'refused' > "$SCEN_STATE/curl-mode"
  run_default
  assert_exit 1
  assert_err_has "corrald is unavailable"
  end_scenario
fi

# =============================================================== summary ====

printf '\nrun dir (preserved): %s\n' "$RUN_DIR"
printf 'scenarios: %s passed, %s failed (of %s)\n' "$PASSED" "$FAILED_SCENARIOS" "$TOTAL_SCENARIOS"
if [[ "$FAILED_SCENARIOS" != 0 ]]; then
  exit 1
fi
exit 0
