#!/usr/bin/env python3
"""Fail-closed structural/browser probes. No extra decision images."""
import sys
sys.dont_write_bytecode=True
import glob, subprocess, tempfile, json, re, html
from pathlib import Path
ROOT=Path(__file__).resolve().parent
PROBE=r'''<script>
window.addEventListener('load',()=>{
const checks=[]; const ck=(name,ok,data)=>checks.push({name,ok:!!ok,data});
const box=e=>e.getBoundingClientRect();
ck('no runtime errors',window.errors.length===0,window.errors);
ck('document native width',document.documentElement.scrollWidth===390,document.documentElement.scrollWidth);
ck('all five summary labels in viewport',[...document.querySelectorAll('.rail-summary .ct')].every(e=>box(e).right<=390&&e.scrollWidth<=e.clientWidth),[...document.querySelectorAll('.rail-summary .ct')].map(e=>({text:e.textContent,right:box(e).right})));
ck('raw nameplates not truncated',[...document.querySelectorAll('.nameplate')].every(e=>e.scrollWidth<=e.clientWidth),null);
ck('horse tap zones >=44',[...document.querySelectorAll('.horse-btn')].every(e=>box(e).width>=44&&box(e).height>=44),null);
ck('paddock sprite native dimensions',[...document.querySelectorAll('.field .horse-svg')].every(e=>box(e).width===148&&box(e).height===112),null);
ck('four horizontal paddocks',document.querySelectorAll('.paddock').length===4,null);
ck('two front rail blocked',[...document.querySelectorAll('.rail-row .horse-btn')].length===2&&[...document.querySelectorAll('.rail-row .state-flag')].every(e=>e.textContent.includes('blocked')),null);
const gate=document.querySelector('.physical-rail');
ck('physical gate above horses below labels',gate.parentElement.classList.contains('rail-row')&&getComputedStyle(gate).zIndex==='2'&&[...document.querySelectorAll('.rail-row .horse-svg')].every(e=>getComputedStyle(e).zIndex==='1'&&box(gate).top<box(e).bottom)&&[...document.querySelectorAll('.rail-row .nameplate')].every(e=>box(e).top>=box(gate).bottom),{gate:box(gate).toJSON()});
const strip=document.querySelector('.paddock-strip');strip.style.scrollBehavior='auto';strip.style.scrollSnapType='none';strip.scrollLeft=strip.clientWidth;
ck('horizontal traversal reachable',strip.scrollLeft>0,strip.scrollLeft);
ck('footer within 844',box(document.querySelector('.hint')).bottom<=844,box(document.querySelector('.hint')).bottom);
const out=document.createElement('pre');out.id='probe-result';out.textContent=JSON.stringify(checks);document.body.append(out);
});</script>'''
shell=sorted(glob.glob(str(Path.home()/'Library/Caches/ms-playwright/chromium_headless_shell-*/chrome-headless-shell-mac-arm64/chrome-headless-shell')))[-1]
with tempfile.TemporaryDirectory(prefix='r2-dom-') as tmp:
    for mode in ['day','night']:
        source=(ROOT/(mode+'.html')).read_text().replace('<head>','<head><script>window.errors=[];window.onerror=(...a)=>window.errors.push(String(a[0]));</script>').replace('</body>',PROBE+'</body>')
        p=Path(tmp)/(mode+'.html');p.write_text(source)
        r=subprocess.run([shell,'--headless','--disable-gpu','--no-first-run',f'--user-data-dir={tmp}/{mode}-profile','--window-size=390,844','--virtual-time-budget=1000','--dump-dom',p.as_uri()],capture_output=True,text=True)
        assert r.returncode==0,r.stderr
        match=re.search(r'<pre id="probe-result">(.*?)</pre>',r.stdout,re.S);assert match,'missing executed probe'
        results=json.loads(html.unescape(match.group(1)))
        print(mode,json.dumps(results));assert all(c['ok'] for c in results),results
print('BROWSER PASS; raw_exit=0')
