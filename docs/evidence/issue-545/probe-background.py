#!/usr/bin/env python3
"""#545: scratch-only discrimination of the finite-completion characterization.

From the implementation worktree, after committing the test:
  flock /tmp/n.lock python3 docs/evidence/issue-545/probe-background.py
No production fix or OS background assertion is installed by this driver.
"""
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[3]
SCRATCH = Path('/tmp/g545-scratch')
BASE = '2efa3577086a794ad064898d8af9d69b33a62325'
TEST = Path('ios/FleetNotifierTests/FleetNotifierTests.swift')
STORE = Path('ios/FleetNotifier/App/FleetStore.swift')
FOCUS = ('FleetNotifierTests/ForegroundReconnectTests/'
         'testFiniteSnapshotCompletionAcrossBackgroundHasNoRetainedBoardBenefit')


def run(argv, cwd, log, seconds=900):
    print('COMMAND=' + json.dumps(argv) + ' CWD=' + str(cwd), flush=True)
    started = time.monotonic()
    with Path(log).open('w') as output:
        child = subprocess.Popen(argv, cwd=cwd, stdout=output,
                                 stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = child.wait(timeout=seconds)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                code = child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                code = child.wait()
            print(f'TIMEOUT raw_child_exit={code} log={log}', flush=True)
            raise RuntimeError('probe deadline exceeded')
    print(f'EXIT={code} SECONDS={time.monotonic()-started:.3f} LOG={log}', flush=True)
    return code


def test(cwd, label, selectors):
    return run(['env', 'HERDR_XCODEBUILD_DIRECT=1', 'HERMES_SIM_TASK_ACTIVE=1',
                'xcodebuild', 'test', '-project', 'ios/FleetNotifier.xcodeproj',
                '-scheme', 'FleetNotifier', '-destination',
                'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793',
                '-derivedDataPath', '/tmp/g545-probe-dd' if cwd == SCRATCH else '/tmp/g545-dd',
                *['-only-testing:' + item for item in selectors]], cwd,
               '/tmp/g545-' + label + '.log')


def hash_file(path):
    subprocess.run(['shasum', '-a', '256', str(path)], check=True)
    return hashlib.sha256(path.read_bytes()).hexdigest()


def findings(log):
    entries = []
    pattern = re.compile(r'^(ios/\S+\.swift):\d+:\d+: error: \[([^]]+)\](.*)')
    for line in Path(log).read_text().splitlines():
        match = pattern.match(line)
        if match:
            entries.append(match.groups())
    if not entries:
        raise RuntimeError(f'no advisory findings parsed from {log}')
    return Counter(entries)


def main():
    if SCRATCH.exists():
        raise RuntimeError('refusing existing scratch path')
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    print('TESTED_SOURCE_HEAD=' + head, flush=True)
    subprocess.run(['git', 'diff', '--exit-code', 'HEAD', '--', 'ios/'], cwd=ROOT, check=True)
    subprocess.run(['git', 'worktree', 'add', '--detach', str(SCRATCH), head], cwd=ROOT, check=True)
    # Own exactly this fresh worktree. Leave it available for audit; never delete
    # an arbitrary pre-existing directory or another lane's scratch tree.
    source_path = SCRATCH / STORE
    source_bytes = source_path.read_bytes()
    test_path = SCRATCH / TEST
    test_bytes = test_path.read_bytes()
    baseline_hash = hash_file(test_path)
    try:
        test_path.write_bytes(subprocess.check_output(['git', 'show', f'{BASE}:{TEST}'], cwd=ROOT))
        subprocess.run(['git', 'diff', '--exit-code', BASE, '--', 'ios/FleetNotifier'],
                       cwd=SCRATCH, check=True)
        code = run(['swift', 'run', '--package-path', str(ROOT / 'ios/tools/anti-slop-swift'),
                    'anti-slop', 'ios/FleetNotifier', 'ios/FleetNotifierTests'], SCRATCH,
                   '/tmp/g545-aslop-base.log', seconds=600)
        print(f'ASLOP_BASE_EXIT={code}', flush=True)
    finally:
        test_path.write_bytes(test_bytes)
        assert hash_file(test_path) == baseline_hash
        print('BASE_TEST_RESTORE=byte-identical', flush=True)
    before = findings('/tmp/g545-aslop-base.log')
    after = findings('/tmp/g545-aslop-head.log')
    comparison = {'base': sum(before.values()), 'head': sum(after.values()),
                  'added': list((after - before).elements()),
                  'removed': list((before - after).elements()),
                  'base_per_rule': dict(Counter(rule for (_, rule, _), n in before.items() for _ in range(n))),
                  'head_per_rule': dict(Counter(rule for (_, rule, _), n in after.items() for _ in range(n)))}
    Path('/tmp/g545-aslop-identity.json').write_text(json.dumps(comparison, indent=2) + '\n')
    print(json.dumps(comparison), flush=True)
    assert not comparison['added']

    mutations = [
        ('m1-apply',
         '        // stays closed until THIS foreground session\'s key check succeeds.\n'
         '        guard !awaitingHostVerification else { return }',
         '        // MUTATION: permit a late refresh through the closed trust boundary.'),
        ('m2-reconnect',
         '    func reconnectIfNeeded(client: CorraldClient) {\n'
         '        guard !awaitingHostVerification else { return }',
         '    func reconnectIfNeeded(client: CorraldClient) {'),
    ]
    for label, old, new in mutations:
        pristine_hash = hash_file(source_path)
        try:
            text = source_bytes.decode()
            assert text.count(old) == 1, label
            source_path.write_text(text.replace(old, new))
            code = test(SCRATCH, label, [FOCUS])
            log = Path('/tmp/g545-' + label + '.log').read_text()
            assert code == 65, (label, code)
            assert "Test Case '-[FleetNotifierTests.ForegroundReconnectTests " + FOCUS.split('/')[-1] + "]' failed" in log
            assert 'XCTAssert' in log and 'Executed 1 test, with' in log
            assert '** TEST FAILED **' in log
        finally:
            source_path.write_bytes(source_bytes)
            assert hash_file(source_path) == pristine_hash
            print(label + '_RESTORE=byte-identical', flush=True)
    subprocess.run(['git', 'diff', '--exit-code'], cwd=SCRATCH, check=True)
    # Final GREEN is the untouched implementation tree, not the mutant build.
    assert test(ROOT, 'pristine-green', [
        'FleetNotifierTests/ScenePhaseLifecycleTests',
        'FleetNotifierTests/ForegroundReconnectTests']) == 0
    print('PROBE_BATTERY=PASS', flush=True)


if __name__ == '__main__':
    main()
