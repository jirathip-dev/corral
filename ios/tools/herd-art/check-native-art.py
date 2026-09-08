#!/usr/bin/env python3
"""#444 correction 1: fail-closed native-art source and actual app gate.

Existing icon inputs are pinned to the dispatch base; no new artwork asset is
allowed anywhere in the app target. The Herd renderer cannot load bitmap/file/
network artwork at all. Built products are inspected independently, including
compiled asset-catalog rendition names (assetutil), not inferred from sources.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import stat
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
EXPECTED = ('HerdArt.swift','HerdModel.swift','HerdView.swift','RanchEnvironment.swift','HerdEnvironmentChoice.swift')
ART_EXTENSIONS = {'.png','.jpg','.jpeg','.webp','.svg','.pdf','.gif','.heic','.tiff','.bmp','.ktx','.atlas','.scn','.sks'}
FORBIDDEN = re.compile(r'\b(?:Image|UIImage|CGImage\w*|CIImage|NSImage|SKTexture\w*|MTKTextureLoader|TextureResource|WKWebView|UIWebView|WebView|Bundle|FileManager|URLSession)\b|\b(?:typealias\s+\w+\s*=\s*)?(?:SwiftUI\s*\.\s*)?Image\b|\b(?:ComfyUI|Comfy|WebKit|base64Encoded|base64|contentsOfFile|contentsOfURL)\b|r2-v1-premium|issue-442/|<\s*(?:html|svg|img)\b', re.I)


def read(path):
    if not path.is_file() or path.is_symlink() or not path.stat().st_mode & (stat.S_IRUSR|stat.S_IRGRP|stat.S_IROTH):
        raise ValueError(f'missing/unreadable/nonregular file: {path}')
    return path.read_bytes()


def files(root):
    if not root.is_dir() or root.is_symlink() or not root.stat().st_mode & stat.S_IRUSR:
        raise ValueError(f'missing/unreadable expected root: {root}')
    result = []
    def fail(error): raise error
    for parent, directories, names in os.walk(root, onerror=fail):
        for name in directories+names:
            if (Path(parent)/name).is_symlink(): raise ValueError(f'symlink not allowed: {parent}/{name}')
        result += [Path(parent)/name for name in names]
    if not result: raise ValueError(f'empty expected root: {root}')
    return result


def source(root):
    app = root/'ios/FleetNotifier'
    renderer = app/'UI/Herd'
    paths = files(renderer)
    for expected in EXPECTED: read(renderer/expected)
    for path in paths:
        if path.suffix != '.swift': raise ValueError(f'non-Swift renderer input: {path}')
        text = read(path).decode('utf-8').replace('`','')
        # Reject names irrespective of whitespace, module qualification or
        # .init spelling; no Image API (including aliasing) belongs here.
        if match := FORBIDDEN.search(text): raise ValueError(f'forbidden renderer reference {match[0]}: {path}')
    allowed = json.loads(read(root/'ios/tools/herd-art/resource-allowlist.json'))
    observed = {}
    for path in files(app):
        if path.suffix == '.swift':
            text = read(path).decode('utf-8').replace('`','')
            for call in re.finditer(r'\bImage\s*(?:\.\s*init\s*)?\(',text):
                if not re.match(r'\s*systemName\s*:',text[call.end():]):
                    raise ValueError(f'non-symbol image loader in app source: {path}')
            if re.search(r'\b(?:UIImage|CGImageSource\w*|SKTexture\w*|WKWebView|UIWebView|WebKit)\b|=\s*(?:SwiftUI\s*\.\s*)?Image\b',text):
                raise ValueError(f'bitmap/web helper outside renderer: {path}')
        if path.suffix.lower() in ART_EXTENSIONS:
            key = str(path.relative_to(root))
            observed[key] = hashlib.sha256(read(path)).hexdigest()
            if observed[key] != allowed['files'].get(key): raise ValueError(f'non-approved app artwork: {key}')
    if observed != allowed['files']: raise ValueError('missing approved icon input or changed artwork inventory')
    print(f'source PASS: {len(paths)} native Swift renderer files; {len(observed)} unchanged approved icon inputs')
    return allowed


def bundle(app, allowed):
    paths = files(app)
    info = plistlib.loads(read(app/'Info.plist'))
    if info.get('CFBundleIdentifier') != 'com.corral.fleetnotifier': raise ValueError('not the actual Corral app product')
    executable = app/info['CFBundleExecutable']
    binary = read(executable)
    if binary[:4] not in (b'\xcf\xfa\xed\xfe',b'\xca\xfe\xba\xbe',b'\xfe\xed\xfa\xcf'):
        raise ValueError('app executable is not Mach-O')
    for path in paths:
        relative = str(path.relative_to(app))
        data = read(path)
        suffix = path.suffix.lower()
        raster = data.startswith((b'\x89PNG',b'\xff\xd8\xff',b'GIF8')) or data[:4] == b'RIFF'
        if suffix in ART_EXTENSIONS or raster:
            if not re.fullmatch(r'AppIcon\d+x\d+(?:@\dx)?(?:~ipad)?\.png',relative):
                raise ValueError(f'forbidden built artwork: {relative}')
        if any(token in relative.lower() for token in ('issue-442','r2-v1-premium','horse-sheet','ranch-day','ranch-night')):
            raise ValueError(f'forbidden export in product: {relative}')
    catalog = app/'Assets.car'; read(catalog)
    output = subprocess.check_output(['xcrun','assetutil','--info',str(catalog)],text=True,timeout=30)
    items = json.loads(output)
    names = {item['Name'] for item in items if 'Name' in item}
    if not names or not names <= set(allowed['catalog_names']):
        raise ValueError(f'non-approved compiled artwork names: {sorted(names-set(allowed["catalog_names"]))}')
    print(f'bundle PASS: Mach-O {executable}; {len(paths)} product files; catalog names {sorted(names)}')


def main():
    parser = argparse.ArgumentParser(); parser.add_argument('--root',type=Path,default=ROOT)
    parser.add_argument('--bundle',type=Path)
    args = parser.parse_args()
    try:
        allowed = source(args.root)
        if args.bundle: bundle(args.bundle,allowed)
    except (ValueError,OSError,UnicodeError,KeyError,subprocess.SubprocessError) as error:
        print(f'native-art FAIL: {error}',file=sys.stderr); return 1
    return 0


if __name__ == '__main__': raise SystemExit(main())
