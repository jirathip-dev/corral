"""Real browser gates + labeled still renders. Run after build.py; fail closed."""
import sys
sys.dont_write_bytecode=True
from pathlib import Path
import json,time,hashlib,math,re,shutil
from PIL import Image,ImageDraw,ImageFont,ImageChops
from browser import Browser,ROOT
E=ROOT/'evidence';E.mkdir(exist_ok=True)
REPORT={'status':'RUNNING','checks':[],'screenshots':[],'contrast':[],'browser':None,'scope':'Chromium browser proof only; not native/device evidence'}
def flush(): (E/'verification.json').write_text(json.dumps(REPORT,indent=2)+'\n')
def check(name,ok,detail=None):
 REPORT['checks'].append({'name':name,'pass':bool(ok),'detail':detail});flush()
 if not ok:raise AssertionError(f'{name}: {detail}')
def label(path,text):
 im=Image.open(path).convert('RGB');d=ImageDraw.Draw(im);d.rectangle((0,0,im.width,12),fill='#181825');d.text((7,1),text,fill='#cdd6f4',font=ImageFont.load_default(size=9));im.save(path)
def capture(b,name,text=None):
 p=b.screenshot('evidence/'+name);label(p,text or name.replace('.png','').upper()+' | PROTOTYPE | HTML PROOF');size=Image.open(p).size
 REPORT['screenshots'].append({'path':'evidence/'+name,'size':list(size),'sha256':hashlib.sha256(p.read_bytes()).hexdigest()});flush()
GEOMETRY=r'''(()=>{const box=e=>e.getBoundingClientRect().toJSON();const visible=e=>{const r=e.getBoundingClientRect();return r.width&&r.height&&getComputedStyle(e).visibility!=='hidden'};const sheet=document.querySelector('#sheet').open;const controls=[...document.querySelectorAll(sheet?'dialog button,dialog a':'#hud button,#bottom button,#herd-content button')].filter(visible);const labels=[...document.querySelectorAll(sheet?'dialog h1,dialog h2,dialog small,dialog .option-row span,dialog p':'#scope-text,#counts span,#repo-title,.horse-name,.horse-state,.horse-host,#health,#page-label')].filter(visible);return {width:innerWidth,height:innerHeight,scrollWidth:document.documentElement.scrollWidth,hud:box(document.querySelector('#hud')),bottom:box(document.querySelector('#bottom')),scene:box(document.querySelector('#scene')),content:box(document.querySelector('#herd-content')),sheet:sheet?box(document.querySelector('#sheet')):null,small:controls.filter(e=>{const r=e.getBoundingClientRect();return r.width<43.99||r.height<43.99}).map(e=>({text:e.textContent,box:box(e)})),clipped:labels.filter(e=>e.scrollWidth>e.clientWidth+1&&e.clientWidth>0).map(e=>({text:e.textContent,client:e.clientWidth,scroll:e.scrollWidth})),errors:window.__errors,missingImages:[...document.images].filter(i=>!i.complete||!i.naturalWidth).map(i=>i.src)}})()'''
def geometry(b,case):
 g=b.js(GEOMETRY)
 check(case+' geometry',g['scrollWidth']==g['width'] and not g['small'] and not g['clipped'] and not g['errors'] and not g['missingImages'] and g['hud']['top']>=54 and g['bottom']['bottom']<=g['height']-24+.1 and (not g['sheet'] or g['sheet']['top']>=54),g)
 check(case+' console',not b.errors,b.errors)
 return g

def luminance(c):
 def channel(x):
  x=x/255;return x/12.92 if x<=.04045 else ((x+.055)/1.055)**2.4
 return sum(channel(c[i])*w for i,w in enumerate([.2126,.7152,.0722]))
def contrast(b,case,selectors):
 data=b.js('(()=>{return '+json.dumps(selectors)+'''.map(s=>{const e=document.querySelector(s);if(!e)return null;const range=document.createRange();range.selectNodeContents(e);const r=range.getBoundingClientRect(),c=getComputedStyle(e);return {selector:s,color:c.color,rect:r.toJSON(),font:c.fontSize}}).filter(e=>e&&e.rect.width&&e.rect.height&&e.rect.top>=0&&e.rect.bottom<=innerHeight)})()''')
 b.js("(()=>{const s=document.createElement('style');s.id='contrast-mask';s.textContent='*{-webkit-text-fill-color:transparent!important;text-shadow:none!important}button svg{visibility:hidden!important}';document.head.append(s)})()")
 p=b.screenshot('tools/.runtime/contrast.png');im=Image.open(p).convert('RGB')
 b.js("document.querySelector('#contrast-mask').remove()")
 for item in data:
  fg=list(map(float,re.findall(r'[\d.]+',item['color'])))[:3];r=item['rect']
  # Sample actual rendered backing across each text box, inset to exclude its border.
  x0=max(0,int(r['x'])+3);x1=min(im.width,int(r['right'])-3);y0=max(0,int(r['y'])+2);y1=min(im.height,int(r['bottom'])-2)
  samples=[im.getpixel((x,y)) for x in range(x0,max(x0+1,x1),3) for y in range(y0,max(y0+1,y1),2)]
  ratios=[(max(luminance(fg),luminance(bg))+.05)/(min(luminance(fg),luminance(bg))+.05) for bg in samples]
  minimum=min(ratios);result={'case':case,**item,'minimum':round(minimum,3),'samples':len(samples),'method':'minimum over sampled rendered pixels behind masked glyphs'};REPORT['contrast'].append(result)
  check(case+' contrast '+item['selector'],minimum>=4.5,result)

def select(b,action,value):b.click(f'[data-action="{action}"][data-value="{value}"]')
def run():
 with Browser() as b:
  REPORT['browser']=b.call('Browser.getVersion')
  for v in ['a','b']:
   for env in ['day','night']:
    b.size(390,844);b.load(f'variant-{v}.html',f'preview=herd&env={env}&still=1');geometry(b,f'{v}-{env}')
    check(f'{v}-{env} fixture counts',b.js('PROOF.state().counts')=={'blocked':2,'working':4,'idle':3,'done':2,'unknown':1})
    capture(b,f'{v}-{env}-390x844.png')
    contrast(b,f'{v}-{env}',['#scope-text','.scope-heading','#compact-status','#health','#counts','#rail-title','.horse-name','.horse-state','.horse-host','#repo-title','#page-label','#lighting-note','[data-light=day]','[data-light=night]','[data-light=auto]'])
    b.click('#scope');geometry(b,f'{v}-filter-{env}');capture(b,f'{v}-filter-{env}-390x844.png');contrast(b,f'{v}-filter-{env}',['#sheet-title','#sheet-subtitle','.option-row span','.option-row small','#sheet-footer button'])
    b.js("document.querySelector('#sheet-content').scrollTop=10000")
    check(f'{v}-{env} filter scrolled last row',b.js("(()=>{const r=document.querySelector('[data-action=repo][data-value=\"dune-tools\"]').getBoundingClientRect(),s=document.querySelector('#sheet-content').getBoundingClientRect();return r.top>=s.top&&r.bottom<=s.bottom})()"))
    capture(b,f'{v}-filter-{env}-end-390x844.png')
    select(b,'host','Meadow');select(b,'repo','cedar-observatory-client');s=b.js('PROOF.state()');check(f'{v}-{env} independent filters',s['host']=='Meadow' and s['repo']=='cedar-observatory-client' and s['rows']==2,s)
    b.click('#close-sheet');geometry(b,f'{v}-{env} long scope');b.click('#scope');select(b,'host','');s=b.js('PROOF.state()');check(f'{v}-{env} clear host retains repo',s['repo']=='cedar-observatory-client' and s['rows']==3,s)
    b.click('[data-action="reset"]');check(f'{v}-{env} reset',b.js('PROOF.state().rows')==12)
    b.click('#close-sheet');b.click('#next');check(f'{v}-{env} next paddock',b.js("document.querySelector('#repo-title').textContent")=='brook-notes');b.click('#previous')
   b.load(f'variant-{v}.html','preview=herd&still=1');b.click('#settings');select(b,'mode','Herd');geometry(b,f'{v}-settings');capture(b,f'{v}-settings-390x844.png');contrast(b,f'{v}-settings',['#sheet-title','#saved-note','.section-note','#close-sheet'])
   b.js("document.querySelector('#sheet-content').scrollTop=10000")
   check(f'{v} Settings final explanation reachable',b.js("(()=>{const r=document.querySelector('#sheet-content .section-note:last-child').getBoundingClientRect(),s=document.querySelector('#sheet-content').getBoundingClientRect();return r.top>=s.top&&r.bottom<=s.bottom})()"))
   capture(b,f'{v}-settings-end-390x844.png')
   # Pointer events, not function-only setters, exercise save + apply + restoration.
   select(b,'mode','Board');check(f'{v} Settings Board immediate',b.js('PROOF.state().mode')=='Board');b.click('#close-sheet');check(f'{v} Board visible',b.js("document.querySelector('#board').getBoundingClientRect().height>0"))
   b.click('#settings');select(b,'mode','Herd');b.click('#close-sheet');b.click('#open-board');s=b.js('PROOF.state()');check(f'{v} temporary recovery',s['mode']=='Board' and s['saved']=='Herd' and s['recovery'],s)
   b.call('Emulation.setFocusEmulationEnabled',enabled=False);b.call('Page.setWebLifecycleState',state='frozen');time.sleep(.2);b.call('Page.setWebLifecycleState',state='active');b.call('Page.bringToFront');b.call('Emulation.setFocusEmulationEnabled',enabled=True);check(f'{v} foreground retains recovery',b.js('PROOF.state().mode')=='Board')
   b.call('Page.reload');time.sleep(.4);check(f'{v} cold reload restores Herd',b.js('PROOF.state().mode')=='Herd')
   b.click('#settings');select(b,'mode','Board');select(b,'mode','Herd');check(f'{v} explicit Settings wins',b.js('PROOF.state().mode')=='Herd');b.click('#close-sheet')
   b.click('#proof-controls');b.click('[data-action="no-saved"]');time.sleep(.4);check(f'{v} no preference Board',b.js('PROOF.state().mode')=='Board')
   b.click('#proof-controls');b.click('[data-action="invalid"]');time.sleep(.4);check(f'{v} unknown preference Board',b.js('PROOF.state().mode')=='Board')
   # All fixture edges in both hierarchies, not just the recommended variant.
   for state in ['offline','connecting','key-mismatch','empty','dense']:
    b.load(f'variant-{v}.html',f'preview=herd&state={state}&still=1');geometry(b,f'{v}-{state}')
    capture(b,f'{v}-{state}-390x844.png')
    if state in ['offline','connecting','key-mismatch']:
     check(f'{v}-{state} truthful',b.js("PROOF.state().counts.unknown===12 && !PROOF.state().running && document.querySelector('#outage').textContent.includes('cannot be confirmed')"))
     check(f'{v}-{state} output unavailable',b.js("[...document.querySelectorAll('.horse-button')].every(e=>e.disabled)"))
     b.click('[data-action="retry"]');check(f'{v}-{state} retry does not fake live',b.js("document.querySelector('#retry-note').textContent.includes('nothing was fetched')"))
    if state=='empty':check(f'{v} empty counts',b.js('PROOF.state().rows')==0)
    if state=='dense':
     check(f'{v} dense counts',b.js('PROOF.state().rows')==30)
     b.js("document.querySelector('#rail-scroll').scrollLeft=10000;document.querySelector('#field').scrollTop=10000")
     check(f'{v} dense overflow reachable',b.js("document.querySelector('#rail-scroll').scrollLeft>0 && document.querySelector('#field').scrollTop>0"))
   for suffix,w,h,params in [('small',360,740,''),('large',430,932,''),('type',390,844,'&large=1'),('small-type',360,740,'&large=1')]:
    b.size(w,h);b.load(f'variant-{v}.html','preview=herd&still=1'+params);geometry(b,f'{v}-{suffix}')
    capture(b,f'{v}-{suffix}-{w}x{h}.png')
    if suffix=='type':
     b.js("document.querySelector('#rail-scroll').scrollLeft=10000")
     check(f'{v} large rail last full label',b.js("(()=>{const r=[...document.querySelectorAll('#rail-horses .horse-caption')].at(-1).getBoundingClientRect(),s=document.querySelector('#rail-scroll').getBoundingClientRect();return r.left>=s.left-1&&r.right<=s.right+1})()"))
     capture(b,f'{v}-type-rail-end-390x844.png')
    b.click('#scope');geometry(b,f'{v}-{suffix}-filter')
    b.js("document.querySelector('#sheet-content').scrollTop=10000")
    check(f'{v}-{suffix} last repo reachable',b.js("(()=>{const e=document.querySelector('[data-action=repo][data-value=\"dune-tools\"]');const r=e.getBoundingClientRect(),s=document.querySelector('#sheet-content').getBoundingClientRect();return r.top>=s.top&&r.bottom<=s.bottom})()"))
    if suffix=='type':capture(b,f'{v}-filter-type-end-390x844.png')
   b.size(390,844);b.load(f'variant-{v}.html','preview=herd&still=1');b.click('.horse-button');check(f'{v} Recent Output synthetic',b.js("document.querySelector('#sheet-title').textContent==='Recent Output' && document.querySelector('#sheet-content').textContent.includes('SYNTHETIC PLACEHOLDER')"));capture(b,f'{v}-output-390x844.png')
  # App flavor is independent of ranch context in both environments.
  for env in ['day','night']:
   b.load('variant-b.html',f'preview=herd&env={env}&still=1');b.click('#settings')
   for theme in ['latte','frappe','macchiato','mocha']:
    select(b,'theme',theme);check(env+' independent '+theme,b.js('PROOF.state().environment')==env)
    geometry(b,f'{env}-{theme}-settings');contrast(b,f'{env}-{theme}-settings',['#sheet-title','#saved-note','#close-sheet'])
    b.click('#close-sheet');b.click('#scope');geometry(b,f'{env}-{theme}-filter');contrast(b,f'{env}-{theme}-filter',['#sheet-title','.option-row span']);b.click('#close-sheet');b.click('#settings')
  # Real-time animation and clock lifecycle: one active rendered scene.
  b.load('variant-b.html','preview=herd&env=day');t0=b.js('PROOF.state()');time.sleep(.8);t1=b.js('PROOF.state()');check('shared clock advances',t1['ticks']>t0['ticks'] and t1['elapsed']>t0['elapsed'],[t0,t1])
  b.click('#scope');t0=b.js('PROOF.state()');time.sleep(.3);t1=b.js('PROOF.state()');check('obscured clock stopped',t0['ticks']==t1['ticks'] and not t1['running'],[t0,t1]);b.click('#close-sheet');check('dismiss resumes',b.js('PROOF.state().running'))
  b.call('Emulation.setFocusEmulationEnabled',enabled=False);b.call('Page.setWebLifecycleState',state='frozen');t0=b.js('PROOF.state()');time.sleep(.4);t1=b.js('PROOF.state()');check('actual browser background freeze',t0['ticks']==t1['ticks'] and t1['documentHidden'],[t0,t1]);b.call('Page.setWebLifecycleState',state='active');b.call('Page.bringToFront');b.call('Emulation.setFocusEmulationEnabled',enabled=True);time.sleep(.1)
  b.call('Emulation.setEmulatedMedia',features=[{'name':'prefers-reduced-motion','value':'reduce'}]);time.sleep(.1);t0=b.js('PROOF.state()');time.sleep(.3);t1=b.js('PROOF.state()');check('system Reduce Motion intentional still',t0['ticks']==t1['ticks'] and t1['elapsed']==0 and t1['reduced'] and not t1['running'],[t0,t1]);capture(b,'b-reduced-motion-390x844.png')
  b.call('Emulation.setEmulatedMedia',features=[]);b.load('variant-b.html','preview=herd');
  for action in ['low','thermal','background']:
   b.click('#proof-controls');b.click(f'[data-action="{action}"]');b.click('#close-sheet');t0=b.js('PROOF.state()');time.sleep(.25);t1=b.js('PROOF.state()');check(action+' fallback stops work',t0['ticks']==t1['ticks'] and not t1['running'],t1);b.click('#proof-controls');b.click(f'[data-action="{action}"]');b.click('#close-sheet')
  b.click('#open-board');t0=b.js('PROOF.state()');time.sleep(.25);check('Board stops work',b.js('PROOF.state().ticks')==t0['ticks'] and not b.js('PROOF.state().running'))
  # Actual rendered geometry and pixel deltas discriminate motion from twinkle.
  for env in ['day','night']:
   b.load('variant-b.html',f'preview=herd&env={env}&still=1');b.js('PROOF.freeze(0)');p0=b.screenshot('tools/.runtime/phase0.png')
   pos0=b.js("[...document.querySelectorAll('.'+document.body.dataset.environment+' .canopy,.'+document.body.dataset.environment+' .cloud-drift,.'+document.body.dataset.environment+' .sky-drift')].map(e=>e.getBoundingClientRect().toJSON())")
   im0=Image.open(p0).convert('RGB');b.js('PROOF.freeze(2.4)');p1=b.screenshot('tools/.runtime/phase1.png');im1=Image.open(p1).convert('RGB');diff=ImageChops.difference(im0,im1);box=diff.getbbox()
   pos1=b.js("[...document.querySelectorAll('.'+document.body.dataset.environment+' .canopy,.'+document.body.dataset.environment+' .cloud-drift,.'+document.body.dataset.environment+' .sky-drift')].map(e=>e.getBoundingClientRect().toJSON())")
   check(env+' rendered positional motion',box is not None and any(abs(a['x']-c['x'])>.2 or abs(a['width']-c['width'])>.2 for a,c in zip(pos0,pos1)),{'pixelDeltaBox':box,'phase0':pos0,'phase2_4':pos1})
  # Deterministic screenshot repeat from a fresh navigation at the same phase.
  b.load('variant-b.html','preview=herd&still=1&env=day');p0=b.screenshot('tools/.runtime/repro0.png');h0=hashlib.sha256(p0.read_bytes()).hexdigest();b.load('variant-b.html','preview=herd&still=1&env=day&repeat=1');p1=b.screenshot('tools/.runtime/repro1.png');h1=hashlib.sha256(p1.read_bytes()).hexdigest();check('same-renderer fixed-phase PNG reproducibility',h0==h1,[h0,h1])
  b.load('index.html','');check('gallery loaded',b.js("document.title.includes('comparison') && [...document.images].every(i=>i.complete&&i.naturalWidth) && document.documentElement.scrollWidth===innerWidth"));capture(b,'gallery-390x844.png')
 REPORT['status']='PASS';REPORT['summary']={'checks':len(REPORT['checks']),'screenshots':len(REPORT['screenshots']),'contrastSamples':len(REPORT['contrast']),'minimumContrast':min(x['minimum'] for x in REPORT['contrast'])};flush();print(json.dumps(REPORT['summary'],indent=2))
if __name__=='__main__':
 try:run()
 except Exception as e:REPORT['status']='FAIL';REPORT['failure']=str(e);flush();raise
