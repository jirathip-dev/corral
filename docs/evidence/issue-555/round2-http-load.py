#!/usr/bin/env python3
"""Real-binary #555 HTTP regression: idle / one / eight competing history readers.

Based on the reviewer's rev555a-compare.py method (1024 retained history events,
new loopback connection per request, 8 GET /history?limit=5000 clients). No live
inputs. Record three runs, retaining every sample; check median-of-run p50/p99.
Fixed BEFORE measuring the repair: candidate concurrent p50 <= 2*base + 1 ms,
p99 <= 2*base + 2 ms. Additive allowances cover sub-ms loopback scheduling noise;
this is a relative regression gate, not the separate 20/50 ms router budget.
Run under daemon-run.py (shared heavy lock, host receipt and deadline).
"""
import argparse
import hashlib
import http.client
import json
import math
from pathlib import Path
import re
import socket
import statistics
import subprocess
import tempfile
import threading
import time

SAMPLES = 128
ROUNDS = 3
PHASES = {'idle': 0, 'single': 1, 'concurrent': 8}


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def percentiles(values):
    values = sorted(values)
    return {name: values[math.ceil(len(values) * percentile) - 1]
            for name, percentile in [('p50', .50), ('p99', .99), ('max', 1)]}


def fixture(root):
    history = root / 'config' / 'history'
    history.mkdir(parents=True)
    for name in ('repos', 'worktrees', 'bin'):
        (root / name).mkdir()
    for segment in range(4):
        ts = 1700000000000 + segment * 1000
        rows = [dict(ts=ts + i, pane_id=f'pane-{i:04d}-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                     agent_id=f'herdr:agent-{i:04d}-bbbbbbbbbbbbbbbbbbbbbbbbbb',
                     old_status='idle', new_status='working', source='herdr',
                     repo='corral-fixture-repository-name') for i in range(256)]
        (history / f'seg-{segment}-{ts}.jsonl').write_text(
            ''.join(json.dumps(row) + '\n' for row in rows))


def record(args):
    assert args.binary.is_file()
    output = dict(source_ref=args.source_ref, binary=str(args.binary.resolve()),
                  binary_sha256=sha(args.binary), harness_sha256=sha(__file__),
                  samples_per_phase=SAMPLES, rounds=ROUNDS, runs=[])
    for run in range(ROUNDS):
        with tempfile.TemporaryDirectory(prefix='g555-r2-http-') as directory:
            root = Path(directory)
            fixture(root)
            with socket.socket() as reservation:
                reservation.bind(('127.0.0.1', 0))
                port = reservation.getsockname()[1]
            env = dict(HOME=directory, PATH=str(root / 'bin'), RUST_LOG='info',
                       CORRAL_CONFIG_DIR=str(root / 'config'),
                       CORRAL_REPO_ROOT=str(root / 'repos'),
                       CORRAL_WORKTREES_ROOT=str(root / 'worktrees'))
            log_path = args.output.with_name(f'{args.output.stem}-run{run}-daemon.log')
            with log_path.open('w') as log:
                child = subprocess.Popen([str(args.binary.resolve()), '--port', str(port),
                                          '--socket', str(root / 'absent.sock')],
                                         env=env, stdout=log, stderr=subprocess.STDOUT)
                started = time.monotonic()
                stop = threading.Event()
                threads = []
                try:
                    def get(route):
                        conn = http.client.HTTPConnection('127.0.0.1', port, timeout=2)
                        try:
                            before = time.perf_counter()
                            conn.request('GET', route, headers={'Connection': 'close'})
                            response = conn.getresponse()
                            body = response.read()
                            elapsed = (time.perf_counter() - before) * 1000
                            assert response.status == 200, (route, response.status)
                            return body, elapsed
                        finally:
                            conn.close()

                    deadline = time.monotonic() + 10
                    while True:
                        assert child.poll() is None, 'fixture daemon exited before readiness'
                        try:
                            assert get('/healthz')[0] == b'ok\n'
                            break
                        except (OSError, http.client.HTTPException):
                            if time.monotonic() >= deadline:
                                raise
                            time.sleep(.02)
                    body, history_ms = get('/history?limit=5000')
                    assert len(json.loads(body)['events']) == 1024, 'history fixture must fill ring'
                    result = dict(run=run, pid=child.pid, history_bytes=len(body),
                                  history_single_ms=history_ms, phases={})
                    for phase, readers in PHASES.items():
                        stop = threading.Event()
                        counts = [0] * readers
                        failures = []
                        ready = [threading.Event() for _ in range(readers)]

                        def hammer(index):
                            try:
                                while not stop.is_set():
                                    page, _ = get('/history?limit=5000')
                                    assert len(page) == len(body), 'history page changed'
                                    counts[index] += 1
                                    ready[index].set()
                            except Exception as error:
                                failures.append(repr(error))
                                ready[index].set()

                        threads = [threading.Thread(target=hammer, args=(i,))
                                   for i in range(readers)]
                        for thread in threads:
                            thread.start()
                        for event in ready:
                            assert event.wait(3), 'history reader did not start within 3s'
                        if readers:
                            time.sleep(1)  # Same one-second warmup as the review probe.
                        before_counts = counts.copy()
                        samples = []
                        try:
                            for _ in range(SAMPLES):
                                snapshot, elapsed = get('/snapshot')
                                value = json.loads(snapshot)
                                samples.append(dict(client_ms=elapsed, bytes=len(snapshot),
                                                    buffer_age_ms=max(0, time.time()*1000 - value['generated_at'])))
                        finally:
                            stop.set()
                            for thread in threads:
                                thread.join(timeout=3)
                            assert all(not t.is_alive() for t in threads), 'reader cleanup deadline'
                        assert not failures, failures
                        assert all(a > b for a, b in zip(counts, before_counts)), 'load did not progress'
                        result['phases'][phase] = dict(
                            readers=readers, history_requests_during_samples=[a-b for a,b in zip(counts,before_counts)],
                            latency_ms=percentiles([s['client_ms'] for s in samples]),
                            buffer_age_ms=percentiles([s['buffer_age_ms'] for s in samples]), samples=samples)
                        output['runs'] = output['runs'][:run] + [result]
                        args.output.write_text(json.dumps(output, indent=2) + '\n')
                finally:
                    stop.set()
                    for thread in threads:
                        thread.join(timeout=3)
                    child.terminate()
                    child.wait(timeout=10)
                duration = time.monotonic() - started
            text = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', log_path.read_text())
            lines = [line for line in text.splitlines() if 'snapshot served' in line]
            result.update(daemon_exit=child.returncode, duration_s=duration,
                          snapshot_info_lines=len(lines), log=str(log_path))
            if args.bounded_logs:
                assert 0 < len(lines) <= math.ceil(duration / 5) + 1, ('unbounded snapshot info', len(lines), duration)
                assert all('max_serve_ms=' in line and 'max_buffer_age_ms=' in line
                           and 'requests=' in line for line in lines), lines
            args.output.write_text(json.dumps(output, indent=2) + '\n')
    summary = {phase: {metric: statistics.median(r['phases'][phase]['latency_ms'][metric]
                                               for r in output['runs'])
                       for metric in ('p50', 'p99')} for phase in PHASES}
    output['median_latency_ms'] = summary
    args.output.write_text(json.dumps(output, indent=2) + '\n')
    print(json.dumps(dict(source_ref=args.source_ref, median_latency_ms=summary,
                         info_lines=[r['snapshot_info_lines'] for r in output['runs']]), indent=2))


def check(args):
    base, candidate = [json.loads(path.read_text()) for path in (args.baseline, args.candidate)]
    assert base['harness_sha256'] == candidate['harness_sha256'] == sha(__file__)
    for item in (base, candidate):
        assert len(item['runs']) == ROUNDS
        for run in item['runs']:
            for phase in PHASES:
                assert len(run['phases'][phase]['samples']) == SAMPLES
    b, c = [item['median_latency_ms']['concurrent'] for item in (base, candidate)]
    bounds = dict(p50=2*b['p50'] + 1, p99=2*b['p99'] + 2)
    verdict = all(c[metric] <= bounds[metric] for metric in bounds)
    print(json.dumps(dict(baseline=b, candidate=c, bound_ms=bounds, passed=verdict), indent=2))
    assert verdict, 'concurrent /snapshot regression exceeds 2x baseline + 1/2 ms p50/p99'


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest='mode', required=True)
    rec = subs.add_parser('record')
    rec.add_argument('--binary', type=Path, required=True)
    rec.add_argument('--source-ref', required=True)
    rec.add_argument('--output', type=Path, required=True)
    rec.add_argument('--bounded-logs', action='store_true')
    compare = subs.add_parser('check')
    compare.add_argument('baseline', type=Path)
    compare.add_argument('candidate', type=Path)
    args = parser.parse_args()
    record(args) if args.mode == 'record' else check(args)
