#!/usr/bin/env python3
"""Prepare the pinned pre-fix binary with two debug observations only."""
import hashlib
import json
from pathlib import Path
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[3]
BASE = 'a42b83ab3e50aa9553076971f39176711f20cb76'
directory = Path('/tmp/g560-base-source')
directory.mkdir()
(directory / '.g560-owned').write_text(str(ROOT)+'\n')
archive = directory / 'source.tar'
with archive.open('wb') as out:
    subprocess.run(['git','archive',BASE,'Cargo.toml','Cargo.lock','rust-toolchain.toml',
                    'src','crates','tests'], cwd=ROOT, stdout=out, check=True, timeout=30)
with tarfile.open(archive) as files:
    files.extractall(directory, filter='data')
archive.unlink()
path = directory / 'src/core/store/published.rs'
old = path.read_text()
anchor = '        let body = wire_bytes(epoch, snapshot);'
assert old.count(anchor) == 1
new = old.replace(anchor, '        tracing::debug!(agents = snapshot.agents.len(), "snapshot agents encoded");\n'+anchor+
                  '\n        tracing::debug!("snapshot published");')
path.write_text(new)
receipt = dict(base=BASE, source=str(directory), instrumentation_file='src/core/store/published.rs',
               original_sha256=hashlib.sha256(old.encode()).hexdigest(),
               instrumented_sha256=hashlib.sha256(new.encode()).hexdigest(),
               change='Only debug observations around the existing full-snapshot encoder; no control-flow change.')
(ROOT/'docs/evidence/issue-560/base-source.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps(receipt))
