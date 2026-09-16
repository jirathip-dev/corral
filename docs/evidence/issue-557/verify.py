#!/usr/bin/env python3
"""#557 receipts. Run under flock /tmp/n.lock; simulator stages need hermes-sim-task."""
import collections
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
OUT = Path(__file__).resolve().parent
DD = Path('/tmp/g557-dd')
BASE = 'ca7400dc4d688da705c39f0d9c9b8b925f999c48'
SOURCE = 'ios/FleetNotifier/UI/Herd/HerdView.swift'
TEST = 'ios/FleetNotifierTests/HerdTests.swift'
os.chdir(ROOT)


def run(name, args, timeout=1200, expected=0):
    subprocess.run(['df', '-h', '/'], check=True)
    log = Path('/tmp/g557-' + name + '.log')
    started = time.monotonic()
    print('RUN', shlex.join(args), flush=True)
    with log.open('w') as stream:
        process = subprocess.Popen(args, stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
        timed_out = False
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(process.pid, signal.SIGTERM)
            try:
                code = process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                code = process.wait(timeout=15)
    receipt = dict(name=name, command=shlex.join(args), exit=code, timed_out=timed_out,
                   seconds=round(time.monotonic() - started, 2), log=str(log))
    with (OUT / 'gates.jsonl').open('a') as stream:
        stream.write(json.dumps(receipt) + '\n')
    print(json.dumps(receipt), flush=True)
    if timed_out or code != expected:
        raise RuntimeError(f'{name}: exit {code}, expected {expected}; inspect {log}')
    return log


def xcode(action, sim=None, configuration='Debug', only=()):
    destination = 'platform=iOS Simulator,id=' + sim if sim else 'generic/platform=iOS Simulator'
    return ['xcodebuild', '-project', 'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier',
            '-configuration', configuration, '-destination', destination,
            '-derivedDataPath', str(DD), 'CODE_SIGNING_ALLOWED=NO',
            '-parallel-testing-enabled', 'NO', *['-only-testing:FleetNotifierTests/' + item for item in only], action]


def captures(sim):
    container = Path(subprocess.check_output(['xcrun', 'simctl', 'get_app_container', sim,
                                             'com.corral.fleetnotifier', 'data'], text=True).strip())
    images = []
    for night in ('day', 'night'):
        for size in ('large', 'accessibility3'):
            for suffix in ('', '-git'):
                name = f'{night}-blocked-{size}{suffix}.png'
                target = OUT / name
                shutil.copyfile(container / 'Documents/g548-evidence' / name, target)
                images.append(dict(file=name, sha256=hashlib.sha256(target.read_bytes()).hexdigest()))
    name = 'day-blocked-accessibility3-git-scrolled.png'
    shutil.copyfile(container / 'Documents/g548-evidence' / name, OUT / name)
    images.append(dict(file=name, sha256=hashlib.sha256((OUT / name).read_bytes()).hexdigest()))
    devices = json.loads(subprocess.check_output(['xcrun', 'simctl', 'list', 'devices', '-j'], text=True))
    device = next(d for group in devices['devices'].values() for d in group if d['udid'] == sim)
    (OUT / 'images.json').write_text(json.dumps(dict(device=device, renderer='HerdRailZoneTests UIHostingController + drawHierarchy; 390x844 pt, scale 1', images=images), indent=2) + '\n')


def main():
    stage = sys.argv[1]
    sim = sys.argv[2] if len(sys.argv) > 2 else None
    if not DD.exists():
        DD.mkdir()
        (DD / '.g557-owned').write_text(str(ROOT) + '\n')
    if stage == 'static':
        run('parse', ['swiftc', '-parse', SOURCE, TEST])
        run('xcodegen', ['xcodegen', 'generate', '--spec', 'ios/project.yml'])
        run('xcodegen-drift', ['git', 'diff', '--exit-code', '--', 'ios/FleetNotifier.xcodeproj/project.pbxproj'])
        run('release-source', ['python3', 'ios/check-release-demo.py'])
        run('release-self-test', ['python3', 'ios/check-release-demo.py', '--self-test'])
    elif stage == 'focused':
        assert sim
        only = ['HerdTests', 'HerdRailZoneTests', 'FullScreenHerdShellWiringTests']
        run('focused', xcode('test', sim, only=only))
        captures(sim)
    elif stage == 'full':
        assert sim
        run('full', xcode('test', sim)[:-1] + ['-only-testing:FleetNotifierTests', 'test'])
    elif stage == 'builds':
        run('debug-build', xcode('build'))
        run('release-build', xcode('build', configuration='Release'))
        run('release-binary', ['python3', 'ios/check-release-demo.py', '--binary',
                              str(DD / 'Build/Products/Release-iphonesimulator/FleetNotifier.app/FleetNotifier')])
    elif stage == 'mutation':
        assert sim
        path = ROOT / SOURCE
        before = path.read_bytes()
        marker = b'                        Text("\xe2\x97\x8f").font(.caption2.weight(.semibold)).foregroundStyle(theme.peach)\n                            .fixedSize()\n'
        assert before.count(marker) == 1
        only = ['HerdTests/testHorseCaptionLabelWiresPositiveGitMarkers']
        run('mutation-before-sha', ['shasum', '-a', '256', SOURCE])
        try:
            path.write_bytes(before.replace(marker, b''))
            run('mutation-red', xcode('test', sim, only=only), expected=65)
        finally:
            path.write_bytes(before)
        run('mutation-restored-sha', ['shasum', '-a', '256', SOURCE])
        assert path.read_bytes() == before
        run('mutation-restore-diff', ['git', 'diff', '--exit-code', '--', SOURCE])
        run('mutation-green', xcode('test', sim, only=only))
        run('mutation-after-green-sha', ['shasum', '-a', '256', SOURCE])
        assert path.read_bytes() == before
    elif stage == 'slop':
        base = DD / 'slop-base'
        for relative in (SOURCE, TEST):
            target = base / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(subprocess.check_output(['git', 'show', BASE + ':' + relative]))
        command = ['swift', 'run', '--package-path', 'ios/tools/anti-slop-swift',
                   '--scratch-path', str(DD / 'slop-build'), 'anti-slop']
        # These two touched files have no baseline findings; nonzero is a real diagnostic.
        old = run('slop-base', command + [str(base / SOURCE), str(base / TEST)])
        new = run('slop-head', command + [SOURCE, TEST])
        pattern = re.compile(r'([^/\s]+\.swift):\d+:\d+: error: \[([^\]]+)\]')
        counts = [collections.Counter(pattern.findall(log.read_text())) for log in (old, new)]
        assert counts[0] == counts[1], counts
        print('ANTI_SLOP_DELTA=0', flush=True)
    else:
        raise ValueError(stage)


if __name__ == '__main__':
    main()
