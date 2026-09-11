#!/usr/bin/env python3
"""#444 correction 1 / #463: fail-closed native-art source and product gate.

Existing icon inputs are pinned to the dispatch base; no new artwork asset is
allowed anywhere in the app target. The Herd renderer cannot load bitmap/file/
network artwork at all. Built products are inspected independently, including
compiled asset-catalog rendition names (assetutil) and the built CFBundleIcons
declarations, not inferred from sources.

#463 narrows the approved shipping inventory to the four Treatment-A horse
app icons: Bay primary (AppIcon) and exactly Palomino, Black and Grey alternates.
The legacy Original and every Treatment B master are forbidden product bytes.

#464 adds the Settings App Icon picker, which previews the SHIPPED catalog
art through four generated loadable imagesets (BayPreview, PalominoPreview,
BlackPreview, GreyPreview): app source may contain bare literal `Image("Name")`
loaders for exactly those four preview names, and nothing else.
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
APPROVAL = 'ios/tools/herd-art/appicon-approval.json'


def read(path):
    if not path.is_file() or path.is_symlink() or not path.stat().st_mode & (stat.S_IRUSR|stat.S_IRGRP|stat.S_IROTH):
        raise ValueError(f'missing/unreadable/nonregular file: {path}')
    return path.read_bytes()


def files(root):
    if not root.is_dir() or root.is_symlink() or not root.stat().st_mode & (stat.S_IRUSR):
        raise ValueError(f'missing/unreadable expected root: {root}')
    result = []
    def fail(error): raise error
    for parent, directories, names in os.walk(root, onerror=fail):
        for name in directories+names:
            if (Path(parent)/name).is_symlink(): raise ValueError(f'symlink not allowed: {parent}/{name}')
        result += [Path(parent)/name for name in names]
    if not result: raise ValueError(f'empty expected root: {root}')
    return result


def approval(root):
    contract = json.loads(read(root/APPROVAL))
    primary = contract['catalog']['primary_set']
    alternates = contract['catalog']['alternate_sets']
    if primary != 'AppIcon' or alternates != ['Palomino', 'Black', 'Grey']:
        raise ValueError('approval metadata no longer matches the approved four-icon contract')
    if 'Bay' in alternates or primary in alternates:
        raise ValueError('Bay must be the primary only, never an alternate')
    # #464: the Settings picker previews the four approved masters through
    # regular imagesets (appiconset renditions are not loadable in-app on
    # iOS 18+); the app-source loader allowlist is exactly these names.
    previews = contract['catalog'].get('preview_sets')
    if previews != ['BayPreview', 'PalominoPreview', 'BlackPreview', 'GreyPreview']:
        raise ValueError('#464 loadable preview imagesets must be exactly the four approved names')
    if set(previews) & {primary, *alternates}:
        raise ValueError('preview imagesets must not reuse an appiconset name')
    master_previews = [contract['masters'][coat].get('preview_set')
                       for coat in ('bay', 'palomino', 'black', 'grey')]
    if master_previews != previews:
        raise ValueError('master preview_set mapping must be one loadable imageset per approved coat')
    forbidden = {contract['forbidden']['legacy_original'], *contract['forbidden']['treatment_b']}
    if len(forbidden) != 5:
        raise ValueError('legacy Original and all four Treatment B hashes must stay forbidden')
    return primary, alternates, previews, forbidden


def source(root):
    app = root/'ios/FleetNotifier'
    renderer = app/'UI/Herd'
    # #464: the Settings App Icon picker previews the SHIPPED #463 catalog
    # art through the four generated imagesets, so those rendition names are
    # the exact allowlist for non-symbol loaders. Everything else stays
    # forbidden.
    primary, alternates, previews, _ = approval(root)
    picker_renditions = set(previews)
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
                tail = text[call.end():]
                if re.match(r'\s*systemName\s*:',tail): continue
                # #464: the ONLY permitted non-symbol loader is a bare,
                # unqualified literal `Image("Name")` naming an approved
                # rendition; qualified/.init/aliased or unknown spellings RED.
                literal = re.match(r'"([^"]*)"\)',tail)
                if (call.group(0) == 'Image('
                        and not (call.start() and text[call.start()-1] in '.:')
                        and literal is not None
                        and literal.group(1) in picker_renditions):
                    continue
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


def bundle(app, allowed, primary, alternates, previews, forbidden):
    names = {primary, *alternates}
    icon_png = re.compile(rf'(?:{"|".join(re.escape(name) for name in sorted(names))})\d+x\d+(?:@\dx)?(?:~ipad)?\.png')
    paths = files(app)
    info = plistlib.loads(read(app/'Info.plist'))
    if info.get('CFBundleIdentifier') != 'com.corral.fleetnotifier': raise ValueError('not the actual Corral app product')
    executable = app/info['CFBundleExecutable']
    binary = read(executable)
    if binary[:4] not in (b'\xcf\xfa\xed\xfe',b'\xca\xfe\xba\xbe',b'\xfe\xed\xfa\xcf'):
        raise ValueError('app executable is not Mach-O')
    loose_primary = 0
    for path in paths:
        relative = str(path.relative_to(app))
        data = read(path)
        if hashlib.sha256(data).hexdigest() in forbidden:
            raise ValueError(f'forbidden legacy Original/Treatment-B artwork in product: {relative}')
        suffix = path.suffix.lower()
        raster = data.startswith((b'\x89PNG',b'\xff\xd8\xff',b'GIF8')) or data[:4] == b'RIFF'
        if suffix in ART_EXTENSIONS or raster:
            if not icon_png.fullmatch(relative):
                raise ValueError(f'forbidden built artwork: {relative}')
            if relative.startswith(primary): loose_primary += 1
        if any(token in relative.lower() for token in ('issue-442','r2-v1-premium','horse-sheet','ranch-day','ranch-night','treatment-b')):
            raise ValueError(f'forbidden export in product: {relative}')
    if not loose_primary: raise ValueError('product has no compiled primary app-icon PNG')
    for key in ('CFBundleIcons','CFBundleIcons~ipad'):
        declared = info.get(key)
        if not isinstance(declared, dict): raise ValueError(f'built Info.plist has no {key}')
        if declared.get('CFBundlePrimaryIcon',{}).get('CFBundleIconName') != primary:
            raise ValueError(f'{key} primary icon must be {primary}')
        alternates_declared = declared.get('CFBundleAlternateIcons')
        if not isinstance(alternates_declared, dict) or set(alternates_declared) != set(alternates):
            raise ValueError(f'{key} alternate icons must be exactly {sorted(alternates)}, found {sorted(alternates_declared or {})}')
    catalog = app/'Assets.car'; read(catalog)
    preview_names = set(allowed.get('preview_catalog_names', []))
    if preview_names != set(previews):
        raise ValueError('#464 preview rendition allowlist must match the approval contract')
    expected_compiled = set(allowed['catalog_names']) | preview_names
    output = subprocess.check_output(['xcrun','assetutil','--info',str(catalog)],text=True,timeout=30)
    items = json.loads(output)
    compiled = {item['Name'] for item in items if 'Name' in item}
    if compiled != expected_compiled:
        raise ValueError(f'compiled catalog names must be exactly {sorted(expected_compiled)}, found {sorted(compiled)}')
    print(f'bundle PASS: Mach-O {executable}; {len(paths)} product files; catalog names {sorted(compiled)}')


def main():
    parser = argparse.ArgumentParser(); parser.add_argument('--root',type=Path,default=ROOT)
    parser.add_argument('--bundle',type=Path)
    args = parser.parse_args()
    try:
        allowed = source(args.root)
        if args.bundle: bundle(args.bundle,allowed,*approval(args.root))
    except (ValueError,OSError,UnicodeError,KeyError,json.JSONDecodeError,subprocess.SubprocessError) as error:
        print(f'native-art FAIL: {error}',file=sys.stderr); return 1
    return 0


if __name__ == '__main__': raise SystemExit(main())
