#!/usr/bin/env python3
"""Capture the real DEBUG scene's bounded marker sequence on a booted phone."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import time


def call(*args):
    return subprocess.check_output(['xcrun','simctl',*map(str,args)],stderr=subprocess.STDOUT,text=True,timeout=60).strip()


def main():
    parser=argparse.ArgumentParser(); parser.add_argument('udid'); parser.add_argument('app',type=Path); parser.add_argument('output',type=Path)
    parser.add_argument('--offline',action='store_true')
    parser.add_argument('--first-only',action='store_true',help='single native old-grazing comparison frame')
    args=parser.parse_args(); args.output.mkdir(parents=True,exist_ok=True)
    devices=json.loads(call('list','devices','--json'))
    device=next(d for ds in devices['devices'].values() for d in ds if d['udid']==args.udid)
    if device['state']=='Shutdown':
        call('boot',args.udid)
        call('bootstatus',args.udid,'-b')
        devices=json.loads(call('list','devices','booted','--json'))
        device=next(d for ds in devices['devices'].values() for d in ds if d['udid']==args.udid)
    assert device['state']=='Booted'
    call('install',args.udid,args.app)
    bundle='com.corral.fleetnotifier'
    subprocess.run(['xcrun','simctl','terminate',args.udid,bundle],capture_output=True,timeout=30)
    container=Path(call('get_app_container',args.udid,bundle,'data'))
    markers=container/'Documents/herd-evidence'
    if markers.exists(): shutil.rmtree(markers)
    flags=['-corralHerdEvidence']+(['-corralHerdOffline'] if args.offline else [])
    print(call('launch',args.udid,bundle,*flags),flush=True)
    deadline=time.monotonic()+65; captured={}
    while time.monotonic()<deadline:
        for marker in sorted(markers.glob('*.json')):
            if marker.stem in captured: continue
            info=json.loads(marker.read_bytes())
            png=args.output/(marker.stem+'.png')
            call('io',args.udid,'screenshot',png)
            data=png.read_bytes(); width,height=struct.unpack('>II',data[16:24])
            captured[marker.stem]={'marker':info,'pixels':[width,height],'sha256':hashlib.sha256(data).hexdigest()}
            shutil.copy2(marker,args.output/marker.name)
            print(marker.stem,width,height,info['sceneID'],info['clockRunning'],flush=True)
        if 'dismissed-stable' in captured or (args.first_only and '01-day-grazing' in captured): break
        time.sleep(0.1)
    (args.output/'capture.json').write_text(json.dumps({'device':device,'frames':captured},indent=2)+'\n')
    if args.first_only:
        assert len(captured)==1
        call('terminate',args.udid,bundle)
        print('PASS: single native old-grazing comparison frame')
        return
    assert len(captured)==9, f'expected 9 runtime markers, observed {len(captured)}'
    day=captured['01-day-grazing']['marker']; night=captured['02-night-same-scene']['marker']
    assert day['sceneID']==night['sceneID'] and day['horseIDs']==night['horseIDs'] and day['paddock']==night['paddock']
    assert not day['night'] and night['night']
    for key in ['repositoryFilter','hostFilter','selectedAgent','scroll']:
        assert day.get(key)==night.get(key),key
    next_page=captured['03-parallax-next']['marker']
    assert next_page['scroll']>0, 'real scroll geometry did not advance'
    ratios={'sky':.05,'hills':.12,'barnTrees':.22,'ground':.4,'rearFences':.4,'foreground':.65,'horses':1,'frontRail':0}
    for plane,ratio in ratios.items():
        if args.offline and plane != 'horses': ratio=0
        assert abs(next_page['planeOffsets'][plane]+next_page['scroll']*ratio)<0.01,plane
    reduced=captured['04-reduce-motion']['marker']
    assert all(value==0 for plane,value in reduced['planeOffsets'].items() if plane!='horses')
    assert reduced['planeOffsets']['horses']==-reduced['scroll'], 'direct non-animated navigation remains available'
    assert not captured['dismissed']['marker']['clockRunning']
    assert captured['dismissed-stable']['marker']['ticks']==captured['dismissed']['marker']['ticks']
    assert not captured['04-reduce-motion']['marker']['clockRunning']
    assert captured['07-clock-running']['marker']['clockRunning'] != args.offline
    assert len(captured['06-dense-blocked']['marker']['horseIDs'])==60
    assert len(day['horseIDs'])==12
    print('PASS: same native scene Day/Night, nine runtime frames, independent parallax, stopped Reduce Motion/dismissal clocks')


if __name__=='__main__': main()
