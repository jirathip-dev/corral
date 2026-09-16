#!/usr/bin/env python3
"""Owned foreground daemons, real read-only Herdr/Git inputs, interval CPU.

Run under flock /tmp/n.lock. No installs, services, auth, pruning or writes
into live checkouts. Raw HTTP bodies/logs remain in the private /tmp run dir;
only the aggregate JSON is intended for committed evidence.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import urllib.request


def cpu_seconds(pid):
    if platform.system() == 'Linux':
        fields = Path(f'/proc/{pid}/stat').read_text().split(') ', 1)[1].split()
        return (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK')
    raw = subprocess.check_output(['ps', '-p', str(pid), '-o', 'time='], text=True).strip()
    days, clock = (raw.split('-', 1) if '-' in raw else ('0', raw))
    parts = [float(p) for p in clock.split(':')]
    value = 0
    for part in parts:
        value = value * 60 + part
    return int(days) * 86400 + value


def inventory(home):
    roots = sorted(p.resolve() for p in (home / 'Projects').iterdir()
                   if p.is_dir() and (p / '.git').exists())
    watched, unavailable = set(), []
    worktrees_root = (home / '.herdr/worktrees').resolve()
    for root in roots:
        result = subprocess.run(['git', '--no-optional-locks', '-C', str(root),
                                 'worktree', 'list', '--porcelain'],
                                capture_output=True, text=True, timeout=15)
        if result.returncode:
            unavailable.append(str(root))
        for line in result.stdout.splitlines():
            if line.startswith('worktree '):
                path = Path(line[9:]).resolve()
                if path.is_dir() and (path == root or path.is_relative_to(worktrees_root)):
                    watched.add(str(path))
    return dict(roots=[str(p) for p in roots], paths=sorted(watched), unavailable=unavailable)


def summarize(body):
    facts = body.get('git_worktree_facts', {})
    agents = list(body['agents'].values())
    repo_agents = [a for a in agents if a.get('workspace', {}).get('repo')]
    live = [a for a in repo_agents if Path(a['workspace'].get('worktree_path') or '/nonexistent-g492').is_dir()]
    return dict(rev=body['rev'], schema_version=body['schema_version'], epoch=body.get('epoch'),
                generated_at=body['generated_at'], agents=len(agents), repo_agents=len(repo_agents),
                live_repo_agents=len(live), live_missing_head=sum(not a['workspace'].get('head_sha') for a in live),
                backlog=body['git_plane_backlog'], alive=body.get('git_plane_alive'),
                last_event_age_ms=body.get('git_plane_last_event_age_ms'), skipped=body.get('git_plane_skipped'),
                worktree_facts=len(facts), stale=sum(f['stale'] for f in facts.values()),
                absent=sum(f['fact_age_ms'] is None for f in facts.values()),
                oldest_fact_ms=max((f['fact_age_ms'] for f in facts.values() if f['fact_age_ms'] is not None), default=None))


class LogReader:
    """Follow the same fd across rename, then the replacement: count rotations too."""
    def __init__(self, path):
        self.path, self.file, self.partial = path, None, ''
        self.warns = self.over_budget = self.bytes = 0

    def drain(self):
        for _ in range(3):
            if self.file is None:
                if not self.path.exists():
                    return
                self.file = self.path.open('r', errors='replace')
            data = self.file.read()
            self.bytes += len(data.encode())
            lines = (self.partial + data).split('\n')
            self.partial = lines.pop()
            for line in lines:
                self.warns += 'WARN' in line
                self.over_budget += 'git plane event over budget' in line
            if self.path.exists() and os.fstat(self.file.fileno()).st_ino != self.path.stat().st_ino:
                self.file.close()
                self.file = None
            else:
                return
        raise RuntimeError('log rotated faster than the bounded reader')

    def counts(self):
        self.drain()
        return self.warns, self.over_budget, self.bytes

    def close(self):
        if self.file:
            self.file.close()


def measure(binary, label, home, directory, warmup, seconds):
    run = directory / label
    run.mkdir()
    scratch_home = run / 'home'
    scratch_home.mkdir()
    # Discovery sees the real, untouched Project roots; other HOME writes stay private.
    (scratch_home / 'Projects').symlink_to(home / 'Projects', target_is_directory=True)
    private_bin = run / 'bin'
    private_bin.mkdir()
    (private_bin / 'git').symlink_to(shutil.which('git'))
    env = os.environ.copy()
    for key in list(env):
        if key.startswith(('GH_', 'GITHUB_', 'APNS_', 'CORRAL_')):
            env.pop(key)
    repositories = sorted(p for p in (home / 'Projects').iterdir()
                          if p.is_dir() and (p / '.git').exists())
    assert repositories, 'real Projects git sources are required'
    fallback = home / 'Projects/corral'
    if not (fallback / '.git').exists():
        fallback = repositories[0]
    env.update(HOME=str(scratch_home), CORRAL_CONFIG_DIR=str(run / 'config'),
               CORRAL_REPO_ROOT=str(fallback),
               CORRAL_WORKTREES_ROOT=str(home / '.herdr/worktrees'),
               PATH=str(private_bin) + ':/usr/bin:/bin',
               RUST_LOG='corrald=info', NO_COLOR='1')
    # No gh executable/credentials in the test process. The real Herdr socket
    # is read-only; no /drive or owner-channel requests are sent by this harness.
    assert shutil.which('gh', path=env['PATH']) is None
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
    url = f'http://127.0.0.1:{port}/snapshot'
    command = [str(binary), '--socket', str(home / '.config/herdr/herdr.sock'),
               '--bind', '127.0.0.1', '--port', str(port)]
    stdout = (run / 'stdout.log').open('wb')
    stderr = (run / 'stderr.log').open('wb')
    child = subprocess.Popen(command, env=env, stdout=stdout, stderr=stderr, start_new_session=True)
    readers = [LogReader(run / 'stdout.log'), LogReader(run / 'stderr.log'),
               LogReader(run / 'config/corrald.log')]
    samples = []
    try:
        deadline = time.monotonic() + 60
        while True:
            assert child.poll() is None, f'daemon exited: {child.returncode}'
            try:
                with urllib.request.urlopen(url, timeout=4) as response:
                    body = json.load(response)
                if body['agents']:
                    break
            except OSError:
                pass
            if time.monotonic() > deadline:
                raise RuntimeError('real /snapshot did not return populated live agents within 60s')
            time.sleep(.2)
        assert ('git_plane_alive' in body) == (label == 'after'), 'binary/source mismatch: unexpected liveness schema'
        time.sleep(warmup)
        before = inventory(home)
        def take_snapshot(index):
            with urllib.request.urlopen(url, timeout=5) as response:
                raw = response.read()
            (run / f'snapshot-{index}.json').write_bytes(raw)
            return summarize(json.loads(raw))
        first = take_snapshot('start')
        counts0 = tuple(sum(v) for v in zip(*(r.counts() for r in readers)))
        start, cpu0 = time.monotonic(), cpu_seconds(child.pid)
        previous_time, previous_cpu = start, cpu0
        for index in range(int(seconds / 5)):
            time.sleep(max(0, start + (index+1)*5 - time.monotonic()))
            now, cpu = time.monotonic(), cpu_seconds(child.pid)
            snapshot = take_snapshot(index)
            samples.append(dict(elapsed=now-start, cpu_seconds=cpu,
                                interval_percent=100*(cpu-previous_cpu)/(now-previous_time), snapshot=snapshot))
            previous_time, previous_cpu = now, cpu
            for reader in readers:
                reader.drain()
        elapsed = previous_time - start
        counts1 = tuple(sum(v) for v in zip(*(r.counts() for r in readers)))
        counts = [b-a for a,b in zip(counts0,counts1)]
        after = inventory(home)
        if label == 'after':
            assert any(s['snapshot']['alive'] is True for s in samples), 'fixed plane was never alive'
            assert any(s['snapshot']['oldest_fact_ms'] is not None for s in samples), 'fixed plane never published facts'
        result = dict(label=label, binary=str(binary), binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                      command=command, pid=child.pid, url=url, warmup_seconds=warmup, measured_seconds=elapsed,
                      daemon_cpu_seconds=previous_cpu-cpu0, daemon_interval_percent=100*(previous_cpu-cpu0)/elapsed,
                      warnings=counts[0], over_budget_warnings=counts[1], log_bytes=counts[2],
                      over_budget_per_minute=counts[1]*60/elapsed, inventory_count=len(before['paths']),
                      inventory_unchanged=before==after, initial_snapshot=first, samples=samples,
                      raw_directory=str(run))
        (run / 'inventory-before.json').write_text(json.dumps(before,indent=2)+'\n')
        (run / 'inventory-after.json').write_text(json.dumps(after,indent=2)+'\n')
        (run / 'measurement.json').write_text(json.dumps(result, indent=2)+'\n')
        assert before == after, 'live inventory changed during leg; comparison is not controlled'
        return result, before
    finally:
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=10)
        for reader in readers:
            reader.close()
        stdout.close(); stderr.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--before', required=True, type=Path)
    parser.add_argument('--after', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--scratch-root', type=Path, default=Path('/tmp'))
    parser.add_argument('--warmup', type=int, default=15)
    parser.add_argument('--seconds', type=int, default=120)
    args = parser.parse_args()
    home = Path.home()
    directory = Path(tempfile.mkdtemp(prefix='g492-cost-', dir=args.scratch_root))
    directory.chmod(0o700)
    (directory / '.g492-owned').touch()
    results, populations = [], []
    for label,binary in [('before',args.before), ('after',args.after)]:
        subprocess.run(['df','-h','.','/tmp'],check=True)
        result, population = measure(binary.resolve(),label,home,directory,args.warmup,args.seconds)
        results.append(result); populations.append(population)
        print(json.dumps(result), flush=True)
        args.output.write_text(json.dumps(dict(host=platform.node(), os=platform.platform(),
            method='100 * delta(daemon user+system CPU seconds) / delta(monotonic seconds); excludes git child CPU',
            foreground_daemons_only=True, github_disabled=True, results=results),indent=2)+'\n')
    assert populations[0] == populations[1], 'before/after watched sets differ'
    assert results[1]['initial_snapshot']['alive'] is True
    assert all(sample['snapshot']['alive'] is True for sample in results[1]['samples'])
    print('G492_MEASUREMENT_COMPLETE same watched set, private daemons stopped',flush=True)

if __name__ == '__main__':
    def terminate(signum, _frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, terminate)
    main()
