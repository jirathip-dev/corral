#!/usr/bin/env python3
"""Full Rust workflow gates under one external lock, per-command raw receipts."""
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
assert Path.cwd() == ROOT
scratch = Path('/tmp/g560-tmux')
scratch.mkdir(mode=0o700, exist_ok=True)
os.environ['TMUX_TMPDIR'] = str(scratch)
commands = [
    ('workspace', 900, ['cargo','test','--workspace']),
    ('deny', 300, ['cargo','deny','--locked','--workspace','check']),
    ('audit', 300, ['cargo','audit','--deny','warnings']),
    ('release', 1200, ['cargo','build','--release']),
    ('measurement', 420, [sys.executable, 'docs/evidence/issue-560/measure.py',
                          '/tmp/g560-base-target/release/corrald', '/tmp/g560-target/release/corrald']),
    ('coverage-clean', 120, ['cargo','llvm-cov','clean','--locked','--workspace']),
    ('coverage', 1200, ['cargo','llvm-cov','--locked','--package','corrald','--package','corrald-client',
                       '--all-targets','--no-fail-fast','--quiet','--fail-under-lines','85','--fail-under-functions','82']),
    ('client-coverage', 120, ['cargo','llvm-cov','report','--locked','--package','corrald-client',
                            '--fail-under-lines','40','--fail-under-functions','35']),
]
results = []
for label, deadline, command in commands:
    result = subprocess.run([sys.executable,'docs/evidence/issue-560/run.py',label,str(deadline),*command])
    results.append(result.returncode)
print('G560_FULL_GATE_EXITS',results,flush=True)
sys.exit(int(any(results)))
