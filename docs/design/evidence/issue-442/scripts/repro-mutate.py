#!/usr/bin/env python3
"""Saved reproducibility discriminator: disposable byte mutation RED, pristine GREEN."""
from pathlib import Path
import subprocess
import sys

EVID = Path(__file__).resolve().parent.parent

def main():
    for label, args, expected in [('RED', ['--inject-mismatch'], 1), ('GREEN', [], 0)]:
        cmd = [sys.executable, str(EVID / 'scripts/repro.py'), *args]
        run = subprocess.run(cmd, capture_output=True, text=True)
        print(f'{label} command: python3 scripts/repro.py {" ".join(args)}'.rstrip())
        print(run.stdout, end='')
        print(run.stderr, end='')
        print(f'{label}_RAW_EXIT={run.returncode}')
        signature = 'MISMATCH_COUNT=1' if label == 'RED' else 'MATCH_COUNT=21 MISMATCH_COUNT=0'
        if run.returncode != expected or signature not in run.stdout:
            return 1
    print('REPRO MUTATION OK: isolated RED exit=1; pristine GREEN exit=0; canonical untouched')
    return 0

if __name__ == '__main__':
    sys.exit(main())
