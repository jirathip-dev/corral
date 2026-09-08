#!/usr/bin/env python3
import sys
sys.dont_write_bytecode=True
import hashlib,json,subprocess,tempfile,glob,re
from pathlib import Path
import xml.etree.ElementTree as E
E.register_namespace('','http://www.w3.org/2000/svg')
from PIL import Image,ImageChops
import art,fixtures as F
ROOT=Path(__file__).resolve().parent
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
names=[a.name for a in F.blocked_heavy().agents]
expected={f'{m}/{n}.svg' for m in ['day','night'] for n in ['00-sky','10-hills','20-ground','30-barn-trees','40-rear-fences','50-foreground']}
expected|={'front-rail.svg','square/lineup.svg'}|{f'square/{n}.svg' for n in names}
expected|={f'{kind}/{n}.svg' for kind in ['horses','grounding','rim'] for n in names+['pose-stand','pose-shift','pose-graze','pose-reduce-motion']}
index=json.loads((ROOT/'layers/index.json').read_text());assets=index['assets']
assert {a['name'] for a in assets}==expected and len(assets)==len(expected)==index['asset_count']
assert {p.relative_to(ROOT/'layers').as_posix() for p in (ROOT/'layers').rglob('*') if p.is_file()}==expected|{'index.json','geometry.json'}
for a in assets:
 p=ROOT/'layers'/a['name'];r=E.parse(p).getroot()
 assert r.get('viewBox')==a['viewBox']
 assert [float(r.get('width')),float(r.get('height'))]==a['dimensions']
 assert sha(p)==a['sha256']
 want=([390,640] if a['name'].startswith(('day/','night/')) else [390,44] if a['name']=='front-rail.svg' else [1024,768] if a['name']=='square/lineup.svg' else [256,256] if a['name'].startswith('square/') else [148,112])
 assert a['dimensions']==want and a['viewBox']==f'0 0 {want[0]} {want[1]}'
 for k in ['z_index','origin','anchor','composition','composition_matrix','painted_geometry_bounds','parallax_ratio','source_generator','reproduction','reproduction_cwd']:assert k in a
 assert a['reproduction']=='python3 -B export-layers.py'
 assert not re.search(r'(?:href|src)="https?://',p.read_text())
 if a['name'].startswith('square/') and a['name']!='square/lineup.svg':
  assert a['dimensions']==[256,256] and a['safe_area']==[24,24,232,232]
  x,y,x2,y2=a['sprite_bounds'];assert 24<=x<x2<=232 and 24<=y<y2<=232
  assert len(a['identity'])==5
  b=a['painted_geometry_bounds'];assert b['x']>=24-1e-6 and b['y']>=24-1e-6 and b['x']+b['width']<=232+1e-6 and b['y']+b['height']<=232+1e-6
for kind in ['horses','grounding','rim']:
 stand=(ROOT/'layers'/kind/'pose-stand.svg').read_text()
 rm=(ROOT/'layers'/kind/'pose-reduce-motion.svg').read_text()
 assert stand.replace('pose-stand','pose-reduce-motion')==rm
print('PASS deliberate Reduce Motion calm-standing source equality')
print('PASS independent expected names/count, viewBoxes/dimensions, hashes, metadata, square safe frames:',len(expected))
freeze=json.loads((ROOT/'decision-freeze.json').read_text());assert len(freeze)==3
assert {p.name for p in ROOT.glob('*.png')}==set(freeze)
assert all(sha(ROOT/n)==h for n,h in freeze.items());print('PASS exact scope-cap decision PNG freeze')
with tempfile.TemporaryDirectory(prefix='r2-layer-repro-') as td:
 out=Path(td)/'layers';subprocess.run([sys.executable,'-B',str(ROOT/'export-layers.py'),'--out',str(out)],check=True)
 for n in expected|{'index.json','geometry.json'}:assert (out/n).read_bytes()==(ROOT/'layers'/n).read_bytes(),n
 print('PASS deterministic layer + metadata rerender equality')
 shell=sorted(glob.glob(str(Path.home()/'Library/Caches/ms-playwright/chromium_headless_shell-*/chrome-headless-shell-mac-arm64/chrome-headless-shell')))[-1]
 for mode in ['day','night']:
  # Side-by-side browser proof: intact scene vs exported alpha stack.
  layers=sorted(a['name'] for a in assets if a['name'].startswith(mode+'/'))
  html='<html><head><style>.world{width:390px;height:640px;display:block}</style></head><body style="margin:0;background:transparent">'+f'<div style="position:absolute;width:390px;height:640px;left:0;top:0">{art.world(mode=="night")}</div>'
  # Compose exported vectors into one Canvas-like scene graph, avoiding
  # per-image antialias/compositing quantization at translucent layer edges.
  roots=[E.parse(ROOT/'layers'/n).getroot() for n in layers]
  merged=E.Element('{http://www.w3.org/2000/svg}svg',{'width':'390','height':'640','viewBox':'0 0 390 640'})
  merged.extend(n for n in roots[0] if n.tag.endswith('}defs'))
  for rr in roots:merged.extend(n for n in rr if not n.tag.endswith('}defs'))
  html+='<div style="position:absolute;width:390px;height:640px;left:390px;top:0">'+E.tostring(merged,encoding='unicode')+'</div></body></html>'
  page=Path(td)/(mode+'.html');page.write_text(html);png=Path(td)/(mode+'.png')
  subprocess.run([shell,'--headless','--disable-gpu','--hide-scrollbars','--no-first-run',f'--user-data-dir={td}/{mode}-profile','--window-size=780,640','--force-device-scale-factor=1','--virtual-time-budget=2000',f'--screenshot={png}',page.as_uri()],capture_output=True,check=True)
  # feTurbulence uses the device-space origin: compare at the SAME origin,
  # not x=0 vs x=390 (that would compare different seeded noise samples).
  original=Image.open(png).convert('RGB').crop((0,0,390,640))
  page.write_text(html.replace('left:390px;top:0','left:0px;top:0'))
  subprocess.run([shell,'--headless','--disable-gpu','--hide-scrollbars','--no-first-run',f'--user-data-dir={td}/{mode}-candidate','--window-size=780,640','--force-device-scale-factor=1','--virtual-time-budget=2000',f'--screenshot={png}',page.as_uri()],capture_output=True,check=True)
  im=Image.open(png).convert('RGB');assert im.size==(780,640)
  diff=ImageChops.difference(original,im.crop((0,0,390,640)))
  if diff.getbbox():
   print('PIXELS',[(q,im.getpixel(q),im.getpixel((q[0]+390,q[1]))) for q in [(0,0),(100,100),(200,300),(200,600)]], 'EXTREMA',diff.getextrema())
   im.save('/tmp/r2-layer-drift.png')
  assert diff.getbbox() is None,('layer composite pixel drift',mode,diff.getbbox())
  print('PASS actual-browser intact vs exported layer-stack pixel equality',mode)
print('LAYER VERIFIER PASS')
