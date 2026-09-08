#!/usr/bin/env python3
"""Assertion-level native RED/GREEN, never mutating the implementation tree.

Run under the shared native flock. Original grazing is compiled from the
pinned V1 procedure into TEMPORARY Swift paths, not loaded by the renderer.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
import export

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def old_grazing():
    source = export.source
    source.COAT = {key: ('body', 'dark') for key in source.COAT}
    source.MANE_C = {key: 'mane' for key in source.MANE_C}
    lines = ['    mutating func grazing() {', '        switch identity.mane*6+identity.accessory*2+(identity.blaze ? 1 : 0) {']
    for mane in range(3):
        for accessory in range(3):
            for blaze in range(2):
                svg = source.horse_svg(dict(coat='chestnut' if blaze else 'bay', breed='draft',
                    mane=['flowing','braided','cropped'][mane], tack='pad', accessory=['bandana','hat','none'][accessory]), 'idle')
                neck = next(node for node in ET.fromstring(svg).iter() if node.get('class') == 'neck')
                inks = export.drawing(ET.tostring(neck, encoding='unicode'), True)
                lines.append(f'        case {mane*6+accessory*2+blaze}:')
                for ink in inks:
                    assert not ink['transforms']
                    color = ink['fill'].removeprefix('#')
                    stroke = ink['stroke'].removeprefix('#')
                    if 'commands' in ink:
                        ops = ','.join('.'+['m','l','q','c','close'][int(op[0])]+('('+','.join(map(str,op[1:]))+')' if len(op)>1 else '') for op in ink['commands'])
                        part = ink['part']
                        if ink['commands'][0] == [0,120.0,75.0]: part='grazing-head'
                        if ink['commands'][0] == [0,102.0,70.0]: part='grazing-ear'
                        if color != 'none':
                            lines.append(f'            add([{ops}],"{color}",opacity:{ink["opacity"]},part:"{part}")')
                        if stroke != 'none':
                            lines.append(f'            add([{ops}],"{stroke}",opacity:{ink["opacity"]},stroke:{ink["width"]})')
                    elif 'ellipse' in ink:
                        x,y,w,h = ink['ellipse']
                        lines.append(f'            ellipse({x+w/2},{y+h/2},{w/2},{h/2},"{color}")')
                    else:
                        raise ValueError(ink)
    lines += ['        default: break', '        }', '    }']
    return '\n'.join(lines)


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--udid', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--skip-grazing', action='store_true', help='resume after separately preserved assertion RED/native-before proof')
    args=parser.parse_args(); args.output.mkdir(parents=True,exist_ok=True)
    results=[]
    with tempfile.TemporaryDirectory(prefix='corral444-native-probes-') as temporary:
        root=Path(temporary)
        shutil.copytree(ROOT/'ios',root/'ios',ignore=shutil.ignore_patterns('.build','build','__pycache__','xcuserdata'))
        (root/'tests/fixtures').mkdir(parents=True)
        for fixture in ['canonical_stream_golden.json','live_session_exchange_golden.json']:
            shutil.copy2(ROOT/'tests/fixtures'/fixture,root/'tests/fixtures'/fixture)
        app_sources={str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in (root/'ios/FleetNotifier').rglob('*.swift')}
        def run(name, command, expected=0, failure=None):
            start=time.monotonic(); log=args.output/(name+'.log')
            with log.open('w') as handle:
                result=subprocess.run(command,cwd=root,stdout=handle,stderr=subprocess.STDOUT,timeout=900,
                    env={**os.environ,'HERDR_XCODEBUILD_DIRECT':'1','HERMES_SIM_TASK_ACTIVE':'1'})
            entry={'name':name,'command':command,'cwd':str(root),'exit':result.returncode,'expected':expected,
                   'seconds':round(time.monotonic()-start,2),'log':str(log)}
            results.append(entry); (args.output/'commands.json').write_text(json.dumps(results,indent=2)+'\n')
            print(json.dumps(entry),flush=True)
            assert result.returncode==expected, name
            if failure:
                assert re.search(r'Test Case .*'+failure+r'.*failed',log.read_text()), 'must fail by XCTest assertion, not compilation'
        run('generate',['xcodegen','generate','--spec','ios/project.yml'])
        dd='/tmp/corral444-mutation-dd'
        base=['xcodebuild','test','-project','ios/FleetNotifier.xcodeproj','-scheme','FleetNotifier',
              '-destination','platform=iOS Simulator,id='+args.udid,'-derivedDataPath',dd,
              '-parallel-testing-enabled','NO','CODE_SIGNING_ALLOWED=NO']
        art=root/'ios/FleetNotifier/UI/Herd/HerdArt.swift'; pristine=art.read_text()
        start=pristine.index('    mutating func grazing() {')
        end=pristine.index('\n    }',start)+6
        if not args.skip_grazing:
            art.write_text(pristine[:start]+old_grazing()+pristine[end:])
            (args.output/'old-grazing.swift.txt').write_text(old_grazing()+'\n')
            method='testOriginalGrazingHasHorseNeckJawAndGroundedMuzzle'
            run('grazing-red',base+['-only-testing:FleetNotifierTests/HerdTests/'+method],65,method)
            run('grazing-before-native',['python3',str(HERE/'capture.py'),args.udid,dd+'/Build/Products/Debug-iphonesimulator/FleetNotifier.app',str(args.output/'old-native'),'--first-only'])
            art.write_text(pristine)
        view=root/'ios/FleetNotifier/UI/Herd/HerdView.swift'; view_pristine=view.read_text()
        victim='.onDisappear {\n            clock.stop()'
        assert view_pristine.count(victim)==1
        view.write_text(view_pristine.replace(victim,'.onDisappear {\n            // Mutation: cancellation intentionally removed'))
        method='testRemovingRealHerdViewCancelsItsAnimationSource'
        run('lifecycle-red',base+['-only-testing:FleetNotifierTests/HerdTests/'+method],65,method)
        view.write_text(view_pristine)
        victim='.frame(minWidth:156,minHeight:44).contentShape(Rectangle())'
        assert view_pristine.count(victim)==1
        view.write_text(view_pristine.replace(victim,'.frame(minWidth:156,minHeight:20).contentShape(Rectangle())'))
        method='testNativeDynamicTypeTapZonesAndSharedDestinationWiring'
        run('tap-zone-red',base+['-only-testing:FleetNotifierTests/HerdTests/'+method],65,method)
        view.write_text(view_pristine)
        for relative,digest in app_sources.items():
            assert hashlib.sha256((root/relative).read_bytes()).hexdigest()==digest,relative
        run('restored-green',base+['-only-testing:FleetNotifierTests/HerdTests','-only-testing:FleetNotifierTests/HerdEnvironmentTests'])
        (args.output/'restoration.json').write_text(json.dumps(app_sources,indent=2)+'\n')
    print('PASS: real dismissal and tap-zone assertion RED; restored GREEN' +
          ('; grazing proof preserved separately (resume)' if args.skip_grazing else '; original-V1 grazing RED + native before frame'))


if __name__=='__main__': main()
