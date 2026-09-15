#!/usr/bin/env python3
"""#546 client-only mutation evidence; no relay, APNs or live host access."""
import hashlib
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[3]
SCRATCH = Path('/tmp/g546-scratch')
APP = 'ios/FleetNotifier/App/AppModel.swift'
DELEGATE = 'ios/FleetNotifier/Notifications/AppDelegate.swift'
VIEW = 'ios/FleetNotifier/UI/FleetViews.swift'
SUITE = 'FleetNotifierTests/BackgroundHintTests/'
# (name, file, exact replacements, discriminating test)
PROBES = [
    ('M01-opt-out', APP, [
        ('guard backgroundRefreshEnabled, backgroundAllowed(), mode == .live,', 'guard backgroundAllowed(), mode == .live,'),
        ('!request.finished && self.backgroundRefreshEnabled && self.backgroundAllowed()', '!request.finished && self.backgroundAllowed()')],
     'testOffByDefaultAndIndependentOfVisiblePreference'),
    ('M02-unknown-host', APP, [
        ('orderedProfiles.first(where: { $0.hostKeyB64 == hint.hostID })', 'orderedProfiles.first')],
     'testUnknownRemovedUnenrolledAndTrustDeniedDoNotFetch'),
    ('M03-local-trust', APP, [
        ('profile.mayConnect, let key = profile.keyId, !key.isEmpty, signer != nil,', 'let key = profile.keyId, !key.isEmpty, signer != nil,')],
     'testUnknownRemovedUnenrolledAndTrustDeniedDoNotFetch'),
    ('M04-enrollment', APP, [
        ('profile.mayConnect, let key = profile.keyId, !key.isEmpty, signer != nil,', 'profile.mayConnect, signer != nil,')],
     'testUnenrolledHostHasNoFetchOrPersistence'),
    ('M05-fresh-pin', APP, [
        ('HostKeyTrust.matches(keyResponse, pinnedKeyB64: hint.hostID)', 'true')],
     'testFreshKeyMismatchStopsBeforeSnapshot'),
    ('M06-closed-top-level', DELEGATE, [
        ('Set(info.keys) == Set(["v", "type", "host_id", "aps"])', 'true')],
     'testClosedSchemaRejectsMalformedAndAllExtraContent'),
    ('M07-closed-aps', DELEGATE, [
        ('Set(aps.keys) == Set(["content-available"])', 'true')],
     'testClosedSchemaRejectsMalformedAndAllExtraContent'),
    ('M08-numeric-schema', DELEGATE, [('return number == 1', 'return true')],
     'testClosedSchemaRejectsMalformedAndAllExtraContent'),
    ('M09-host-ceiling', APP, [('backgroundNow() - $0.attemptedAt < 1800', 'backgroundNow() - $0.attemptedAt < 0')],
     'testHostThirtyMinuteCeilingAndDeviceHourlyCeiling'),
    ('M10-device-ceiling', APP, [('attempts.count < 2,', 'attempts.count < 3,')],
     'testHostThirtyMinuteCeilingAndDeviceHourlyCeiling'),
    ('M11-inflight', APP, [('              backgroundRefreshes[profile.id] == nil,\n', '')],
     'testCoalescesInflightEvenAfterRateWindow'),
    ('M12-exactly-once', DELEGATE, [('        guard !finished else { return }\n', '')],
     'testCompletionOwnerIsExactlyOnceAcrossTerminalPaths'),
    ('M13-local-deadline', APP, [
        ('do { try await sleep(remaining) } catch { return }\n            request.finish(.failed)',
         'do { try await sleep(remaining) } catch { return }\n            return')],
     'testLocalDeadlineCompletesOnceWithoutHostResponseAndCancels'),
    ('M14-cancel-opt-out', APP, [('        if !enabled { cancelBackgroundRefreshes() }', '        if !enabled { return }')],
     'testDisableCancelsAndLateResultCannotApply'),
    ('M15-foreground', APP, [('                self?.cancelBackgroundRefreshes()', '                _ = self')],
     'testForegroundNotificationCancelsWithoutSceneHandlerChanges'),
    ('M16-late-budget', APP, [(' && self.backgroundUptime() - started < 15', '')],
     'testLateSnapshotIsRefusedEvenBeforeDeadlineTaskRuns'),
    ('M17-newer-stream', APP, [
        ('&& target.lastEventId == revision && target.lastEventEpoch == epoch\n                && target.agents == agents', '&& true')],
     'testNewerStreamDataAndNewEpochWinEvenOverHigherHintRev'),
    ('M18-epoch-downgrade', APP, [('epoch == nil || snapshot.epoch == epoch', 'true')],
     'testOlderAndCrossEpochSnapshotsCannotResetCursor'),
    ('M19-older-snapshot', APP, [('                target.applyRefresh(snapshot)',
                                 '                target.restoreCursor(rev: nil)\n                target.applyRefresh(snapshot)')],
     'testOlderAndCrossEpochSnapshotsCannotResetCursor'),
    # Deliberately add an illicit liveness publication after snapshot persistence.
    # verifyHostKey removes the restored pending posture; generic apply marks Live.
    ('M20-no-live', APP, [('                request.finish(.newData)',
                          '                target.verifyHostKey()\n                target.apply(.snapshot(snapshot))\n                request.finish(.newData)')],
     'testOneSnapshotPersistsOnlyBoardMetadataAndNeverMarksLive'),
    ('M21-one-snapshot', APP, [('                let snapshot = try await client.fetchSnapshot()\n                guard current(),',
                               '                _ = try await client.fetchSnapshot()\n                let snapshot = try await client.fetchSnapshot()\n                guard current(),')],
     'testOneSnapshotPersistsOnlyBoardMetadataAndNeverMarksLive'),
    ('M22-removed-host', APP, [(' && self.profileStore?.profile(id: profile.id) == profile', '')],
     'testRemovedHostCannotBeRecreatedByLateSnapshot'),
    ('M23-os-availability', APP, [
        ('guard backgroundRefreshEnabled, backgroundAllowed(), mode == .live,', 'guard backgroundRefreshEnabled, mode == .live,'),
        ('!request.finished && self.backgroundRefreshEnabled && self.backgroundAllowed()', '!request.finished && self.backgroundRefreshEnabled')],
     'testOSUnavailableAndNetworkErrorStayStaleWithoutRetry'),
    ('M24-settings-wiring', VIEW, [('set: { model.setBackgroundRefreshEnabled($0) }', 'set: { model.setNotificationsEnabled($0) }')],
     'testSettingsUsesIndependentRefreshBindingAndTruthfulGuidance'),
]


def run(args, log, cwd, timeout=240):
    print(shlex.join(args), '>', log, '2>&1', flush=True)
    start = time.monotonic()
    with open(log, 'w') as output:
        process = subprocess.Popen(args, cwd=cwd, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
            timed_out = False
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            code = process.returncode
            timed_out = True
    print('RAW_EXIT=', code, 'TIMED_OUT=', timed_out, flush=True)
    return dict(command=shlex.join(args), log=log, exit=code, timed_out=timed_out,
                seconds=round(time.monotonic()-start, 2))


def main():
    if SCRATCH.exists():
        raise SystemExit('scratch already exists; refusing to reuse or delete an unowned tree')
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    subprocess.run(['git', 'worktree', 'add', '--detach', str(SCRATCH), head], cwd=ROOT, check=True)
    command = ['env', 'HERDR_XCODEBUILD_DIRECT=1', 'HERMES_SIM_TASK_ACTIVE=1', 'xcodebuild',
               'test', '-project', 'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier',
               '-destination', 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793',
               '-derivedDataPath', '/tmp/g546-probe-dd']
    results = []
    output = Path('/tmp/g546-probe-results.json')
    def save():
        output.write_text(json.dumps(dict(head=head, scratch=str(SCRATCH), results=results), indent=2)+'\n')
    initial = run(command+['-only-testing:FleetNotifierTests/BackgroundHintTests'], '/tmp/g546-probe-initial-green.log', SCRATCH)
    initial['name'] = 'INITIAL_GREEN'
    results.append(initial)
    save()
    if initial['exit'] != 0 or initial['timed_out']:
        raise SystemExit('initial scratch GREEN failed; no mutations attempted')
    misses = 0
    for name, relative, replacements, test in PROBES:
        path = SCRATCH/relative
        original = path.read_bytes()
        digest = hashlib.sha256(original).hexdigest()
        print(name, 'BEFORE', flush=True)
        subprocess.run(['shasum', '-a', '256', str(path)], check=True)
        mutated = original.decode()
        for old, new in replacements:
            if mutated.count(old) != 1:
                raise SystemExit(f'{name}: anchor count {mutated.count(old)}: {old!r}')
            mutated = mutated.replace(old, new)
        try:
            path.write_text(mutated)
            result = run(command+['-only-testing:'+SUITE+test], '/tmp/g546-probe-'+name+'.log', SCRATCH)
        finally:
            path.write_bytes(original)
            os.utime(path, None)
            print(name, 'RESTORED', flush=True)
            subprocess.run(['shasum', '-a', '256', str(path)], check=True)
            assert path.read_bytes() == original
        log = Path(result['log']).read_text()
        assertion_red = result['exit'] == 65 and not result['timed_out'] and (
            f"Test Case '-[FleetNotifierTests.BackgroundHintTests {test}]' failed" in log)
        result.update(name=name, file=relative, replacements=replacements, test=test,
                      assertion_red=assertion_red, before_sha256=digest,
                      after_sha256=hashlib.sha256(path.read_bytes()).hexdigest())
        results.append(result)
        save()
        print(name, 'ASSERTION_RED=', assertion_red, flush=True)
        misses = 0 if assertion_red else misses+1
        if misses >= 2:
            print('Two consecutive invalid/green probes: breaker.', flush=True)
            break
    final = run(command+['-only-testing:FleetNotifierTests/BackgroundHintTests',
                         '-only-testing:FleetNotifierTests/PushPayloadTests',
                         '-only-testing:FleetNotifierTests/EpochRecoveryTests'], '/tmp/g546-probe-final-green.log', SCRATCH)
    final['name'] = 'FINAL_GREEN'
    results.append(final)
    save()
    subprocess.run(['git', 'diff', '--exit-code'], cwd=SCRATCH, check=True)
    okay = (len(results) == len(PROBES)+2 and final['exit'] == 0 and not final['timed_out']
            and all(r.get('assertion_red', True) for r in results))
    print('PROBE_BATTERY_PASS=', okay, 'RESULTS=', output, flush=True)
    raise SystemExit(0 if okay else 1)


if __name__ == '__main__':
    main()
