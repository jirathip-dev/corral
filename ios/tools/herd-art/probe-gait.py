#!/usr/bin/env python3
"""#448 gait: assertion-level native RED/GREEN through the REAL renderer.

The committed worktree project is never mutated or regenerated: `ios/` is
copied to a temp root, where `xcodegen` registers the new (unregistered)
`FleetNotifierTests/HerdGaitTests.swift` and every mutation runs against the
disposable copy. Candidate GREEN first, then one-mutation-at-a-time REDs —
fixed-angle renderer, broken diagonal coordination, reduce-motion ignored —
each of which MUST fail by XCTest assertion (never compilation), then
byte-verified restoration and GREEN again.

Run under the shared native flock; logs stay outside the source tree.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--udid', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--derived-data', type=Path, default=Path('/tmp/corral448-gait-dd'))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    results = []
    with tempfile.TemporaryDirectory(prefix='corral448-gait-') as temporary:
        root = Path(temporary)
        shutil.copytree(ROOT / 'ios', root / 'ios',
                        ignore=shutil.ignore_patterns('.build', 'build', '__pycache__', 'xcuserdata'))
        (root / 'tests/fixtures').mkdir(parents=True)
        for fixture in ['canonical_stream_golden.json', 'live_session_exchange_golden.json']:
            shutil.copy2(ROOT / 'tests/fixtures' / fixture, root / 'tests/fixtures' / fixture)
        sources = {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
                   for parent in (root / 'ios/FleetNotifier', root / 'ios/FleetNotifierTests')
                   for path in parent.rglob('*.swift')}

        def run(name, command, expected=0, failure=None):
            start = time.monotonic()
            log = args.output / (name + '.log')
            with log.open('w') as handle:
                result = subprocess.run(command, cwd=root, stdout=handle, stderr=subprocess.STDOUT,
                                        timeout=900,
                                        env={**os.environ, 'HERDR_XCODEBUILD_DIRECT': '1',
                                             'HERMES_SIM_TASK_ACTIVE': '1'})
            entry = {'name': name, 'command': command, 'cwd': str(root), 'exit': result.returncode,
                     'expected': expected, 'seconds': round(time.monotonic() - start, 2), 'log': str(log)}
            results.append(entry)
            (args.output / 'commands.json').write_text(json.dumps(results, indent=2) + '\n')
            print(json.dumps(entry), flush=True)
            assert result.returncode == expected, name
            if failure:
                assert re.search(r'Test Case .*' + failure + r'.*failed', log.read_text()), \
                    f'{name}: must fail by XCTest assertion, not compilation'

        run('generate', ['xcodegen', 'generate', '--spec', 'ios/project.yml'])
        base = ['xcodebuild', 'test', '-project', 'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier',
                '-destination', 'platform=iOS Simulator,id=' + args.udid,
                '-derivedDataPath', str(args.derived_data),
                '-parallel-testing-enabled', 'NO', 'CODE_SIGNING_ALLOWED=NO']
        focused = base + ['-only-testing:FleetNotifierTests/HerdGaitTests']
        run('gait-green', focused)

        art = root / 'ios/FleetNotifier/UI/Herd/HerdArt.swift'
        model = root / 'ios/FleetNotifier/UI/Herd/HerdModel.swift'

        def mutate(path, anchor, replacement, name, failure):
            pristine = path.read_text()
            assert pristine.count(anchor) == 1, f'{name}: anchor not unique'
            path.write_text(pristine.replace(anchor, replacement))
            try:
                run(name, focused, expected=65, failure=failure)
            finally:
                path.write_text(pristine)

        mutate(art,
               'let step = gait.isStepping && (pose == .working || pose == .stand)'
               ' ? gait.swing * sin(2 * .pi * gait.phase) : 0',
               'let step = 0.0  // #448 probe M1: fixed-angle mutation',
               'gait-fixed-angle-red', 'testWorkingGaitMovesHoovesAlongTheApprovedArc')
        mutate(art,
               'let legs = [rest[0] + step, rest[1] - step, rest[2] - step, rest[3] + step]',
               'let legs = [rest[0] + step, rest[1] - step, rest[2] - step, '
               'rest[3] - step]  // #448 probe M2: coordination mutation',
               'gait-coordination-red', 'testOpposingDiagonalPairsStepOppositeAndHoovesStayAnchored')
        mutate(model,
               'guard !reduceMotion, elapsed.isFinite else { return .standstill }',
               'guard elapsed.isFinite else { return .standstill }  // #448 probe M3: reduce-motion mutation',
               'gait-reduce-motion-red', 'testHerdHorseGaitDerivationIsStateAppropriateAndMotionSafe')

        for relative, digest in sources.items():
            assert hashlib.sha256((root / relative).read_bytes()).hexdigest() == digest, relative
        run('restored-green', base + ['-only-testing:FleetNotifierTests/HerdGaitTests',
                                      '-only-testing:FleetNotifierTests/HerdTests',
                                      '-only-testing:FleetNotifierTests/HerdEnvironmentTests'])
        (args.output / 'restoration.json').write_text(json.dumps(sources, indent=2) + '\n')
    print('PASS: #448 gait candidate GREEN; fixed-angle, coordination and reduce-motion mutations '
          'each RED by assertion; restored byte-identical and GREEN')


if __name__ == '__main__':
    main()
