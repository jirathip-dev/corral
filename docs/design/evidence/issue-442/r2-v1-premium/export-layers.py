#!/usr/bin/env python3
"""Deterministic separable exports of frozen proof; no icon integration."""
import sys
sys.dont_write_bytecode=True
import argparse, copy, hashlib, json, re, subprocess, tempfile, glob, html
from pathlib import Path
import xml.etree.ElementTree as E
import art, fixtures as F
ROOT=Path(__file__).resolve().parent
NS='http://www.w3.org/2000/svg';E.register_namespace('',NS)
def tag(n):return n.tag.split('}')[-1]
def svg(nodes,defs=(),w=390,h=640):
    r=E.Element('{'+NS+'}svg',{'viewBox':f'0 0 {w} {h}','width':str(w),'height':str(h)})
    for n in [*defs,*nodes]:r.append(copy.deepcopy(n))
    return r
def encode(r):return E.tostring(r,encoding='utf-8',xml_declaration=True)
def geometry():
    script='''<script>addEventListener('load',()=>{const b=e=>e.getBoundingClientRect().toJSON();const out={viewport:[innerWidth,innerHeight],measurementDevicePixelRatio:devicePixelRatio,decisionCaptureDPR:2,decisionOutputScale:1,scroll:{pageX:scrollX,pageY:scrollY,paddock:document.querySelector(".paddock-strip").scrollLeft,rail:document.querySelector(".rail-row").scrollLeft},elements:{},horses:[]};for(const s of ['.chrome','.scope','.rail-summary','.pasture','.rail-row','.physical-rail','.paddock-strip','.paddock','.paddock-head','.field','.dots','.hint']){const e=document.querySelector(s);if(e){const c=getComputedStyle(e);out.elements[s]={rect:b(e),font:c.font,color:c.color,background:c.background,padding:c.padding,gap:c.gap,z:c.zIndex}}}for(const e of document.querySelectorAll('.horse-btn'))out.horses.push({name:e.querySelector('.nameplate').textContent,button:b(e),sprite:b(e.querySelector('.horse-svg')),nameplate:b(e.querySelector('.nameplate')),state:b(e.querySelector('.state-flag'))});const p=document.createElement('pre');p.id='geometry';p.textContent=JSON.stringify(out);document.body.append(p)});</script>'''
    shell=sorted(glob.glob(str(Path.home()/'Library/Caches/ms-playwright/chromium_headless_shell-*/chrome-headless-shell-mac-arm64/chrome-headless-shell')))[-1]
    with tempfile.TemporaryDirectory(prefix='r2-geometry-') as td:
        p=Path(td)/'proof.html';p.write_text((ROOT/'day.html').read_text().replace('</body>',script+'</body>'))
        r=subprocess.run([shell,'--headless','--disable-gpu','--no-first-run',f'--user-data-dir={td}/profile','--window-size=390,844','--virtual-time-budget=1000','--dump-dom',p.as_uri()],capture_output=True,text=True,check=True)
        return json.loads(html.unescape(re.search('<pre id="geometry">(.*?)</pre>',r.stdout,re.S)[1]))
def build(out):
    out.mkdir(parents=True,exist_ok=True);entries=[];geo=geometry()
    (out/'geometry.json').write_text(json.dumps(geo,indent=2,sort_keys=True)+'\n')
    pasture=geo['elements']['.pasture']['rect']
    def emit(name,r,z,anchor,composition,parallax,source,**extra):
        b=encode(r);p=out/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(b)
        entries.append(dict(name=name,format='SVG',dimensions=[float(r.get('width')),float(r.get('height'))],viewBox=r.get('viewBox'),aspect_ratio_policy='affine-to-composition; no horse deformation',z_index=z,parent_context='native-world; labels/HUD above scene',origin=[0,0],anchor=anchor,composition=composition,composition_matrix=[composition['width']/float(r.get('width')),0,0,composition['height']/float(r.get('height')),composition['x'],composition['y']],parallax_ratio=parallax,parallax_axis='horizontal',parallax_status='native proposal; static proof is zero',reproduction_cwd='docs/design/evidence/issue-442/r2-v1-premium/',sha256=hashlib.sha256(b).hexdigest(),source_generator=source,reproduction='python3 -B export-layers.py',**extra))
    for mode in ['day','night']:
        root=E.fromstring(art.world(mode=='night'));nodes=list(root);defs=[n for n in nodes if tag(n)=='defs']
        ix=lambda f:next(j for j,n in enumerate(nodes) if f(n))
        hill=ix(lambda n:'ridge-volume0' in E.tostring(n).decode())
        ground=ix(lambda n:n.get('d','').startswith('M0 259Q80'))
        barn=ix(lambda n:n.get('transform')=='translate(156 207) scale(.68)')
        fence=ix(lambda n:n.get('d','').startswith('M0 271Q180'))
        grass=ix(lambda n:n.get('d')=='M385 528V548')+1
        ranges=[('00-sky',0,hill,0,.05),('10-hills',hill,ground,10,.12),('20-ground',ground,barn,20,.4),('30-barn-trees',barn,fence,30,.22),('40-rear-fences',fence,grass,40,.4),('50-foreground',grass,len(nodes),50,.65)]
        assert 0<hill<ground<barn<fence<grass<len(nodes)
        for name,a,b,z,ratio in ranges:
            emit(f'{mode}/{name}.svg',svg([n for n in nodes[a:b] if tag(n)!='defs'],defs),z,[0,0],dict(x=pasture['x'],y=pasture['y'],width=pasture['width'],height=pasture['height'],coordinate_space='390x844 proof viewport; world stretched from 390x640'),ratio,'art.py:world')
    railroot=E.fromstring(art.rail());railroot.set('width','390');railroot.set('height','44')
    emit('front-rail.svg',railroot,80,[0,0],geo['elements']['.physical-rail']['rect'],0,'art.py:rail')
    identities={a.name:F.horse_identity(a.name) for a in F.blocked_heavy().agents}
    agents={a.name:a for a in F.blocked_heavy().agents}
    lineup=[]
    def parts(name,identity,pose,uid,state='idle',facing=1,square=False):
        root=E.fromstring(art.horse(identity,state,pose,uid,facing));root.set('width','148');root.set('height','112')
        defs=root.find('{'+NS+'}defs');g=root.find('{'+NS+'}g');contact=[]
        for n in list(g):
            if n.get('class')=='cast-shadow' or (tag(n)=='ellipse' and n.get('fill')=='#132521'):
                contact.append(copy.deepcopy(n));g.remove(n)
        rim=[]
        for n in list(g):
            if n.get('class')=='moon-rim':rim.append(copy.deepcopy(n));g.remove(n)
        data=next((x for x in geo['horses'] if x['name']==name),None)
        composition=data['sprite'] if data else dict(x=0,y=0,width=148,height=112,coordinate_space='standalone pose specimen')
        if not square:
            emit(f'horses/{uid}.svg',root,60,[74,107],composition,0 if state=='blocked' else 1,'art.py:horse + paint_surface',identity=identity,pose=pose,agent=name,facing=facing,facing_transform=[facing,0,0,1,148 if facing==-1 else 0,0],geometry_key='horses[name='+name+'].sprite')
            for kind,children,z in [('grounding',contact,55),('rim',rim,61)]:
                group=E.Element('{'+NS+'}g',dict(g.attrib));group.extend(children)
                emit(f'{kind}/{uid}.svg',svg([group],[defs],148,112),z,[74,107],composition,0 if state=='blocked' else 1,'art.py:horse',identity=identity,pose=pose,agent=name,facing=facing,facing_transform=[facing,0,0,1,148 if facing==-1 else 0,0],geometry_key='horses[name='+name+'].sprite')
        else:
            # No flag, ground, text, frame, app mask, or current-icon dependency.
            root[:]=[defs,g]
            outer=svg([],w=256,h=256);group=E.SubElement(outer,'{'+NS+'}g',{'transform':'translate(24 49.2972972973) scale(1.4054054054054055)'})
            group.append(root)
            emit(f'square/{name}.svg',outer,0,[128,200],dict(x=0,y=0,width=256,height=256,coordinate_space='future-icon SOURCE ONLY; not app asset'),0,'art.py:horse; export-layers.py:parts',identity=identity,pose='stand',safe_area=[24,24,232,232],sprite_bounds=[24,49.2972972973,232,206.7027027027],source_anchor=[74,107])
            lineup.append((name,outer))
    day=(ROOT/'day.html').read_text()
    for name,identity in identities.items():
        raw=next(b for b in re.findall(r'<button class="horse-btn.*?</button>',day,re.S) if f'>{name}</span>' in b)
        facing=-1 if 'scale(-1 1)' in raw else 1
        state=agents[name].state;parts(name,identity,'graze' if state=='idle' else 'stand',name,state,facing)
        parts(name,identity,'stand','square-'+name,square=True)
    for pose in ['stand','shift','graze','reduce-motion']:
        parts('willow-bend',identities['willow-bend'],'stand' if pose=='reduce-motion' else pose,'pose-'+pose)
    sheet=svg([],w=1024,h=768)
    for j,(name,r) in enumerate(lineup):
        g=E.SubElement(sheet,'{'+NS+'}g',{'transform':f'translate({j%4*256} {j//4*256})','data-agent':name});g.append(copy.deepcopy(r))
    emit('square/lineup.svg',sheet,0,[0,0],dict(x=0,y=0,width=1024,height=768,coordinate_space='4 columns x 3 source cells, 256 each'),0,'export-layers.py:build',cells=[name for name,_ in lineup],cell_size=[256,256],cell_layout=[dict(identity=name,origin=[j%4*256,j//4*256],bounds=[j%4*256,j//4*256,(j%4+1)*256,(j//4+1)*256],anchor=[j%4*256+128,j//4*256+200],source='square/'+name+'.svg') for j,(name,_) in enumerate(lineup)])
    # Rendered geometric bounds, not inferred from the square placement.
    shell=sorted(glob.glob(str(Path.home()/'Library/Caches/ms-playwright/chromium_headless_shell-*/chrome-headless-shell-mac-arm64/chrome-headless-shell')))[-1]
    with tempfile.TemporaryDirectory(prefix='r2-layer-bounds-') as td:
        content='<html><body style="margin:0">'+''.join('<div class="asset" data-name="'+a['name']+'">'+(out/a['name']).read_text().split('?>',1)[1]+'</div>' for a in entries)
        content+="<script>addEventListener('load',()=>{const p=document.createElement('pre');p.id='bounds';p.textContent=JSON.stringify([...document.querySelectorAll('.asset')].map(e=>{const b=e.querySelector('svg').getBBox();return {name:e.dataset.name,b:{x:b.x,y:b.y,width:b.width,height:b.height}}}));document.body.append(p)});</script></body></html>"
        page=Path(td)/'bounds.html';page.write_text(content)
        result=subprocess.run([shell,'--headless','--disable-gpu','--no-first-run',f'--user-data-dir={td}/profile','--window-size=1024,768','--virtual-time-budget=1500','--dump-dom',page.as_uri()],capture_output=True,text=True,check=True)
        bounds={a['name']:a['b'] for a in json.loads(html.unescape(re.search('<pre id="bounds">(.*?)</pre>',result.stdout,re.S)[1]))}
        for a in entries:
            a['painted_geometry_bounds']=bounds[a['name']]
            a['bounds_policy']='Browser SVG getBBox geometric bounds; filter bleed clipped by root viewport'
            if a['name'].startswith('square/') and 'identity' in a:
                a['placement_transform']=[208/148,0,0,208/148,24,49.2972972973]
                a['lineage']='art.py:horse('+a['name'].split('/')[-1].removesuffix('.svg')+', stand)'
                b=bounds[a['name']];assert b['x']>=24-1e-6 and b['y']>=24-1e-6 and b['x']+b['width']<=232+1e-6 and b['y']+b['height']<=232+1e-6,(a['name'],b)
    (out/'index.json').write_text(json.dumps(dict(schema=1,decision_png_count=3,asset_count=len(entries),units='SVG user units / reference pixels; native points at 390pt reference',parallax_note='Proposed native ratios; current static proof has zero parallax. See NATIVE-HANDOFF.md.',assets=entries),indent=2,sort_keys=True)+'\n')
    print('EXPORTED',len(entries),'SVG layers; no decision PNG changed')
if __name__=='__main__':
    a=argparse.ArgumentParser();a.add_argument('--out',type=Path,default=ROOT/'layers');build(a.parse_args().out)
