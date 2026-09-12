"""Rebuild and exercise the local prototype; stop on any failed gate."""
from pathlib import Path
import subprocess,sys
ROOT=Path(__file__).resolve().parents[1]
log=[]
for name in ['build.py','verify-layers.py','verify-browser.py','verify-workflows.py','verify-art-geometry.py','record-motion.py','contact-sheets.py','inspect-motion.py','check-reachability.py']:
 p=subprocess.run([sys.executable,'-B',str(ROOT/'tools'/name)],cwd=ROOT,text=True,capture_output=True)
 log.extend([f'=== {name} | exit {p.returncode} ===',p.stdout,p.stderr]);print(name,'PASS' if p.returncode==0 else 'FAIL',flush=True)
 if p.returncode:
  (ROOT/'evidence/run-log.txt').write_text('\n'.join(log));print(p.stdout[-3000:],p.stderr);sys.exit(p.returncode)
(ROOT/'evidence/run-log.txt').write_text('\n'.join(log))
subprocess.run([sys.executable,'-B',str(ROOT/'tools/package.py'),'--repro','--seal'],cwd=ROOT,check=True)
subprocess.run([sys.executable,'-B',str(ROOT/'tools/package.py'),'--verify'],cwd=ROOT,check=True)
