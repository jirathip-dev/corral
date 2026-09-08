#!/usr/bin/env python3
"""Run the real CLI guard against disposable source and actual product copies."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--app', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    results = []

    def check(name, root, app=None, expected=1):
        command = [sys.executable, str(HERE/'check-native-art.py'), '--root', str(root)]
        if app is not None:
            command += ['--bundle', str(app)]
        result = subprocess.run(command, capture_output=True, text=True, timeout=60)
        (args.output/(name+'.log')).write_text(result.stdout+result.stderr)
        results.append({'name': name, 'command': command, 'exit': result.returncode, 'expected': expected})
        (args.output/'results.json').write_text(json.dumps(results, indent=2)+'\n')
        print(name, 'exit='+str(result.returncode), (result.stdout+result.stderr).strip(), flush=True)
        assert result.returncode == expected, name
        if expected:
            assert 'native-art FAIL:' in result.stderr, 'failure must come from guard, not launcher'

    check('pristine-green', ROOT, args.app, 0)
    with tempfile.TemporaryDirectory(prefix='corral444-art-guards-') as temp:
        root = Path(temp)/'source'
        shutil.copytree(ROOT/'ios/FleetNotifier', root/'ios/FleetNotifier')
        shutil.copytree(HERE, root/'ios/tools/herd-art', ignore=shutil.ignore_patterns('__pycache__'))
        victim = root/'ios/FleetNotifier/UI/Herd/HerdArt.swift'
        pristine = victim.read_bytes()
        probes = {
            'image-call': '\nlet forbidden = Image("ranch-day")\n',
            'qualified-multiline-image': '\nlet forbidden = SwiftUI . `Image` . init (\n "ranch-day"\n)\n',
            'uiimage-call': '\nlet forbidden = UIImage(named: "horse-sheet")\n',
            'qualified-multiline-uiimage': '\nlet forbidden = UIKit . UIImage . init (\n named: "horse-sheet"\n)\n',
            'flattened-background-path': '\nlet forbidden = CGImageSourceCreateWithURL(sceneURL, nil)\n',
            'r2-horse-reference': '\nlet forbidden = "r2-v1-premium/layers/horses/blocked.png"\n',
            'web-runtime': '\nlet forbidden = WKWebView()\n',
        }
        for name, addition in probes.items():
            victim.write_bytes(pristine+addition.encode())
            check(name, root)
            victim.write_bytes(pristine)
        outside = root/'ios/FleetNotifier/UI/HiddenBitmapHelper.swift'
        outside.write_text('let forbidden = Image("ranch-day")\n')
        check('outside-renderer-helper', root)
        outside.unlink()
        raster = root/'ios/FleetNotifier/ranch-day.png'
        raster.write_bytes(b'\x89PNG\r\n\x1a\n'+b'forbidden whole-scene test asset')
        check('source-scene-raster', root)
        raster.unlink()
        victim.chmod(0)
        try:
            check('unreadable-source', root)
        finally:
            victim.chmod(0o644)
        hidden = victim.with_suffix('.saved')
        victim.rename(hidden)
        check('missing-required-source', root)
        hidden.rename(victim)
        check('missing-root', Path(temp)/'missing')
        assert victim.read_bytes() == pristine
        check('restored-source-green', root, expected=0)

        app = Path(temp)/'FleetNotifier.app'
        shutil.copytree(args.app, app)
        for filename in ['ranch-day.png', 'horse-sheet.webp', 'hidden-raster.dat']:
            injected = app/filename
            injected.write_bytes(b'\x89PNG\r\n\x1a\n'+b'forbidden bundle raster')
            check('bundle-'+filename.replace('.','-'), root, app)
            injected.unlink()
        assets = app/'Assets.car'
        saved = assets.read_bytes()
        assets.write_bytes(b'not a valid catalog')
        check('unreadable-catalog', root, app)
        assets.write_bytes(saved)
        executable = app/'FleetNotifier'
        executable.unlink()
        check('missing-executable', root, app)
    check('restored-real-green', ROOT, args.app, 0)
    print(f'PASS: {len(results)} real CLI checks, disposable mutations only')


if __name__ == '__main__':
    main()
