#!/usr/bin/env python3
"""Export actual Herd XCTest captures and measured frames, without resizing."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import struct
import subprocess


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--log', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    text = args.log.read_text()
    bundles = re.findall(r'^\s*(/.*\.xcresult)\s*$', text, re.M)
    assert len(bundles) == 1, bundles
    exported = args.output / 'attachments'
    command = ['xcrun', 'xcresulttool', 'export', 'attachments', '--test-id',
               'HerdRailZoneTests/testReservedZoneKeepsFirstHorseRowFixed()',
               '--path', bundles[0], '--output-path', str(exported)]
    result = subprocess.run(command, capture_output=True, text=True, timeout=60)
    print(json.dumps(dict(command=command, exit=result.returncode, output=result.stdout, error=result.stderr)))
    result.check_returncode()
    manifest = json.loads((exported / 'manifest.json').read_text())
    captures = {}
    for test in manifest:
        for attachment in test['attachments']:
            name = attachment['suggestedHumanReadableName'].split('_0_')[0].removeprefix('548-')
            source = exported / attachment['exportedFileName']
            data = source.read_bytes()
            dimensions = list(struct.unpack('>II', data[16:24]))
            assert dimensions == [390, 844], (name, dimensions)
            assert name not in captures, name
            destination = args.output / (name + '.png')
            shutil.copyfile(source, destination)
            captures[name] = dict(path=str(destination), pixels=dimensions,
                                  sha256=hashlib.sha256(data).hexdigest(),
                                  deviceId=attachment['deviceId'], deviceName=attachment['deviceName'])
    measurements = {}
    for name, safe, frame_text in re.findall(r'^G548_MEASURE (\S+) safeTop=([0-9.]+) frames=(.+)$', text, re.M):
        assert name not in measurements, name
        frames = {key: list(map(float, values.split(', ')))
                  for key, values in re.findall(r'"([^"]+)": \(([^)]+)\)', frame_text)}
        assert 'field-0' in frames, name
        measurements[name] = dict(safeTop=float(safe), frames=frames,
                                  firstRowInHerd=frames['field-0'][1],
                                  firstRowInWindow=frames['field-0'][1]+float(safe))
    required = {f'{light}-{state}-large' for light in ('day', 'night') for state in ('empty', 'blocked', 'last-known')}
    assert required <= captures.keys()
    assert required <= measurements.keys()
    assert 'day-blocked-accessibility3' in captures
    report = dict(log=str(args.log), xcresult=bundles[0], captures=captures, measurements=measurements)
    (args.output / 'capture.json').write_text(json.dumps(report, indent=2, sort_keys=True)+'\n')
    print(json.dumps(dict(captures=len(captures), measurements=len(measurements),
                         output=str(args.output / 'capture.json'))))


if __name__ == '__main__':
    main()
