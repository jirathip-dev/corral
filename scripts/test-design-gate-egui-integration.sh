#!/usr/bin/env bash
# Real egui design-gate integration: scratch corrald + fake herdr socket +
# native corrald-ui/wgpu capture. The default mode is a read-only verifier for
# committed evidence. Native regeneration and publication are explicit via
# `--publish`, so normal verification never overwrites tracked artifacts.
#
# Run with:
#   bash scripts/test-design-gate-egui-integration.sh
#   bash scripts/test-design-gate-egui-integration.sh --publish
#
# The harness owns every process it starts, uses a fresh loopback port/config,
# creates a real scratch git repo for the fake agent, prepares a registered UI
# config, and asks the design-gate script to capture that target. The wake
# helper brings only the exact corrald-ui pid's process frontmost and sends one
# Escape key event to wake its native event loop; it does not broadcast input
# or click arbitrary windows. The EXIT trap
# uses TERM, a short grace period, then KILL for the direct children it owns.

set -euo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SCRIPT="$SCRIPT_DIR/design-gate-evidence.sh"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || true)}"
DAEMON_BIN="$REPO_DIR/target/release/corrald"
UI_BIN="$REPO_DIR/target/release/corrald-ui"
MODE="verify"

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --publish|--regenerate)
      MODE="publish"
      shift
      ;;
    --verify)
      MODE="verify"
      shift
      ;;
    -h|--help)
      printf '%s\n' \
        "Usage: $0 [--verify|--publish]" \
        "  --verify   read and validate committed four-tab evidence (default)" \
        "  --publish  run native captures and explicitly replace issue-206 evidence"
      exit 0
      ;;
    *)
      printf 'egui integration: error: unknown option: %s\n' "$1" >&2
      exit 2
      ;;
  esac
done

[[ -n "$PYTHON_BIN" && -x "$PYTHON_BIN" ]] \
  || {
    printf 'egui integration: error: Python 3 is required\n' >&2
    exit 1
  }
CONTENT_IDENTITY_HELPER="$SCRIPT_DIR/design-gate-content-identity.py"
[[ -f "$CONTENT_IDENTITY_HELPER" ]] \
  || {
    printf 'egui integration: error: implementation identity helper is missing\n' >&2
    exit 1
  }

STATUS_BEFORE="$(git -C "$REPO_DIR" status --porcelain=v1)"

verify_committed_evidence() {
  local status_after
  "$PYTHON_BIN" "$SCRIPT_DIR/verify-design-gate-egui-evidence.py" \
    "$REPO_DIR" "$CONTENT_IDENTITY_HELPER"
  status_after="$(git -C "$REPO_DIR" status --porcelain=v1)"
  if [[ "$status_after" != "$STATUS_BEFORE" ]]; then
    printf 'egui integration: error: read-only verification changed git status\n' >&2
    git -C "$REPO_DIR" status --short >&2
    return 1
  fi
  printf 'egui integration: git status unchanged by verification\n'
}

if [[ "$MODE" == "verify" ]]; then
  verify_committed_evidence
  exit 0
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/corral-design-gate-egui.XXXXXX")"
HERDR_PID=""
DAEMON_PID=""
PROXY_PID=""
CLEANED=0
TERM_GRACE_SECONDS=2
KILL_GRACE_SECONDS=2

die() {
  printf 'egui integration: error: %s\n' "$*" >&2
  exit 1
}

process_is_running() {
  local pid="$1"
  local state
  kill -0 "$pid" 2>/dev/null || return 1
  state="$(ps -p "$pid" -o stat= 2>/dev/null | awk 'NR == 1 { print $1 }')"
  [[ -n "$state" && "$state" != Z* ]]
}

process_is_owned() {
  local pid="$1"
  local parent_pid
  parent_pid="$(ps -p "$pid" -o ppid= 2>/dev/null | awk 'NR == 1 { print $1 }')"
  [[ "$parent_pid" == "$$" ]]
}

stop_owned_child() {
  local pid="$1"
  local label="$2"
  local deadline
  [[ -n "$pid" ]] || return 0
  if ! process_is_running "$pid"; then
    wait "$pid" 2>/dev/null || true
    return 0
  fi
  process_is_owned "$pid" \
    || {
      printf 'egui integration: error: refusing to terminate non-owned %s pid %s\n' "$label" "$pid" >&2
      return 1
    }
  kill -TERM "$pid" 2>/dev/null || true
  deadline=$((SECONDS + TERM_GRACE_SECONDS))
  while process_is_running "$pid" && [[ $SECONDS -lt $deadline ]]; do
    sleep 0.1
  done 2>/dev/null
  if process_is_running "$pid"; then
    kill -KILL "$pid" 2>/dev/null || true
    deadline=$((SECONDS + KILL_GRACE_SECONDS))
    while process_is_running "$pid" && [[ $SECONDS -lt $deadline ]]; do
      sleep 0.1
    done 2>/dev/null
  fi
  process_is_running "$pid" \
    && {
      printf 'egui integration: error: %s pid %s survived TERM/KILL cleanup\n' "$label" "$pid" >&2
      return 1
    }
  wait "$pid" 2>/dev/null || true
}

work_process_pids() {
  ps -axo pid=,command= 2>/dev/null \
    | awk -v work="$WORK" -v self="$$" \
      '$1 != self && index($0, "--user-data-dir=" work) { print $1 }'
}

stop_owned_work_processes() {
  local pid
  local deadline
  local remaining

  # Browser.close can reparent a helper before the direct Chrome child is
  # reaped. The unique scratch path is the ownership boundary for this
  # harness; never match a broad executable name or a shared browser profile.
  for pid in $(work_process_pids); do
    kill -TERM "$pid" 2>/dev/null || true
  done
  deadline=$((SECONDS + TERM_GRACE_SECONDS))
  while [[ $SECONDS -lt "$deadline" ]]; do
    remaining="$(work_process_pids)"
    [[ -z "$remaining" ]] && return 0
    sleep 0.1
  done
  for pid in $(work_process_pids); do
    kill -KILL "$pid" 2>/dev/null || true
  done
  deadline=$((SECONDS + KILL_GRACE_SECONDS))
  while [[ $SECONDS -lt "$deadline" ]]; do
    remaining="$(work_process_pids)"
    [[ -z "$remaining" ]] && return 0
    sleep 0.1
  done
  remaining="$(work_process_pids)"
  if [[ -n "$remaining" ]]; then
    printf 'egui integration: error: scratch-owned processes survived TERM/KILL: %s\n' "$remaining" >&2
    return 1
  fi
}

cleanup() {
  local cleanup_status=0
  if [[ "$CLEANED" -eq 0 ]]; then
    if ! stop_owned_child "$DAEMON_PID" corrald; then
      cleanup_status=1
    fi
    if ! stop_owned_child "$HERDR_PID" fake-herdr; then
      cleanup_status=1
    fi
    if ! stop_owned_child "$PROXY_PID" issues-proxy; then
      cleanup_status=1
    fi
    if ! stop_owned_work_processes; then
      cleanup_status=1
    fi
    if [[ "$cleanup_status" -eq 0 ]]; then
      CLEANED=1
    fi
  fi
  if [[ "$cleanup_status" -eq 0 && -d "$WORK" ]]; then
    rm -rf -- "$WORK"
  elif [[ "$cleanup_status" -ne 0 ]]; then
    printf 'egui integration: retaining scratch directory because owned-child cleanup failed: %s\n' "$WORK" >&2
  fi
  return "$cleanup_status"
}
trap cleanup EXIT

[[ "$(uname -s)" == "Darwin" ]] \
  || { printf 'SKIP: real egui integration requires a macOS window server\n'; exit 0; }
[[ -n "$PYTHON_BIN" && -x "$PYTHON_BIN" ]] \
  || die "Python 3 is required"
command -v cargo >/dev/null 2>&1 || die "cargo is required"
command -v curl >/dev/null 2>&1 || die "curl is required"
command -v openssl >/dev/null 2>&1 || die "openssl is required to prepare the scratch device key"
command -v osascript >/dev/null 2>&1 || die "osascript is required for the exact-pid window wake"
[[ -x "$SCRIPT" ]] || die "design-gate script is not executable: $SCRIPT"

printf 'egui integration: building real corrald and corrald-ui\n'
(cd "$REPO_DIR" && cargo build --release -p corrald -p corrald-ui)
[[ -x "$DAEMON_BIN" && -x "$UI_BIN" ]] || die "release binaries were not produced"

mkdir -p "$WORK/home" "$WORK/daemon-config" "$WORK/ui-config" "$WORK/repo" "$WORK/worktrees"
git -C "$WORK/repo" init -q -b main
git -C "$WORK/repo" config user.email design-gate@example.test
git -C "$WORK/repo" config user.name design-gate-fixture
printf 'design-gate fixture\n' >"$WORK/repo/README.md"
git -C "$WORK/repo" add README.md
git -C "$WORK/repo" commit -q -m 'design-gate: scratch fixture'

for repo in plush-meadow sendmeter; do
  mkdir -p "$WORK/$repo"
  git -C "$WORK/$repo" init -q -b main
  git -C "$WORK/$repo" config user.email design-gate@example.test
  git -C "$WORK/$repo" config user.name design-gate-fixture
  printf 'design-gate fixture for %s\n' "$repo" >"$WORK/$repo/README.md"
  git -C "$WORK/$repo" add README.md
  git -C "$WORK/$repo" commit -q -m 'design-gate: scratch fixture'
done

cat >"$WORK/daemon-config/fleets.json" <<JSON
{
  "fleets": [
    {
      "name": "corral",
      "gh_repo": "jirathip-dev/corral",
      "local": "$WORK/repo",
      "worktree_dir": "corral",
      "orch": "orch-corral",
      "workers": ["design-gate-fixture"],
      "models": {
        "orch": "codex/gpt-5.6-sol",
        "impl": "codex/gpt-5.6-luna",
        "review": "claude/opus"
      },
      "admission_note": "preserved design-gate fixture field"
    },
    {
      "name": "plush-meadow",
      "gh_repo": "jirathip-dev/plush-meadow",
      "local": "$WORK/plush-meadow",
      "worktree_dir": "plush-meadow",
      "orch": "orch-plush-meadow",
      "workers": ["design-gate-fixture"],
      "models": {
        "orch": "codex/gpt-5.6-sol",
        "impl": "codex/gpt-5.6-luna",
        "review": "claude/opus"
      }
    },
    {
      "name": "sendmeter",
      "gh_repo": "jirathip-dev/sendmeter",
      "local": "$WORK/sendmeter",
      "worktree_dir": "sendmeter",
      "orch": "orch-sendmeter",
      "workers": ["design-gate-fixture"],
      "models": {
        "orch": "codex/gpt-5.6-sol",
        "impl": "codex/gpt-5.6-luna",
        "review": "claude/opus"
      }
    }
  ]
}
JSON

CORRAL_FLEETS_PATH="$WORK/daemon-config/fleets.json" \
  "$DAEMON_BIN" fleet check --registry "$WORK/daemon-config/fleets.json" \
  >"$WORK/fleet-check.log" 2>&1 \
  || {
    cat "$WORK/fleet-check.log" >&2
    die "real corrald fleet check rejected the scratch registry"
  }

SOCKET="$WORK/herdr.sock"
PORT="$($PYTHON_BIN - <<'PY'
import socket

with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
DAEMON_URL="http://127.0.0.1:$PORT"

cat >"$WORK/fake-herdr.py" <<'PY'
import asyncio
import json
from pathlib import Path
import sys

socket_path = Path(sys.argv[1])
repos = sys.argv[2:]
agents = []
for index, repo in enumerate(repos, start=1):
    agents.append(
        {
            "agent": "claude" if index == 1 else "codex",
            "agent_status": "working",
            "cwd": repo,
            "foreground_cwd": repo,
            "focused": index == 1,
            "interactive_ready": True,
            "name": f"design-gate-fixture-{index}",
            "pane_id": f"design-gate:p{index}",
            "revision": 1,
            "state_labels": {},
            "state_change_seq": 1,
            "title": f"Design gate {Path(repo).name} agent",
            "terminal_title_stripped": f"Design gate {Path(repo).name} agent",
            "workspace_id": f"design-gate-{Path(repo).name}",
        }
    )


async def handle(reader, writer):
    try:
        while line := await reader.readline():
            if not line.strip():
                continue
            request = json.loads(line)
            request_id = request.get("id")
            method = request.get("method")
            if method == "agent.list":
                result = {"agents": agents}
            elif method == "events.subscribe":
                result = None
            else:
                result = {"ok": True}
            writer.write((json.dumps({"id": request_id, "result": result}) + "\n").encode())
            await writer.drain()
    finally:
        writer.close()
        await writer.wait_closed()


async def main():
    socket_path.unlink(missing_ok=True)
    server = await asyncio.start_unix_server(handle, path=str(socket_path))
    async with server:
        await server.serve_forever()


asyncio.run(main())
PY

HOME="$WORK/home" \
  "$PYTHON_BIN" "$WORK/fake-herdr.py" "$SOCKET" "$WORK/repo" \
  "$WORK/plush-meadow" "$WORK/sendmeter" \
  >"$WORK/fake-herdr.log" 2>&1 &
HERDR_PID=$!

socket_ready=0
attempt=0
while [[ "$attempt" -lt 100 ]]; do
  if [[ -S "$SOCKET" ]]; then
    socket_ready=1
    break
  fi
  if ! process_is_running "$HERDR_PID"; then
    tail -80 "$WORK/fake-herdr.log" >&2 || true
    die "fake Herdr exited before creating its Unix socket"
  fi
  attempt=$((attempt + 1))
  sleep 0.05
done
[[ "$socket_ready" -eq 1 ]] || die "fake Herdr did not create its Unix socket"

HOME="$WORK/home" \
CORRAL_CONFIG_DIR="$WORK/daemon-config" \
CORRAL_FLEETS_PATH="$WORK/daemon-config/fleets.json" \
CORRAL_REPO_ROOT="$WORK/repo" \
CORRAL_WORKTREES_ROOT="$WORK/worktrees" \
  "$DAEMON_BIN" --port "$PORT" --socket "$SOCKET" \
  >"$WORK/corrald.log" 2>&1 &
DAEMON_PID=$!

ready=0
attempt=0
while [[ "$attempt" -lt 200 ]]; do
  if curl --fail --silent --show-error --max-time 1 "$DAEMON_URL/healthz" >/dev/null 2>&1; then
    ready=1
    break
  fi
  attempt=$((attempt + 1))
  sleep 0.1
done
[[ "$ready" -eq 1 ]] || {
  tail -80 "$WORK/corrald.log" >&2 || true
  die "real corrald did not become healthy at $DAEMON_URL"
}

PROXY_PORT="$($PYTHON_BIN - <<'PY'
import socket

with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
BASE_URL="http://127.0.0.1:$PROXY_PORT"

cat >"$WORK/issues-proxy.py" <<'PY'
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import sys
from urllib.request import Request, urlopen

listen_port = int(sys.argv[1])
daemon_url = sys.argv[2]
issues = {
    "repos": {
        "corral": [
            {
                "repo": "corral",
                "number": 206,
                "state": "OPEN",
                "title": "egui board redesign — persistent left bar",
                "labels": [{"name": "ui", "color": "2dd4bf"}],
                "url": "https://github.com/jirathip-dev/corral/issues/206",
            },
            {
                "repo": "corral",
                "number": 199,
                "state": "CLOSED",
                "title": "retired board experiment",
                "labels": [{"name": "closed", "color": "8b949e"}],
                "url": "https://github.com/jirathip-dev/corral/issues/199",
            },
        ],
        "plush-meadow": [
            {
                "repo": "plush-meadow",
                "number": 34,
                "state": "OPEN",
                "title": "worker fleet grouping",
                "labels": [{"name": "fleet", "color": "58a6ff"}],
                "url": "https://github.com/jirathip-dev/plush-meadow/issues/34",
            }
        ],
        "sendmeter": [
            {
                "repo": "sendmeter",
                "number": 8,
                "state": "CLOSED",
                "title": "old delivery dashboard",
                "labels": [{"name": "done", "color": "3fb950"}],
                "url": "https://github.com/jirathip-dev/sendmeter/issues/8",
            }
        ],
    }
}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format, *args):
        pass

    def do_GET(self):
        if self.path.split("?", 1)[0] == "/issues":
            body = json.dumps(issues).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            self.wfile.flush()
            return
        self.forward()

    def do_POST(self):
        self.forward()

    def forward(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length) if length else None
        headers = {
            key: value
            for key, value in self.headers.items()
            if key.lower() not in {"host", "content-length", "connection"}
        }
        request = Request(
            daemon_url + self.path,
            data=body,
            headers=headers,
            method=self.command,
        )
        try:
            with urlopen(request, timeout=None) as response:
                self.send_response(response.status)
                for key, value in response.headers.items():
                    if key.lower() not in {"connection", "transfer-encoding"}:
                        self.send_header(key, value)
                self.end_headers()
                read_chunk = getattr(response, "read1", response.read)
                while chunk := read_chunk(65536):
                    self.wfile.write(chunk)
                    self.wfile.flush()
        except Exception as error:
            message = str(error).encode("utf-8")
            self.send_response(502)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(message)))
            self.end_headers()
            self.wfile.write(message)


ThreadingHTTPServer(("127.0.0.1", listen_port), Handler).serve_forever()
PY

"$PYTHON_BIN" "$WORK/issues-proxy.py" "$PROXY_PORT" "$DAEMON_URL" \
  >"$WORK/issues-proxy.log" 2>&1 &
PROXY_PID=$!

ready=0
attempt=0
while [[ "$attempt" -lt 100 ]]; do
  if curl --fail --silent --show-error --max-time 1 "$BASE_URL/healthz" >/dev/null 2>&1; then
    ready=1
    break
  fi
  attempt=$((attempt + 1))
  sleep 0.1
done
[[ "$ready" -eq 1 ]] || die "loopback issues proxy did not become healthy at $BASE_URL"

snapshot_path="$WORK/snapshot.json"
AGENT_ID=""
attempt=0
while [[ "$attempt" -lt 200 ]]; do
  curl --fail --silent --show-error "$BASE_URL/snapshot" >"$snapshot_path"
  AGENT_ID="$($PYTHON_BIN - "$snapshot_path" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    snapshot = json.load(stream)
agents = sorted(snapshot.get("agents", {}))
if agents:
    print(agents[0])
PY
)" || true
  if [[ -n "$AGENT_ID" ]]; then
    break
  fi
  attempt=$((attempt + 1))
  sleep 0.1
done
if [[ -z "$AGENT_ID" ]]; then
  printf 'egui integration: fake-herdr log:\n' >&2
  tail -80 "$WORK/fake-herdr.log" >&2 || true
  printf 'egui integration: corrald log:\n' >&2
  tail -120 "$WORK/corrald.log" >&2 || true
  die "could not select the real target from /snapshot"
fi
printf 'egui integration: real corrald selected target %s\n' "$AGENT_ID"

openssl genpkey -algorithm ED25519 -out "$WORK/device-key.pem" 2>"$WORK/openssl.log"
openssl pkey -in "$WORK/device-key.pem" -outform DER -out "$WORK/device-private.der" 2>>"$WORK/openssl.log"
openssl pkey -in "$WORK/device-key.pem" -pubout -outform DER -out "$WORK/device-public.der" 2>>"$WORK/openssl.log"
HOME="$WORK/home" \
CORRAL_CONFIG_DIR="$WORK/daemon-config" \
CORRAL_FLEETS_PATH="$WORK/daemon-config/fleets.json" \
  "$PYTHON_BIN" - "$BASE_URL" "$WORK/daemon-config" "$WORK/ui-config" \
  "$WORK/device-private.der" "$WORK/device-public.der" <<'PY'
import base64
import hashlib
import json
from pathlib import Path
import sys
from urllib.request import Request, urlopen

base_url, daemon_dir, ui_dir, private_path, public_path = sys.argv[1:]
daemon_dir = Path(daemon_dir)
ui_dir = Path(ui_dir)
private_der = Path(private_path).read_bytes()
public_der = Path(public_path).read_bytes()
seed = private_der[-32:]
public_key = base64.b64encode(public_der[-32:]).decode("ascii")
token = (daemon_dir / "registration-token").read_text(encoding="utf-8").strip()
host = json.load(urlopen(f"{base_url}/host-key", timeout=5))
fingerprint = hashlib.sha256(host["public_key"].encode("utf-8")).hexdigest()[:16]
request = Request(
    f"{base_url}/register",
    data=json.dumps({"token": token, "public_key": public_key}).encode("utf-8"),
    headers={"Content-Type": "application/json"},
)
registration = json.load(urlopen(request, timeout=5))
if registration.get("grants") != []:
    raise SystemExit(f"fixture registration was not read-only: {registration}")
(ui_dir / "keys").mkdir(parents=True, exist_ok=True)
(ui_dir / "keys" / f"{fingerprint}.key").write_text(
    base64.b64encode(seed).decode("ascii") + "\n", encoding="ascii"
)
(ui_dir / "keys" / f"{fingerprint}.key").chmod(0o600)
(ui_dir / "config.json").write_text(
    json.dumps(
        {
            "host_url": base_url,
            "registration": {
                "host_fingerprint": fingerprint,
                "key_id": registration["key_id"],
                "grants": [],
                "denied": [],
            },
            "auto_reconnect": True,
            "group_by_repo": True,
            "show_idle_collapsed": True,
            "stick_to_bottom": True,
            "theme": "dark",
        },
        indent=2,
    )
    + "\n",
    encoding="utf-8",
)
(ui_dir / "config.json").chmod(0o600)
PY

cat >"$WORK/wake-window.sh" <<'WAKE'
#!/usr/bin/env bash
set -euo pipefail
: "${CORRAL_UI_SCREENSHOT_PID:?missing screenshot pid}"
: "${CORRAL_TEST_WAKE_LOG:?missing wake log path}"
set +e
osascript >"$CORRAL_TEST_WAKE_LOG" 2>&1 <<APPLESCRIPT
tell application "System Events"
  tell first application process whose unix id is ${CORRAL_UI_SCREENSHOT_PID}
    set frontmost to true
    key code 53
  end tell
end tell
APPLESCRIPT
status=$?
set -e
if [[ "$status" -ne 0 ]]; then
  printf 'exact-PID wake failed (status %s):\n' "$status" >&2
  sed -n '1,80p' "$CORRAL_TEST_WAKE_LOG" >&2 || true
  exit "$status"
fi
WAKE
chmod +x "$WORK/wake-window.sh"

export HOME="$WORK/home"
export CORRAL_CONFIG_DIR="$WORK/daemon-config"
export CORRAL_FLEETS_PATH="$WORK/daemon-config/fleets.json"
export CORRALD_BIN="$DAEMON_BIN"
export CORRAL_UI_CONFIG_DIR="$WORK/ui-config"
export CORRAL_UI_DISABLE_KEYRING=1
export CORRAL_TEST_WAKE_LOG="$WORK/wake-osascript.log"

printf 'egui integration: capturing all four native #206 tabs\n'
for tab in board issues registry settings; do
  printf 'egui integration: capturing tab %s\n' "$tab"
  bash "$SCRIPT" \
    --issue 206 \
    --surface egui \
    --prototype "$REPO_DIR/docs/design/corral-ux-egui-redesign-prototype.html" \
    --egui-tab "$tab" \
    --host-url "$BASE_URL" \
    --live-agent "$AGENT_ID" \
    --egui-binary "$UI_BIN" \
    --no-build \
    --delay-ms 12000 \
    --timeout-seconds 45 \
    --chrome-timeout-seconds 30 \
    --egui-wake-command "$WORK/wake-window.sh" \
    --provenance-note "real scratch corrald, fake Herdr, three scratch repos, and loopback /issues-only proxy; registry check passed" \
    --output-root "$WORK/evidence/$tab"

  OUTPUT_DIR="$WORK/evidence/$tab/issue-206"
  PUBLISH_DIR="$REPO_DIR/docs/design/evidence/issue-206/$tab"
  [[ -s "$OUTPUT_DIR/live-after.png" ]] || die "real egui $tab capture PNG is missing"
  grep -F -- "- Egui tab: \`$tab\`" "$OUTPUT_DIR/conformance.md" \
    || die "$tab conformance did not record the requested tab"
  grep -F -- "- Selected live agent: \`$AGENT_ID\`" "$OUTPUT_DIR/conformance.md" \
    || die "$tab conformance did not record the /snapshot target selection"
  grep -F -- "native screenshot evidence selected live agent" "$OUTPUT_DIR/capture.log" \
    || die "$tab native app log did not prove target selection"
  grep -F -- "$AGENT_ID" "$OUTPUT_DIR/capture.log" \
    || die "$tab native app log did not contain the selected target id"
  if [[ "$MODE" == "publish" ]]; then
    mkdir -p "$PUBLISH_DIR"
    for artifact in prototype.png live-after.png comparison.png conformance.md capture.log; do
      cp -- "$OUTPUT_DIR/$artifact" "$PUBLISH_DIR/$artifact"
    done
  fi
done

"$PYTHON_BIN" - \
  "$REPO_DIR/docs/design/evidence/issue-206"/board/live-after.png \
  "$REPO_DIR/docs/design/evidence/issue-206"/issues/live-after.png \
  "$REPO_DIR/docs/design/evidence/issue-206"/registry/live-after.png \
  "$REPO_DIR/docs/design/evidence/issue-206"/settings/live-after.png <<'PY'
from pathlib import Path
import struct
import sys
import zlib

for argument in sys.argv[1:]:
    data = Path(argument).read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", f"{argument} is not a PNG"
    offset = 8
    seen_iend = False
    idat = []
    while offset < len(data):
        assert len(data) - offset >= 12, f"{argument} has a truncated chunk"
        length = struct.unpack(">I", data[offset : offset + 4])[0]
        kind = data[offset + 4 : offset + 8]
        payload_end = offset + 8 + length
        assert payload_end + 4 <= len(data), f"{argument} has a truncated payload"
        payload = data[offset + 8 : payload_end]
        crc = struct.unpack(">I", data[payload_end : payload_end + 4])[0]
        assert zlib.crc32(kind + payload) & 0xFFFFFFFF == crc, f"{argument} PNG CRC failed"
        if kind == b"IHDR":
            width, height = struct.unpack(">II", payload[:8])
        elif kind == b"IDAT":
            idat.append(payload)
        elif kind == b"IEND":
            assert length == 0, f"{argument} IEND is not empty"
            seen_iend = True
            offset = payload_end + 4
            break
        offset = payload_end + 4
    assert seen_iend and offset == len(data), f"{argument} PNG is incomplete or has trailing data"
    decoder = zlib.decompressobj()
    decoder.decompress(b"".join(idat))
    decoder.flush()
    assert decoder.eof and not decoder.unused_data, f"{argument} PNG IDAT stream is incomplete"
    print(f"complete native PNG: {Path(argument).parent.name}/{width}x{height}")
PY

cleanup

"$PYTHON_BIN" "$SCRIPT_DIR/verify-design-gate-egui-evidence.py" \
  "$REPO_DIR" "$CONTENT_IDENTITY_HELPER"

"$PYTHON_BIN" - "$WORK" "$UI_BIN" "$DAEMON_BIN" <<'PY'
import os
import subprocess
import sys

work, ui_bin, daemon_bin = sys.argv[1:]
rows = subprocess.check_output(["ps", "-axo", "pid=,command="], text=True).splitlines()
matches = []
for row in rows:
    if row.split(None, 1)[0] == str(os.getpid()):
        continue
    if ui_bin in row or (daemon_bin in row and work in row) or (work in row and "Google Chrome" in row):
        matches.append(row.strip())
if matches:
    raise SystemExit("owned integration processes survived cleanup:\n" + "\n".join(matches))
print("no scratch corrald, corrald-ui, or Chrome process remains")
PY

printf 'OK: real corrald/native egui target selection, complete PNG, safe wake, and cleanup\n'
