"""Read-only provenance collection; writes only into this prototype bundle."""
from pathlib import Path
import subprocess, json, hashlib, shutil, importlib.util, os
ROOT=Path(__file__).resolve().parents[1]
# Portability/safety: source-run-only. Running this inside the promoted package
# would overwrite the retained references/ snapshots from external checkouts.
if os.environ.get('CORRAL455_GATHER_OK')!='1':
 raise SystemExit('gather.py is source-run-only (see tools/PORTABILITY-NOTES.md); set CORRAL455_GATHER_OK=1 to run it intentionally.')
REF=ROOT.parent/'442-herd-view'
SHA='86d1287a9e5c611ec6bef677c95544c53e71b982'
def run(*args): return subprocess.check_output(args,text=True)
def save(rel,data):
 p=ROOT/rel;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(data)
sources={}
for folder,name in [('UI/Herd','HerdView.swift'),('UI/Herd','RanchEnvironment.swift'),('UI/Herd','HerdModel.swift'),('UI/Herd','HerdArt.swift'),('UI','FleetViews.swift'),('UI','AppTheme.swift'),('App','AppModel.swift')]:
 path=f'ios/FleetNotifier/{folder}/{name}'
 data=run('git','-C','/Users/jirathip/Projects/corral','show',f'{SHA}:{path}')
 save('references/source/'+name,data)
 sources[path]=hashlib.sha256(data.encode()).hexdigest()
 if name=='FleetViews.swift':
  lines=data.splitlines();print('\n'.join(f'{i+1}: {lines[i]}' for start,end in [(788,805),(957,970),(1494,1535),(2451,2465),(3050,3085)] for i in range(start-1,end)))
 if name=='AppModel.swift':
  lines=data.splitlines();print('\n'.join(f'{i+1}: {line}' for i,line in enumerate(lines) if 'fleetPresentation' in line))
for n in range(455,461):save(f'references/issue-{n}.json',run('gh','issue','view',str(n),'--repo','jirathip-dev/corral','--json','number,title,body,comments'))
for src, dest in [(REF/'r2-v1-premium/art.py','references/r2/art.py'),(REF/'r2-v1-premium/fixtures.py','references/r2/fixtures.py'),(REF/'scripts/horsesvg.py','references/v1/horsesvg.py'),(REF/'scripts/fixtures.py','references/v1/fixtures.py'),(REF/'r2-v1-premium/v1-r2-day-390x844.png','references/r2-day.png'),(REF/'r2-v1-premium/v1-r2-night-390x844.png','references/r2-night.png'),(REF/'r2-v1-premium/manifest.sha256','references/r2-manifest.sha256')]:
 target=ROOT/dest;target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(src,target)
 sources[str(src.relative_to(ROOT.parent))]=hashlib.sha256(src.read_bytes()).hexdigest()
save('references/provenance.json',json.dumps({'sourceCommit':SHA,'sha256':sources},indent=2)+'\n')
save('evidence/baseline-status.txt',run('git','-C',str(ROOT.parents[1]),'status','--short'))
print('Python modules:',{x:bool(importlib.util.find_spec(x)) for x in ['playwright','PIL','websocket']})
print('Chrome:',list((Path.home()/'Library/Caches/ms-playwright').glob('chromium_headless_shell-*/chrome-headless-shell-mac-arm64/chrome-headless-shell')))
