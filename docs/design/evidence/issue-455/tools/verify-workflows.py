"""Shared-scope and keyboard interaction probes on actual loaded A/B pages."""
from browser import Browser,ROOT
from PIL import Image,ImageDraw,ImageFont
import json
checks=[]
def ck(name,value):
 checks.append({'name':name,'pass':bool(value)});assert value,name
with Browser() as b:
 for v in ['a','b']:
  b.load(f'variant-{v}.html','preview=herd&still=1');b.click('#next');b.click('#settings');b.click('[data-action=mode][data-value=Board]');b.click('[data-action=mode][data-value=Herd]');b.click('#close-sheet');ck(v+' mode round-trip retains paddock',b.js('PROOF.state().paddock')==1)
  b.click('#scope');b.click('[data-action=host][data-value=Meadow]');b.click('[data-action=repo][data-value="cedar-observatory-client"]');b.click('#close-sheet');expected=b.js("[...document.querySelectorAll('#field [data-horse]')].map(e=>e.dataset.horse).sort()")
  b.click('#settings');b.click('[data-action=mode][data-value=Board]');b.click('#close-sheet');actual=b.js("[...document.querySelectorAll('#board-rows [data-horse]')].map(e=>e.dataset.horse).sort()")
  ck(v+' shared filtered identity projection',actual==expected and len(actual)==2)
  b.click('#board-rows [data-horse]');ck(v+' Board shares output destination',b.js("document.querySelector('#sheet-title').textContent==='Recent Output' && document.querySelector('#sheet-content').textContent.includes('SYNTHETIC PLACEHOLDER')"));b.click('#close-sheet')
  b.click('#scope')
  for i in range(24):
   b.call('Input.dispatchKeyEvent',type='keyDown',key='Tab',code='Tab',windowsVirtualKeyCode=9);b.call('Input.dispatchKeyEvent',type='keyUp',key='Tab',code='Tab',windowsVirtualKeyCode=9)
   ck(v+f' modal keyboard containment {i}',b.js("!!document.activeElement.closest('#sheet') || document.activeElement===document.body"))
  b.call('Input.dispatchKeyEvent',type='keyDown',key='Escape',code='Escape',windowsVirtualKeyCode=27);b.call('Input.dispatchKeyEvent',type='keyUp',key='Escape',code='Escape',windowsVirtualKeyCode=27)
  ck(v+' Escape closes sheet',not b.js("document.querySelector('#sheet').open"))
  b.click('#scope');b.click('[data-action=reset]');b.click('#close-sheet')
  p=b.screenshot(f'evidence/{v}-board-390x844.png');im=Image.open(p).convert('RGB');d=ImageDraw.Draw(im);d.rectangle((0,0,390,12),fill='#181825');d.text((7,1),f'{v.upper()} BOARD 390x844 | PROTOTYPE SYNTHETIC PROOF',fill='#cdd6f4',font=ImageFont.load_default(size=9));im.save(p)
(ROOT/'evidence/workflow-verification.json').write_text(json.dumps({'status':'PASS','checks':checks},indent=2)+'\n')
print('PASS',len(checks),'shared-scope and keyboard checks')
