"""Verify six layer exports by actual same-origin browser reassembly."""
import sys
sys.dont_write_bytecode=True
from browser import Browser,ROOT
from PIL import Image,ImageChops
import json,hashlib
report=[]
with Browser() as b:
 for env in ['day','night']:
  b.load('variant-b.html',f'preview=herd&env={env}&still=1')
  b.js("document.querySelectorAll('.canopy,.grass-wind,.cloud-drift,.sky-drift').forEach(e=>e.style.transform='none')")
  p=b.screenshot('tools/.runtime/intact.png');original=Image.open(p).convert('RGBA')
  planes=['00-sky','10-hills','20-ground','30-barn-trees','40-rear-fences','50-foreground']
  paths=[f'assets/layers/{env}/{name}.svg' for name in planes]
  b.js("document.querySelector('.environment.'+document.body.dataset.environment).innerHTML="+json.dumps(''.join(f'<img src="{p}" style="position:absolute;inset:0;width:100%;height:100%;object-fit:cover">' for p in paths)))
  b.js("Promise.all([...document.images].map(i=>i.decode()))")
  p=b.screenshot('tools/.runtime/reassembled.png');reassembled=Image.open(p).convert('RGBA')
  diff=ImageChops.difference(original,reassembled);box=diff.convert('RGB').getbbox()
  report.append({'environment':env,'planes':paths,'sameOrigin':True,'pixelEqual':box is None,'differenceBounds':box})
  (ROOT/'evidence/layer-verification.json').write_text(json.dumps(report,indent=2)+'\n')
  assert box is None,report[-1]
print(json.dumps(report,indent=2))
