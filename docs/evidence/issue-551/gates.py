#!/usr/bin/env python3
"""Run the issue-551 brief gates once, after G1 and the source commit."""
import json
from pathlib import Path
import shlex
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
runner = root / 'docs/evidence/issue-547/bounded-run.py'
records = []
heavy = ['flock', '/tmp/n.lock', 'env', 'HERDR_XCODEBUILD_DIRECT=1', 'HERMES_SIM_TASK_ACTIVE=1']
project = ['-project', 'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier']


def run(label, args, seconds=900, allowed=(0,)):
    print(shlex.join(args), flush=True)
    status = subprocess.run([sys.executable, str(runner), label, str(seconds), *args], cwd=root).returncode
    records.append(json.loads(Path('/tmp', label + '.json').read_text()))
    Path('/tmp/g551-gate-results.json').write_text(json.dumps(records, indent=2) + '\n')
    if status not in allowed:
        sys.exit(status)


run('g551-full', heavy + ['xcodebuild', 'test', *project, '-destination',
    'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793',
    '-derivedDataPath', '/tmp/g551-dd', '-only-testing:FleetNotifierTests'])
run('g551-check-release', ['python3', 'ios/check-release-demo.py'])
run('g551-self-test', ['python3', 'ios/check-release-demo.py', '--self-test'])
run('g551-debug', heavy + ['xcodebuild', 'build', *project, '-configuration', 'Debug',
    '-destination', 'generic/platform=iOS Simulator', '-derivedDataPath', '/tmp/g551-debug-dd'])
run('g551-release', heavy + ['xcodebuild', 'build', *project, '-configuration', 'Release',
    '-sdk', 'iphonesimulator', '-destination', 'generic/platform=iOS Simulator',
    '-derivedDataPath', '/tmp/g551-release-dd', 'CODE_SIGNING_ALLOWED=NO'])
run('g551-binary', ['python3', 'ios/check-release-demo.py', '--binary',
    '/tmp/g551-release-dd/Build/Products/Release-iphonesimulator/FleetNotifier.app/FleetNotifier'])
run('g551-xcodegen', ['bash', '-c', 'cd ios && xcodegen generate --spec project.yml'])
run('g551-xcodegen-diff', ['git', 'diff', '--exit-code', '--', 'ios/'])
run('g551-aslop-head', ['flock', '/tmp/n.lock', 'swift', 'run', '--package-path',
    'ios/tools/anti-slop-swift', 'anti-slop', 'ios/FleetNotifier', 'ios/FleetNotifierTests'], allowed=(0, 1))
run('g551-diffcheck', ['git', 'diff', '--check'])
