import json
import os
from pathlib import Path
import subprocess
import time

ROOT = Path('/Users/jirathip/.herdr/worktrees/corral/impl562-pathmatch')
commands = [
    ('workspace', ['cargo', 'test', '--workspace']),
    ('clippy', ['cargo', 'clippy', '--all-targets', '--all-features', '--', '-D', 'warnings']),
    ('deny', ['cargo', 'deny', '--locked', '--workspace', 'check']),
    ('audit', ['cargo', 'audit', '--deny', 'warnings']),
    ('release', ['cargo', 'build', '--release']),
    ('coverage-clean', ['cargo', 'llvm-cov', 'clean', '--locked', '--workspace']),
    ('coverage', ['cargo', 'llvm-cov', '--locked', '--package', 'corrald', '--package', 'corrald-client', '--all-targets', '--no-fail-fast', '--quiet', '--fail-under-lines', '85', '--fail-under-functions', '82']),
    ('coverage-client', ['cargo', 'llvm-cov', 'report', '--locked', '--package', 'corrald-client', '--fail-under-lines', '40', '--fail-under-functions', '35']),
]
results = []
for name, argv in commands:
    log = Path(f'/tmp/g562-{name}.log')
    started = time.monotonic()
    with log.open('w') as output:
        try:
            result = subprocess.run(argv, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT, timeout=900, env={**os.environ, 'CARGO_TERM_COLOR': 'never'})
            status = result.returncode
        except subprocess.TimeoutExpired:
            status = 'timeout-900s'
    row = {'name': name, 'command': argv, 'exit': status, 'seconds': round(time.monotonic()-started, 3), 'log': str(log)}
    results.append(row)
    Path('/tmp/g562-gates.json').write_text(json.dumps(results, indent=2)+'\n')
    print(json.dumps(row), flush=True)
