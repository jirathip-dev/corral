#!/usr/bin/env python3
"""Run the real CLI guard against disposable source and actual product copies."""
import argparse
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
DESIGN_COMMIT = '060220c32c4b5de638f4950dd8bd71fa71814816'


def read_real(path):
    assert path.is_file(), f'missing real repository fixture: {path}'
    return path.read_bytes()


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
        # #464: the four generated preview imageset names are the ONLY
        # permitted non-symbol loaders, spelled as a bare literal call.
        picker = root/'ios/FleetNotifier/UI/AppIconPicker.swift'
        pristine_picker = picker.read_bytes()
        picker.write_text('func preview() -> Image { return Image("PalominoPreview") }\n')
        check('picker-approved-preview-rendition', root, expected=0)
        for name, probe in {
            'picker-appiconset-rendition': 'func preview() -> Image { return Image("Palomino") }\n',
            'picker-unapproved-rendition': 'func preview() -> Image { return Image("Original") }\n',
            'picker-qualified-rendition': 'func preview() -> Image { return SwiftUI.Image("PalominoPreview") }\n',
            'picker-init-rendition': 'func preview() -> Image { return Image.init("PalominoPreview") }\n',
            'picker-uiimage-rendition': 'func preview() -> Image { return Image(uiImage: UIImage(named: "PalominoPreview")) }\n',
        }.items():
            picker.write_text(probe)
            check(name, root)
            picker.write_bytes(pristine_picker)
        # The approved preview name must still be forbidden INSIDE the renderer.
        victim.write_bytes(pristine+b'func preview() -> Image { return Image("PalominoPreview") }\n')
        check('renderer-approved-name-still-forbidden', root)
        victim.write_bytes(pristine)
        # #464: the allowlist is derived from the approval contract, so
        # tampering with the preview metadata must fail closed.
        approval_path = root/'ios/tools/herd-art/appicon-approval.json'
        pristine_approval = approval_path.read_bytes()

        def rewrite_approval(change):
            contract = json.loads(pristine_approval)
            change(contract)
            approval_path.write_text(json.dumps(contract, indent=2) + '\n')

        rewrite_approval(lambda c: c['catalog'].update(
            preview_sets=[*c['catalog']['preview_sets'], 'BayPreview2']))
        check('picker-extra-preview-set', root)
        approval_path.write_bytes(pristine_approval)
        rewrite_approval(lambda c: c['masters']['grey'].update(preview_set='BayPreview'))
        check('picker-preview-set-mismatch', root)
        approval_path.write_bytes(pristine_approval)
        picker.write_bytes(pristine_picker)
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
        executable_data = executable.read_bytes()
        executable.unlink()
        check('missing-executable', root, app)
        executable.write_bytes(executable_data)

        # #463: the built product must declare and carry exactly the approved
        # four Treatment-A horse icons.
        info_path = app/'Info.plist'
        pristine_info = info_path.read_bytes()

        def mutate_info(change):
            info = plistlib.loads(pristine_info)
            change(info)
            info_path.write_bytes(plistlib.dumps(info))

        def restore_info():
            info_path.write_bytes(pristine_info)

        mutate_info(lambda info: info['CFBundleIcons']['CFBundleAlternateIcons'].pop('Palomino'))
        check('bundle-missing-alternate-declaration', root, app)
        restore_info()
        mutate_info(lambda info: info['CFBundleIcons']['CFBundleAlternateIcons'].update(
            {'Bay': {'CFBundleIconName': 'Bay'}}))
        check('bundle-duplicate-bay-declaration', root, app)
        restore_info()
        mutate_info(lambda info: info['CFBundleIcons']['CFBundlePrimaryIcon'].update(
            CFBundleIconName='Bay'))
        check('bundle-primary-misnamed', root, app)
        restore_info()
        mutate_info(lambda info: info.pop('CFBundleIcons~ipad'))
        check('bundle-missing-ipad-declaration', root, app)
        restore_info()
        loose = sorted(app.glob('AppIcon*.png'))
        assert loose, 'built product has no loose primary app-icon PNG'
        saved_loose = [(png, png.read_bytes()) for png in loose]
        for png, _ in saved_loose:
            png.unlink()
        check('bundle-missing-loose-primary', root, app)
        for png, data in saved_loose:
            png.write_bytes(data)
        legacy = read_real(ROOT/'assets/icon/corral-icon-1024.png')
        injected = app/'AppIcon-512@2x.png'
        injected.write_bytes(legacy)
        check('bundle-legacy-original-png', root, app)
        injected.unlink()
        treatment_b = subprocess.run(
            ['git', '-C', str(ROOT), 'show',
             f'{DESIGN_COMMIT}:docs/design/evidence/issue-462-horse-icons/masters/treatment-b/bay-1024.png'],
            capture_output=True, timeout=60)
        if treatment_b.returncode == 0:
            injected = app/'Palomino60x60@2x.png'
            injected.write_bytes(treatment_b.stdout)
            check('bundle-treatment-b-png', root, app)
            injected.unlink()
        reduced = Path(temp)/'reduced-catalog'
        shutil.copytree(ROOT/'ios/FleetNotifier/Assets.xcassets', reduced)
        shutil.rmtree(reduced/'Palomino.appiconset')
        compiled = Path(temp)/'reduced-car'
        compiled.mkdir()
        actool = subprocess.run(
            ['xcrun', 'actool', str(reduced), '--compile', str(compiled),
             '--platform', 'iphonesimulator', '--minimum-deployment-target', '17.0',
             '--app-icon', 'AppIcon',
             '--alternate-app-icon', 'Black', '--alternate-app-icon', 'Grey',
             '--target-device', 'iphone', '--target-device', 'ipad',
             '--output-partial-info-plist', str(compiled/'partial.plist'),
             '--errors', '--warnings'], capture_output=True, text=True, timeout=180)
        assert actool.returncode == 0, f'reduced catalog did not compile: {actool.stdout}{actool.stderr}'
        assets.write_bytes((compiled/'Assets.car').read_bytes())
        check('bundle-missing-alternate-rendition', root, app)
        assets.write_bytes(saved)
    check('restored-real-green', ROOT, args.app, 0)
    print(f'PASS: {len(results)} real CLI checks, disposable mutations only')


if __name__ == '__main__':
    main()
