#!/usr/bin/env python3
"""Run the real app-icons CLI against disposable source copies with mutations.

Every probe must make the guard RED with an attributable 'app-icons FAIL:'
message; the untouched tree must stay GREEN. Disposable copies only — the
repository tree is never mutated. Stdlib only.
"""
import argparse
import binascii
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
FIXTURE = ('ios/FleetNotifier/Assets.xcassets', 'ios/tools/herd-art', 'ios/project.yml')
LEGACY_ORIGINAL = 'e2c754cf3dd7cbc8f10090597360eb56c56fb4672856d48407453dc8190e15e7'
DESIGN_COMMIT = '060220c32c4b5de638f4950dd8bd71fa71814816'
TREATMENT_B_BAY = '9f1298d17c7995d08f26da63236cbbe6010c3d6a0b68ef10aa71c97e2c053137'


def png(width, height, color_type, rgb=(7, 7, 7), alpha=255):
    """Deterministic minimal PNG (color type 2 = RGB, 6 = RGBA)."""
    pixel = bytes((*rgb, alpha)[:3 if color_type == 2 else 4])

    def chunk(kind, payload):
        return (struct.pack('>I', len(payload)) + kind + payload
                + struct.pack('>I', binascii.crc32(kind + payload) & 0xffffffff))

    raw = b''.join(b'\x00' + pixel * width for _ in range(height))
    return (b'\x89PNG\r\n\x1a\n'
            + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, color_type, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(raw))
            + chunk(b'IEND', b''))


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def make_fixture(destination):
    for relative in FIXTURE:
        source = ROOT / relative
        target = destination / relative
        if source.is_dir():
            shutil.copytree(source, target, ignore=shutil.ignore_patterns('__pycache__'))
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
    return destination


def run(root):
    command = [sys.executable, str(HERE / 'app-icons.py'), '--check', '--root', str(root)]
    return subprocess.run(command, capture_output=True, text=True, timeout=60)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    results = []

    def check(name, root, expected=1):
        result = run(root)
        log = result.stdout + result.stderr
        (args.output / f'{name}.log').write_text(log)
        results.append({'name': name, 'exit': result.returncode, 'expected': expected})
        (args.output / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
        print(name, 'exit=' + str(result.returncode), log.strip(), flush=True)
        assert result.returncode == expected, name
        if expected:
            assert 'app-icons FAIL:' in result.stderr, f'failure must come from the guard: {name}'

    legacy = (ROOT / 'assets/icon/corral-icon-1024.png').read_bytes()
    assert sha256(legacy) == LEGACY_ORIGINAL, 'fixture integrity: repository legacy Original bytes changed'
    check('pristine-green', ROOT, expected=0)

    with tempfile.TemporaryDirectory(prefix='corral463-app-icons-') as temporary:
        base = make_fixture(Path(temporary) / 'base')
        check('fixture-green', base, expected=0)

    def probe(name, mutate):
        with tempfile.TemporaryDirectory(prefix='corral463-app-icons-') as temporary:
            root = make_fixture(Path(temporary) / 'root')
            mutate(root)
            check(name, root)

    catalog = lambda root: root / 'ios/FleetNotifier/Assets.xcassets'
    approval_path = lambda root: root / 'ios/tools/herd-art/appicon-approval.json'
    project_path = lambda root: root / 'ios/project.yml'

    def rewrite_approval(root, change):
        contract = json.loads(approval_path(root).read_text())
        change(contract)
        approval_path(root).write_text(json.dumps(contract, indent=2) + '\n')

    def rewrite_project(root, change):
        text = project_path(root).read_text()
        project_path(root).write_text(change(text))

    def flip_byte(path):
        data = bytearray(path.read_bytes())
        data[len(data) // 2] ^= 1
        path.write_bytes(data)

    def add_duplicate_bay(root):
        directory = catalog(root) / 'Bay.appiconset'
        directory.mkdir()
        (directory / 'Bay-1024.png').write_bytes(
            (root / 'ios/tools/herd-art/appicon-masters/treatment-a/bay-1024.png').read_bytes())
        (directory / 'Contents.json').write_text(
            (catalog(root) / 'AppIcon.appiconset/Contents.json').read_text())

    def alpha_master(root):
        path = root / 'ios/tools/herd-art/appicon-masters/treatment-a/grey-1024.png'
        path.write_bytes(png(1024, 1024, 6, alpha=255))
        rewrite_approval(root, lambda c: c['masters']['grey'].update(sha256=sha256(path.read_bytes())))

    probe('delete-alternate',
          lambda root: shutil.rmtree(catalog(root) / 'Palomino.appiconset'))
    probe('misname-alternate',
          lambda root: (catalog(root) / 'Palomino.appiconset').rename(catalog(root) / 'PalominoIcon.appiconset'))
    probe('swap-legacy-primary',
          lambda root: (catalog(root) / 'AppIcon.appiconset/AppIcon-1024.png').write_bytes(legacy))
    probe('swap-legacy-alternate',
          lambda root: (catalog(root) / 'Grey.appiconset/Grey-1024.png').write_bytes(legacy))
    probe('duplicate-bay-alternate', add_duplicate_bay)
    probe('wrong-hash',
          lambda root: flip_byte(catalog(root) / 'Black.appiconset/Black-1024.png'))
    probe('wrong-dimension',
          lambda root: (catalog(root) / 'Grey.appiconset/Grey-1024.png').write_bytes(png(512, 512, 2)))
    probe('alpha-bearing',
          lambda root: (catalog(root) / 'AppIcon.appiconset/AppIcon-1024.png').write_bytes(
              png(1024, 1024, 6, alpha=0)))
    probe('stale-generated-output',
          lambda root: (catalog(root) / 'Palomino.appiconset/Palomino-1024.png').write_bytes(
              png(1024, 1024, 2, rgb=(1, 2, 3))))
    probe('stale-contents-json',
          lambda root: (catalog(root) / 'AppIcon.appiconset/Contents.json').write_text(
              (catalog(root) / 'AppIcon.appiconset/Contents.json').read_text().replace(
                  'AppIcon-1024.png', 'AppIcon-512@2x.png')))
    probe('stray-catalog-file',
          lambda root: (catalog(root) / 'AppIcon.appiconset/stray.dat').write_bytes(b'not-approved-artwork'))
    probe('project-missing-alternates',
          lambda root: rewrite_project(root, lambda text: text.replace(
              '        ASSETCATALOG_COMPILER_ALTERNATE_APPICON_NAMES: "Palomino Black Grey"\n', '')))
    probe('project-extra-bay',
          lambda root: rewrite_project(root, lambda text: text.replace(
              'ASSETCATALOG_COMPILER_ALTERNATE_APPICON_NAMES: "Palomino Black Grey"',
              'ASSETCATALOG_COMPILER_ALTERNATE_APPICON_NAMES: "Palomino Black Grey Bay"')))
    probe('project-wrong-primary',
          lambda root: rewrite_project(root, lambda text: text.replace(
              'ASSETCATALOG_COMPILER_APPICON_NAME: AppIcon',
              'ASSETCATALOG_COMPILER_APPICON_NAME: Bay')))
    probe('project-include-all',
          lambda root: rewrite_project(root, lambda text: text.replace(
              '        ASSETCATALOG_COMPILER_GENERATE_ASSET_SYMBOLS: YES\n',
              '        ASSETCATALOG_COMPILER_GENERATE_ASSET_SYMBOLS: YES\n'
              '        ASSETCATALOG_COMPILER_INCLUDE_ALL_APPICON_ASSETS: YES\n')))
    probe('project-device-family-drop',
          lambda root: rewrite_project(root, lambda text: text.replace(
              'TARGETED_DEVICE_FAMILY: "1,2"', 'TARGETED_DEVICE_FAMILY: "1"')))
    probe('approval-treatment-b',
          lambda root: rewrite_approval(root, lambda c: c['approval'].update(treatment='B')))
    probe('approval-master-hash',
          lambda root: rewrite_approval(
              root, lambda c: c['masters']['palomino'].update(sha256='0' * 64)))
    probe('approval-alternate-set',
          lambda root: rewrite_approval(
              root, lambda c: c['catalog'].update(alternate_sets=['Palomino', 'Black', 'Grey', 'Bay'])))
    probe('master-missing',
          lambda root: (root / 'ios/tools/herd-art/appicon-masters/treatment-a/black-1024.png').unlink())
    probe('master-structure-alpha', alpha_master)
    # #464: the four loadable preview imagesets must stay generated,
    # byte-identical to the approved masters and contract-pinned.
    probe('preview-missing-imageset',
          lambda root: shutil.rmtree(catalog(root) / 'BayPreview.imageset'))
    probe('preview-misnamed-imageset',
          lambda root: (catalog(root) / 'GreyPreview.imageset').rename(
              catalog(root) / 'GreyPreviewIcon.imageset'))
    probe('preview-wrong-bytes',
          lambda root: (catalog(root) / 'PalominoPreview.imageset/PalominoPreview-1024.png').write_bytes(legacy))
    probe('preview-swapped-master',
          lambda root: (catalog(root) / 'BlackPreview.imageset/BlackPreview-1024.png').write_bytes(
              (root / 'ios/tools/herd-art/appicon-masters/treatment-a/grey-1024.png').read_bytes()))
    probe('preview-stale-contents',
          lambda root: (catalog(root) / 'BayPreview.imageset/Contents.json').write_text(
              (catalog(root) / 'BayPreview.imageset/Contents.json').read_text().replace(
                  'BayPreview-1024.png', 'BayPreview-512.png')))
    probe('preview-stray-file',
          lambda root: (catalog(root) / 'GreyPreview.imageset/stray.dat').write_bytes(b'not-approved-artwork'))
    probe('preview-approval-extra-set',
          lambda root: rewrite_approval(
              root, lambda c: c['catalog'].update(
                  preview_sets=[*c['catalog']['preview_sets'], 'BayPreview2'])))
    probe('preview-approval-mapping',
          lambda root: rewrite_approval(
              root, lambda c: c['masters']['grey'].update(preview_set='BayPreview')))
    probe('preview-approval-reuses-appiconset',
          lambda root: rewrite_approval(
              root, lambda c: c['catalog'].update(
                  preview_sets=['AppIcon', 'PalominoPreview', 'BlackPreview', 'GreyPreview'])))
    probe('canonical-source-drift',
          lambda root: (root / 'ios/tools/herd-art/horsesvg.py').write_bytes(
              (root / 'ios/tools/herd-art/horsesvg.py').read_bytes() + b'\n'))

    def forbidden_mechanism(root):
        path = catalog(root) / 'Palomino.appiconset/Palomino-1024.png'
        path.write_bytes(png(1024, 1024, 2, rgb=(9, 9, 9)))

        def swap_hash(contract):
            contract['forbidden']['treatment_b'][0] = sha256(path.read_bytes())

        rewrite_approval(root, swap_hash)

    probe('forbidden-hash-mechanism', forbidden_mechanism)

    try:
        treatment_b = subprocess.run(
            ['git', '-C', str(ROOT), 'show',
             f'{DESIGN_COMMIT}:docs/design/evidence/issue-462-horse-icons/masters/treatment-b/bay-1024.png'],
            capture_output=True, timeout=60)
    except (OSError, subprocess.SubprocessError):
        treatment_b = None
    if treatment_b is not None and treatment_b.returncode == 0:
        assert sha256(treatment_b.stdout) == TREATMENT_B_BAY, 'fixture integrity: Treatment B bytes changed'
        probe('swap-real-treatment-b',
              lambda root: (catalog(root) / 'Palomino.appiconset/Palomino-1024.png').write_bytes(
                  treatment_b.stdout))
    else:
        print('swap-real-treatment-b SKIPPED: design commit object unavailable in this clone')
        results.append({'name': 'swap-real-treatment-b', 'exit': None, 'expected': 1, 'skipped': True})
        (args.output / 'results.json').write_text(json.dumps(results, indent=2) + '\n')

    check('restored-real-green', ROOT, expected=0)
    print(f'PASS: {len(results)} real app-icons CLI checks, disposable mutations only')


if __name__ == '__main__':
    main()
