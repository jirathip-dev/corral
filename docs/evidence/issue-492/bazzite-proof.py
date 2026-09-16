#!/usr/bin/env python3
"""Build both pinned archives and exercise the Bazzite foreground daemons.
Run only AFTER the Mac proof, with flock /tmp/n.lock. No service/install calls.
The two archives and the evidence scripts must be staged into /tmp first.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile

root = Path.home() / '.cache/g492-proof-r3'
root.mkdir(parents=True, exist_ok=False)
(root / 'tmp').mkdir()
(root / 'runtime').mkdir()
os.environ['TMPDIR'] = str(root / 'tmp')
os.environ['CARGO_BUILD_JOBS'] = '4'
(root / '.g492-owned').touch()
for label in ['fixed', 'base']:
    directory = root / label
    directory.mkdir()
    archive = Path(f'/tmp/g492-{label}-source.tar')
    with tarfile.open(archive) as stream:
        stream.extractall(directory,filter='data')
os.environ['CARGO_TARGET_DIR'] = str(root / 'fixed-target')
os.environ['RUSTUP_TOOLCHAIN'] = '1.97.1'
os.environ['RUSTUP_AUTO_INSTALL'] = '0'
os.chdir(root / 'fixed')
runner = '/tmp/g492-run-command.py'
subprocess.run(['rustc','--version'],check=True)
provenance = {label+'_archive_sha256': hashlib.sha256(Path(f'/tmp/g492-{label}-source.tar').read_bytes()).hexdigest()
              for label in ['fixed','base']}

def run(label,command,seconds=1800):
    code = subprocess.call([sys.executable,runner,'bazzite-r3-'+label,str(seconds),*command])
    if code:
        sys.exit(code)

run('release',['cargo','build','--release'])
fixed = root / 'fixed-corrald'
shutil.copy2(root / 'fixed-target/release/corrald',fixed)
run('liveness',['cargo','test','-p','corrald','--lib','g492_','--','--nocapture'])
run('logger',['cargo','test','-p','corrald','--bin','corrald','bounded_log','--','--nocapture'])
os.environ['CARGO_TARGET_DIR'] = str(root / 'base-target')
run('baseline-release',['cargo','build','--release','--manifest-path',str(root/'base/Cargo.toml')])
base = root / 'base-corrald'
shutil.copy2(root / 'base-target/release/corrald',base)
for label,path in [('fixed',fixed),('base',base)]:
    provenance[label+'_binary_sha256'] = hashlib.sha256(path.read_bytes()).hexdigest()
Path('/tmp/g492-bazzite-r3-builds.json').write_text(json.dumps(provenance,indent=2)+'\n')
run('cost',[sys.executable,'/tmp/g492-measure.py','--before',str(base),'--after',str(fixed),
            '--output','/tmp/g492-bazzite-r3-cost-data.json','--scratch-root',str(root/'runtime')],600)
print('G492_BAZZITE_PROOF_COMPLETE',flush=True)
