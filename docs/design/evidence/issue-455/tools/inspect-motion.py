from pathlib import Path
from PIL import Image,ImageChops,ImageDraw
import json
R=Path(__file__).resolve().parents[1]
a=json.loads((R/'evidence/motion-verification.json').read_text())
for c in a['clips']:
 print(c['environment'],'start',c['clockStart'],'end',c['clockEnd'])
 env=c['environment'];im=Image.open(R/f'evidence/{env}-motion-start.png').convert('RGB');end=Image.open(R/f'evidence/{env}-motion-end.png').convert('RGB');diff=ImageChops.difference(im,end)
 for name,box in [('sky',(0,15,390,278)),('tree-right',(335,278,390,400)),('grass',(0,520,390,704))]:
  crop=diff.crop(box);vals=list(crop.getdata());changed=sum(max(x)>8 for x in vals);print(name,'changed >8:',changed,'of',len(vals),'max:',crop.getextrema())
 diff.point(lambda x:min(255,x*8)).save(R/f'evidence/{env}-motion-difference-8x.png')
