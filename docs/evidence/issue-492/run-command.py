#!/usr/bin/env python3
"""Bound one owned command; retain unfiltered output and its raw status."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

label, seconds, *command = sys.argv[1:]
log = Path('/tmp') / f'g492-{label}.log'
receipt = log.with_suffix('.json')
subprocess.run(['df', '-h', '.', '/tmp'], check=True)
started = time.monotonic()
with log.open('wb') as stream:
    child = subprocess.Popen(command, stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
    timed_out = False
    try:
        code = child.wait(timeout=float(seconds))
    except subprocess.TimeoutExpired:
        timed_out = True
        os.killpg(child.pid, signal.SIGTERM)
        try:
            code = child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            code = child.wait(timeout=10)
result = dict(command=command, cwd=os.getcwd(), raw_exit=code, timed_out=timed_out,
              elapsed_seconds=round(time.monotonic()-started, 3), log=str(log),
              target=os.environ.get('CARGO_TARGET_DIR'))
receipt.write_text(json.dumps(result, indent=2)+'\n')
print(json.dumps(result), flush=True)
sys.exit(124 if timed_out else code if code >= 0 else 128-code)
