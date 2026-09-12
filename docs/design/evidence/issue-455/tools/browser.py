"""Owned headless Chrome CDP driver; no shared browser/server/config changes."""
from pathlib import Path
import subprocess,json,time,urllib.request,websocket,shutil,base64,os
ROOT=Path(__file__).resolve().parents[1]
# Portability: base URL of the served copy (default = the canonical existing
# gallery server). Override with CORRAL455_BASE when serving a copy elsewhere.
BASE=os.environ.get('CORRAL455_BASE','http://127.0.0.1:8777/corral/455-immersive-herd')
class Browser:
 def __init__(self):
  self.runtime=ROOT/'tools/.runtime';self.runtime.mkdir(parents=True,exist_ok=True)
  self.profile=self.runtime/f'profile-{os.getpid()}';self.profile.mkdir()
  shells=sorted((Path.home()/'Library/Caches/ms-playwright').glob('chromium_headless_shell-*/chrome-headless-shell-mac-arm64/chrome-headless-shell'));assert shells
  self.shell=str(shells[-1]);self.log=open(self.runtime/'chrome.log','w')
  self.proc=subprocess.Popen([self.shell,'--headless','--no-first-run','--hide-scrollbars','--remote-debugging-port=0','--remote-allow-origins=*',f'--user-data-dir={self.profile}','about:blank'],stdout=self.log,stderr=self.log)
  portfile=self.profile/'DevToolsActivePort'
  until=time.monotonic()+15
  while not portfile.exists():
   assert self.proc.poll() is None,'Chrome exited'
   assert time.monotonic()<until,'Chrome did not start'
   time.sleep(.05)
  self.port=int(portfile.read_text().splitlines()[0]);targets=json.load(urllib.request.urlopen(f'http://127.0.0.1:{self.port}/json'))
  self.ws=websocket.create_connection(next(x['webSocketDebuggerUrl'] for x in targets if x['type']=='page'),timeout=20)
  self.n=0;self.errors=[];self.events=[]
  self.call('Page.enable');self.call('Runtime.enable');self.call('Log.enable')
  self.call('Page.addScriptToEvaluateOnNewDocument',source="window.__errors=[];addEventListener('error',e=>__errors.push(e.message));addEventListener('unhandledrejection',e=>__errors.push(String(e.reason)));")
  self.size(390,844)
 def call(self,method,**params):
  self.n+=1;n=self.n;self.ws.send(json.dumps({'id':n,'method':method,'params':params}))
  while True:
   r=json.loads(self.ws.recv())
   if 'method' in r:
    if r['method']=='Runtime.exceptionThrown':self.errors.append(r)
    if r['method']=='Log.entryAdded' and r['params']['entry']['level']=='error':self.errors.append(r)
    self.events.append(r)
   if r.get('id')==n:
    if 'error' in r:raise RuntimeError(r['error'])
    return r.get('result',{})
 def js(self,code):
  r=self.call('Runtime.evaluate',expression=code,returnByValue=True,awaitPromise=True)
  if 'exceptionDetails' in r:raise RuntimeError(r['exceptionDetails'])
  return r.get('result',{}).get('value')
 def size(self,w,h):self.call('Emulation.setDeviceMetricsOverride',width=w,height=h,deviceScaleFactor=1,mobile=True)
 def load(self,path='variant-b.html',params='preview=herd&still=1'):
  self.errors=[];self.events=[]
  self.call('Page.navigate',url=f'{BASE}/{path}?{params}')
  end=time.monotonic()+20
  while time.monotonic()<end:
   if self.js("document.readyState==='complete' && (typeof PROOF!=='undefined'||document.title.includes('comparison'))"):break
   time.sleep(.1)
  else:raise RuntimeError('Page load timeout')
  self.js("document.fonts.ready")
 def click(self,selector):
  b=self.js(f"(()=>{{const e=document.querySelector({json.dumps(selector)});if(!e)throw Error('missing target');e.scrollIntoView({{block:'nearest'}});const r=e.getBoundingClientRect();return {{x:r.x+r.width/2,y:r.y+r.height/2,disabled:e.disabled}}}})()")
  assert not b['disabled'],f'disabled {selector}'
  self.call('Input.dispatchMouseEvent',type='mousePressed',x=b['x'],y=b['y'],button='left',clickCount=1)
  self.call('Input.dispatchMouseEvent',type='mouseReleased',x=b['x'],y=b['y'],button='left',clickCount=1)
  time.sleep(.08)
  self.js('document.readyState')
 def screenshot(self,path):
  data=self.call('Page.captureScreenshot',format='png',captureBeyondViewport=False)['data'];p=ROOT/path;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(base64.b64decode(data));return p
 def close(self):
  self.ws.close();self.proc.terminate();self.proc.wait(timeout=10);self.log.close();shutil.rmtree(self.profile)
 def __enter__(self):return self
 def __exit__(self,*args):self.close()
if __name__=='__main__':
 with Browser() as b:
  b.load();print(b.js('PROOF.state()'));b.screenshot('evidence/initial-b.png')
  print(b.js("JSON.stringify({hud:document.querySelector('#hud').getBoundingClientRect(),content:document.querySelector('#herd-content').getBoundingClientRect(),bottom:document.querySelector('#bottom').getBoundingClientRect(),errors:__errors})"))
