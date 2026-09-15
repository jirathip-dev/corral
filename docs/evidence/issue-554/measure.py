#!/usr/bin/env python3
"""Opt-in #554 scene-seam measurements; invoke under flock /tmp/n.lock.

Five fresh XCTest processes, each a fresh cold model followed by 30/300-second
scene-seam background intervals. This is not OS suspension or a daemon benchmark.
Same harness method the #551 lane used (G551_MEASURE / testWarmReturnStageMeasurements),
re-pointed at this lane's simulator and derived data so the #554 fix is measured
by the identical production scene seam.
"""
import os
from pathlib import Path
import plistlib
import shlex
import subprocess

ROOT = Path(__file__).resolve().parents[3]
DD = Path('/tmp/g554-dd')
DEST = 'platform=iOS Simulator,id=C8E69C58-66A4-440D-850C-8C1628C720E6'
BOUNDED = ROOT / 'docs/evidence/issue-547/bounded-run.py'
os.chdir(ROOT)
os.environ.update(HERDR_XCODEBUILD_DIRECT='1', HERMES_SIM_TASK_ACTIVE='1')


def run(label, seconds, args):
    print(shlex.join(args), flush=True)
    subprocess.run(['python3', str(BOUNDED), label, str(seconds), *args], check=True)


run('g554-measure-build', 600, ['xcodebuild', 'build-for-testing', '-project',
    'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier', '-destination', DEST,
    '-derivedDataPath', str(DD)])
products = DD / 'Build/Products'
source = products / 'FleetNotifier_iphonesimulator26.5-arm64.xctestrun'
config = plistlib.loads(source.read_bytes())
config['FleetNotifierTests']['EnvironmentVariables']['G551_MEASURE'] = '1'
target = products / 'g554-measure.xctestrun'
target.write_bytes(plistlib.dumps(config))
run('g554-measure', 2100, ['xcodebuild', 'test-without-building', '-xctestrun', str(target),
    '-destination', DEST, '-test-iterations', '5', '-test-repetition-relaunch-enabled', 'YES',
    '-parallel-testing-enabled', 'NO',
    '-only-testing:FleetNotifierTests/ForegroundReconnectTests/testWarmReturnStageMeasurements'])
