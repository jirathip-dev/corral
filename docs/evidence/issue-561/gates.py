#!/usr/bin/env python3
"""One bounded, serial invocation of the brief and Rust CI-equivalent gates."""
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
runner = root / 'docs/evidence/issue-561/run.py'
commands = [
    ('fmt', 120, ['cargo', 'fmt', '--all', '--', '--check']),
    ('deny', 180, ['cargo', 'deny', '--locked', '--workspace', 'check']),
    ('audit', 180, ['cargo', 'audit', '--deny', 'warnings']),
    ('clippy', 600, ['cargo', 'clippy', '--all-targets', '--all-features', '--', '-D', 'warnings']),
    ('release', 900, ['cargo', 'build', '--release']),
    ('full', 900, ['cargo', 'test', '--workspace']),
    ('green', 180, ['cargo', 'test', '-p', 'corrald', 'g561_generation_preserves_facts', '--', '--nocapture']),
    ('g492', 180, ['cargo', 'test', '-p', 'corrald', 'g492_', '--', '--nocapture']),
    ('coverage-clean', 120, ['cargo', 'llvm-cov', 'clean', '--locked', '--workspace']),
    ('coverage', 1200, ['cargo', 'llvm-cov', '--locked', '--package', 'corrald', '--package', 'corrald-client',
                        '--all-targets', '--no-fail-fast', '--quiet', '--fail-under-lines', '85', '--fail-under-functions', '82']),
    ('client-coverage', 120, ['cargo', 'llvm-cov', 'report', '--locked', '--package', 'corrald-client',
                             '--fail-under-lines', '40', '--fail-under-functions', '35']),
]
prefix = sys.argv[1] if len(sys.argv) > 1 else ''
results = []
for name, deadline, command in commands:
    name = prefix + name
    print(f'BEGIN {name}', flush=True)
    code = subprocess.run([sys.executable, str(runner), name, str(deadline), *command], cwd=root).returncode
    results.append((name, code))
    print(f'END {name} raw_runner_exit={code}', flush=True)
    if code:
        print('STOP: first failing gate, no automatic retry', flush=True)
        break
# Also retain the brief's requested aggregate raw Cargo log location.
with Path('/tmp/g561-tests.log').open('ab') as combined:
    for name, code in results:
        combined.write(f'\nCOMMAND {name}\n'.encode())
        combined.write((runner.parent / f'{name}.log').read_bytes())
        combined.write(f'\n{name}_exit={code}\n'.encode())
print(results, flush=True)
sys.exit(next((code for _, code in results if code), 0))
