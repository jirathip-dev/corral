#!/usr/bin/env python3
"""Opt-in #551 scene-seam measurements; invoke under flock /tmp/n.lock.

Five fresh XCTest processes, each a fresh cold model followed by 30/300-second
scene-seam background intervals. This is not OS suspension or a daemon benchmark.
"""
import os
from pathlib import Path
import plistlib
import shlex
import subprocess

ROOT = Path(__file__).resolve().parents[3]
DD = Path('/tmp/g551-dd')
DEST = 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793'
BOUNDED = ROOT / 'docs/evidence/issue-547/bounded-run.py'
os.chdir(ROOT)
os.environ.update(HERDR_XCODEBUILD_DIRECT='1', HERMES_SIM_TASK_ACTIVE='1')


def run(label, seconds, args):
    print(shlex.join(args), flush=True)
    subprocess.run(['python3', str(BOUNDED), label, str(seconds), *args], check=True)


run('g551-measure-build', 600, ['xcodebuild', 'build-for-testing', '-project',
    'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier', '-destination', DEST,
    '-derivedDataPath', str(DD)])
products = DD / 'Build/Products'
source = products / 'FleetNotifier_iphonesimulator26.5-arm64.xctestrun'
config = plistlib.loads(source.read_bytes())
config['FleetNotifierTests']['EnvironmentVariables']['G551_MEASURE'] = '1'
target = products / 'g551-measure.xctestrun'
target.write_bytes(plistlib.dumps(config))
run('g551-measure', 2100, ['xcodebuild', 'test-without-building', '-xctestrun', str(target),
    '-destination', DEST, '-test-iterations', '5', '-test-repetition-relaunch-enabled', 'YES',
    '-parallel-testing-enabled', 'NO', '-only-testing:FleetNotifierTests/ForegroundReconnectTests/testWarmReturnStageMeasurements'])
