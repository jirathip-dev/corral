#!/usr/bin/env python3
"""Mac continuation after exact full gates; one heavy-lock invocation."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

here = Path(__file__).parent
runner = str(here / 'run-command.py')
release = json.loads(Path('/tmp/g492-verified-release.json').read_text())
assert release['raw_exit'] == 0 and not release['timed_out']
fixed = Path('/tmp/g492-fixed-corrald')
base = Path('/tmp/g492-base-corrald')
shutil.copy2('/tmp/g492-target/release/corrald', fixed)
provenance = dict(source_head=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
                  fixed_binary_sha256=hashlib.sha256(fixed.read_bytes()).hexdigest(),
                  baseline='92ac61d32068c882ff8e971a1470847717e12cd7')
Path('/tmp/g492-mac-builds.json').write_text(json.dumps(provenance,indent=2)+'\n')
for label, seconds, command in [
    ('mutation-battery', '1800', [sys.executable, str(here / 'mutations.py')]),
    ('baseline-release', '1800', ['cargo','build','--release','--manifest-path','/tmp/g492-base-source/Cargo.toml']),
]:
    if label == 'baseline-release':
        os.environ['CARGO_TARGET_DIR'] = '/tmp/g492-base-target'
    result = subprocess.call([sys.executable,runner,label,seconds,*command])
    if result:
        sys.exit(result)
shutil.copy2('/tmp/g492-base-target/release/corrald', base)
provenance['baseline_binary_sha256'] = hashlib.sha256(base.read_bytes()).hexdigest()
Path('/tmp/g492-mac-builds.json').write_text(json.dumps(provenance,indent=2)+'\n')
result = subprocess.call([sys.executable,runner,'mac-cost','600',sys.executable,str(here / 'measure.py'),
                          '--before',str(base),'--after',str(fixed),'--output','/tmp/g492-mac-cost-data.json'])
sys.exit(result)
