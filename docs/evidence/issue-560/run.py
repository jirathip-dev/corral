#!/usr/bin/env python3
"""Bound one command, retain raw output/exit, and never hide a timeout."""
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
label, seconds, *command = sys.argv[1:]
assert Path.cwd() == ROOT
logs = ROOT / 'docs/evidence/issue-560/runs'
logs.mkdir(exist_ok=True)
log = logs / f'{label}.log'
receipt = logs / f'{label}.json'
assert not log.exists() and not receipt.exists(), 'do not overwrite prior evidence'
env = dict(os.environ, CARGO_TARGET_DIR='/tmp/g560-target', CARGO_BUILD_JOBS='2',
           CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0',
           CARGO_INCREMENTAL='0', CARGO_TERM_COLOR='never')
source_hashes = {p: hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in (
    'src/core/store.rs','src/core/store/published.rs','src/core/store/publication_tests.rs','src/main.rs')}
start = time.monotonic()
with log.open('wb') as out:
    child = subprocess.Popen(command, stdout=out, stderr=subprocess.STDOUT,
                             env=env, start_new_session=True)
    timeout = False
    try:
        code = child.wait(timeout=float(seconds))
    except subprocess.TimeoutExpired:
        timeout = True
        os.killpg(child.pid, signal.SIGTERM)
        try:
            code = child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            code = child.wait(timeout=10)
result = dict(command=command, cwd=str(ROOT), raw_exit=code, timed_out=timeout,
              source_sha256=source_hashes,
              elapsed_seconds=time.monotonic()-start, log=str(log),
              head=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
              environment={k: v for k, v in env.items() if k.startswith('CARGO_')})
receipt.write_text(json.dumps(result, indent=2)+'\n')
print(json.dumps(result), flush=True)
sys.exit(124 if timeout else code)
