#!/usr/bin/env python3
"""#551 invariant mutations in a disposable, byte-restored worktree.

These inject candidate defects; they do NOT claim the defects existed at base.
Run via bash /tmp/g551-probes.sh (which acquires /tmp/n.lock once).
"""
import hashlib
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
scratch = Path('/tmp/g551-probe-tree')
runner = root / 'docs/evidence/issue-547/bounded-run.py'
assert not scratch.exists(), 'Refusing to reuse scratch'
subprocess.run(['git', '-C', str(root), 'diff', '--exit-code', 'HEAD', '--', 'ios/'], check=True)
head = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
subprocess.run(['git', '-C', str(root), 'worktree', 'add', '--detach', str(scratch), head], check=True)
tests = [
    'testWarmReturnDoesNotInheritEitherHostsRetryDelay',
    'testWarmReturnRetainsRenderedStatesUntilVerifiedReplacement',
    'testSceneGrantsReadCanStayInFlightWhileFirstFrameApplies',
    'testMismatchDiscardsBufferedFramesAndNeverRevivesLiveWork',
]
command = ['env', 'HERDR_XCODEBUILD_DIRECT=1', 'HERMES_SIM_TASK_ACTIVE=1',
    'xcodebuild', 'test', '-project', 'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier',
    '-destination', 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793',
    '-derivedDataPath', '/tmp/g551-probe-dd']
results = []


def save():
    Path('/tmp/g551-probe-results.json').write_text(json.dumps({'source_head': head, 'results': results}, indent=2) + '\n')


def run(label, selection):
    args = command + ['-only-testing:FleetNotifierTests/ForegroundReconnectTests/' + name for name in selection]
    print(label, shlex.join(args), flush=True)
    status = subprocess.run([sys.executable, str(runner), 'g551-probe-' + label, '1200', *args], cwd=scratch).returncode
    log = Path('/tmp/g551-probe-' + label + '.log')
    text = log.read_text()
    failures = re.findall(r'error: -\[FleetNotifierTests.ForegroundReconnectTests (\w+)\] : ([^\n]+)', text)
    record = {'label': label, 'exit': status, 'command': shlex.join(args), 'log': str(log), 'assertion_failures': failures}
    results.append(record)
    save()
    return status, text, record


status, text, _ = run('control', tests)
assert status == 0 and 'Executed 4 tests, with 0 failures' in text, 'Invalid GREEN control'
mutations = [
    ('M1-inherit-ladder', 'ios/FleetNotifier/App/AppModel.swift',
     '        keyContinuityTask?.cancel()\n        keyContinuityTask = nil\n        keyContinuityTaskId = nil\n        // #454 pre-review fix 3: the background boundary resets',
     '        // Mutation: preserve the old ladder and its wait across background.\n        // #454 pre-review fix 3: the background boundary resets',
     tests[0]),
    ('M2-blank-on-reconnect', 'ios/FleetNotifier/App/FleetStore.swift',
     '    func beginReconnectTiming() {\n        reconnectTiming = ReconnectTiming()',
     '    func beginReconnectTiming() {\n        agents.removeAll()\n        reconnectTiming = ReconnectTiming()',
     tests[1]),
]
for label, relative, old, new, test in mutations:
    path = scratch / relative
    pristine = path.read_bytes()
    before = hashlib.sha256(pristine).hexdigest()
    source = pristine.decode()
    assert source.count(old) == 1, label
    subprocess.run(['shasum', '-a', '256', str(path)], check=True)
    try:
        path.write_text(source.replace(old, new, 1))
        status, text, record = run(label, [test])
        record.update(file=relative, old=old, new=new, before_sha256=before)
        save()
        assert status == 65 and any(name == test for name, _ in record['assertion_failures']), 'Not the required assertion RED'
        assert '** TEST FAILED **' in text
    finally:
        path.write_bytes(pristine)
        after = hashlib.sha256(path.read_bytes()).hexdigest()
        assert before == after, 'Restore differs'
        subprocess.run(['shasum', '-a', '256', str(path)], check=True)
        subprocess.run(['git', 'diff', '--exit-code'], cwd=scratch, check=True)
        if results[-1]['label'] == label:
            results[-1]['restored_sha256'] = after
            results[-1]['restore_diff_exit'] = 0
        save()
status, text, _ = run('restored-green', tests)
assert status == 0 and 'Executed 4 tests, with 0 failures' in text, 'Final pristine GREEN failed'
subprocess.run(['git', 'diff', '--exit-code'], cwd=scratch, check=True)
subprocess.run(['git', '-C', str(root), 'worktree', 'remove', str(scratch)], check=True)
print('PROBES=2 ASSERTION_RED=2 RESTORED_GREEN=4/4 RESTORE=BYTE_IDENTICAL', flush=True)
