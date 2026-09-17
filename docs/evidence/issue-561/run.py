#!/usr/bin/env python3
"""Bound a lane command, retain its complete log and raw exit receipt."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

name, seconds, *command = sys.argv[1:]
root = Path(__file__).resolve().parents[3]
out = root / 'docs/evidence/issue-561'
start = time.monotonic()
with (out / f'{name}.log').open('wb') as log:
    process = subprocess.Popen(command, cwd=root, stdout=log, stderr=subprocess.STDOUT,
                               start_new_session=True)
    timed_out = False
    try:
        code = process.wait(timeout=int(seconds))
    except subprocess.TimeoutExpired:
        timed_out = True
        os.killpg(process.pid, signal.SIGTERM)
        try:
            code = process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            code = process.wait()
receipt = dict(command=command, cwd=str(root), raw_exit=code, timed_out=timed_out,
               elapsed_s=time.monotonic()-start, log=f'{name}.log')
(out / f'{name}.json').write_text(json.dumps(receipt, indent=2)+'\n')
print(json.dumps(receipt), flush=True)
sys.exit(124 if timed_out else code)
