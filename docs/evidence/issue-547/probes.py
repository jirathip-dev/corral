#!/usr/bin/env python3
"""#547 mutation proof: one disposable worktree; exact byte restores."""
import hashlib
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
scratch = Path('/tmp/g547-scratch')
runner = Path('/tmp/g547-run.py')
assert runner.is_file(), 'Copy bounded-run.py to /tmp/g547-run.py first'
assert not scratch.exists(), 'Refusing to reuse an existing scratch worktree'
source_head = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
subprocess.run(['git', '-C', str(root), 'diff', '--exit-code', '--', 'ios/'], check=True)
subprocess.run(['git', '-C', str(root), 'worktree', 'add', '--detach', str(scratch), source_head], check=True)
base_command = ['flock', '/tmp/n.lock', 'env', 'HERDR_XCODEBUILD_DIRECT=1', 'HERMES_SIM_TASK_ACTIVE=1',
    'xcodebuild', 'test', '-project', 'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier',
    '-destination', 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793',
    '-derivedDataPath', '/tmp/g547-probe-dd']
store = 'ios/FleetNotifier/App/FleetStore.swift'
app = 'ios/FleetNotifier/App/AppModel.swift'
coordinator = 'ios/FleetNotifier/Profiles/HostStreamCoordinator.swift'
probes = [
    ('M1-verify', store, '        awaitingHostVerification = true', '        awaitingHostVerification = false',
     ['testSceneOpensBeforeKeyAndAppliesBufferedDeltasInOrder', 'testNeverVerifiedColdStartBuffersItsFirstSnapshot']),
    ('M2-cap', store,
     '        if frames > policy.maxFrames || bytes > policy.maxBytes\n            || deliveries.count >= policy.maxFrames + 8 {',
     '        if false {',
     ['testCapDiscardsWholeWindowAndResumesOnlyAfterVerification', 'testByteCapRejectsOneOversizedFrameBeforeApplication']),
    ('M3-active-serial', app,
     '                beginKeyContinuityCheck(for: profile)\n            }\n        }\n        let client = CorraldClient(host: hostURL, session: session)',
     '                beginKeyContinuityCheck(for: profile)\n                return\n            }\n        }\n        let client = CorraldClient(host: hostURL, session: session)',
     ['testSceneOpensBeforeKeyAndAppliesBufferedDeltasInOrder']),
    ('M4-coordinator-serial', coordinator,
     '        session.store.requireHostVerification()\n        openStream(profile: profile, session: session, client: client)\n        session.continuityGeneration += 1',
     '        session.store.requireHostVerification()\n        session.continuityGeneration += 1',
     ['testSlowCoordinatorDoesNotDelayHealthyActiveHost']),
]
results = []

def run(label, tests=()):
    selected = ['-only-testing:FleetNotifierTests/ForegroundReconnectTests' + ('/' + name if name else '')
                for name in (tests or [''])]
    command = base_command + selected
    print(label, shlex.join(command), flush=True)
    code = subprocess.run([sys.executable, str(runner), 'g547-probe-' + label, '1200', *command], cwd=scratch).returncode
    log = Path('/tmp/g547-probe-' + label + '.log')
    text = log.read_text()
    failures = re.findall(r'error: -\[FleetNotifierTests.ForegroundReconnectTests (\w+)\] : ([^\n]+)', text)
    record = {'label': label, 'exit': code, 'log': str(log), 'command': shlex.join(command),
              'assertion_failures': failures}
    results.append(record)
    Path('/tmp/g547-probe-results.json').write_text(json.dumps({'head': source_head, 'results': results}, indent=2)+'\n')
    print(json.dumps(record), flush=True)
    return code, text, record

code, text, _ = run('control')
assert code == 0 and 'Executed 13 tests, with 0 failures' in text, 'Invalid GREEN control'
for label, relative, old, new, tests in probes:
    path = scratch / relative
    pristine = path.read_bytes()
    content = pristine.decode()
    assert content.count(old) == 1, (label, 'mutation anchor must match once')
    before = hashlib.sha256(pristine).hexdigest()
    print('BEFORE', label, flush=True)
    subprocess.run(['shasum', '-a', '256', str(path)], check=True)
    try:
        path.write_text(content.replace(old, new, 1))
        code, text, record = run(label, tests)
        record.update({'file': relative, 'old': old, 'new': new, 'before_sha256': before})
        assert code == 65 and record['assertion_failures'], 'Not an assertion RED'
        assert any(name in tests for name, _ in record['assertion_failures']), 'Wrong failure signature'
        assert '** TEST FAILED **' in text, 'Missing XCTest failure verdict'
    finally:
        path.write_bytes(pristine)
        after = hashlib.sha256(path.read_bytes()).hexdigest()
        assert before == after, 'Restore differs'
        print('RESTORED', label, flush=True)
        subprocess.run(['shasum', '-a', '256', str(path)], check=True)
        if results[-1]['label'] == label:
            results[-1]['restored_sha256'] = after
        Path('/tmp/g547-probe-results.json').write_text(json.dumps({'head': source_head, 'results': results}, indent=2)+'\n')
        subprocess.run(['git', 'diff', '--exit-code', '--', relative], cwd=scratch, check=True)
code, text, _ = run('restored-green')
assert code == 0 and 'Executed 13 tests, with 0 failures' in text, 'Restored GREEN failed'
subprocess.run(['git', 'diff', '--exit-code'], cwd=scratch, check=True)
print('PROBES=4 RED=4 RESTORED_GREEN=13/13 SOURCE_RESTORE=BYTE_IDENTICAL', flush=True)
