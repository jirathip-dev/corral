#!/usr/bin/env python3
"""Invoke once under flock /tmp/n.lock; preserve each unfiltered receipt."""
import subprocess
import sys
from pathlib import Path
runner = str(Path(__file__).with_name('run-command.py'))
prefix = sys.argv[1] + '-' if len(sys.argv) > 1 else ''
for label, command in [
    ('workspace', ['cargo', 'test', '--workspace']),
    ('clippy', ['cargo', 'clippy', '--all-targets', '--', '-D', 'warnings']),
    ('fmt', ['cargo', 'fmt', '--all', '--check']),
    ('deny', ['cargo', 'deny', 'check']),
    ('release', ['cargo', 'build', '--release']),
]:
    code = subprocess.call([sys.executable, runner, prefix + label, '1800', *command])
    if code:
        sys.exit(code)
