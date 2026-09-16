#!/usr/bin/env python3
"""Isolated daemon + fictional Herdr lane, never the installed service.

No-token/refusal run without credentials. env/file use G556_LIVE_TOKEN supplied
by the operator only to this driver (never printed or saved in evidence).
GitHub responses in those modes are LIVE; the Herdr lane and git checkout are
fixtures. The credential's temporary file is private and removed on exit.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import socketserver
import subprocess
import tempfile
import threading
import time
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument("binary", type=Path)
parser.add_argument("mode", choices=["none", "env", "file", "refused"])
parser.add_argument("--seconds", type=int, default=300)
parser.add_argument("--branch", default="unbound-556-fixture")
parser.add_argument("--pr", type=int)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
binary = args.binary.resolve()
args.output.mkdir(parents=True, exist_ok=True)
assert args.seconds >= 1
live = args.mode in ("env", "file")
credential = os.environ.get("G556_LIVE_TOKEN", "") if live else ""
if live and not credential.strip():
    raise SystemExit("live mode needs operator-provided G556_LIVE_TOKEN")
assert not shutil.which("gh", path="/usr/bin:/bin"), "fixture PATH must not contain gh"
with tempfile.TemporaryDirectory(prefix="g556-", dir="/tmp") as temp:
    root = Path(temp)
    config = root / "config"
    config.mkdir(mode=0o700)
    checkout = root / "repo"
    checkout.mkdir()
    for command in [
        ["git", "init", "-b", args.branch, str(checkout)],
        ["git", "-C", str(checkout), "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-m", "Fictional #556 lane"],
        ["git", "-C", str(checkout), "remote", "add", "origin", "https://github.com/jirathip-dev/corral.git"],
    ]:
        subprocess.run(command, check=True, stdout=subprocess.DEVNULL, timeout=15)
    class Herdr(socketserver.StreamRequestHandler):
        def handle(self):
            for line in self.rfile:
                request = json.loads(line)
                if request["method"] == "agent.list":
                    result = {"agents": [{"agent": "opencode", "agent_status": "idle", "cwd": str(checkout),
                        "name": "github-fixture", "pane_id": "p556", "state_labels": {},
                        "terminal_title_stripped": "Fictional GitHub binding evidence", "workspace_id": "w556"}]}
                else:
                    result = {"ok": True}
                self.wfile.write((json.dumps({"id": request["id"], "result": result}) + "\n").encode())
                self.wfile.flush()
    class Server(socketserver.ThreadingUnixStreamServer):
        daemon_threads = True
    server = Server(str(root / "herdr.sock"), Herdr)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    # Do not inherit host service credentials, proxies, or debug key-log paths.
    env = {}
    env.update(HOME=temp, PATH="/usr/bin:/bin", CORRAL_CONFIG_DIR=str(config),
        CORRAL_REPO_ROOT=str(checkout), CORRAL_WORKTREES_ROOT=str(root / "worktrees"), RUST_LOG="info")
    if args.mode == "env":
        env["GITHUB_TOKEN"] = credential
        # A distinct losing file also proves env precedence at the real client.
        value = "".join(("ghp_", "FAKE556loser"))
    elif args.mode == "refused":
        value = "".join(("ghp_", "FAKE556refused"))
    else:
        value = credential
    if args.mode != "none":
        token_path = config / "github-token"
        fd = os.open(token_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(value + "\n")
        if args.mode == "refused":
            token_path.chmod(0o644)
    command = [str(binary), "--socket", str(root / "herdr.sock"), "--port", str(port)]
    receipt = {"mode": args.mode, "fixture": "Herdr agent and checkout; live GitHub only in env/file modes",
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "command": command,
        "PATH": env["PATH"], "gh_on_PATH": False, "branch": args.branch, "expected_pr": args.pr}
    def snapshot():
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/snapshot", timeout=3) as response:
            return json.load(response)
    def history_bytes():
        return {str(p.relative_to(config)): p.stat().st_size for p in (config / "history").rglob("*") if p.is_file()}
    child = subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    sse = None
    try:
        deadline = time.monotonic() + 30
        while True:
            assert child.poll() is None, "daemon exited during startup"
            try:
                before = snapshot()
                if before["agents"] and all(a["workspace"]["branch"] for a in before["agents"].values()):
                    break
            except (OSError, ValueError):
                pass
            assert time.monotonic() < deadline, "fixture lane did not converge"
            time.sleep(0.2)
        (args.output / "before.json").write_text(json.dumps(before, indent=2) + "\n")
        history_before = history_bytes()
        # Real subscriber: no-token evidence cannot pass just because the SSE
        # client gate or an empty repository set prevented a poll.
        sse = subprocess.Popen(["/usr/bin/curl", "-sS", "-N", "--max-time", str(args.seconds + 10), f"http://127.0.0.1:{port}/events"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        started = time.monotonic()
        samples = []
        with (args.output / "connections.log").open("w") as connection_log:
            while time.monotonic() - started < args.seconds:
                assert child.poll() is None, "daemon exited during observation"
                probe = subprocess.run(["/usr/sbin/lsof", "-nP", "-a", "-p", str(child.pid), "-i"], capture_output=True, text=True, timeout=5)
                elapsed = round(time.monotonic() - started, 3)
                samples.append({"elapsed_s": elapsed, "exit": probe.returncode})
                connection_log.write(f"elapsed_s={elapsed} lsof_exit={probe.returncode}\n{probe.stdout}{probe.stderr}")
                connection_log.flush()
                if not live:
                    assert all("127.0.0.1:" in line for line in probe.stdout.splitlines()[1:]), "non-loopback socket in disabled daemon"
                time.sleep(1)
        after = snapshot()
        (args.output / "after.json").write_text(json.dumps(after, indent=2) + "\n")
        history_after = history_bytes()
        logs = "\n".join(p.read_text() for p in sorted(config.glob("*log*")) if p.is_file())
        assert not credential or credential not in logs, "credential leaked into daemon log"
        for p in (args.output / "before.json", args.output / "after.json", args.output / "connections.log"):
            assert not credential or credential not in p.read_text(), "credential leaked into evidence"
        (args.output / "daemon.log").write_text(logs)
        workspaces = [a["workspace"] for a in after["agents"].values()]
        if live:
            assert logs.count("gh plane round-trip complete") >= 2, "need two successful live polls"
            assert args.pr and any(w["pr_number"] == args.pr and w["ci_status"] is not None for w in workspaces), "live PR branch did not bind"
        else:
            assert logs.count("github: disabled (no token)") == 1
            assert "gh plane round-trip complete" not in logs
            assert before["agents"] == after["agents"], "agent snapshot changed"
            assert before["rev"] == after["rev"], "new agent delta"
            assert history_before == history_after, "new transition journal bytes"
            assert all(w["pr_number"] is None and w["ci_status"] is None for w in workspaces)
            if args.mode == "refused":
                assert "github: token file refused" in logs
        receipt.update(seconds=round(time.monotonic()-started,3), samples=samples,
            history_before=history_before, history_after=history_after,
            agent_snapshot_unchanged=before["agents"] == after["agents"], raw_exit=0)
    finally:
        if sse and sse.poll() is None:
            sse.terminate()
            sse.wait(timeout=5)
        if child.poll() is None:
            child.terminate()
            child.wait(timeout=5)
        receipt["daemon_termination_exit"] = child.returncode
        server.shutdown()
        server.server_close()
        (args.output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({k:v for k,v in receipt.items() if k != "samples"}, indent=2))
