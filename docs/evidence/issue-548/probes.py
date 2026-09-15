#!/usr/bin/env python3
"""#548 assertion-level probes in a fresh detached, disposable worktree.

Invoke from the delivered checkout under flock /tmp/n.lock.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time

ROOT = Path.cwd()
SCRATCH = Path('/tmp/g548-scratch')
OUT = Path('/tmp/g548-evidence/probes')
SOURCE = Path('ios/FleetNotifier/UI/Herd/HerdView.swift')
METHODS = {
    'reservation': 'testReservedZoneKeepsFirstHorseRowFixed',
    'label': 'testEmptyRailHasNoLabelOrFenceAndKeepsTheSharedDestination',
    'fence': 'testEmptyRailHasNoLabelOrFenceAndKeepsTheSharedDestination',
    'zero-suffix': 'testRepositoryCaptionOnlyAddsPositiveRailCounts',
    'positive-suffix': 'testRepositoryCaptionOnlyAddsPositiveRailCounts',
}


def main():
    assert not SCRATCH.exists(), 'refuse to overwrite an existing scratch worktree'
    OUT.mkdir(parents=True, exist_ok=True)
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True, timeout=30).strip()
    subprocess.run(['git', 'worktree', 'add', '--detach', str(SCRATCH), head], cwd=ROOT, check=True, timeout=60)
    path = SCRATCH / SOURCE
    pristine = path.read_bytes()
    text = pristine.decode()
    mutations = {
        'reservation': ('.hidden()\n        .frame(maxWidth:.infinity,alignment:.leading)',
                        '.hidden()\n        .frame(height:rail.isEmpty ? 0 : nil)\n        .frame(maxWidth:.infinity,alignment:.leading)'),
        'label': ('.overlay(alignment:.topLeading) {\n            if !rail.isEmpty {',
                  '.overlay(alignment:.topLeading) {\n            if rail.isEmpty { Text("! FRONT RAIL") }\n            if !rail.isEmpty {'),
        'fence': ('.overlay(alignment:.topLeading) {\n            if !rail.isEmpty {',
                  '.overlay(alignment:.topLeading) {\n            if rail.isEmpty { RanchFrontRail(night:lighting.night) }\n            if !rail.isEmpty {'),
        'zero-suffix': ('paddock.blockedCount > 0 ?', 'paddock.blockedCount >= 0 ?'),
        'positive-suffix': ('paddock.blockedCount > 0 ?', 'paddock.blockedCount > 1 ?'),
    }
    base = ['xcodebuild', 'test', '-project', 'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier',
            '-destination', 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793',
            '-derivedDataPath', '/tmp/g548-probe-dd']
    env = {**os.environ, 'HERDR_XCODEBUILD_DIRECT': '1', 'HERMES_SIM_TASK_ACTIVE': '1'}
    results = []

    def run(name, method=None):
        command = base + ['-only-testing:FleetNotifierTests/HerdRailZoneTests' + ('/' + method if method else '')]
        log = OUT / (name + '.log')
        start = time.monotonic()
        with log.open('w') as stream:
            result = subprocess.run(command, cwd=SCRATCH, env=env, stdout=stream, stderr=subprocess.STDOUT, timeout=900)
        entry = dict(name=name, command=command, cwd=str(SCRATCH), head=head,
                     exit=result.returncode, seconds=round(time.monotonic()-start, 2), log=str(log))
        results.append(entry)
        (OUT / 'commands.json').write_text(json.dumps(results, indent=2) + '\n')
        print(json.dumps(entry), flush=True)
        expected = 65 if method else 0
        assert result.returncode == expected, f'{name}: expected {expected}, got {result.returncode}'
        if method:
            assert re.search(r'Test Case .*' + method + r'.*failed', log.read_text()), 'require an assertion failure, not a build failure'

    for name, (old, new) in mutations.items():
        assert text.count(old) == 1, name
        subprocess.run(['shasum', '-a', '256', str(path)], check=True, timeout=30)
        try:
            path.write_text(text.replace(old, new))
            run(name, METHODS[name])
        finally:
            path.write_bytes(pristine)
            os.utime(path, None)
            subprocess.run(['shasum', '-a', '256', str(path)], check=True, timeout=30)
            assert hashlib.sha256(path.read_bytes()).digest() == hashlib.sha256(pristine).digest()
            subprocess.run(['git', 'diff', '--exit-code', '--', str(SOURCE)], cwd=SCRATCH, check=True, timeout=30)
    run('restored-green')
    assert path.read_bytes() == pristine
    print('PASS: five assertion RED probes; byte-identical restores; pristine GREEN', flush=True)


if __name__ == '__main__':
    main()
