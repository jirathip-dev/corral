#!/usr/bin/env python3
"""#554 invariant mutations in a disposable, byte-restored worktree.

Each mutation is a CANDIDATE defect (they do not claim the defect existed at
base): the point is that the #554 acceptance tests go RED for exactly the
mechanism they are supposed to pin, and that the source is restored
byte-identically afterwards (sha256 before/after + `git diff --exit-code`).

P1  keep the base/shared session as the live session (the pre-#554 production
    behaviour)                     -> (a) identity RED
P2  never invalidate the retired session                -> (a) + (e) RED
P3  raise the production preflight bound to 60s         -> (c) RED
P4  shrink the live session's stream timeout            -> (d) RED
P5  drop the live-session identity guard on a completion
    path (candidate)                                    -> (b) RED

The battery also carries the round-3 boundary-stress scenario in EVERY leg
(`testBoundaryJitterWithRetryNeverCreatesATaskOnARetiredTransport` = the
reviewer's production route, `testBoundaryJitterWithoutRetryIsClean` = its
scoping control), so a mutation that breaks the live-session seam while a
boundary dispatch is in flight shows up here too.

Round 2's P6 (retire WITHOUT repairing the holder) was REMOVED in round 3, and
that removal is itself a finding: the round-3 retirement defers
`invalidateAndCancel()` until the pre-boundary owners terminate, so the
holder defect no longer reproduces as an abort — a post-boundary dispatch runs
on a doomed-but-still-valid session and is cancelled by the drained
invalidation instead. The round-3 defect class (a task created one await hop
after a guard) has NO deterministic RED at this seam: it manifests only as the
nondeterministic `NSGenericException` abort, which the round-2 reviewer
reproduced by execution at `1bcdd24` (2/2, exit 65 — see
`f1-retirement-repair.md`) and which five of my own stress recipes (up to 600
jittered cycles) did NOT reproduce. Keeping a probe whose expected RED is
"usually" would be a false claim, so it is dropped rather than kept.

Run via bash docs/evidence/issue-554/run-probes.sh (one /tmp/n.lock invocation).
"""
import hashlib
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
scratch = Path('/tmp/g554-probe-tree')
runner = root / 'docs/evidence/issue-547/bounded-run.py'
assert not scratch.exists(), 'Refusing to reuse scratch'
subprocess.run(['git', '-C', str(root), 'diff', '--exit-code', '--', 'ios/'], check=True)
head = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
subprocess.run(['git', '-C', str(root), 'worktree', 'add', '--detach', str(scratch), head], check=True)
battery = [
    'LiveSessionTransportTests/testCompletionFromTheRetiredSessionIsNeverApplied',
    'LiveSessionTransportTests/testForegroundCycleInstallsANewSessionAndInvalidatesTheRetiredOne',
    'LiveSessionTransportTests/testLiveSessionKeepsTheStreamTimeoutAndTheLivenessBudget',
    'LiveSessionTransportTests/testNeverRespondingHostKeyFailsWithinTheBoundAndEngagesTheLadder',
    'LiveSessionTransportTests/testRapidPhaseFlappingLeavesOneLiveSessionAndNoLeakedTasks',
    'LiveSessionTransportTests/testRetryDuringRetirementNeverDispatchesOnTheRetiredTransport',
    'LiveSessionTransportTests/testBoundaryJitterWithRetryNeverCreatesATaskOnARetiredTransport',
    'LiveSessionTransportTests/testBoundaryJitterWithoutRetryIsClean',
]
BATTERY_SIZE = len(battery)
EXPECTED_GREEN = 'Executed %d tests, with 0 failures' % BATTERY_SIZE
SIM_UDID = 'C8E69C58-66A4-440D-850C-8C1628C720E6'
command = ['env', 'HERDR_XCODEBUILD_DIRECT=1', 'HERMES_SIM_TASK_ACTIVE=1',
    'xcodebuild', 'test', '-project', 'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier',
    '-destination', 'platform=iOS Simulator,id=' + SIM_UDID,
    '-derivedDataPath', '/tmp/g554-probe-dd']
results = []


def reset_simulator():
    """Erase the lane's own simulator before EVERY leg.

    This host runs at 95-100% disk; one XCTest leg grows the lane simulator's
    device directory by hundreds of MiB, and a leg that runs out of space
    silently truncates its own log (which is how the earlier attempts failed).
    Erasing only the lane's own simulator before each leg bounds the footprint
    to a single leg and gives every leg the same device state.
    """
    subprocess.run(['xcrun', 'simctl', 'shutdown', SIM_UDID],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(['xcrun', 'simctl', 'erase', SIM_UDID],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def save():
    Path('/tmp/g554-probe-results.json').write_text(
        json.dumps({'source_head': head, 'results': results}, indent=2) + '\n')


def run(label, selection):
    args = command + ['-only-testing:FleetNotifierTests/' + name for name in selection]
    reset_simulator()
    print(label, shlex.join(args), flush=True)
    status = subprocess.run([sys.executable, str(runner), 'g554-probe-' + label, '1800', *args],
                            cwd=scratch).returncode
    log = Path('/tmp/g554-probe-' + label + '.log')
    text = log.read_text()
    failures = [cls + '/' + method for cls, method, _ in
                re.findall(r'error: -\[FleetNotifierTests\.(\w+) (\w+)\] : ([^\n]+)', text)]
    record = {'label': label, 'exit': status, 'command': shlex.join(args), 'log': str(log),
              'assertion_failures': failures}
    results.append(record)
    save()
    return status, text, record


status, text, _ = run('control', battery)
assert status == 0 and EXPECTED_GREEN in text, 'Invalid GREEN control'
mutations = [
    ('P1-shared-revert', 'ios/FleetNotifier/App/AppModel.swift',
     '    private func installLiveSession() {\n'
     '        guard liveSession == nil else { return }\n'
     '        let configuration = session.configuration\n'
     '        configuration.waitsForConnectivity = false\n'
     '        liveSession = URLSession(configuration: configuration)\n'
     '    }',
     '    private func installLiveSession() {\n'
     '        // Candidate defect: the pre-#554 production behaviour — no\n'
     '        // per-live-session transport, so every client rides the base/shared\n'
     '        // session and no session is ever invalidated.\n'
     '        liveSession = nil\n'
     '    }',
     ['LiveSessionTransportTests/testForegroundCycleInstallsANewSessionAndInvalidatesTheRetiredOne']),
    ('P2-no-invalidate', 'ios/FleetNotifier/App/AppModel.swift',
     '            transport.invalidateAndCancel()',
     '            // Candidate defect: retire without invalidating the transport,\n'
     '            // so the retired session keeps its pooled connections and the\n'
     '            // identity/leak tests cannot observe a teardown.\n'
     '            _ = transport',
     ['LiveSessionTransportTests/testForegroundCycleInstallsANewSessionAndInvalidatesTheRetiredOne',
      'LiveSessionTransportTests/testRapidPhaseFlappingLeavesOneLiveSessionAndNoLeakedTasks']),
    ('P3-slow-preflight', 'ios/FleetNotifier/Profiles/HostStreamCoordinator.swift',
     '    var attemptTimeout: TimeInterval = 5',
     '    var attemptTimeout: TimeInterval = 60',
     ['LiveSessionTransportTests/testNeverRespondingHostKeyFailsWithinTheBoundAndEngagesTheLadder']),
    ('P4-shrunk-stream-timeout', 'ios/FleetNotifier/App/AppModel.swift',
     '        configuration.waitsForConnectivity = false',
     '        configuration.waitsForConnectivity = false\n'
     '        // Candidate defect: shorten the live session\'s own stream timeout.\n'
     '        configuration.timeoutIntervalForRequest = 5',
     ['LiveSessionTransportTests/testLiveSessionKeepsTheStreamTimeoutAndTheLivenessBudget']),
    ('P5-no-session-guard', 'ios/FleetNotifier/App/AppModel.swift',
     '                guard !Task.isCancelled, self.isCurrent(context),\n'
     '                      self.isLiveTransport(transport) else { return }\n'
     '                self.banner = .error("fleet_refresh",',
     '                // Candidate defect: apply a retired session\'s failure to\n'
     '                // state — the completion path checks neither cancellation\n'
     '                // nor transport identity (round 3 registers the pull\n'
     '                // refresh as a life-path owner, so cancellation alone would\n'
     '                // otherwise mask this candidate and the probe would stop\n'
     '                // discriminating).\n'
     '                guard self.isCurrent(context) else { return }\n'
     '                self.banner = .error("fleet_refresh",',
     ['LiveSessionTransportTests/testCompletionFromTheRetiredSessionIsNeverApplied']),
]
for label, relative, old, new, expected in mutations:
    path = scratch / relative
    pristine = path.read_bytes()
    before = hashlib.sha256(pristine).hexdigest()
    source = pristine.decode()
    assert source.count(old) == 1, label
    subprocess.run(['shasum', '-a', '256', str(path)], check=True)
    try:
        path.write_text(source.replace(old, new, 1))
        status, text, record = run(label, expected)
        record.update(file=relative, old=old, new=new, before_sha256=before, expected_red=expected)
        save()
        # A mutated battery must fail: 65 for reported assertion failures, and a
        # test process that aborts on the candidate defect is also a non-zero RED.
        assert status != 0, 'Not a failing exit: ' + label
        for selector in expected:
            assert selector in record['assertion_failures'], (label, selector, record['assertion_failures'])
    finally:
        path.write_bytes(pristine)
        after = hashlib.sha256(path.read_bytes()).hexdigest()
        assert before == after, 'Restore differs: ' + label
        subprocess.run(['shasum', '-a', '256', str(path)], check=True)
        subprocess.run(['git', 'diff', '--exit-code'], cwd=scratch, check=True)
        if results[-1]['label'] == label:
            results[-1]['restored_sha256'] = after
            results[-1]['restore_diff_exit'] = 0
        save()
status, text, _ = run('restored-green', battery)
assert status == 0 and EXPECTED_GREEN in text, 'Final pristine GREEN failed'
subprocess.run(['git', 'diff', '--exit-code'], cwd=scratch, check=True)
subprocess.run(['git', '-C', str(root), 'worktree', 'remove', str(scratch)], check=True)
print('PROBES=%d ASSERTION_RED=%d RESTORED_GREEN=%d/%d RESTORE=BYTE_IDENTICAL'
      % (len(mutations), len(mutations), BATTERY_SIZE, BATTERY_SIZE), flush=True)
