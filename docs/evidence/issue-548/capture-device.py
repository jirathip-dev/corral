#!/usr/bin/env python3
"""Capture #548 on a true 390x844 iPhone 14 simulator using the native tests.

Reuses herd-art/capture.py's bounded simctl helper and the committed XCTest
window renderer; no live data, screenshots are neither cropped nor resized.
Run under flock /tmp/n.lock, from the checkout root.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path.cwd()
sys.path.insert(0, str(ROOT / 'ios/tools/herd-art'))
from capture import call


def main():
    output = Path('/tmp/g548-evidence')
    output.mkdir(parents=True, exist_ok=True)
    udid = call('create', 'g548-iPhone14', 'com.apple.CoreSimulator.SimDeviceType.iPhone-14',
                'com.apple.CoreSimulator.SimRuntime.iOS-26-5')
    print('CREATED=' + udid, flush=True)
    try:
        devices = json.loads(call('list', 'devices', '--json'))
        device = next(d for group in devices['devices'].values() for d in group if d['udid'] == udid)
        (output / 'device.json').write_text(json.dumps(device, indent=2)+'\n')
        command = ['xcodebuild', 'test', '-project', 'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier',
                   '-destination', 'platform=iOS Simulator,id=' + udid, '-derivedDataPath', '/tmp/g548-dd',
                   '-only-testing:FleetNotifierTests/HerdRailZoneTests']
        start = time.monotonic()
        log = Path('/tmp/g548-native-capture.log')
        with log.open('w') as stream:
            result = subprocess.run(command, cwd=ROOT,
                                    env={**os.environ, 'HERDR_XCODEBUILD_DIRECT': '1', 'HERMES_SIM_TASK_ACTIVE': '1'},
                                    stdout=stream, stderr=subprocess.STDOUT, timeout=900)
        record = dict(command=command, cwd=str(ROOT), exit=result.returncode,
                      seconds=round(time.monotonic()-start, 2), log=str(log), device=device)
        (output / 'native-command.json').write_text(json.dumps(record, indent=2)+'\n')
        print(json.dumps(record), flush=True)
        result.check_returncode()
        subprocess.run(['python3', 'docs/evidence/issue-548/collect.py', '--log', str(log),
                        '--output', str(output / 'native')], check=True, timeout=90)
    finally:
        try:
            print(call('shutdown', udid), flush=True)
        finally:
            print(call('delete', udid), flush=True)
            print('DELETED=' + udid, flush=True)


if __name__ == '__main__':
    main()
