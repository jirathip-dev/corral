"""Adjudicate visual-review clipping claims using live painted SVG bounds."""
from browser import Browser,ROOT
import json
with Browser() as b:
 b.load('variant-b.html','preview=herd&env=day&still=1')
 data=b.js("""(()=>{const rect=e=>{const r=e.getBoundingClientRect();return {x:r.x,y:r.y,right:r.right,bottom:r.bottom,width:r.width,height:r.height}};return [...document.querySelectorAll('.horse-button')].map(e=>{const svg=e.querySelector('.normal svg');const shapes=[...svg.querySelectorAll('path,ellipse,circle,rect,polygon,polyline,line')].filter(p=>!p.closest('defs'));const rs=shapes.map(rect);return {id:e.dataset.horse,button:rect(e),art:rect(svg),caption:rect(e.querySelector('.horse-caption')),paint:{x:Math.min(...rs.map(r=>r.x)),y:Math.min(...rs.map(r=>r.y)),right:Math.max(...rs.map(r=>r.right)),bottom:Math.max(...rs.map(r=>r.bottom))},parent:rect(e.closest('#rail-horses')?document.querySelector('#rail-scroll'):e.parentElement),header:rect(document.querySelector(e.closest('#rail-horses')?'#rail-title':'#repo-title'))}})})()""")
 for x in data:
  p=x['paint'];x['paintInsideArt']=p['x']>=x['art']['x']-.1 and p['right']<=x['art']['right']+.1 and p['y']>=x['art']['y']-.1 and p['bottom']<=x['art']['bottom']+.1
  x['paintAboveCaption']=p['bottom']<=x['caption']['y'];x['paintBelowHeader']=p['y']>=x['header']['bottom'];x['paintInsideScrollViewport']=p['y']>=x['parent']['y'] and p['bottom']<=x['parent']['bottom']
 (ROOT/'evidence/art-geometry.json').write_text(json.dumps(data,indent=2)+'\n')
 print(json.dumps(data,indent=2))
 # Accepted V1 shadows/tack extend beyond viewBox; SVG overflow is intentionally visible.
 assert all(x['paintAboveCaption'] and x['paintBelowHeader'] and x['paintInsideScrollViewport'] for x in data)
