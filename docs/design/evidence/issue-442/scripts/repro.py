#!/usr/bin/env python3
"""Render twice into isolated outputs, compare every byte, then promote only compared PNGs.
--inject-mismatch changes one disposable output byte: expected exit 1; never promotes.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

EVID = Path(__file__).resolve().parent.parent

def digest(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--inject-mismatch', action='store_true')
    parser.add_argument('--promote', action='store_true')
    args = parser.parse_args()
    assert not (args.inject_mismatch and args.promote)
    specs = json.loads((EVID / 'stage/specs.json').read_text())
    names = sorted(f"{s['name']}-390x844.png" for s in specs)
    assert len(names) == len(set(names)) == 21
    inputs = {p.name: digest(p) for p in (EVID / 'stage').iterdir() if p.is_file()}
    with tempfile.TemporaryDirectory(prefix='repro442-') as tmp:
        outputs = [Path(tmp) / 'a', Path(tmp) / 'b']
        for label, output in zip(('A', 'B'), outputs):
            cmd = [sys.executable, str(EVID / 'scripts/render.py'), '--output-dir', str(output)]
            r = subprocess.run(cmd, capture_output=True, text=True,
                               env=dict(os.environ, PYTHONDONTWRITEBYTECODE='1'))
            print(f'RUN {label}: python3 scripts/render.py --output-dir <isolated-{label}>')
            print(r.stdout, end='')
            if r.stderr:
                print(r.stderr)
            print(f'RENDER_{label}_RAW_EXIT={r.returncode}')
            if r.returncode:
                return 1
            assert sorted(p.name for p in output.iterdir()) == names
        assert inputs == {p.name: digest(p) for p in (EVID / 'stage').iterdir() if p.is_file()}, 'inputs changed during capture'
        if args.inject_mismatch:
            p = outputs[1] / names[0]
            data = bytearray(p.read_bytes())
            data[-1] ^= 1
            p.write_bytes(data)
            print(f'MUTATION: flip final byte of isolated B/{names[0]} only')
        mismatches = 0
        for name in names:
            a, b = (o / name for o in outputs)
            same = a.read_bytes() == b.read_bytes() and digest(a) == digest(b)
            mismatches += not same
            print(f'{"MATCH" if same else "MISMATCH"} {name} A={digest(a)} B={digest(b)}')
        print(f'PNG_COUNT=21 MATCH_COUNT={21-mismatches} MISMATCH_COUNT={mismatches}')
        if mismatches:
            print('REPRO FAIL')
            return 1
        if args.promote:
            for name in names:
                shutil.copyfile(outputs[0] / name, EVID / name)
                assert (EVID / name).read_bytes() == (outputs[1] / name).read_bytes()
            print('PROMOTED: compared bytes only, canonical 21/21 equals A and B')
        print('REPRO OK')
    return 0

if __name__ == '__main__':
    sys.exit(main())
