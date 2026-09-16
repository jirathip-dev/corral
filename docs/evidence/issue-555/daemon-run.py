#!/usr/bin/env python3
"""Bounded, serialized #555 gates. Only owns its target and spawned children."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
TARGET = Path('/tmp/g555-daemon-target')
parser = argparse.ArgumentParser()
parser.add_argument('--locked', action='store_true')
parser.add_argument('--hog', action='store_true')
parser.add_argument('--budget', action='store_true')
parser.add_argument('--timeout', type=int, default=600)
parser.add_argument('label')
parser.add_argument('command', nargs=argparse.REMAINDER)
args = parser.parse_args()
assert Path.cwd() == ROOT, (Path.cwd(), ROOT)
assert args.command
if not args.locked:
    command = ['flock', '/tmp/n.lock', sys.executable, __file__, '--locked', *sys.argv[1:]]
    child = subprocess.Popen(command, start_new_session=True)
    try:
        sys.exit(child.wait(timeout=args.timeout + 600))
    except subprocess.TimeoutExpired:
        os.killpg(child.pid, signal.SIGTERM)
        child.wait(timeout=10)
        sys.exit(124)

deadline = time.monotonic() + 180
while True:
    rows = subprocess.check_output(['ps', '-axo', 'pid,comm'], text=True).splitlines()
    heavy = [r for r in rows if len(r.split()) > 1 and Path(r.split()[1]).name in
             ('cargo', 'rustc', 'xcodebuild', 'swift-frontend')]
    if not heavy:
        break
    print('WAIT sibling heavy processes:', heavy, flush=True)
    if time.monotonic() >= deadline:
        print('BLOCKED sibling heavy deadline (180s)', flush=True)
        sys.exit(75)
    time.sleep(5)
if not TARGET.exists():
    TARGET.mkdir()
    (TARGET / '.g555-owned').write_text(str(ROOT) + '\n')
assert (TARGET / '.g555-owned').read_text().strip() == str(ROOT)
env = dict(os.environ, CARGO_TARGET_DIR=str(TARGET))
log_path = Path('/tmp') / f'g555-{args.label}.log'
host_path = Path('/tmp') / f'g555-{args.label}-host.log'
hogs = []
monitor = None
child = None
started = time.monotonic()
raw_exit = None
try:
    with host_path.open('w') as host:
        for command in (['date', '-u'], ['df', '-h', '/'], ['uptime']):
            subprocess.run(command, stdout=host, stderr=subprocess.STDOUT, check=True, timeout=10)
        if args.hog:
            assert os.getpriority(os.PRIO_PROCESS, 0) == 0, 'benchmark parent must be nice 0'
            for _ in range(os.cpu_count() or 1):
                hogs.append(subprocess.Popen(['/usr/bin/nice', '-n', '0', '/usr/bin/yes'],
                                             stdout=subprocess.DEVNULL, start_new_session=True))
            subprocess.run(['ps', '-p', ','.join(str(p.pid) for p in hogs), '-o', 'pid,ni,comm'],
                           stdout=host, check=True, timeout=10)
            monitor = subprocess.Popen(['top', '-l', '2', '-s', '1', '-n', '0'],
                                       stdout=host, stderr=subprocess.STDOUT)
        with log_path.open('w') as log:
            log.write(f'COMMAND={args.command!r}\n')
            log.flush()
            child = subprocess.Popen(args.command, env=env, stdout=log, stderr=subprocess.STDOUT,
                                     start_new_session=True)
            try:
                raw_exit = child.wait(timeout=args.timeout)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGTERM)
                raw_exit = child.wait(timeout=10)
                log.write('DEADLINE_EXCEEDED=true\n')
            log.write(f'RAW_EXIT={raw_exit}\n')
finally:
    for p in hogs:
        if p.poll() is None:
            p.terminate()
        p.wait(timeout=10)
    if monitor is not None:
        if monitor.poll() is None:
            monitor.terminate()
        monitor.wait(timeout=10)

receipt = dict(label=args.label, command=args.command, raw_exit=raw_exit,
               duration_s=round(time.monotonic()-started, 3),
               head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
               cpu_hogs=len(hogs), log=str(log_path), host_log=str(host_path))
if args.hog and raw_exit == 0:
    samples = [json.loads(line.split(' ', 1)[1]) for line in log_path.read_text().splitlines()
               if line.startswith('G555_SAMPLE ')]
    assert len(samples) == 512, len(samples)
    elapsed = sorted(s['serve_ms'] for s in samples)
    ages = sorted(s['buffer_age_ms'] for s in samples)
    receipt.update(samples=len(samples), p50_ms=elapsed[255], p99_ms=elapsed[506],
                   buffer_age_p50_ms=ages[255], buffer_age_p99_ms=ages[506],
                   buffer_age_max_ms=ages[-1], bytes_min=min(s['bytes'] for s in samples),
                   bytes_max=max(s['bytes'] for s in samples))
    receipt['revisions_seen'] = sorted({s['rev'] for s in samples})
    receipt['budget_pass'] = (elapsed[255] < 20 and elapsed[506] < 50
                              and len(receipt['revisions_seen']) > 1)
Path(f'/tmp/g555-{args.label}-receipt.json').write_text(json.dumps(receipt, indent=2)+'\n')
print(json.dumps(receipt, indent=2), flush=True)
if args.budget and not receipt.get('budget_pass', False):
    sys.exit(1)
sys.exit(raw_exit if raw_exit is not None and raw_exit >= 0 else 124)
