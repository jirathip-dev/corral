#!/usr/bin/env python3
"""Paired private corrald processes: real read-only fleet, 120s CPU/encoder windows.

Reuses issue-492's cumulative CPU parser. Does not restart or configure a live
service. Only aggregate numbers are committed; raw snapshots/logs stay private.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location('g492_measure', ROOT/'docs/evidence/issue-492/measure.py')
g492 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(g492)
SECONDS = 120
WARMUP = 15
HOME = Path.home()
DIRECTORY = Path(tempfile.mkdtemp(prefix='g560-cost-'))
DIRECTORY.chmod(0o700)
(DIRECTORY/'.g560-owned').write_text(str(ROOT)+'\n')


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(binary, label):
    directory = DIRECTORY/label
    directory.mkdir()
    home = directory/'home'
    home.mkdir()
    (home/'Projects').symlink_to(HOME/'Projects', target_is_directory=True)
    bin_dir = directory/'bin'
    bin_dir.mkdir()
    (bin_dir/'git').symlink_to(shutil.which('git'))
    env = {k:v for k,v in os.environ.items() if not k.startswith(('GH_', 'GITHUB_', 'APNS_', 'CORRAL_'))}
    env.update(HOME=str(home), CORRAL_CONFIG_DIR=str(directory/'config'),
               CORRAL_REPO_ROOT=str(HOME/'Projects/corral'),
               CORRAL_WORKTREES_ROOT=str(HOME/'.herdr/worktrees'),
               PATH=str(bin_dir)+':/usr/bin:/bin', NO_COLOR='1',
               RUST_LOG='corrald::core::store::published=debug')
    assert shutil.which('gh', path=env['PATH']) is None
    with socket.socket() as listener:
        listener.bind(('127.0.0.1',0))
        port = listener.getsockname()[1]
    url = f'http://127.0.0.1:{port}'
    command = [str(binary), '--socket', str(HOME/'.config/herdr/herdr.sock'),
               '--bind','127.0.0.1','--port',str(port)]
    output = (directory/'stdio.log').open('wb')
    child = subprocess.Popen(command, env=env, stdout=output, stderr=subprocess.STDOUT,
                             start_new_session=True)
    sse = None
    def snapshot():
        request = urllib.request.Request(url+'/snapshot', headers={'Connection':'close'})
        with urllib.request.urlopen(request, timeout=5) as response:
            return json.load(response)
    try:
        deadline = time.monotonic()+60
        while time.monotonic() < deadline:
            assert child.poll() is None, 'private daemon exited before readiness'
            try:
                initial = snapshot()
                if len(initial['agents']) >= 38:
                    break
            except OSError:
                pass
            time.sleep(.25)
        else:
            raise RuntimeError('current 38+ agent fleet not ready within 60s')
        sse = subprocess.Popen(['/usr/bin/curl','-sS','-N','--max-time','180',url+'/events'],
                               stdout=subprocess.DEVNULL, stderr=output)
        time.sleep(WARMUP)
        log_path = directory/'config/corrald.log'
        with log_path.open('rb') as log:
            log.seek(0,2)
            first = snapshot()
            cpu0 = g492.cpu_seconds(child.pid)
            start = time.monotonic()
            load_start = subprocess.check_output(['uptime'],text=True).strip()
            samples = []
            for index in range(1,25):
                time.sleep(max(0,start+index*5-time.monotonic()))
                assert child.poll() is None and sse.poll() is None
                cpu = g492.cpu_seconds(child.pid)
                elapsed = time.monotonic()-start
                body = snapshot()
                samples.append(dict(elapsed_seconds=elapsed, cpu_seconds=cpu,
                                    agents=len(body['agents']), rev=body['rev'],
                                    facts=len(body['git_worktree_facts']), alive=body['git_plane_alive']))
            raw = log.read()
            assert os.fstat(log.fileno()).st_ino == log_path.stat().st_ino, 'unexpected log rotation'
        counts = [len(first['agents'])]+[s['agents'] for s in samples]
        assert min(counts) >= 38
        encoded = raw.count(b'snapshot agents encoded')
        publications = raw.count(b'snapshot published')
        assert publications > 0, 'encoder observation missing'
        result = dict(label=label, binary_sha256=sha(binary), command=command,
                      method='delta(ps time user+system CPU)/monotonic wall seconds; child CPU excluded; one SSE subscriber; 5s samples',
                      warmup_seconds=WARMUP, requested_seconds=SECONDS, elapsed_seconds=elapsed,
                      cpu_seconds_start=cpu0, cpu_seconds_end=cpu, daemon_cpu_seconds=cpu-cpu0,
                      cpu_percent=100*(cpu-cpu0)/elapsed,
                      publications=publications, full_agent_encodes=encoded,
                      full_agent_encodes_per_minute=encoded*60/elapsed,
                      agent_count_min=min(counts), agent_count_max=max(counts),
                      initial_agent_set_sha256=hashlib.sha256('\n'.join(sorted(first['agents'])).encode()).hexdigest(),
                      final_agent_set_sha256=hashlib.sha256('\n'.join(sorted(body['agents'])).encode()).hexdigest(),
                      load_start=load_start, load_end=subprocess.check_output(['uptime'],text=True).strip(),
                      encoder_window_log_sha256=hashlib.sha256(raw).hexdigest(),
                      private_directory=str(directory), samples=samples)
        (directory/'encoder-window.log').write_bytes(raw)
        print(json.dumps(result),flush=True)
        return result
    finally:
        if sse and sse.poll() is None:
            sse.terminate()
            sse.wait(timeout=10)
        if child.poll() is None:
            os.killpg(child.pid,signal.SIGTERM)
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid,signal.SIGKILL)
                child.wait(timeout=10)
        output.close()


results = []
for label, path in zip(('before','after'),sys.argv[1:]):
    results.append(run(Path(path).resolve(),label))
    (ROOT/'docs/evidence/issue-560/measurements.json').write_text(json.dumps(results,indent=2)+'\n')
assert len(results) == 2
print('G560_PAIRED_MEASUREMENT_COMPLETE private processes stopped',flush=True)
