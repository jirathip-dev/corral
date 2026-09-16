import subprocess, sys, time
from pathlib import Path
assert Path.cwd()==Path('/Users/jirathip/.herdr/worktrees/corral/impl555-daemon')
deadline=time.monotonic()+900
previous=None
while True:
    rows=subprocess.check_output(['ps','-axo','pid,comm'],text=True).splitlines()
    heavy=[r.strip() for r in rows if len(r.split())>1 and Path(r.split()[1]).name in
           ('cargo','rustc','xcodebuild','swift-frontend')]
    if not heavy:
        print('PAIRED_ADMISSION_RAW_EXIT=0',flush=True)
        break
    if heavy!=previous:
        print('WAIT',heavy,flush=True)
        previous=heavy
    if time.monotonic()>=deadline:
        print('PAIRED_ADMISSION_RAW_EXIT=75 (900s deadline; no experiment started)',flush=True)
        sys.exit(75)
    time.sleep(5)
sys.exit(subprocess.run([sys.executable,'/tmp/g555-r2-probes.py'],timeout=1800).returncode)
