#!/usr/bin/env python3
"""Run under flock /tmp/n.lock after committing sources; restore even on failure."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
VIEW = ROOT / 'ios/FleetNotifier/UI/Herd/HerdView.swift'
RUNNER = ROOT / 'docs/evidence/issue-568/run-simulator.py'


def run(label, tests):
    args = [sys.executable, str(RUNNER), '--label', label]
    for test in tests:
        args += ['--only', 'FleetNotifierTests/HerdTests' + test]
    return subprocess.run(args, cwd=ROOT, timeout=1500).returncode


def main():
    subprocess.run(['git', 'diff', '--exit-code', 'HEAD', '--', str(VIEW)], cwd=ROOT, check=True)
    pristine = VIEW.read_bytes()
    sha = lambda value: hashlib.sha256(value).hexdigest()
    source = pristine.decode()
    opacity = '''let opacity = HerdEdgeGeometry.opacity(group:group,viewport:viewport,
                                                          allowsOversized:allowsOversized)'''
    snap = 'return candidates.min { abs($0-bounded) < abs($1-bounded) } ?? bounded'
    assert source.count(opacity) == source.count(snap) == 1
    mutated = source.replace(opacity, 'let opacity = 1.0').replace(snap, 'return bounded')
    evidence = {'before_sha256': sha(pristine), 'mutation_sha256': sha(mutated.encode()),
                'mutation': 'Disable painted group opacity and return unsnapped proposed offset.'}
    try:
        VIEW.write_text(mutated)
        evidence['red_exit'] = run('final-red', ['/testEdgeOpacityBoundariesUseMeasuredCaptionAndViewport',
                                               '/testEdgeRowAndEndSnapsUseMeasuredRowsAndAreIdempotent'])
    finally:
        VIEW.write_bytes(pristine)
        os.utime(VIEW, None)
        evidence['restored_sha256'] = sha(VIEW.read_bytes())
        assert evidence['restored_sha256'] == evidence['before_sha256']
        Path('/tmp/g568-a2-red-witness.json').write_text(json.dumps(evidence, indent=2) + '\n')
    log = Path('/tmp/g568-a2-final-red/test.log').read_text()
    assert evidence['red_exit'] == 65, evidence
    assert 'Executed 2 tests, with 2 failures' in log, 'must be assertion RED, not a compile failure'
    for test in ['testEdgeOpacityBoundariesUseMeasuredCaptionAndViewport',
                 'testEdgeRowAndEndSnapsUseMeasuredRowsAndAreIdempotent']:
        assert f"{test}]' failed" in log, test
    evidence['green_exit'] = run('final-green-restored', [''])
    Path('/tmp/g568-a2-red-witness.json').write_text(json.dumps(evidence, indent=2) + '\n')
    # The restored GREEN leg is the deterministic Herd class; the group-anchor
    # regression also lives in it, so it runs inside the focused Herd suite.
    green_log = Path('/tmp/g568-a2-final-green-restored/test.log').read_text()
    assert evidence['green_exit'] == 0, evidence
    assert 'Executed 21 tests, with 0 failures' in green_log, green_log[-2000:]
    print(json.dumps(evidence, indent=2))
    assert evidence['green_exit'] == 0


if __name__ == '__main__':
    main()
