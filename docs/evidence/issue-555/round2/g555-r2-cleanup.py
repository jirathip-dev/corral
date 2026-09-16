import hashlib, json, shutil, subprocess
from pathlib import Path
root=Path('/Users/jirathip/.herdr/worktrees/corral/impl555-daemon')
assert Path.cwd()==root
assert 'PROBE_SESSION_WRAPPER_RAW_EXIT=75' in Path('/tmp/g555-r2-probe-admitted-session.log').read_text()
target=Path('/tmp/g555-daemon-target')
owner=(target/'.g555-owned').read_text().strip()
assert owner==str(root)
rows=subprocess.check_output(['ps','-axo','pid,args'],text=True).splitlines()
active=[r for r in rows if str(target) in r]
assert not active,active
assert subprocess.run(['git','diff','--exit-code','HEAD','--','src','tests']).returncode==0
binaries={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in target.glob('r2-*-corrald')}
receipt=dict(owner=owner,source_unchanged=True,active_target_processes=active,binary_hashes_before_cleanup=binaries,
             unfinished_verification='paired executor mutation/controls blocked by 900s admission; initial relative p99 flag retained')
shutil.rmtree(target)
assert not target.exists()
receipt.update(raw_exit=0,target_absent=True)
Path('/tmp/g555-r2-cleanup.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps(receipt,indent=2))
subprocess.run(['df','-h','/'],check=True)
