#!/usr/bin/env python3
"""Run the real #512 display-name guard against disposable source and product copies.

The real `check-display-name.py` is invoked as a subprocess; every probe mutates
a throwaway copy only. Each mutation must exit 1 with a guard-owned
`display-name FAIL:` line, and the restored tree must return to exit 0.
"""
import argparse
import hashlib
import json
import plistlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
PROJECT = 'ios/project.yml'
PLIST = 'ios/FleetNotifier/Info.plist'
SPEC_LINE = 'CFBundleDisplayName: "Corral"'


def read_real(path):
    assert path.is_file(), f'missing real repository fixture: {path}'
    return path.read_bytes()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--app', type=Path,
                        help='optional built FleetNotifier.app for the bundle-mode probes')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    results = []

    def check(name, root, app=None, expected=1):
        command = [sys.executable, str(HERE / 'check-display-name.py'), '--root', str(root)]
        if app is not None:
            command += ['--bundle', str(app)]
        result = subprocess.run(command, capture_output=True, text=True, timeout=60)
        (args.output / (name + '.log')).write_text(result.stdout + result.stderr)
        results.append({'name': name, 'command': command, 'exit': result.returncode,
                        'expected': expected})
        (args.output / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
        print(name, 'exit=' + str(result.returncode), (result.stdout + result.stderr).strip(),
              flush=True)
        assert result.returncode == expected, name
        if expected:
            assert 'display-name FAIL:' in result.stderr, 'failure must come from guard, not launcher'

    real_project = read_real(ROOT / PROJECT)
    real_plist = read_real(ROOT / PLIST)
    real_hashes = {name: hashlib.sha256(data).hexdigest()
                   for name, data in ((PROJECT, real_project), (PLIST, real_plist))}

    check('pristine-green', ROOT, args.app, 0)

    with tempfile.TemporaryDirectory(prefix='corral512-display-name-') as temp:
        root = Path(temp) / 'root'
        (root / 'ios/FleetNotifier').mkdir(parents=True)
        (root / PROJECT).write_bytes(real_project)
        (root / PLIST).write_bytes(real_plist)
        spec_path = root / PROJECT
        plist_path = root / PLIST

        def set_spec(text):
            spec_path.write_text(text)

        def set_plist_bytes(data):
            plist_path.write_bytes(data)

        def mutate_plist(change):
            info = plistlib.loads(real_plist)
            change(info)
            plist_path.write_bytes(plistlib.dumps(info))

        def restore():
            spec_path.write_bytes(real_project)
            plist_path.write_bytes(real_plist)

        def with_spec_value(scalar):
            text = real_project.decode()
            assert SPEC_LINE in text, 'pristine spec no longer has the pinned line'
            return text.replace(SPEC_LINE, f'CFBundleDisplayName: {scalar}')

        # The generated plist is the value that reaches the product.
        for name, change in {
            'plist-truncating-name': lambda info: info.update(CFBundleDisplayName='Corral: Agent Fleet'),
            'plist-empty-name': lambda info: info.update(CFBundleDisplayName=''),
            'plist-missing-key': lambda info: info.pop('CFBundleDisplayName'),
            'plist-lowercase-near-miss': lambda info: info.update(CFBundleDisplayName='corral'),
            'plist-trailing-space': lambda info: info.update(CFBundleDisplayName='Corral '),
            'plist-non-string': lambda info: info.update(CFBundleDisplayName=512),
        }.items():
            mutate_plist(change)
            check(name, root)
            restore()
        set_plist_bytes(b'not a plist')
        check('plist-unparseable', root)
        restore()
        plist_path.unlink()
        check('plist-missing', root)
        restore()

        # The XcodeGen spec must stay pinned too, so a spec-only edit (before
        # regeneration) still fails closed.
        for name, scalar in {
            'spec-truncating-name': '"Corral: Agent Fleet"',
            'spec-lowercase-name': '"corral"',
            'spec-trailing-space': '"Corral "',
        }.items():
            set_spec(with_spec_value(scalar))
            check(name, root)
            restore()
        set_spec(''.join(line for line in real_project.decode().splitlines(keepends=True)
                         if 'CFBundleDisplayName:' not in line))
        check('spec-missing-key', root)
        restore()
        set_spec(real_project.decode() + f'{SPEC_LINE}\n')
        check('spec-duplicate-key', root)
        restore()
        spec_path.unlink()
        check('spec-missing', root)
        restore()
        set_spec(with_spec_value('Corral'))
        check('spec-unquoted-accepted', root, expected=0)
        restore()
        check('restored-green', root, expected=0)

        if args.app is not None:
            app = Path(temp) / 'FleetNotifier.app'
            shutil.copytree(args.app, app)
            info_path = app / 'Info.plist'
            pristine_info = info_path.read_bytes()

            def mutate_info(change):
                info = plistlib.loads(pristine_info)
                change(info)
                info_path.write_bytes(plistlib.dumps(info))

            check('bundle-pristine-green', root, app, 0)
            mutate_info(lambda info: info.update(CFBundleDisplayName='Corral: Agent Fleet'))
            check('bundle-truncating-name', root, app)
            info_path.write_bytes(pristine_info)
            mutate_info(lambda info: info.pop('CFBundleDisplayName'))
            check('bundle-missing-key', root, app)
            info_path.write_bytes(pristine_info)
            mutate_info(lambda info: info.update(CFBundleIdentifier='com.example.notcorral'))
            check('bundle-wrong-product', root, app)
            info_path.write_bytes(pristine_info)
            check('bundle-restored-green', root, app, 0)

    check('restored-real-green', ROOT, args.app, 0)
    for name, digest in real_hashes.items():
        assert hashlib.sha256(read_real(ROOT / name)).hexdigest() == digest, f'real file mutated: {name}'
    print(f'PASS: {len(results)} real CLI checks, disposable mutations only')


if __name__ == '__main__':
    main()
