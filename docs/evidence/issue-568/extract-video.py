#!/usr/bin/env python3
"""Extract completed, individually recorded UI cases without wall-clock trimming."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess


def command(args):
    result = subprocess.run(args, capture_output=True, text=True, timeout=180)
    if result.returncode:
        raise RuntimeError(f'{args}: exit {result.returncode}\n{result.stderr}')
    return result.stdout


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('run', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    receipt = json.loads((args.run / 'receipt.json').read_text())
    log = (args.run / 'test.log').read_text()
    manifest = {'receipt': receipt, 'cases': [], 'measurements': []}
    for epoch, payload in re.findall(r'G568_GEOMETRY epoch=([\d.]+) (\{[^\n]+\})', log):
        manifest['measurements'].append({'epoch': float(epoch), **json.loads(payload)})
    args.output.mkdir(parents=True, exist_ok=True)
    for recorded in receipt['recordings']:
        if not recorded['complete']:
            continue
        assert recorded['exit'] == 0, recorded
        name, video = recorded['name'], Path(recorded['path'])
        probe = json.loads(command(['ffprobe', '-v', 'error', '-show_format', '-show_streams', '-of', 'json', str(video)]))
        clip = args.output / (name + '.mp4')
        # Lossless remux of the whole per-case recordVideo output. No claim that
        # simctl's media clock matches XCTest's wall-clock phase timestamps.
        command(['ffmpeg', '-v', 'error', '-y', '-i', str(video), '-an', '-c:v', 'copy', '-movflags', '+faststart', str(clip)])
        frames = Path('/tmp') / ('g568-frames-' + args.output.name) / name
        frames.mkdir(parents=True, exist_ok=True)
        command(['ffmpeg', '-v', 'error', '-y', '-i', str(clip), '-vf', 'fps=4', str(frames / '%04d.png')])
        phases = [{'name': phase, 'epoch': float(epoch)}
                  for phase, epoch in re.findall(r'G568_PHASE (\S+) epoch=([\d.]+)', log)
                  if phase.startswith(name + '-')]
        manifest['cases'].append({'name': name, 'path': str(clip), 'media': probe, 'phases': phases,
                                  'frames_directory': str(frames), 'sample_fps': 4,
                                  'frames': len(list(frames.glob('*.png'))),
                                  'sha256': hashlib.sha256(clip.read_bytes()).hexdigest()})
    assert manifest['cases'], 'no completed gesture case in the real test log'
    (args.output / 'capture.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(json.dumps({'exit': receipt['exit'], 'cases': [{k: v for k, v in case.items() if k != 'media'}
                                                       for case in manifest['cases']],
                      'measurement_count': len(manifest['measurements'])}, indent=2))


if __name__ == '__main__':
    main()
