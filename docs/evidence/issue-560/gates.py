#!/usr/bin/env python3
"""Run the named Cargo gates serially; each has its own raw receipt/log."""
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
    ('green-regression', ['cargo','test','-p','corrald','g560_unchanged_ticks_keep_publication_identity','--','--nocapture']),
    ('green-focused', ['cargo','test','-p','corrald','g560_','--','--nocapture']),
    ('oracle', ['cargo','test','-p','corrald','randomized_publications_match_old_snapshot_and_sse_bytes','--','--nocapture']),
    ('fmt', ['cargo','fmt','--all','--','--check']),
    ('clippy', ['cargo','clippy','--all-targets','--all-features','--','-D','warnings']),
]
for label, command in commands:
    result = subprocess.run([sys.executable,'docs/evidence/issue-560/run.py',label,'900',*command])
    if result.returncode:
        sys.exit(result.returncode)
print('G560_FOCUSED_GATES_COMPLETE',flush=True)
