import hashlib, json, os, shutil, subprocess, sys, time
from pathlib import Path
root=Path('/Users/jirathip/.herdr/worktrees/corral/impl555-daemon')
assert Path.cwd()==root
session=Path('/tmp/g555-r2-gate-session-final.log')
deadline=time.monotonic()+900
while 'GATE_SESSION_WRAPPER_RAW_EXIT=' not in session.read_text():
    if time.monotonic()>=deadline: sys.exit(75)
    time.sleep(2)
assert 'GATE_SESSION_WRAPPER_RAW_EXIT=0' in session.read_text(), 'canonical gate session failed'
head=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
env=dict(os.environ,CARGO_PROFILE_DEV_DEBUG='0',CARGO_PROFILE_TEST_DEBUG='0',CARGO_INCREMENTAL='0')
base='/tmp/g555-r2-http-base-close.json'
helper='docs/evidence/issue-555/daemon-run.py'
load='docs/evidence/issue-555/round2-http-load.py'
build='docs/evidence/issue-555/round2-build.py'
target=Path('/tmp/g555-daemon-target')
fixed=target/'r2-fixed-corrald'
shutil.copyfile(target/'release/corrald',fixed)
fixed.chmod(0o755)
assert hashlib.sha256(fixed.read_bytes()).hexdigest()==json.loads(Path('/tmp/g555-r2-http-fixed.json').read_text())['binary_sha256']

def check(label,candidate,expected):
    command=['python3',load,'check',base,candidate]
    log=Path('/tmp/g555-'+label+'.log')
    with log.open('w') as out:
        out.write('COMMAND='+repr(command)+'\n'); out.flush()
        rc=subprocess.run(command,stdout=out,stderr=subprocess.STDOUT,timeout=30).returncode
        out.write('RAW_EXIT='+str(rc)+'\n')
    print(label,'RAW_EXIT',rc,flush=True)
    assert rc==expected,(label,rc)
    if expected: assert 'concurrent /snapshot regression exceeds' in log.read_text()

def run(step,expected=0,assertion=None):
    print('RUN',step,flush=True)
    rc=subprocess.run(['python3',helper,*step],env=env,timeout=1250).returncode
    print('STEP_RAW_EXIT',rc,flush=True)
    assert rc==expected,(step,rc)
    if assertion: assert assertion in Path('/tmp/g555-'+step[0]+'.log').read_text()

check('r2-fixed-bound','/tmp/g555-r2-http-fixed.json',0)
run(['r2-mutant-build','python3',build,head,'mutant'])
run(['--timeout','120','r2-http-mutant','python3',load,'record','--binary',str(target/'r2-mutant-corrald'),
     '--source-ref',head,'--output','/tmp/g555-r2-http-mutant.json','--bounded-logs'])
check('r2-mutant-bound','/tmp/g555-r2-http-mutant.json',1)
run(['r2-logging-red','python3',build,'60851ed946ae479ce08a114a058ec70b9ee7e61d','logging-red'],101,
    'per-request info is not bounded')
run(['r2-restored-logging','cargo','test','--release','--test','read_logging','--','--nocapture'])
run(['r2-restored-publication','cargo','test','--release','--lib','core::store::publication_tests','--','--nocapture'])
run(['--timeout','120','r2-http-restored','python3',load,'record','--binary',str(fixed),'--source-ref',head,
     '--output','/tmp/g555-r2-http-restored.json','--bounded-logs'])
check('r2-restored-bound','/tmp/g555-r2-http-restored.json',0)
assert subprocess.run(['git','diff','--exit-code','HEAD','--','src','tests']).returncode==0
print('PROBE_SESSION_RAW_EXIT=0; source never mutated; all owned scratch builds removed',flush=True)
