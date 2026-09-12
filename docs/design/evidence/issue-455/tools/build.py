"""Deterministic browser proof. Reuses immutable V1/R2 sources; no product writes."""
import sys
sys.dont_write_bytecode=True
from pathlib import Path
import json,re,importlib.util,hashlib,xml.etree.ElementTree as ET
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'references/r2'))
import art
sys.path.insert(0,str(ROOT/'references/v1'))
import horsesvg as H
# Load original fixture identities independently of the R2 module import cache.
spec=importlib.util.spec_from_file_location('v1fixtures',ROOT/'references/v1/fixtures.py')
F=importlib.util.module_from_spec(spec);sys.modules['v1fixtures']=F;spec.loader.exec_module(F)
def put(path,text):
 p=ROOT/path;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(text)
def grazing(identity):
 """Exact corrected head/neck path coordinates from pinned HerdArt.grazing()."""
 body,dark=H.COAT[identity['coat']];mane=H.MANE_C[identity['coat']]
 shapes=[('M74 40Q91 34 102 48Q109 55 111 64L104 75Q98 66 92 60Q86 55 80 53Z',body,1),('M110 61Q116 62 120 75L125 90Q125 95 119 96L114 91L109 82Q101 78 104 71Z',body,1),('M108 64L107 54L113 60Z',body,1),('M115 66L118 57L119 69Z',body,1),('M109 62L108 57L111 61Z',H.MUC,1),('M104 74Q108 82 114 81L110 85Q103 80 104 74Z',dark,.35),('M118 86L124 87L125 91Q125 95 119 96L115 91Z',H.MUC,1)]
 if identity['coat'] in H.BLAZE_COATS:shapes.append(('M117 72L121 86L119 88L115 74Z',H.BLAZE,1))
 if identity['mane']=='flowing':shapes.append(('M78 36Q95 37 103 50Q108 57 108 65L103 68Q102 55 96 50Q88 40 76 40Z',mane,1))
 elif identity['mane']=='cropped':shapes.append(('M76 36Q94 38 104 53L101 57Q91 42 74 41Z',mane,1))
 s='<g class="neck corrected-graze">'+''.join(f'<path d="{d}" fill="{c}" opacity="{o}"/>' for d,c,o in shapes)
 s+='<circle cx="121.5" cy="91" r="1" fill="#2f2a26"/><circle cx="114.5" cy="71" r="1.6" fill="#1d1a17"/>'
 if identity['mane']=='braided':s+=''.join(f'<circle cx="{x}" cy="{y}" r="3.4" fill="{mane}"/>' for x,y in [(89,41),(98,49),(104,58)])
 if identity['accessory']=='hat':s+='<ellipse cx="110" cy="58" rx="12" ry="4" fill="#d8b46a"/><path d="M104.5 58C104.5 54.96 106.96 52.5 110 52.5C113.04 52.5 115.5 54.96 115.5 58Z" fill="#d8b46a" stroke="#8a5a33" stroke-width="1.3"/>'
 if identity['accessory']=='bandana':s+='<path d="M76 36L90 42L83 48Z" fill="#c2543f"/>'
 return s+'</g>'
def horse(name,state,static=False):
 identity=F.horse_identity(name)
 svg=H.horse_svg(identity, 'stand' if static and state=='idle' else state,reduce_motion=True)
 if state=='idle' and not static:svg=re.sub(r'<g class="neck"[^>]*>.*?</g>',grazing(identity),svg,flags=re.S)
 return svg
# R2 environment source is retained byte-for-byte. Wrappers add shared motion only.
src=(ROOT/'references/r2/art.py').read_text()
src=src.replace("s+=path(band,'url(#galactic)',filter='url(#haze)')","s+='<g class=\"sky-drift\">';s+=path(band,'url(#galactic)',filter='url(#haze)')")
src=src.replace("s+='<circle cx=\"327\"", "s+='</g><circle cx=\"327\"")
src=src.replace("s+=f'<g transform=\"translate({x} {y}) scale({k})\" opacity=\".56\"", "s+=f'<g class=\"cloud-drift\"><g transform=\"translate({x} {y}) scale({k})\" opacity=\".56\"")
src=src.replace('fill=\"#f8f3da\"/></g>\'', 'fill=\"#f8f3da\"/></g></g>\'')
src=src.replace('        # Small overlapping leaf clusters with asymmetric lit crowns.','        t+=\'<g class="canopy" style="transform-origin:0px 30px">\'\n        # Small overlapping leaf clusters with asymmetric lit crowns.')
src=src.replace("return t+'</g>'","return t+'</g></g>'")
module={'__name__':'motion_art'};exec(compile(src,'retained-r2-with-motion-wrappers','exec'),module)
ET.register_namespace('','http://www.w3.org/2000/svg')
worlds={}
for env in ['day','night']:
 raw=module['world'](env=='night')
 raw=raw.replace('viewBox="0 0 390 640" preserveAspectRatio="none"','viewBox="0 -110 390 844" preserveAspectRatio="xMidYMid slice"')
 # Extend edge fields, not landmarks, anatomy or aspect ratio.
 raw=raw.replace('M0 0H390V640H0Z','M-100 -300H490V1100H-100Z').replace('V640H0Z','V1100H0Z')
 root=ET.fromstring(raw)
 ns='{http://www.w3.org/2000/svg}'
 # Guard-band duplicates of the same seeded stars, outside the resting viewport.
 # Bounded +/-70px drift cannot expose a star-free rectangular edge.
 import copy
 for group in root.iter(ns+'g'):
  if group.get('class')=='sky-drift':
   for star in list(group):
    if star.tag!=ns+'circle':continue
    x=float(star.get('cx','0'))
    if x<90 or x>300:
     duplicate=copy.deepcopy(star);duplicate.set('cx',str(x+(390 if x<90 else -390)));group.append(duplicate)
 grasses=[]
 for el in list(root):
  if el.tag==ns+'path' and el.get('stroke-width') and el.get('opacity')=='.55':grasses.append(el);root.remove(el)
 bands={}
 for el in grasses:
  vals=re.match(r'M([\d.]+) ([\d.]+)',el.get('d',''))
  y=float(vals[2]); band=int((y-260)//32)
  if band not in bands:bands[band]=ET.Element(ns+'g',{'class':'grass-wind','style':f'transform-origin:195px {276+band*32}px','data-band':str(band)})
  bands[band].append(el)
 for group in bands.values():root.insert(len(root)-2,group)
 raw=ET.tostring(root,encoding='unicode')
 # Prefix all defs for two independently mounted environments.
 ids=re.findall(r'id="([^"]+)"',raw)
 for ident in ids:raw=raw.replace(f'id="{ident}"',f'id="{env}-{ident}"').replace(f'url(#{ident})',f'url(#{env}-{ident})')
 worlds[env]=raw
 put(f'assets/ranch-{env}.svg',raw)
 # Six separate procedural planes, using the accepted R2 export boundaries.
 tree=ET.fromstring(raw);nodes=list(tree)
 ix=lambda predicate:next(j for j,n in enumerate(nodes) if predicate(n))
 hill=ix(lambda n:'ridge-volume0' in ET.tostring(n,encoding='unicode'))
 ground=ix(lambda n:n.get('d','').startswith('M0 259Q80'))
 barn=ix(lambda n:n.get('transform')=='translate(156 207) scale(.68)')
 fence=ix(lambda n:n.get('d','').startswith('M0 271Q180'))
 grass=ix(lambda n:n.get('d')=='M385 528V548')+1
 ranges=[('00-sky',0,hill),('10-hills',hill,ground),('20-ground',ground,barn),('30-barn-trees',barn,fence),('40-rear-fences',fence,grass),('50-foreground',grass,len(nodes))]
 assert 0<hill<ground<barn<fence<grass<len(nodes)
 outerdefs=''.join(ET.tostring(n,encoding='unicode') for n in nodes if n.tag==ns+'defs')
 for name,start,end in ranges:
  body=''.join(ET.tostring(n,encoding='unicode') for n in nodes[start:end] if n.tag!=ns+'defs')
  put(f'assets/layers/{env}/{name}.svg','<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 -110 390 844" preserveAspectRatio="xMidYMid slice">'+outerdefs+body+'</svg>')
 # Export addressable motion groups with all defs, at the identical world origin.
 tree=ET.fromstring(raw);defs=''.join(ET.tostring(e,encoding='unicode') for e in tree.iter(ns+'defs'))
 for cls in ['canopy','grass-wind','cloud-drift','sky-drift']:
  parents={child:parent for parent in tree.iter() for child in parent}
  groups=[]
  for el in tree.iter():
   if el.get('class')!=cls:continue
   fragment=ET.tostring(el,encoding='unicode');parent=parents.get(el)
   while parent is not None and parent is not tree:
    if parent.get('transform'):fragment='<g transform="'+parent.get('transform')+'">'+fragment+'</g>'
    parent=parents.get(parent)
   groups.append(fragment)
  if not groups:
   (ROOT/f'assets/layers/{env}-{cls}.svg').unlink(missing_ok=True)
   continue
  put(f'assets/layers/{env}-{cls}.svg','<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 -110 390 844">'+defs+''.join(groups)+'</svg>')
names=['birch-clearing','oak-before-dark','spruce-hollow','willow-bend','cedar-ridge','aspen-grove','maple-stand','hazel-hollow','juniper-south','elder-flats','hawthorn-fork','sumac-row']
states=['blocked','blocked','done','idle','working','working','idle','working','done','unknown','working','idle']
repos=['atlas-vector','brook-notes','atlas-vector','atlas-vector','brook-notes','brook-notes','brook-notes','cedar-observatory-client','cedar-observatory-client','cedar-observatory-client','dune-tools','dune-tools']
fixtures=[];sprites={}
for i,(name,state,repo) in enumerate(zip(names,states,repos)):
 host='Meadow' if i%3 else 'Orchard'
 fixtures.append(dict(id=f'{host}::{name}',name=name,state=state,repo=repo,host=host))
 sprites[name]={'normal':horse(name,state),'static':horse(name,state,True)}
 put(f'assets/horses/{name}.svg',sprites[name]['normal'])
put('fixtures.json',json.dumps(fixtures,indent=2)+'\n')
css=(ROOT/'tools/app.css').read_text();js=(ROOT/'tools/app.js').read_text()
template=(ROOT/'tools/template.html').read_text()
for variant in ['a','b']:
 page=template.replace('@@VARIANT@@',variant).replace('@@CSS@@',css).replace('@@JS@@',js).replace('@@FIXTURES@@',json.dumps(fixtures)).replace('@@SPRITES@@',json.dumps(sprites)).replace('@@WORLDS@@',''.join(f'<div class="environment {env}" data-env="{env}">{svg}</div>' for env,svg in worlds.items()))
 put(f'variant-{variant}.html',page)
print('Built A/B; V1 silhouettes, corrected grazing, R2 world. Fixtures:',len(fixtures))
