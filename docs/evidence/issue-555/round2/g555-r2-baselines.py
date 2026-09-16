import subprocess, sys, time
from pathlib import Path
root = Path('/Users/jirathip/.herdr/worktrees/corral/impl555-daemon')
assert Path.cwd() == root
deadline = time.monotonic() + 900
previous = None
while True:
    heavy = [r for r in subprocess.check_output(['ps','-axo','pid,comm'], text=True).splitlines()
             if len(r.split()) > 1 and Path(r.split()[1]).name in ('cargo','rustc','xcodebuild','swift-frontend')]
    if heavy != previous:
        print('ADMISSION', heavy, flush=True)
        previous = heavy
    if not heavy:
        break
    if time.monotonic() >= deadline:
        print('ADMISSION_RAW_EXIT=75 deadline=900s', flush=True)
        sys.exit(75)
    time.sleep(10)
steps = [
 ['r2-base-build','python3','docs/evidence/issue-555/round2-build.py','11cc1d26f3b1c22716d4527fb9e7679eaa861aba','base'],
 ['--timeout','120','r2-http-base','python3','docs/evidence/issue-555/round2-http-load.py','record','--binary','/tmp/g555-daemon-target/r2-base-corrald','--source-ref','11cc1d26f3b1c22716d4527fb9e7679eaa861aba','--output','/tmp/g555-r2-http-base.json'],
 ['r2-reviewed-build','python3','docs/evidence/issue-555/round2-build.py','60851ed946ae479ce08a114a058ec70b9ee7e61d','reviewed'],
 ['--timeout','120','r2-http-reviewed','python3','docs/evidence/issue-555/round2-http-load.py','record','--binary','/tmp/g555-daemon-target/r2-reviewed-corrald','--source-ref','60851ed946ae479ce08a114a058ec70b9ee7e61d','--output','/tmp/g555-r2-http-reviewed.json'],
]
for step in steps:
    rc = subprocess.run(['python3','docs/evidence/issue-555/daemon-run.py',*step], timeout=1250).returncode
    print('STEP_RAW_EXIT', step, rc, flush=True)
    if rc: sys.exit(rc)
