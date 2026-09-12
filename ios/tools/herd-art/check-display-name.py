#!/usr/bin/env python3
"""#512: fail-closed installed-display-name gate.

The installed app's Home Screen label must be exactly `Corral`; the truncating
`Corral: Agent Fleet` is pinned out. XcodeGen owns the generated
`ios/FleetNotifier/Info.plist` (`info.path` in `ios/project.yml`), so this gate
asserts the value that actually reaches the product instead of a substring of
the spec:

- the generated, committed `ios/FleetNotifier/Info.plist` declares exactly
  `Corral`;
- `ios/project.yml` declares exactly one `CFBundleDisplayName` and its scalar
  is `Corral`, so a spec-only edit fails before anyone regenerates;
- with `--bundle`, the BUILT product's `Info.plist` declares exactly `Corral`
  and is still the Corral app product (bundle id `com.corral.fleetnotifier`).

Stdlib only; the source mode runs on Linux CI.
"""
from __future__ import annotations

import argparse
import plistlib
import re
import sys
from pathlib import Path
from xml.parsers.expat import ExpatError

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
EXPECTED = 'Corral'
PROJECT_RELATIVE = 'ios/project.yml'
PLIST_RELATIVE = 'ios/FleetNotifier/Info.plist'
PRODUCT_BUNDLE_ID = 'com.corral.fleetnotifier'


def fail(message: str) -> None:
    print(f'display-name FAIL: {message}', file=sys.stderr)
    raise SystemExit(1)


def read(path: Path) -> bytes:
    if not path.is_file() or path.is_symlink():
        fail(f'missing/unreadable/nonregular file: {path}')
    return path.read_bytes()


def read_text(path: Path) -> str:
    try:
        return read(path).decode('utf-8')
    except UnicodeError as error:
        fail(f'not UTF-8: {path}: {error}')


def project_scalar(root: Path) -> str:
    text = read_text(root / PROJECT_RELATIVE)
    found = re.findall(r'^\s*CFBundleDisplayName:[ \t]*(\S.*?)[ \t]*$', text, re.MULTILINE)
    if len(found) != 1:
        fail(f'{PROJECT_RELATIVE} must declare exactly one CFBundleDisplayName, found {len(found)}')
    scalar = found[0]
    if len(scalar) > 1 and scalar[0] == scalar[-1] == '"':
        scalar = scalar[1:-1]
    return scalar


def plist_display_name(path: Path, label: str) -> str:
    try:
        info = plistlib.loads(read(path))
    except (plistlib.InvalidFileException, ValueError, ExpatError) as error:
        fail(f'{label} is not a readable plist: {error}')
    if not isinstance(info, dict):
        fail(f'{label} is not a plist dictionary')
    value = info.get('CFBundleDisplayName')
    if not isinstance(value, str):
        fail(f'{label} has no string CFBundleDisplayName (found {value!r})')
    return value


def source(root: Path) -> str:
    scalar = project_scalar(root)
    if scalar != EXPECTED:
        fail(f'{PROJECT_RELATIVE} CFBundleDisplayName must be exactly {EXPECTED!r}, found {scalar!r}')
    value = plist_display_name(root / PLIST_RELATIVE, PLIST_RELATIVE)
    if value != EXPECTED:
        fail(f'generated {PLIST_RELATIVE} CFBundleDisplayName must be exactly {EXPECTED!r}, '
             f'found {value!r} (regenerate with xcodegen after editing ios/project.yml)')
    print(f'display-name PASS: {PROJECT_RELATIVE} and {PLIST_RELATIVE} both declare exactly {EXPECTED!r}')
    return value


def bundle(app: Path) -> None:
    info_path = app / 'Info.plist'
    try:
        info = plistlib.loads(read(info_path))
    except (plistlib.InvalidFileException, ValueError, ExpatError) as error:
        fail(f'{info_path} is not a readable plist: {error}')
    if not isinstance(info, dict):
        fail(f'{info_path} is not a plist dictionary')
    if info.get('CFBundleIdentifier') != PRODUCT_BUNDLE_ID:
        fail(f'not the actual Corral app product: {info_path} CFBundleIdentifier '
             f'is {info.get("CFBundleIdentifier")!r}')
    value = info.get('CFBundleDisplayName')
    if value != EXPECTED:
        fail(f'built product CFBundleDisplayName must be exactly {EXPECTED!r}, found {value!r}')
    print(f'display-name PASS: built product {app} declares CFBundleDisplayName {value!r}')


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('--bundle', type=Path,
                        help='built FleetNotifier.app whose Info.plist is the shipped value')
    args = parser.parse_args()
    source(args.root.resolve())
    if args.bundle:
        bundle(args.bundle.resolve())
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
