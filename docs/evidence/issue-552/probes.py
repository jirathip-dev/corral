#!/usr/bin/env python3
"""#552 assertion-level probes in an owned disposable worktree."""
import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[3]
SCRATCH = Path('/tmp/g552-probe')
DD = Path('/tmp/g552-probe-dd')
OUT = Path('/tmp/g552-evidence/probes')
BASE = 'b6b69f377883d6b2507f8418b6b4dc34f7452145'
VIEW = Path('ios/FleetNotifier/UI/Herd/HerdView.swift')
TEST = Path('ios/FleetNotifierTests/HerdTests.swift')
UDID = '59DDC0C5-891E-4EC0-91AF-4F50DF68D793'


def main():
    assert not SCRATCH.exists() and not DD.exists(), 'refuse to overwrite a pre-existing scratch/cache'
    OUT.mkdir(parents=True, exist_ok=True)
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    subprocess.run(['git', 'worktree', 'add', '--detach', str(SCRATCH), head], cwd=ROOT, check=True, timeout=60)
    assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=SCRATCH, text=True).strip() == head
    original = (SCRATCH/VIEW).read_bytes()
    test_bytes = (SCRATCH/TEST).read_bytes()
    records = []
    command = ['flock', '/tmp/n.lock', 'env', 'HERDR_XCODEBUILD_DIRECT=1', 'HERMES_SIM_TASK_ACTIVE=1',
               'xcodebuild', 'test', '-project', 'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier',
               '-destination', 'platform=iOS Simulator,id=' + UDID, '-derivedDataPath', str(DD)]

    def run(label, method, expected):
        disk = subprocess.check_output(['df', '-h', '/'], text=True)
        print(disk, flush=True)
        (OUT/(label + '-disk.txt')).write_text(disk)
        argv = command + ['-only-testing:FleetNotifierTests/HerdRailZoneTests' + ('/' + method if method else '')]
        log = Path('/tmp/g552-probe-' + label + '.log')
        started = time.monotonic()
        with log.open('w') as stream:
            result = subprocess.run(argv, cwd=SCRATCH, stdout=stream, stderr=subprocess.STDOUT, timeout=900)
        record = dict(label=label, cwd=str(SCRATCH), command=argv, exit=result.returncode,
                      expected=expected, log=str(log), seconds=round(time.monotonic()-started, 2))
        records.append(record)
        (OUT/'commands.json').write_text(json.dumps(records, indent=2) + '\n')
        print(json.dumps(record), flush=True)
        assert result.returncode == expected, record
        if expected:
            text = log.read_text()
            assert 'XCTAssert' in text and re.search(r'Test Case .*' + re.escape(method) + r'.*failed', text), 'require assertion RED, not build failure'
            assert 'error: emit-module' not in text and 'Testing cancelled because the build failed' not in text
        else:
            assert '** TEST SUCCEEDED **' in log.read_text()

    def restore(label):
        (SCRATCH/VIEW).write_bytes(original)
        os.utime(SCRATCH/VIEW, None)
        assert (SCRATCH/VIEW).read_bytes() == original
        assert (SCRATCH/TEST).read_bytes() == test_bytes
        output = subprocess.check_output(['shasum', '-a', '256', str(SCRATCH/VIEW), str(SCRATCH/TEST)], text=True)
        (OUT/(label + '-restoration.txt')).write_text(output)
        print(label, output, flush=True)

    (OUT/'before-shasum.txt').write_text(subprocess.check_output(['shasum', '-a', '256', str(SCRATCH/VIEW), str(SCRATCH/TEST)], text=True))
    (OUT/'provenance.json').write_text(json.dumps(dict(head=head, base=BASE, viewSHA256=hashlib.sha256(original).hexdigest()), indent=2) + '\n')
    try:
        (SCRATCH/VIEW).write_bytes(subprocess.check_output(['git', 'show', BASE + ':' + str(VIEW)], cwd=ROOT))
        run('base-font-red', 'testRepositoryChipUsesPrimaryNameAndSecondaryCountFonts', 65)
        restore('base-font')
        mutations = [
            ('font-red', 'let name = Text(paddock.title).font(.subheadline.weight(.semibold))',
             'let name = Text(paddock.title).font(.caption2)', 'testRepositoryChipUsesPrimaryNameAndSecondaryCountFonts'),
            ('gap-red', '.padding(.horizontal,12).padding(.top,hudSpacing)',
             '.padding(.horizontal,12).padding(.top,2)', 'testRepositoryChipSharesHUDRhythmAndKeepsIntrinsicChrome'),
        ]
        (OUT/'mutations.json').write_text(json.dumps(mutations, indent=2) + '\n')
        for label, old, new, method in mutations:
            text = original.decode()
            assert text.count(old) == 1, old
            (SCRATCH/VIEW).write_text(text.replace(old, new))
            run(label, method, 65)
            restore(label)
        run('restored-green', '', 0)
        assert not subprocess.check_output(['git', 'status', '--porcelain'], cwd=SCRATCH).strip()
    finally:
        restore('final')
    subprocess.run(['git', 'worktree', 'remove', str(SCRATCH)], cwd=ROOT, check=True, timeout=60)
    # Created exclusively by this invocation; raw logs and hashes are outside DD.
    if DD.exists():
        shutil.rmtree(DD)
    print('PASS: base-font RED, font RED, measured-gap RED, byte-identical restore, full rail class GREEN; owned scratch/cache removed')


if __name__ == '__main__':
    main()
