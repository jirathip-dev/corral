import json, os, subprocess, sys
from pathlib import Path
assert Path.cwd() == Path('/Users/jirathip/.herdr/worktrees/corral/impl555-daemon')
assert json.loads(Path('/tmp/g555-r2-http-base-close-receipt.json').read_text())['raw_exit'] == 0
head=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
print('GATED_HEAD='+head,flush=True)
# Debug symbols/incremental caches consume scarce shared disk; assertions and
# optimization levels are unchanged. Release legs keep normal release settings.
env=dict(os.environ,CARGO_PROFILE_DEV_DEBUG='0',CARGO_PROFILE_TEST_DEBUG='0',CARGO_INCREMENTAL='0')
print('ENV CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0',flush=True)
steps=[
 ['r2-reviewed-build','python3','docs/evidence/issue-555/round2-build.py','60851ed946ae479ce08a114a058ec70b9ee7e61d','reviewed'],
 ['--timeout','120','r2-http-reviewed','python3','docs/evidence/issue-555/round2-http-load.py','record','--binary','/tmp/g555-daemon-target/r2-reviewed-corrald','--source-ref','60851ed946ae479ce08a114a058ec70b9ee7e61d','--output','/tmp/g555-r2-http-reviewed.json'],
 ['r2-focused-timing','cargo','test','--release','--lib','api::read_timing','--','--nocapture'],
 ['r2-focused-logging','cargo','test','--release','--test','read_logging','--','--nocapture'],
 ['r2-publication','cargo','test','--release','--lib','core::store::publication_tests','--','--nocapture'],
 ['r2-http','cargo','test','--release','--test','http','--','--nocapture'],
 ['r2-fmt','cargo','fmt','--all','--check'],
 ['r2-clippy','cargo','clippy','--all-targets','--','-D','warnings'],
 ['r2-workspace','cargo','test','--workspace'],
 ['r2-deny','cargo','deny','check'],
 ['r2-release','cargo','build','--release'],
 ['r2-load-build','cargo','test','--release','--test','snapshot_load','--no-run'],
 ['--hog','--budget','--timeout','120','r2-hog','cargo','test','--release','--test','snapshot_load','--','--ignored','--nocapture'],
 ['--timeout','120','r2-http-fixed','python3','docs/evidence/issue-555/round2-http-load.py','record','--binary','/tmp/g555-daemon-target/release/corrald','--source-ref',head,'--output','/tmp/g555-r2-http-fixed.json','--bounded-logs'],
 ['--timeout','30','r2-smoke','python3','docs/evidence/issue-555/daemon-smoke.py'],
]
for step in steps:
    assert subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()==head
    print('RUN',step,flush=True)
    rc=subprocess.run(['python3','docs/evidence/issue-555/daemon-run.py',*step],env=env,timeout=1250).returncode
    print('STEP_RAW_EXIT',rc,flush=True)
    if rc: sys.exit(rc)
print('GATE_SESSION_RAW_EXIT=0',flush=True)
