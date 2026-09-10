#!/usr/bin/env python3
"""#463 — approved Treatment-A horse app-icon packaging (source-pinned).

Materializes the byte-exact approved #462 Treatment-A masters into the
shipping asset catalog and fails closed when the catalog, the XcodeGen
source of truth, or the approval metadata drift from the pinned contract:

    primary   AppIcon    <- Bay   (nil alternateIconName restores Bay)
    alternate Palomino, Black, Grey   (exactly three; no Bay alternate)

No horse art is authored or re-rendered here. The masters are the approved
design-gate outputs (060220c32c4b5de638f4950dd8bd71fa71814816) and are copied
verbatim; the legacy Original and every Treatment B master are forbidden
shipping bytes. Stdlib only — deterministic and byte-stable across runs.

    python3 ios/tools/herd-art/app-icons.py --check
    python3 ios/tools/herd-art/app-icons.py --write
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
PNG_SIGNATURE = b'\x89PNG\r\n\x1a\n'
PNG_CHUNKS = (b'IHDR', b'IDAT', b'IEND')
APPROVAL_RELATIVE = 'ios/tools/herd-art/appicon-approval.json'
PROJECT_RELATIVE = 'ios/project.yml'


def fail(message: str) -> None:
    print(f'app-icons FAIL: {message}', file=sys.stderr)
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


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def png_info(data: bytes, label: str) -> tuple[int, int, int, int, int, tuple[bytes, ...]]:
    """Return (width, height, bit depth, color type, interlace, chunk types)."""
    if not data.startswith(PNG_SIGNATURE):
        fail(f'not a PNG: {label}')
    chunks: list[bytes] = []
    offset = len(PNG_SIGNATURE)
    while offset + 8 <= len(data):
        length = struct.unpack('>I', data[offset:offset + 4])[0]
        kind = data[offset + 4:offset + 8]
        chunks.append(kind)
        offset += 12 + length
        if kind == b'IEND':
            break
    if tuple(chunks) != PNG_CHUNKS:
        fail(f'PNG must contain exactly IHDR/IDAT/IEND (no alpha, palette or ancillary chunks): {label}')
    width, height, depth, color, compression, filtering, interlace = struct.unpack(
        '>IIBBBBB', data[16:29])
    if compression != 0 or filtering != 0:
        fail(f'unsupported PNG compression/filter method: {label}')
    return width, height, depth, color, interlace, tuple(chunks)


def contents_bytes(filename: str) -> bytes:
    """Canonical single-size iOS appiconset Contents.json (Xcode's own style)."""
    return (
        '{\n'
        '  "images" : [\n'
        '    {\n'
        f'      "filename" : "{filename}",\n'
        '      "idiom" : "universal",\n'
        '      "platform" : "ios",\n'
        '      "size" : "1024x1024"\n'
        '    }\n'
        '  ],\n'
        '  "info" : {\n'
        '    "author" : "xcode",\n'
        '    "version" : 1\n'
        '  }\n'
        '}\n'
    ).encode('utf-8')


def image_name(catalog: dict, name: str) -> str:
    return catalog['image_name'].replace('{set}', name)


def load_approval(root: Path) -> dict:
    try:
        approval = json.loads(read_text(root / APPROVAL_RELATIVE))
    except json.JSONDecodeError as error:
        fail(f'invalid approval metadata {APPROVAL_RELATIVE}: {error}')
    if approval.get('issue') != 463:
        fail('approval metadata is not the #463 contract')
    entry = approval.get('approval', {})
    if entry.get('treatment') != 'A':
        fail(f"approved treatment must be A, found {entry.get('treatment')!r}")
    if not re.fullmatch(r'[0-9a-f]{40}', str(entry.get('design_commit', ''))):
        fail('approval design commit must be a full 40-hex commit')
    catalog = approval.get('catalog', {})
    if catalog.get('primary_set') != 'AppIcon':
        fail('primary app icon set must be AppIcon')
    if catalog.get('alternate_sets') != ['Palomino', 'Black', 'Grey']:
        fail('alternate app icon sets must be exactly Palomino, Black, Grey')
    if catalog.get('nil_restores') != 'AppIcon':
        fail('nil alternateIconName must restore the primary AppIcon (Bay)')
    masters = approval.get('masters', {})
    if set(masters) != {'bay', 'palomino', 'black', 'grey'}:
        fail('master allowlist must be exactly bay, palomino, black, grey')
    sets = [masters[coat].get('set') for coat in ('bay', 'palomino', 'black', 'grey')]
    if sets != ['AppIcon', 'Palomino', 'Black', 'Grey']:
        fail('master set mapping must be bay->AppIcon and one set per alternate')
    for coat, meta in masters.items():
        if not re.fullmatch(r'[0-9a-f]{64}', str(meta.get('sha256', ''))):
            fail(f'master {coat} has no pinned SHA-256')
    forbidden = approval.get('forbidden', {})
    if not re.fullmatch(r'[0-9a-f]{64}', str(forbidden.get('legacy_original', ''))):
        fail('legacy Original forbidden hash is not pinned')
    if len(forbidden.get('treatment_b', [])) != 4:
        fail('all four Treatment B forbidden hashes must be pinned')
    for digest in [forbidden['legacy_original'], *forbidden['treatment_b']]:
        if not re.fullmatch(r'[0-9a-f]{64}', digest):
            fail(f'forbidden hash is not a SHA-256: {digest!r}')
        if digest in {meta['sha256'] for meta in masters.values()}:
            fail('an approved master hash is also listed as forbidden')
    return approval


def master_path(root: Path, approval: dict, coat: str) -> Path:
    return root / approval['catalog']['master_dir'] / f'{coat}-1024.png'


def check_masters(root: Path, approval: dict) -> dict[str, bytes]:
    master_dir = root / approval['catalog']['master_dir']
    if not master_dir.is_dir() or master_dir.is_symlink():
        fail(f'missing approved master directory: {approval["catalog"]["master_dir"]}')
    expected = {f'{coat}-1024.png' for coat in approval['masters']}
    actual = {path.name for path in master_dir.iterdir()}
    if actual != expected:
        fail(f'approved master directory must contain exactly {sorted(expected)}, found {sorted(actual)}')
    catalog = approval['catalog']
    payloads: dict[str, bytes] = {}
    for coat, meta in approval['masters'].items():
        data = read(master_path(root, approval, coat))
        digest = sha256(data)
        if digest != meta['sha256']:
            fail(f'approved master {coat} hash drift: expected {meta["sha256"]}, found {digest}')
        width, height, depth, color, interlace, _ = png_info(data, f'{coat}-1024.png')
        expected_pixels = (catalog['pixel_size'], catalog['pixel_size'],
                           catalog['bit_depth'], catalog['color_type'], catalog['interlace'])
        if (width, height, depth, color, interlace) != expected_pixels:
            fail(f'approved master {coat} must be opaque RGB {catalog["pixel_size"]}x'
                 f'{catalog["pixel_size"]}, found {width}x{height} depth {depth} '
                 f'color type {color} interlace {interlace}')
        payloads[coat] = data
    return payloads


def forbidden_hashes(approval: dict) -> dict[str, str]:
    forbidden = approval['forbidden']
    result = {'legacy Original': forbidden['legacy_original']}
    for index, digest in enumerate(forbidden['treatment_b']):
        result[f'Treatment B master {index}'] = digest
    return result


def check_catalog(root: Path, approval: dict, payloads: dict[str, bytes]) -> None:
    catalog = root / approval['catalog']['root']
    if not catalog.is_dir() or catalog.is_symlink():
        fail(f'missing shipping asset catalog: {approval["catalog"]["root"]}')
    forbidden = forbidden_hashes(approval)
    for path in sorted(catalog.rglob('*')):
        if path.is_symlink():
            fail(f'symlink not allowed in the shipping catalog: {path.relative_to(root)}')
        if path.is_file():
            digest = sha256(read(path))
            for label, digest_forbidden in forbidden.items():
                if digest == digest_forbidden:
                    fail(f'forbidden {label} artwork in the shipping catalog: '
                         f'{path.relative_to(root)}')
    expected_sets = [approval['catalog']['primary_set'], *approval['catalog']['alternate_sets']]
    found_sets = sorted(path.name for path in catalog.iterdir()
                        if path.is_dir() and path.name.endswith('.appiconset'))
    if found_sets != sorted(f'{name}.appiconset' for name in expected_sets):
        fail(f'shipping catalog appiconsets must be exactly {sorted(expected_sets)}, '
             f'found {[name[:-len(".appiconset")] for name in found_sets]}')
    stray = [path.name for path in catalog.iterdir() if path.is_file() and path.name != 'Contents.json']
    if stray:
        fail(f'unexpected files at the catalog root: {sorted(stray)}')
    name_for_coat = {meta['set']: coat for coat, meta in approval['masters'].items()}
    for name in expected_sets:
        directory = catalog / f'{name}.appiconset'
        coat = name_for_coat[name]
        image = image_name(approval['catalog'], name)
        found = sorted(path.name for path in directory.iterdir() if path.is_file())
        if found != sorted(['Contents.json', image]):
            fail(f'{name}.appiconset must contain exactly Contents.json and {image}, found {found}')
        if read(directory / image) != payloads[coat]:
            fail(f'{name}.appiconset/{image} is not the approved master bytes')
        expected_contents = contents_bytes(image)
        if read(directory / 'Contents.json') != expected_contents:
            fail(f'{name}.appiconset/Contents.json does not match the generated contract')


def check_project(root: Path, approval: dict) -> None:
    text = read_text(root / PROJECT_RELATIVE)
    alternates = ' '.join(approval['catalog']['alternate_sets'])
    expected = [
        (r'^\s*ASSETCATALOG_COMPILER_APPICON_NAME:\s*AppIcon\s*$', 'primary AppIcon selection'),
        (rf'^\s*ASSETCATALOG_COMPILER_ALTERNATE_APPICON_NAMES:\s*"{re.escape(alternates)}"\s*$',
         f'alternate app icon sets "{alternates}"'),
        (r'^\s*TARGETED_DEVICE_FAMILY:\s*"1,2"\s*$', 'iPhone+iPad device family'),
        (r'^\s*- path:\s*FleetNotifier/Assets\.xcassets\s*\n\s*buildPhase:\s*resources\s*$',
         'asset catalog resources source'),
    ]
    for pattern, label in expected:
        if len(re.findall(pattern, text, re.MULTILINE)) != 1:
            fail(f'XcodeGen source of truth {PROJECT_RELATIVE} must declare exactly one {label}')
    if re.search(r'^\s*ASSETCATALOG_COMPILER_INCLUDE_ALL_APPICON_ASSETS:\s*YES\s*$', text,
                 re.MULTILINE):
        fail('include-all-app-icons would ship appiconsets outside the approved four')


def write_catalog(root: Path, approval: dict, payloads: dict[str, bytes]) -> None:
    catalog = root / approval['catalog']['root']
    catalog.mkdir(parents=True, exist_ok=True)
    allowed = {f'{approval["catalog"]["primary_set"]}.appiconset',
               *[f'{name}.appiconset' for name in approval['catalog']['alternate_sets']]}
    for path in list(catalog.iterdir()):
        if path.is_dir() and path.name.endswith('.appiconset') and path.name not in allowed:
            shutil.rmtree(path)
    written = []
    for coat, meta in approval['masters'].items():
        name = meta['set']
        directory = catalog / f'{name}.appiconset'
        if directory.exists():
            shutil.rmtree(directory)
        directory.mkdir()
        image = image_name(approval['catalog'], name)
        (directory / image).write_bytes(payloads[coat])
        (directory / 'Contents.json').write_bytes(contents_bytes(image))
        written.append(name)
    print(f'app-icons WRITE: {len(written)} appiconsets materialized '
          f'({", ".join(sorted(written))}); Bay is the primary, nil restores AppIcon')


def verify_source(root: Path, approval: dict) -> None:
    canonical = approval['canonical_source']
    source_hash = sha256(read(root / canonical['path']))
    if source_hash != canonical['sha256']:
        fail(f'canonical horse source hash drift: {canonical["path"]} expected '
             f'{canonical["sha256"]}, found {source_hash}')


def check(root: Path) -> None:
    approval = load_approval(root)
    verify_source(root, approval)
    payloads = check_masters(root, approval)
    check_catalog(root, approval, payloads)
    check_project(root, approval)
    alternates = approval['catalog']['alternate_sets']
    print(f'app-icons PASS: primary AppIcon=bay; alternates exactly {alternates}; '
          f'4 opaque RGB 1024x1024 masters pinned; no legacy Original/Treatment-B bytes')


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--write', action='store_true',
                      help='materialize the approved masters into the asset catalog')
    mode.add_argument('--check', action='store_true',
                      help='verify the catalog, masters and project wiring (default)')
    args = parser.parse_args()
    root = args.root.resolve()
    if args.write:
        approval = load_approval(root)
        verify_source(root, approval)
        write_catalog(root, approval, check_masters(root, approval))
    else:
        check(root)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
