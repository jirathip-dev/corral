#!/usr/bin/env python3
"""Correct the rejected shared-target baseline without replaying passed gates.
Requires baseline-isolated-release.json and the saved, original fixed binary.
Run under flock /tmp/n.lock; preserves all rejected-attempt receipts.
"""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys

assert json.loads(Path('/tmp/g492-baseline-isolated-release.json').read_text())['raw_exit'] == 0
original = json.loads(Path('/tmp/g492-mac-builds.json').read_text())
fixed = Path('/tmp/g492-fixed-corrald')
assert hashlib.sha256(fixed.read_bytes()).hexdigest() == original['fixed_binary_sha256']
base = Path('/tmp/g492-base-isolated-corrald')
shutil.copy2('/tmp/g492-base-target/release/corrald', base)
original.update(baseline_binary_sha256=hashlib.sha256(base.read_bytes()).hexdigest(),
                baseline_target='/tmp/g492-base-target', fixed_binary=str(fixed), baseline_binary=str(base))
Path('/tmp/g492-mac-corrected-builds.json').write_text(json.dumps(original,indent=2)+'\n')
here = Path(__file__).parent
sys.exit(subprocess.call([sys.executable,str(here/'run-command.py'),'mac-cost-corrected','600',
                          sys.executable,str(here/'measure.py'),'--before',str(base),'--after',str(fixed),
                          '--output','/tmp/g492-mac-cost-corrected-data.json']))
