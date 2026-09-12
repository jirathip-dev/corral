"""Seal/verify the complete local prototype package, with no repo writes."""
import sys
sys.dont_write_bytecode=True
from pathlib import Path
import argparse,hashlib,json,re,subprocess,shutil,xml.etree.ElementTree as ET
from PIL import Image
ROOT=Path(__file__).resolve().parents[1];MANIFEST=ROOT/'manifest.sha256'
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def files():return sorted(p for p in ROOT.rglob('*') if p.is_file() and p!=MANIFEST)
def generated():return sorted([ROOT/'variant-a.html',ROOT/'variant-b.html',ROOT/'fixtures.json',*list((ROOT/'assets').rglob('*.svg'))])
def validate():
 bad=[str(p.relative_to(ROOT)) for p in ROOT.rglob('*') if p.name in ['__pycache__','.DS_Store','.runtime'] or p.suffix in ['.pyc','.pyo','.swp','.tmp','.bak']]
 assert not bad,('Transient artifacts',bad)
 report=json.loads((ROOT/'evidence/verification.json').read_text());assert report['status']=='PASS' and all(c['pass'] for c in report['checks'])
 workflow=json.loads((ROOT/'evidence/workflow-verification.json').read_text());assert workflow['status']=='PASS' and all(x['pass'] for x in workflow['checks'])
 art=json.loads((ROOT/'evidence/art-geometry.json').read_text());assert all(x['paintAboveCaption'] and x['paintBelowHeader'] and x['paintInsideScrollViewport'] for x in art)
 reach=json.loads((ROOT/'evidence/reachability.json').read_text());assert all(x.get('status')==200 and x['matchesLocal'] for x in reach['targets'])
 for x in reach['targets']:
  rel=x['url'].split('/corral/455-immersive-herd/',1)[1];assert sha(ROOT/rel)==x['sha256']
 for s in report['screenshots']:
  p=ROOT/s['path'];assert sha(p)==s['sha256'],p
  with Image.open(p) as im:assert list(im.size)==s['size']
  m=re.search(r'-(\d+)x(\d+)\.png$',p.name)
  if m:
   with Image.open(p) as im:assert im.size==(int(m[1]),int(m[2])),p
 layers=json.loads((ROOT/'evidence/layer-verification.json').read_text());assert len(layers)==2 and all(x['pixelEqual'] for x in layers)
 for env in ['day','night']:
  actual={p.name for p in (ROOT/f'assets/layers/{env}').glob('*.svg')};assert actual=={'00-sky.svg','10-hills.svg','20-ground.svg','30-barn-trees.svg','40-rear-fences.svg','50-foreground.svg'}
  assert ET.parse(ROOT/f'assets/ranch-{env}.svg').getroot().get('preserveAspectRatio')=='xMidYMid slice'
 for p in (ROOT/'assets/horses').glob('*.svg'):
  root=ET.parse(p).getroot();assert root.get('viewBox')=='0 0 132 100';assert not list(root.iter('{http://www.w3.org/2000/svg}image')),'Horse art must remain V1 vector'
 motion=json.loads((ROOT/'evidence/motion-verification.json').read_text());assert len(motion['clips'])==2 and not motion['timeCompression']
 for clip in motion['clips']:
  assert sha(ROOT/clip['file'])==clip['sha256'];assert abs(clip['duration']-clip['wallSeconds'])<.15;assert clip['clockEnd']['ticks']>clip['clockStart']['ticks'];assert clip['ffprobe']['streams'][0]['width']==390 and clip['ffprobe']['streams'][0]['height']==844
 fixtures=json.loads((ROOT/'fixtures.json').read_text());assert len(fixtures)==12 and len({x['id'] for x in fixtures})==12
 # Snapshot integrity: exact pinned source objects, independent of checked-out HEAD.
 # Portability: in this promoted package the accepted #442 reference artifacts
 # are retained inside references/ (source paths recorded in provenance.json).
 PACKAGED_REFERENCE_MAP={
  '442-herd-view/r2-v1-premium/art.py':'references/r2/art.py',
  '442-herd-view/r2-v1-premium/fixtures.py':'references/r2/fixtures.py',
  '442-herd-view/r2-v1-premium/v1-r2-day-390x844.png':'references/r2-day.png',
  '442-herd-view/r2-v1-premium/v1-r2-night-390x844.png':'references/r2-night.png',
  '442-herd-view/r2-v1-premium/manifest.sha256':'references/r2-manifest.sha256',
  '442-herd-view/scripts/fixtures.py':'references/v1/fixtures.py',
  '442-herd-view/scripts/horsesvg.py':'references/v1/horsesvg.py',
 }
 provenance=json.loads((ROOT/'references/provenance.json').read_text())
 for source,digest in provenance['sha256'].items():
  if source.startswith('ios/'):
   p=ROOT/'references/source'/Path(source).name
  else:
   p=ROOT/PACKAGED_REFERENCE_MAP[source]
  assert sha(p)==digest,('Protected accepted reference changed',p)
 for path in [ROOT/'variant-a.html',ROOT/'variant-b.html',ROOT/'fixtures.json']:
  text=path.read_text()
  for forbidden in ['jirathip@','macbook-air','Bazzite','ghp_','sk-proj-','tailscale serve','/Users/']:
   assert forbidden not in text,(path,forbidden)
 return report

def main():
 parser=argparse.ArgumentParser();parser.add_argument('--repro',action='store_true');parser.add_argument('--seal',action='store_true');parser.add_argument('--verify',action='store_true');args=parser.parse_args()
 if args.repro:
  before={str(p.relative_to(ROOT)):sha(p) for p in generated()}
  subprocess.run([sys.executable,'-B',str(ROOT/'tools/build.py')],check=True)
  after={str(p.relative_to(ROOT)):sha(p) for p in generated()}
  assert before==after,'Generated-source mismatch'
  (ROOT/'evidence/source-reproducibility.json').write_text(json.dumps({'pass':True,'generatedFiles':len(before),'sha256':after},indent=2)+'\n')
 if args.seal:
  runtime=ROOT/'tools/.runtime'
  if runtime.exists():
   assert not list(runtime.glob('profile-*')),'Owned Chrome still active: close it before sealing'
   shutil.rmtree(runtime)
  (ROOT/'evidence/initial-b.png').unlink(missing_ok=True)
  # Portability: the source-run scope audit compared the design-output working
  # tree (baseline-status.txt) around the run; that audit is source-run-only and
  # is retained verbatim in evidence/scope-audit.json. The sealed manifest below
  # is the packaged copy's own integrity record; the promotion-time worktree
  # scope proof for this package lives in package-verification/.
  report=validate()
  inventory=[]
  for p in (ROOT/'assets').rglob('*.svg'):
   root=ET.parse(p).getroot();inventory.append({'path':str(p.relative_to(ROOT)),'viewBox':root.get('viewBox'),'sha256':sha(p),'bytes':p.stat().st_size})
  (ROOT/'assets/index.json').write_text(json.dumps({'nativeUse':'Procedural Canvas/Path reference only; do not ship these browser SVGs or embedded ground paint','layersAndSprites':sorted(inventory,key=lambda x:x['path'])},indent=2)+'\n')
  MANIFEST.write_text(''.join(f'{sha(p)}  {p.relative_to(ROOT)}\n' for p in files()))
 if args.verify or args.seal:
  report=validate();expected={}
  for line in MANIFEST.read_text().splitlines():digest,rel=line.split('  ',1);assert rel not in expected;expected[rel]=digest
  actual={str(p.relative_to(ROOT)):sha(p) for p in files()};assert expected==actual,{'missing':sorted(expected.keys()-actual.keys()),'extra':sorted(actual.keys()-expected.keys()),'changed':[p for p in expected.keys()&actual.keys() if expected[p]!=actual[p]]}
  summary={'status':'PASS','manifestFiles':len(expected),'rawFilesIncludingManifest':len(actual)+1,'bytesIncludingManifest':sum(p.stat().st_size for p in files())+MANIFEST.stat().st_size,'pngFiles':len(list(ROOT.rglob('*.png'))),'svgFiles':len(list(ROOT.rglob('*.svg'))),'mp4Files':len(list(ROOT.rglob('*.mp4'))),'browser':report['summary'],'manifestIncludesItself':False,'transientFiles':0}
  print(json.dumps(summary,indent=2))
if __name__=='__main__':main()
