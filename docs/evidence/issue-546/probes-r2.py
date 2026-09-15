#!/usr/bin/env python3
"""Rerun #546 guards plus R2 witnesses, using the original bounded runner."""
import hashlib
import json
import os
from pathlib import Path
import runpy
import subprocess

PREVIOUS = runpy.run_path(str(Path(__file__).with_name('probes.py')))
ROOT = Path(__file__).resolve().parents[3]
SCRATCH = Path('/tmp/g546-r2-scratch')
APP = PREVIOUS['APP']
DELEGATE = PREVIOUS['DELEGATE']
SUITE = PREVIOUS['SUITE']
run = PREVIOUS['run']
PROBES = PREVIOUS['PROBES'] + [
    ('R201-runtime-posture', APP, [
        ('              (profile.id == activeProfileID || coordinator?.allowsLiveWork(profileID: profile.id) != false),\n', '')],
     'testRuntimeCoordinatorMismatchRefusesHintWithoutQuota'),
    ('R202-foreign-stamp', APP, [
        ('                      FleetStore.conformsToPinnedHost(.snapshot(snapshot), pin: hint.hostID),\n', '')],
     'testForeignHostSnapshotIsRefusedBeforeStoreMutation'),
    ('R203-stream-admission', APP, [('              !target.isStreaming,\n', '')],
     'testAlreadyStreamingHintHasNoRequestsOrQuota'),
    ('R204-live-mode', APP, [
        ('guard backgroundRefreshEnabled, backgroundAllowed(), mode == .live,',
         'guard backgroundRefreshEnabled, backgroundAllowed(),')],
     'testNonLiveModesRefuseHintWithoutQuota'),
    ('R205-quota-count', APP, [('              attempts.count <= 2,\n', '')],
     'testOversizedAndMalformedQuotaBlobsFailClosed'),
    ('R206-model-absent-completion', DELEGATE, [
        ('guard let model = backgroundRefreshModel else {\n            completionHandler(.noData)\n            return',
         'guard let model = backgroundRefreshModel else {\n            return')],
     'testDelegateWithoutRestoredModelCompletesNoDataOnce'),
    ('R207-finite-masked-by-decoder', APP, [
        ('attempts.allSatisfy({ $0.attemptedAt.isFinite })', 'true')],
     'testOversizedAndMalformedQuotaBlobsFailClosed'),
    ('R208-stream-masked-by-generation', APP, [
        ('&& !target.isStreaming && target.connectionGeneration == generation',
         '&& target.connectionGeneration == generation')],
     'testStreamOpenedDuringHintPreventsLateApply'),
    ('R209-stream-and-generation', APP, [
        ('&& !target.isStreaming && target.connectionGeneration == generation', '&& true')],
     'testStreamOpenedDuringHintPreventsLateApply'),
]
# These two observations are disclosed masking, NOT claimed discrimination.
EXPECTED_GREEN = {
    'R207-finite-masked-by-decoder': 'JSONDecoder rejects nonfinite values before isFinite.',
    'R208-stream-masked-by-generation': 'Opening a stream also changes connectionGeneration.',
}


def main():
    if SCRATCH.exists():
        raise SystemExit('scratch exists; refusing to reuse/delete an unowned tree')
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    subprocess.run(['git', 'worktree', 'add', '--detach', str(SCRATCH), head], cwd=ROOT, check=True)
    command = ['env', 'HERDR_XCODEBUILD_DIRECT=1', 'HERMES_SIM_TASK_ACTIVE=1', 'xcodebuild',
               'test', '-project', 'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier',
               '-destination', 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793',
               '-derivedDataPath', '/tmp/g546-r2-probe-dd']
    results = []
    output = Path('/tmp/g546-r2-probe-results.json')

    def save():
        output.write_text(json.dumps(dict(head=head, scratch=str(SCRATCH), results=results), indent=2)+'\n')

    def control(name, suites):
        result = run(command+['-only-testing:FleetNotifierTests/'+s for s in suites],
                     f'/tmp/g546-r2-probe-{name.lower()}.log', SCRATCH)
        result['name'] = name
        results.append(result)
        save()
        return result['exit'] == 0 and not result['timed_out']

    if not control('INITIAL_GREEN', ['BackgroundHintTests']):
        raise SystemExit('initial scratch GREEN failed; no mutations attempted')
    misses = 0
    for name, relative, replacements, test in PROBES:
        path = SCRATCH/relative
        original = path.read_bytes()
        before = hashlib.sha256(original).hexdigest()
        print(name, 'BEFORE', flush=True)
        subprocess.run(['shasum', '-a', '256', str(path)], check=True)
        mutated = original.decode()
        for old, new in replacements:
            assert mutated.count(old) == 1, (name, old, mutated.count(old))
            mutated = mutated.replace(old, new)
        try:
            path.write_text(mutated)
            result = run(command+['-only-testing:'+SUITE+test], f'/tmp/g546-r2-probe-{name}.log', SCRATCH)
        finally:
            path.write_bytes(original)
            os.utime(path, None)
            print(name, 'RESTORED', flush=True)
            subprocess.run(['shasum', '-a', '256', str(path)], check=True)
            assert path.read_bytes() == original
        log = Path(result['log']).read_text()
        assertion_red = result['exit'] == 65 and not result['timed_out'] and (
            f"Test Case '-[FleetNotifierTests.BackgroundHintTests {test}]' failed" in log
            and ('XCTAssert' in log or 'failed - bounded fixture wait' in log))
        observed_green = result['exit'] == 0 and not result['timed_out'] and (
            f"Test Case '-[FleetNotifierTests.BackgroundHintTests {test}]' passed" in log)
        matched = observed_green if name in EXPECTED_GREEN else assertion_red
        result.update(name=name, file=relative, replacements=replacements, test=test,
                      assertion_red=assertion_red, observed_green=observed_green,
                      expected='GREEN (masked)' if name in EXPECTED_GREEN else 'assertion RED',
                      masking=EXPECTED_GREEN.get(name), matched=matched,
                      before_sha256=before, after_sha256=hashlib.sha256(path.read_bytes()).hexdigest())
        results.append(result)
        save()
        print(name, 'ASSERTION_RED=', assertion_red, 'MATCHED_EXPECTATION=', matched, flush=True)
        misses = 0 if matched else misses+1
        if misses >= 2:
            print('Two consecutive unexpected outcomes: breaker.', flush=True)
            break
    final = control('FINAL_GREEN', ['BackgroundHintTests', 'PushPayloadTests', 'EpochRecoveryTests'])
    subprocess.run(['git', 'diff', '--exit-code'], cwd=SCRATCH, check=True)
    okay = final and len(results) == len(PROBES)+2 and all(r.get('matched', True) for r in results)
    print('PROBE_BATTERY_PASS=', okay, 'RESULTS=', output, flush=True)
    raise SystemExit(0 if okay else 1)


if __name__ == '__main__':
    main()
