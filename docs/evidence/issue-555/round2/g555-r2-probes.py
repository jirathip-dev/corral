"""One paired-window diagnostic; retain the earlier failed split-window check.
Hypothesis: compilation/hogs/host changes between baseline and fixed distorted
the comparison. Do not change the harness or its 2x+1/2ms bound. Compile before
the measurement window; record base/fixed/mutant/fixed under ONE held lock.
Stop after this experiment, including if a fixed control still flags.
"""
import hashlib, json, os, subprocess, sys
from pathlib import Path
root=Path('/Users/jirathip/.herdr/worktrees/corral/impl555-daemon')
assert Path.cwd()==root
assert 'GATE_SESSION_WRAPPER_RAW_EXIT=0' in Path('/tmp/g555-r2-gate-session-final.log').read_text()
head=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
env=dict(os.environ,CARGO_PROFILE_DEV_DEBUG='0',CARGO_PROFILE_TEST_DEBUG='0',CARGO_INCREMENTAL='0')
base='/tmp/g555-r2-http-paired-base.json'
helper='docs/evidence/issue-555/daemon-run.py'
load='docs/evidence/issue-555/round2-http-load.py'
build='docs/evidence/issue-555/round2-build.py'
target=Path('/tmp/g555-daemon-target')
fixed=target/'r2-fixed-corrald'
assert hashlib.sha256(fixed.read_bytes()).hexdigest()==json.loads(Path('/tmp/g555-r2-http-fixed.json').read_text())['binary_sha256']
locked=len(sys.argv)>1 and sys.argv[1]=='matrix'

def check(label,candidate):
    command=['python3',load,'check',base,candidate]
    log=Path('/tmp/g555-'+label+'.log')
    with log.open('w') as out:
        out.write('COMMAND='+repr(command)+'\n'); out.flush()
        rc=subprocess.run(command,stdout=out,stderr=subprocess.STDOUT,timeout=30).returncode
        out.write('RAW_EXIT='+str(rc)+'\n')
    print(label,'RAW_EXIT',rc,flush=True)
    if rc: assert 'concurrent /snapshot regression exceeds' in log.read_text()
    return rc

def run(step,expected=0,assertion=None):
    print('RUN',step,flush=True)
    rc=subprocess.run(['python3',helper,*(['--locked'] if locked else []),*step],env=env,timeout=1250).returncode
    print('STEP_RAW_EXIT',rc,flush=True)
    assert rc==expected,(step,rc)
    if assertion: assert assertion in Path('/tmp/g555-'+step[0]+'.log').read_text()

if not locked:
    run(['r2-mutant-build','python3',build,head,'mutant'])
    run(['r2-logging-red','python3',build,'60851ed946ae479ce08a114a058ec70b9ee7e61d','logging-red'],101,
        'per-request info is not bounded')
    run(['r2-restored-logging','cargo','test','--release','--test','read_logging','--','--nocapture'])
    run(['r2-restored-publication','cargo','test','--release','--lib','core::store::publication_tests','--','--nocapture'])
    sys.exit(subprocess.run(['flock','/tmp/n.lock',sys.executable,__file__,'matrix'],timeout=900).returncode)

for label,binary,ref,bounded in [
    ('r2-http-paired-base',target/'r2-base-corrald','11cc1d26f3b1c22716d4527fb9e7679eaa861aba',False),
    ('r2-http-paired-fixed',fixed,head,True),
    ('r2-http-mutant',target/'r2-mutant-corrald',head,True),
    ('r2-http-restored',fixed,head,True),
]:
    run(['--timeout','120',label,'python3',load,'record','--binary',str(binary),'--source-ref',ref,
         '--output','/tmp/g555-'+label+'.json',*(['--bounded-logs'] if bounded else [])])
verdicts=[check('r2-paired-fixed-bound','/tmp/g555-r2-http-paired-fixed.json'),
          check('r2-mutant-bound','/tmp/g555-r2-http-mutant.json'),
          check('r2-restored-bound','/tmp/g555-r2-http-restored.json')]
assert subprocess.run(['git','diff','--exit-code','HEAD','--','src','tests']).returncode==0
print('PAIRED_VERDICTS',verdicts,'(expected [0,1,0]); source never mutated',flush=True)
assert verdicts==[0,1,0], 'paired experiment flags persist; stop, do not tune/retry'
print('PROBE_SESSION_RAW_EXIT=0; all owned scratch builds removed',flush=True)
