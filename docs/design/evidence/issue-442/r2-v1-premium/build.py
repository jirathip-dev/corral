#!/usr/bin/env python3
"""Original R2 vector illustration. No network, randomness seeded per world."""
import sys
sys.dont_write_bytecode = True
from pathlib import Path
import re, random, json, argparse
import fixtures as F
ROOT = Path(__file__).resolve().parent
from art import horse, world, rail

CSS='''
.world{position:absolute;inset:0;width:100%;height:100%;pointer-events:none}
.v1 .pasture{background:#52643a;padding-top:108px}
.v1 .rail-row,.v1 .paddock-strip,.v1 .dots,.v1 .hint{z-index:1}
.chrome{background:linear-gradient(125deg,#283143ee,#161e30f5);border-bottom:1px solid #ffffff24}
.seg{background:#ffffff0c;border-color:#ffffff25}.seg button[aria-pressed="true"]{background:#ffffff22;color:#edf0fa;box-shadow:inset 0 1px #ffffff30}
.rail-summary{background:#202a38;gap:3px;padding-left:6px;padding-right:6px;flex-wrap:wrap}.rail-summary .ct{padding:3px 4px;font-size:10px}.rail-summary .lead{display:none}
.v1 .rail-row{background:linear-gradient(transparent,#1b293324);border-bottom:2px solid #b6a078;min-height:160px}
.v1 .rail-row .horse-btn{width:164px}.v1 .rail-row .horse-svg{width:112px;height:85px}
.v1 .paddock-head{background:#1d293bea;margin:-8px -12px 0;padding:0 12px;border-top:1px solid #ffffff24;gap:5px}
.v1 .paddock-name{font-size:16px}.v1 .paddock-count{font-size:10px;color:#cdd6f4}
.v1 .field{background:transparent}.nameplate{color:#f1e9dd;background:#202937f5;border:1px solid #bfa98a66;border-radius:4px}
.state-flag{background:#202937!important;color:#e3e8ef!important}.state-flag.blocked{color:#f6b0b5!important;border-color:#d99c9b}
.v1 .rail-label{color:#ffd4d1;background:#452f39ed;font-size:10px;letter-spacing:.04em}
.v1 .dots{background:#1b2636f0;padding-top:8px}.v1 .hint{background:#1b2636f0;color:#d0d6df;font-size:11px;padding-bottom:12px}
/* Unified native glass label, not floating RPG state markers. */
.horse-btn{padding:0 2px 50px;isolation:isolate;overflow:visible}
.nameplate{position:absolute;bottom:19px;left:50%;transform:translateX(-50%);z-index:3;background:none;border:none;padding:0;font-size:11px!important;line-height:14px;color:#f0ede5;max-width:156px!important}
.state-flag{position:absolute;top:auto;bottom:1px;left:50%;border:none!important;background:none!important;padding:0!important;font-size:11px!important;line-height:16px;z-index:3}
.horse-btn:before{content:"";position:absolute;bottom:-2px;left:4px;right:4px;height:41px;background:linear-gradient(125deg,#273240ec,#172630f5);border:1px solid #c5cdd42e;border-radius:9px;box-shadow:0 2px 5px #19242924;z-index:2}
.horse-btn .horse-svg{filter:drop-shadow(0 -1px 0 #e7dab23b)}
.v1 .rail-row{padding-top:30px;padding-bottom:9px;min-height:208px;border:none;background:none;overflow-y:visible}
.v1 .rail-row .horse-svg{width:148px;height:112px}
.v1 .rail-row::before{display:none}
.physical-rail{position:absolute;bottom:42px;left:0;width:100%;height:44px;z-index:2;pointer-events:none}
.v1 .rail-row .horse-btn{z-index:auto;flex-shrink:0}
.v1 .rail-row .horse-svg{position:relative;z-index:1}
.v1 .rail-label{top:0;background:#2b3445e8;border:1px solid #c8bfb32d;letter-spacing:0;font:600 11px -apple-system,BlinkMacSystemFont,Arial;color:#ffd1cc}
.v1 .paddock-head{margin:0;padding:0 7px;background:#202d3acc;border:1px solid #ffffff24;border-radius:9px;min-height:38px;flex:none}
.v1 .paddock{padding-top:14px}.v1 .paddock-strip{overflow-y:hidden}
.v1 .field{gap:8px 6px}.v1 .paddock-name{font-size:15px}.v1 .paddock-count{font-size:10px}
.moon-rim{display:none}.night .moon-rim{display:block}
.night .horse-svg{filter:drop-shadow(1px -1px 0 #c5d5e030)}
.night .physical-rail{filter:drop-shadow(0 -1px 0 #c4d6e0a0)}
.night .cast-shadow{transform:translate(135px,0) scale(-1,1);transform-origin:0 0}

.leg,.body-g,.neck,.hb i,.state-flag{animation:none!important}.hb i{opacity:.85}
'''

def build(out):
    out.mkdir(parents=True,exist_ok=True)
    control=(ROOT/'control-input.html').read_text()
    names=[]
    def swap(match):
        raw=match.group(0)
        attrs={k:re.search('data-'+k+'="([^"]+)"',raw).group(1) for k in ['coat','breed','mane','tack','accessory']}
        state=re.search('data-state="([^"]+)"',raw).group(1)
        idx=len(names); names.append(attrs)
        return horse(attrs,state,'graze' if state=='idle' else 'stand',f'horse{idx}',-1 if 'scale(-1' in raw else 1)
    control=re.sub(r'<svg class="horse-svg".*?</svg>',swap,control,flags=re.S)
    assert len(names)==12, len(names)
    for mode in ['day','night']:
        text=control.replace('herd-v1-mocha-blocked-heavy',f'V1 R2 · {mode.title()}')
        text=text.replace('</head>',f'<style>{CSS}</style></head>')
        text=text.replace('<main class="pasture">',f'<main class="pasture {mode}">'+world(mode=='night'))
        text=text.replace('<div class="paddock-strip"',rail()+'<div class="paddock-strip"')
        # Physical gate anchored to front-rail row rather than overall pasture.
        text=text.replace(rail(),'')
        marker=text.index('<div class="paddock-strip"')
        close=text.rfind('</div>',0,marker)
        text=text[:close]+rail()+text[close:]
        (out/f'{mode}.html').write_text(text)
    ident=F.horse_identity('willow-bend')
    rows=[]
    for j,(pose,title,note) in enumerate([('stand','Calm standing','Ears and poll distinct from forehead.\nBroad shoulder, tapered neck; long cannon bones.'),('shift','Weight shift','One hind hoof lifts; hock angle stays readable.\nCroup, barrel and identity remain unchanged.'),('graze','Grazing','Broad neck bends to a distinct poll and jaw.\nLong face lowers to ground; legs stay load-bearing.'),('stand','Reduce Motion · selected','Intentional calm standing. No roaming or parallax.\nNot a frozen grazing or transitional pose.')]):
        rows.append(f'<section><div class="specimen">{horse(ident,pose=pose,uid="study"+str(j))}</div><div><h2>{title}</h2><p>{note.replace(chr(10),"<br>")}</p></div></section>')
    study='''<!doctype html><html><head><meta charset="utf-8"><style>*{box-sizing:border-box}body{margin:0;width:600px;height:760px;background:#e9e4d7;color:#28342f;font:14px -apple-system,BlinkMacSystemFont,Arial}header{padding:27px 28px 20px;border-bottom:1px solid #b4b5a2}h1{font-size:23px;margin:8px 0}header p{margin:8px 0;color:#566053;font-size:12px}small{font-size:11px;letter-spacing:.1em}section{height:142px;border-bottom:1px solid #c7c8b6;display:grid;grid-template-columns:180px 1fr;align-items:center;padding:0 24px;gap:12px}.specimen{width:148px;height:112px;background:linear-gradient(transparent 89%,#b8be97 90%,#a3ad88)}.horse-svg{width:148px;height:112px}.moon-rim{display:none}h2{font-size:16px;margin:0 0 10px}section p{font-size:12px;line-height:1.65;margin:0;color:#4f5c4f}footer{font-size:11px;padding:16px 28px;color:#59644f}</style></head><body><header><small>V1 R2 / ANATOMY &amp; POSE STUDY</small><h1>One idle horse. Four intentional poses.</h1><p>willow-bend · original deterministic identity · fictional fixture<br>Every specimen is 148 × 112 px — exactly the V1 paddock sprite size.</p></header>'''+''.join(rows)+'<footer>Roan · draft breed · cropped mane · red pad · no accessory. Original vector art.</footer></body></html>'
    (out/'study.html').write_text(study)
    print('BUILD: day.html night.html study.html; 12 unchanged fixture identities; raw exits 0')

if __name__=='__main__':
    ap=argparse.ArgumentParser(); ap.add_argument('--out',type=Path,default=ROOT); build(ap.parse_args().out)
