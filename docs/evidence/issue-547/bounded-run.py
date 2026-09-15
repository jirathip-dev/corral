#!/usr/bin/env python3
"""Run one exact gate with an outer deadline and a raw exit record."""
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys

label, seconds, *command = sys.argv[1:]
log = Path('/tmp') / (label + '.log')
record = {'command': shlex.join(command), 'cwd': str(Path.cwd()),
          'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()}
with log.open('w') as out:
    process = subprocess.Popen(command, stdout=out, stderr=subprocess.STDOUT, start_new_session=True)
    try:
        code = process.wait(timeout=int(seconds))
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=10)
        code = 124
    out.write(f'\nRAW_EXIT={code}\n')
record['exit'] = code
record['log'] = str(log)
log.with_suffix('.json').write_text(json.dumps(record, indent=2) + '\n')
print(label + '_EXIT=' + str(code), flush=True)
sys.exit(code)
