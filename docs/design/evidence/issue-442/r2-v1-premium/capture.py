#!/usr/bin/env python3
import sys
sys.dont_write_bytecode=True
import argparse, glob, subprocess, tempfile, struct
from pathlib import Path
ROOT=Path(__file__).resolve().parent
SPECS={'day':('v1-r2-day-390x844.png',390,844),'night':('v1-r2-night-390x844.png',390,844),'study':('v1-r2-idle-anatomy-study.png',600,760)}
def capture(src,out):
    shells=sorted(glob.glob(str(Path.home()/'Library/Caches/ms-playwright/chromium_headless_shell-*/chrome-headless-shell-mac-arm64/chrome-headless-shell')))
    assert shells, 'No Chrome headless shell'
    shell=shells[-1]; print('Renderer:',shell)
    out.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='r2-capture-') as td:
        for key,(name,w,h) in SPECS.items():
            tmp=Path(td); profile=tmp/key; raw=tmp/(key+'.png')
            cmd=[shell,'--headless','--disable-gpu','--hide-scrollbars','--no-first-run',f'--user-data-dir={profile}',f'--window-size={w},{h}','--force-device-scale-factor=2','--virtual-time-budget=2000',f'--screenshot={raw}',(src/(key+'.html')).as_uri()]
            r=subprocess.run(cmd,capture_output=True,text=True); print(' '.join(cmd),'raw_exit=',r.returncode)
            assert r.returncode==0,r.stderr
            assert struct.unpack('>II',raw.read_bytes()[16:24])==(w*2,h*2)
            r=subprocess.run(['sips','-z',str(h),str(w),str(raw),'--out',str(out/name)],capture_output=True,text=True)
            print('sips',name,'raw_exit=',r.returncode); assert r.returncode==0
            assert struct.unpack('>II',(out/name).read_bytes()[16:24])==(w,h)
            print('PASS',name,w,h)
if __name__=='__main__':
    ap=argparse.ArgumentParser();ap.add_argument('--src',type=Path,default=ROOT);ap.add_argument('--out',type=Path,default=ROOT);a=ap.parse_args();capture(a.src.resolve(),a.out.resolve())
