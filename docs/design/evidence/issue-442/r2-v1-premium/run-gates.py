#!/usr/bin/env python3
"""Saved exact commands and raw exits. No justfile; no piped gate verdicts."""
import sys
sys.dont_write_bytecode=True
from pathlib import Path
import subprocess, hashlib, json
ROOT=Path(__file__).resolve().parent
LOG=ROOT/'logs';LOG.mkdir(exist_ok=True)
exits={}
for name,args in [('build',['build.py']),('capture',['capture.py']),('browser',['probe.py']),('verify',['verify.py','--repro'])]:
    command=[sys.executable,'-B']+args
    r=subprocess.run(command,cwd=ROOT,capture_output=True,text=True)
    (LOG/(name+'.log')).write_text('COMMAND: '+' '.join(command)+'\n'+r.stdout+r.stderr+f'\nRAW_EXIT: {r.returncode}\n')
    exits[name]=r.returncode
    (LOG/'exit-codes.json').write_text(json.dumps(exits,indent=2)+'\n')
    print(name,'raw_exit=',r.returncode)
    if r.returncode:sys.exit(r.returncode)
# Manifest is generated after participating source, docs, images and logs are final.
exclude={'manifest.sha256','logs/manifest-check.log'}
files=sorted(p for p in ROOT.rglob('*') if p.is_file() and p.relative_to(ROOT).as_posix() not in exclude)
manifest=''.join(hashlib.sha256(p.read_bytes()).hexdigest()+'  '+p.relative_to(ROOT).as_posix()+'\n' for p in files)
(ROOT/'manifest.sha256').write_text(manifest)
r=subprocess.run(['shasum','-a','256','-c','manifest.sha256'],cwd=ROOT,capture_output=True,text=True)
(LOG/'manifest-check.log').write_text('COMMAND: shasum -a 256 -c manifest.sha256\n'+r.stdout+r.stderr+f'\nRAW_EXIT: {r.returncode}\n')
print('manifest independent check raw_exit=',r.returncode)
assert r.returncode==0
raw=list(p for p in ROOT.rglob('*') if p.is_file())
print(json.dumps({'manifest_entries':len(files),'manifest_png_entries':sum(p.suffix=='.png' for p in files),'raw_files':len(raw),'raw_png_count':sum(p.suffix=='.png' for p in raw),'manifest_sha256':hashlib.sha256((ROOT/'manifest.sha256').read_bytes()).hexdigest()},indent=2))
