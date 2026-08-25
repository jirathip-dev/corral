#!/usr/bin/env bash
# Real egui design-gate integration: scratch corrald + fake herdr socket +
# native corrald-ui/wgpu capture. This is separate from the fast hermetic
# seam tests because it needs a macOS window server and a real Chrome.
#
# Run with:
#   bash scripts/test-design-gate-egui-integration.sh
#
# The harness owns every process it starts, uses a fresh loopback port/config,
# creates a real scratch git repo for the fake agent, prepares a registered UI
# config, and asks the design-gate script to capture that target. The wake
# helper brings only the exact corrald-ui pid's process frontmost; it does not
# send keystrokes, broadcast input, or click arbitrary windows. The EXIT trap
# uses TERM, a short grace period, then KILL for the direct children it owns.

set -euo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SCRIPT="$SCRIPT_DIR/design-gate-evidence.sh"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || true)}"
DAEMON_BIN="$REPO_DIR/target/release/corrald"
UI_BIN="$REPO_DIR/target/release/corrald-ui"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/corral-design-gate-egui.XXXXXX")"
HERDR_PID=""
DAEMON_PID=""
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

cleanup() {
  local cleanup_status=0
  if [[ "$CLEANED" -eq 0 ]]; then
    if ! stop_owned_child "$DAEMON_PID" corrald; then
      cleanup_status=1
    fi
    if ! stop_owned_child "$HERDR_PID" fake-herdr; then
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

SOCKET="$WORK/herdr.sock"
PORT="$($PYTHON_BIN - <<'PY'
import socket

with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
BASE_URL="http://127.0.0.1:$PORT"

cat >"$WORK/fake-herdr.py" <<'PY'
import asyncio
import json
from pathlib import Path
import sys

socket_path = Path(sys.argv[1])
repo = sys.argv[2]
agent = {
    "agent": "claude",
    "agent_status": "working",
    "cwd": repo,
    "foreground_cwd": repo,
    "focused": False,
    "interactive_ready": True,
    "name": "design-gate-fixture",
    "pane_id": "design-gate:p1",
    "revision": 1,
    "state_labels": {},
    "state_change_seq": 1,
    "title": "Design gate fixture agent",
    "terminal_title_stripped": "Design gate fixture agent",
    "workspace_id": "design-gate",
}


async def handle(reader, writer):
    try:
        while line := await reader.readline():
            if not line.strip():
                continue
            request = json.loads(line)
            request_id = request.get("id")
            method = request.get("method")
            if method == "agent.list":
                result = {"agents": [agent]}
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
  >"$WORK/fake-herdr.log" 2>&1 &
HERDR_PID=$!

HOME="$WORK/home" \
CORRAL_CONFIG_DIR="$WORK/daemon-config" \
CORRAL_REPO_ROOT="$WORK/repo" \
CORRAL_WORKTREES_ROOT="$WORK/worktrees" \
  "$DAEMON_BIN" --port "$PORT" --socket "$SOCKET" \
  >"$WORK/corrald.log" 2>&1 &
DAEMON_PID=$!

ready=0
attempt=0
while [[ "$attempt" -lt 200 ]]; do
  if curl --fail --silent --show-error --max-time 1 "$BASE_URL/healthz" >/dev/null 2>&1; then
    ready=1
    break
  fi
  attempt=$((attempt + 1))
  sleep 0.1
done
[[ "$ready" -eq 1 ]] || {
  tail -80 "$WORK/corrald.log" >&2 || true
  die "real corrald did not become healthy at $BASE_URL"
}

snapshot_path="$WORK/snapshot.json"
curl --fail --silent --show-error "$BASE_URL/snapshot" >"$snapshot_path"
AGENT_ID="$($PYTHON_BIN - "$snapshot_path" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    snapshot = json.load(stream)
agents = sorted(snapshot.get("agents", {}))
if not agents:
    raise SystemExit("real corrald snapshot has no fake-herdr agent")
print(agents[0])
PY
)"
[[ -n "$AGENT_ID" ]] || die "could not select the real target from /snapshot"
printf 'egui integration: real corrald selected target %s\n' "$AGENT_ID"

openssl genpkey -algorithm ED25519 -out "$WORK/device-key.pem" 2>"$WORK/openssl.log"
openssl pkey -in "$WORK/device-key.pem" -outform DER -out "$WORK/device-private.der" 2>>"$WORK/openssl.log"
openssl pkey -in "$WORK/device-key.pem" -pubout -outform DER -out "$WORK/device-public.der" 2>>"$WORK/openssl.log"
HOME="$WORK/home" \
CORRAL_CONFIG_DIR="$WORK/daemon-config" \
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
osascript >"$CORRAL_TEST_WAKE_LOG" 2>&1 <<APPLESCRIPT
tell application "System Events"
  tell first application process whose unix id is ${CORRAL_UI_SCREENSHOT_PID}
    set frontmost to true
  end tell
end tell
APPLESCRIPT
WAKE
chmod +x "$WORK/wake-window.sh"

export HOME="$WORK/home"
export CORRAL_CONFIG_DIR="$WORK/daemon-config"
export CORRAL_UI_CONFIG_DIR="$WORK/ui-config"
export CORRAL_TEST_WAKE_LOG="$WORK/wake-osascript.log"

printf 'egui integration: capturing the real native board\n'
bash "$SCRIPT" \
  --issue 225 \
  --surface egui \
  --host-url "$BASE_URL" \
  --live-agent "$AGENT_ID" \
  --egui-binary "$UI_BIN" \
  --no-build \
  --delay-ms 8000 \
  --timeout-seconds 45 \
  --chrome-timeout-seconds 30 \
  --egui-wake-command "$WORK/wake-window.sh" \
  --output-root "$WORK/evidence"

OUTPUT_DIR="$WORK/evidence/issue-225"
[[ -s "$OUTPUT_DIR/live-after.png" ]] || die "real egui capture PNG is missing"
grep -F -- "- Selected live agent: \`$AGENT_ID\`" "$OUTPUT_DIR/conformance.md" \
  || die "conformance did not record the /snapshot target selection"
grep -F -- "native screenshot evidence selected live agent" "$OUTPUT_DIR/capture.log" \
  || die "native app log did not prove target selection"
grep -F -- "$AGENT_ID" "$OUTPUT_DIR/capture.log" \
  || die "native app log did not contain the selected target id"

"$PYTHON_BIN" - "$OUTPUT_DIR/live-after.png" <<'PY'
from pathlib import Path
import struct
import sys
import zlib

data = Path(sys.argv[1]).read_bytes()
assert data[:8] == b"\x89PNG\r\n\x1a\n", "capture is not a PNG"
offset = 8
seen_iend = False
idat = []
while offset < len(data):
    assert len(data) - offset >= 12, "PNG has a truncated chunk"
    length = struct.unpack(">I", data[offset : offset + 4])[0]
    kind = data[offset + 4 : offset + 8]
    payload_end = offset + 8 + length
    assert payload_end + 4 <= len(data), "PNG has a truncated payload"
    payload = data[offset + 8 : payload_end]
    crc = struct.unpack(">I", data[payload_end : payload_end + 4])[0]
    assert zlib.crc32(kind + payload) & 0xFFFFFFFF == crc, "PNG CRC failed"
    if kind == b"IHDR":
        width, height = struct.unpack(">II", payload[:8])
    elif kind == b"IDAT":
        idat.append(payload)
    elif kind == b"IEND":
        assert length == 0, "IEND is not empty"
        seen_iend = True
        offset = payload_end + 4
        break
    offset = payload_end + 4
assert seen_iend and offset == len(data), "PNG is incomplete or has trailing data"
decoder = zlib.decompressobj()
decoder.decompress(b"".join(idat))
decoder.flush()
assert decoder.eof and not decoder.unused_data, "PNG IDAT stream is incomplete"
print(f"complete native PNG: {width}x{height}")
PY

cleanup

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
