#!/usr/bin/env python3
"""issue-462 — deterministic horse app-icon prototype generator.

Produces, under docs/design/evidence/issue-462-horse-icons/:
  masters/    unrounded opaque 1024x1024 PNGs — treatment A (head-and-neck
              portrait) and B (tight full horse) x {original, bay, palomino,
              black, grey}. The Original is a BYTE-EXACT copy of the shipping
              ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset icon.
  previews/   phone-icon-size previews (masked 180/120, plain 060). Masks are
              PREVIEW-ONLY approximations (22.5% rounded rect, not Apple's
              exact squircle).
  html/       index.html gallery + stage HTML sources for the rendered PNGs.
  contexts/   390x844 Home Screen + Settings-picker PNGs (light/dark, A/B).
  build-summary.json  counts + hashes consumed by tools/verify_bundle.py.

Art provenance: horses are rendered from the CANONICAL generator
ios/tools/herd-art/horsesvg.py (imported unmodified; sha256 recorded).
No horse art is authored in this file. Crops are viewBox windows in
canonical 132x100 coordinates, derived from parsed geometry with fixed
constants, identical for every coat within a treatment.

Deterministic: stdlib + Pillow only, no timestamps, fixed supersampling.
"""
from __future__ import annotations

import hashlib
import json
import math
import shutil
import subprocess
import sys
from pathlib import Path
from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent            # .../issue-462-horse-icons/tools
ISSUE = HERE.parent                                # .../issue-462-horse-icons
ROOT = ISSUE.parents[3]                            # worktree root
sys.path.insert(0, str(ROOT / 'ios' / 'tools' / 'herd-art'))
sys.path.insert(0, str(HERE))
import horsesvg                                    # canonical generator, unmodified
import svgmini

# ---- fixed design constants (documented in README) ----
CANVAS = 1024
SS = 2                     # supersample factor (draw 2048, downscale LANCZOS)
POSE = 'stand'             # canonical static stand (reduce-motion equivalent)
FACING = -1                # mirror: face left, matching the Original icon
BREED, MANE, TACK, ACC = 'light', 'flowing', 'none', 'none'
COATS = ['bay', 'palomino', 'black', 'grey']
A_PAD = 1.30               # A window side = neck-bbox max-dim * 1.30
B_PAD = 1.06               # B window side = full-bbox max-dim * 1.06
BG = {                     # opaque ranch background per coat (contrast-checked;
                           # luminance-opposed to the coat: light coats sit on
                           # dark grounds, dark coats on light grounds)
    'bay':      ('#ecdcb4', 'hay'),
    'palomino': ('#1f2a38', 'night pasture'),
    'black':    ('#d9c39a', 'sand'),
    'grey':     ('#22302a', 'deep pine'),
}
ORIGINAL_SRC = ROOT / 'ios' / 'FleetNotifier' / 'Assets.xcassets' / \
    'AppIcon.appiconset' / 'AppIcon-512@2x.png'
SHELL_GLOB = sorted(Path.home().glob(
    'Library/Caches/ms-playwright/chromium_headless_shell-*/'
    'chrome-headless-shell-mac-arm64/chrome-headless-shell'))
MASK_RADIUS = 0.225        # preview-only rounded-rect mask (22.5%)
PREVIEW_SPECS = [('mask', 180), ('mask', 120), ('plain', 60)]


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def hexrgb(h: str):
    h = h.lstrip('#')
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4))


def identity(coat: str) -> dict:
    return {'coat': coat, 'breed': BREED, 'mane': MANE, 'tack': TACK,
            'accessory': ACC}


# ---------- rendering ----------

def render_window(prims, crop, bg_hex, out: Path, size=CANVAS):
    """Rasterize canonical primitives through the crop window onto an opaque
    background. crop = (cx, cy, side) in canonical 132x100 units.
    Painter's algorithm in canonical order: opaque fills overwrite (RGB-exact),
    semi-transparent prims composite via an RGBA layer."""
    cx, cy, side = crop
    big = size * SS
    scale = big / side
    base = Image.new('RGBA', (big, big), hexrgb(bg_hex) + (255,))

    def to_px(x, y):
        return ((x - (cx - side / 2)) * scale, (y - (cy - side / 2)) * scale)

    for p in prims:
        alpha = int(round(255 * p['opacity']))
        semi = alpha < 255
        layer = Image.new('RGBA', (big, big), (0, 0, 0, 0)) if semi else None
        target = ImageDraw.Draw(layer) if semi else ImageDraw.Draw(base)
        fill = p['fill']
        stroke = p['stroke']
        if fill not in ('none', '') and alpha > 0:
            rgba = hexrgb(fill) + ((alpha,) if semi else (255,))
            for sp in p['subpaths']:
                pts = [to_px(x, y) for x, y in sp]
                if len(pts) >= 3:
                    target.polygon(pts, fill=rgba)
        if stroke not in ('none', '') and alpha > 0:
            rgba = hexrgb(stroke) + ((alpha,) if semi else (255,))
            w = max(1, int(round(p['width'] * scale)))
            for sp in p['subpaths']:
                pts = [to_px(x, y) for x, y in sp]
                target.line(pts, fill=rgba, width=w, joint='curve')
                # round caps (canonical stroke-linecap="round")
                r = w / 2.0
                for ex, ey in (pts[0], pts[-1]):
                    target.ellipse((ex - r, ey - r, ex + r, ey + r), fill=rgba)
        if semi:
            base = Image.alpha_composite(base, layer)
    base.convert('RGB').resize((size, size), Image.LANCZOS).save(out, 'PNG')


def crop_windows(prims):
    """Derive A/B windows from parsed canonical geometry (same for all coats;
    geometry is asserted identical across coats by the caller)."""
    nx0, ny0, nx1, ny1 = svgmini.bbox(prims, exclude_groups=('body',))
    fx0, fy0, fx1, fy1 = svgmini.bbox(prims)
    a_side = max(nx1 - nx0, ny1 - ny0) * A_PAD
    b_side = max(fx1 - fx0, fy1 - fy0) * B_PAD
    a = ((nx0 + nx1) / 2, (ny0 + ny1) / 2, a_side)
    b = ((fx0 + fx1) / 2, (fy0 + fy1) / 2, b_side)
    return a, b, (nx0, ny0, nx1, ny1), (fx0, fy0, fx1, fy1)


def mask_preview(src: Path, out: Path, size: int):
    im = Image.open(src).convert('RGB').resize((size, size), Image.LANCZOS)
    mask = Image.new('L', (size, size), 0)
    r = round(MASK_RADIUS * size)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size - 1, size - 1),
                                           radius=r, fill=255)
    rgba = im.convert('RGBA')
    rgba.putalpha(mask)
    rgba.save(out, 'PNG')


def plain_preview(src: Path, out: Path, size: int):
    Image.open(src).convert('RGB').resize((size, size),
                                          Image.LANCZOS).save(out, 'PNG')


# ---------- contrast ----------

def rel_lum(hx: str) -> float:
    def lin(c):
        c /= 255
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4
    r, g, b = hexrgb(hx)
    return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)


def contrast(a: str, b: str) -> float:
    la, lb = rel_lum(a), rel_lum(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


# ---------- stage HTML (home screen + picker) ----------

def icon_img(treat, coat, size_px):
    return (f'<img src="../../masters/treatment-{treat}/{coat}-1024.png" '
            f'alt="" style="width:{size_px}px;height:{size_px}px;'
            f'border-radius:{round(size_px * MASK_RADIUS)}px;'
            f'display:block">')

HOME_CSS = """
*{margin:0;padding:0;box-sizing:border-box}
html,body{width:390px;height:844px;overflow:hidden}
body{font-family:-apple-system,'Helvetica Neue',Arial,sans-serif;
 -webkit-font-smoothing:antialiased;position:relative}
.phone{width:390px;height:844px;position:relative;overflow:hidden}
.wall{position:absolute;inset:0}
.status{position:absolute;top:14px;left:0;right:0;height:20px;display:flex;
 justify-content:space-between;align-items:center;padding:0 26px;
 font-size:14px;font-weight:600;color:var(--fg)}
.batt{width:24px;height:11px;border:1px solid var(--fg);border-radius:3px;
 position:relative;opacity:.9}
.batt:after{content:'';position:absolute;inset:1.5px;right:5px;
 background:var(--fg);border-radius:1px}
.sig{letter-spacing:1px;font-size:11px}
.band{position:absolute;top:150px;left:0;right:0;text-align:center;
 color:var(--fg)}
.band h1{font-size:16px;font-weight:700}
.band p{font-size:11px;opacity:.75;margin-top:3px}
.row{position:absolute;top:210px;left:0;right:0;display:flex;
 justify-content:center;gap:9px}
.tile{width:64px;display:flex;flex-direction:column;align-items:center;
 padding-top:4px;padding-bottom:6px;border-radius:12px}
.tile .lbl{font-size:9.5px;margin-top:5px;color:var(--fg);font-weight:500;
 text-shadow:0 1px 2px rgba(0,0,0,.35);white-space:nowrap}
.grid{position:absolute;top:318px;left:20px;right:20px;display:grid;
 grid-template-columns:repeat(4,1fr);gap:18px 16px}
.app{display:flex;flex-direction:column;align-items:center}
.app .ic{width:58px;height:58px;border-radius:13px}
.app .lb{font-size:9.5px;margin-top:4px;color:var(--fg);font-weight:500;
 text-shadow:0 1px 2px rgba(0,0,0,.35)}
.dots{position:absolute;bottom:130px;left:0;right:0;display:flex;gap:7px;
justify-content:center}
.dot{width:6px;height:6px;border-radius:3px;background:var(--fg);opacity:.35}
.dot.on{opacity:.9}
.dock{position:absolute;bottom:16px;left:10px;right:10px;height:88px;
 border-radius:26px;background:var(--dock);display:flex;align-items:center;
 justify-content:space-around;padding:0 8px}
.dock .ic{width:58px;height:58px;border-radius:13px}
.proto{position:absolute;bottom:106px;left:0;right:0;text-align:center;
 font-size:8.5px;color:var(--fg);opacity:.55;letter-spacing:.2px}
"""

PICKER_CSS = """
*{margin:0;padding:0;box-sizing:border-box}
html,body{width:390px;height:844px;overflow:hidden}
body{font-family:-apple-system,'Helvetica Neue',Arial,sans-serif;
 -webkit-font-smoothing:antialiased;background:var(--pg)}
.nav{position:absolute;top:0;left:0;right:0;height:90px;
 background:var(--nb);border-bottom:1px solid var(--sep)}
.nav .back{position:absolute;left:16px;bottom:24px;font-size:17px;
 color:var(--blue)}
.nav .ttl{position:absolute;left:0;right:0;bottom:24px;text-align:center;
 font-size:17px;font-weight:600;color:var(--fg)}
.hdr{position:absolute;top:104px;left:20px;right:20px;font-size:12px;
 font-weight:600;letter-spacing:.5px;color:var(--mut);text-transform:uppercase}
.list{position:absolute;top:128px;left:0;right:0;background:var(--card);
 border-top:1px solid var(--sep)}
.row{min-height:60px;display:flex;align-items:center;padding:8px 16px;
 border-bottom:1px solid var(--sep);gap:12px}
.row img{width:29px;height:29px;border-radius:6.5px;display:block}
.row .nm{flex:1;font-size:17px;color:var(--fg)}
.row .sub{font-size:12px;color:var(--mut);margin-top:1px}
.chk{width:18px;height:18px;color:var(--blue);font-size:19px;font-weight:700;
 line-height:18px;text-align:center}
.ftr{position:absolute;top:472px;left:20px;right:20px;font-size:12px;
 color:var(--mut);line-height:1.45}
.note{position:absolute;bottom:24px;left:0;right:0;text-align:center;
 font-size:9.5px;color:var(--mut);opacity:.8}
"""

THEMES = {
    'light': {
        'home': {'--fg': '#1c1c1e', '--dock': 'rgba(245,245,245,.55)'},
        'picker': {'--pg': '#f2f2f7', '--nb': '#f9f9f9', '--sep':
                   'rgba(60,60,67,.29)', '--fg': '#000', '--mut':
                   'rgba(60,60,67,.6)', '--blue': '#007aff',
                   '--card': '#fff'},
    },
    'dark': {
        'home': {'--fg': '#fff', '--dock': 'rgba(40,40,45,.55)'},
        'picker': {'--pg': '#000', '--nb': '#1c1c1e', '--sep':
                   'rgba(84,84,88,.65)', '--fg': '#fff', '--mut':
                   'rgba(235,235,245,.6)', '--blue': '#0a84ff',
                   '--card': '#1c1c1e'},
    },
}
WALL = {
    'light': 'linear-gradient(180deg,#a7c4dc 0%,#cfd9c8 55%,#e8d9b8 100%)',
    'dark': 'linear-gradient(180deg,#232c3d 0%,#171d29 55%,#0d1117 100%)',
}
CHOICES = [('original', 'Original')] + [(c, c.capitalize()) for c in COATS]


def css_vars(theme, screen):
    return '\n'.join(f'{k}:{v};' for k, v in THEMES[theme][screen].items())


def home_html(treat, theme):
    tiles = ''.join(
        f'<div class="tile">{icon_img(treat, coat, 60)}'
        f'<div class="lbl">{name}</div></div>'
        for coat, name in CHOICES)
    grays = ['#9aa2ad', '#8d9688', '#a89a8d', '#7f8a99',
             '#a3ad98', '#98867a', '#87909f', '#9c948a']
    grid = ''.join(
        f'<div class="app"><div class="ic" style="background:{c}"></div>'
        f'<div class="lb">App {i + 1}</div></div>'
        for i, c in enumerate(grays))
    dock = ''.join(f'<div class="ic" style="background:{c}"></div>'
                   for c in ['#b0b6bd', '#9aa78f', '#b8a58c', '#8f9aa8'])
    title = f'Treatment {treat.upper()} — ' + (
        'head & neck portrait' if treat == 'a' else 'tight full horse')
    return ('<!doctype html><html><head><meta charset="utf-8"><style>'
            ':root{' + css_vars(theme, 'home') + '}' + HOME_CSS +
            f'.wall{{background:{WALL[theme]}}}' +
            '</style></head><body><div class="phone">'
            '<div class="wall"></div>'
            '<div class="status"><span>9:41</span>'
            '<span style="display:flex;gap:6px;align-items:center">'
            '<span class="sig">\u25b4\u25b4\u25b6</span>'
            '<span class="batt"></span></span></div>'
            f'<div class="band"><h1>{title}</h1>'
            '<p>Corral icon choices on this wallpaper</p></div>'
            f'<div class="row">{tiles}</div>'
            f'<div class="grid">{grid}</div>'
            '<div class="dots"><div class="dot on"></div>'
            '<div class="dot"></div><div class="dot"></div></div>'
            '<div class="proto">PROTOTYPE \u2014 does not change the real '
            'app icon</div>'
            f'<div class="dock">{dock}</div>'
            '</div></body></html>')


def picker_html(treat, theme):
    rows = []
    for i, (coat, name) in enumerate(CHOICES):
        sub = ('Current icon (unchanged)' if coat == 'original'
               else f'Horse alternative \u00b7 treatment {treat.upper()}')
        chk = '<div class="chk">\u2713</div>' if i == 0 else \
              '<div style="width:18px"></div>'
        rows.append(
            f'<div class="row">{icon_img(treat, coat, 29)}'
            f'<div class="nm">{name}<div class="sub">{sub}</div></div>{chk}'
            f'</div>')
    return ('<!doctype html><html><head><meta charset="utf-8"><style>'
            ':root{' + css_vars(theme, 'picker') + '}' + PICKER_CSS +
            '</style></head><body>'
            '<div class="nav"><div class="back">\u2039 Settings</div>'
            '<div class="ttl">Appearance</div></div>'
            '<div class="hdr">App Icon</div>'
            f'<div class="list">{"".join(rows)}</div>'
            '<div class="ftr">Choose the Corral app icon. Original restores '
            'the current default icon. Changes apply to the Home Screen '
            'after you confirm.</div>'
            '<div class="note">PROTOTYPE mockup \u2014 not a live Settings '
            'screen; changes no real icon</div>'
            '</body></html>')


def render_stage(html_path: Path, png_out: Path):
    shell = SHELL_GLOB[-1]
    tmp = Path('/tmp/corral-462-render.png')
    cmd = [str(shell), '--headless', '--disable-gpu', '--no-first-run',
           '--hide-scrollbars', '--allow-file-access-from-files',
           f'--user-data-dir=/tmp/corral-462-profile',
           '--window-size=390,844', '--force-device-scale-factor=2',
           f'--screenshot={tmp}', html_path.as_uri()]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    if r.returncode != 0:
        raise RuntimeError(f'chrome failed rc={r.returncode}: {r.stderr[-400:]}')
    subprocess.run(['sips', '-z', '844', '390', str(tmp),
                    '--out', str(png_out)], check=True, capture_output=True)


# ---------- index.html gallery ----------

def index_html(treats_meta):
    return """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Corral #462 — horse app icon prototype</title>
<style>
:root{--ink:#2f2a26;--paper:#f6f1e7;--card:#fff;--accent:#8a5a33;
 --mut:#6f675e;--sep:#e2d9c8}
*{margin:0;padding:0;box-sizing:border-box}
body{font-family:-apple-system,'Helvetica Neue',Arial,sans-serif;background:
 var(--paper);color:var(--ink);padding:20px 16px 60px}
.wrap{max-width:720px;margin:0 auto}
h1{font-size:22px;margin-bottom:4px}
.badge{display:inline-block;background:#f3e2c8;color:#7a4f2c;border:1px solid
 #d8b46a;border-radius:999px;padding:4px 12px;font-size:12px;font-weight:600;
 margin:8px 0 4px}
p.lead{color:var(--mut);font-size:14px;line-height:1.5;margin:8px 0 20px}
.controls{display:flex;gap:10px;flex-wrap:wrap;margin-bottom:22px}
button{min-height:44px;min-width:44px;padding:10px 18px;border-radius:12px;
 border:1.5px solid var(--sep);background:var(--card);font-size:15px;
 font-weight:600;color:var(--ink);cursor:pointer}
button[aria-pressed="true"]{background:var(--accent);color:#fff;
 border-color:var(--accent)}
button:focus-visible{outline:3px solid #b06a2f;outline-offset:2px}
h2{font-size:17px;margin:26px 0 10px}
.tiles{display:grid;grid-template-columns:repeat(auto-fill,minmax(96px,1fr));
 gap:14px}
.tile{background:var(--card);border:1.5px solid var(--sep);border-radius:16px;
 padding:12px 8px 10px;text-align:center;cursor:pointer}
.tile[aria-pressed="true"]{border-color:var(--accent);box-shadow:0 0 0 2px
 var(--accent)}
.tile img{width:72px;height:72px;border-radius:16px;display:block;margin:0
 auto 6px}
.tile .nm{font-size:13px;font-weight:600}
.tile .sz{font-size:11px;color:var(--mut)}
.shots{display:grid;grid-template-columns:repeat(auto-fill,minmax(170px,1fr));
 gap:14px}
.shot{background:var(--card);border:1.5px solid var(--sep);border-radius:16px;
 padding:10px}
.shot img{width:100%;border-radius:10px;display:block}
.shot .cap{font-size:12px;color:var(--mut);margin-top:6px;text-align:center}
.small{display:flex;gap:10px;flex-wrap:wrap;align-items:flex-end}
.small figure{text-align:center}
.small img{display:block;border-radius:14px}
.small figcaption{font-size:11px;color:var(--mut);margin-top:4px}
footer{margin-top:34px;font-size:12px;color:var(--mut);line-height:1.55;
 border-top:1px solid var(--sep);padding-top:14px}
@media (prefers-reduced-motion: reduce){*{transition:none!important}}
</style>
</head>
<body>
<div class="wrap">
<h1>Corral #462 — selectable horse app icons</h1>
<span class="badge">PROTOTYPE — awaiting Guy icon-art approval</span>
<p class="lead">Design gate comparing <b>Treatment A</b> (large
head-and-neck portrait, recommended) vs <b>Treatment B</b> (tightly framed
full horse), each shown as the unchanged Original plus Bay, Palomino, Black
and Grey alternatives rendered from the canonical herd-art generator.
This prototype changes no real icon, asset, or app source.</p>

<div class="controls" role="group" aria-label="Treatment">
<button id="ta" aria-pressed="true">A · Portrait</button>
<button id="tb" aria-pressed="false">B · Full horse</button>
<button id="tz" aria-pressed="false" aria-label="Toggle dark preview
 background">Dark bg</button>
</div>

<h2 id="choices-h">The five choices — treatment A</h2>
<p class="lead" id="crop-note"></p>
<div class="tiles" id="tiles" role="group" aria-label="Icon choices"></div>

<h2>Small-size recognition</h2>
<div class="small" id="small"></div>

<h2>Home Screen contexts (390×844)</h2>
<div class="shots" id="home"></div>

<h2>Settings → Appearance → App Icon (mockup)</h2>
<div class="shots" id="picker"></div>

<footer>Prototype only: nothing here changes the shipping app icon or asset
catalog — the Original icon bytes are preserved byte-exact and no app source
is modified. All art is rendered from the canonical
ios/tools/herd-art/horsesvg.py geometry and palettes. Return to Guy for a
NEW explicit icon-art approval; issue #455 V1/A approval does not cover
these compositions.</footer>
</div>
<script>
'use strict';
var COATS=[['original','Original'],['bay','Bay'],['palomino','Palomino'],
 ['black','Black'],['grey','Grey']];
var TREAT='a',DARK=false,PICK=null;
function $(id){return document.getElementById(id)}
function img(t,c){return 'masters/treatment-'+t+'/'+c+'-1024.png'}
function prev(t,c,s){return 'previews/'+s+'-treatment-'+t+'-'+c+'.png'}
function ctx(t,theme,kind){return 'contexts/'+kind+'-'+t+'-'+theme+
 '-390x844.png'}
function render(){
 $('choices-h').textContent='The five choices — treatment '+TREAT.toUpperCase();
 $('crop-note').textContent=TREAT==='a'
  ?'A — square window centered on the canonical neck+head group, identical for every coat. See README for the exact rule.'
  :'B — square window around the full canonical horse incl. ground shadow, identical for every coat. See README for the exact rule.';
 var t=$('tiles');t.innerHTML='';
 COATS.forEach(function(pair){
  var c=pair[0],n=pair[1];
  var d=document.createElement('button');
  d.className='tile';d.setAttribute('aria-pressed',String(PICK===c));
  d.setAttribute('aria-label',n+' icon, treatment '+TREAT.toUpperCase());
  d.innerHTML='<img src="'+img(TREAT,c)+'" alt="">'+
   '<div class="nm">'+n+'</div><div class="sz">'+(c==='original'
   ?'unchanged bytes':'proposal')+'</div>';
  d.onclick=function(){PICK=(PICK===c?null:c);render()};
  t.appendChild(d)});
 var s=$('small');s.innerHTML='';
 [180,120,60].forEach(function(sz){
  COATS.forEach(function(pair){
   var f=document.createElement('figure');
   f.innerHTML='<img src="'+prev(TREAT,pair[0],'mask'+sz)+'" width="'+
    Math.round(sz/2.5)+'" height="'+Math.round(sz/2.5)+'" alt="'+pair[1]+
    ' at '+sz+'px">'+
    '<figcaption>'+pair[1]+' · '+sz+'px</figcaption>';
   s.appendChild(f)})});
 var h=$('home');h.innerHTML='';
 ['light','dark'].forEach(function(th){
  var f=document.createElement('div');f.className='shot';
  f.innerHTML='<img src="'+ctx(TREAT,th,'home')+'" alt="Home Screen '+th+
   ' context, treatment '+TREAT.toUpperCase()+'">'+
   '<div class="cap">Home Screen · '+th+'</div>';h.appendChild(f)});
 var p=$('picker');p.innerHTML='';
 ['light','dark'].forEach(function(th){
  var f=document.createElement('div');f.className='shot';
  f.innerHTML='<img src="'+ctx(TREAT,th,'picker')+'" alt="Settings picker '+
   th+' mockup, treatment '+TREAT.toUpperCase()+'">'+
   '<div class="cap">Settings picker · '+th+'</div>';p.appendChild(f)});
 $('ta').setAttribute('aria-pressed',String(TREAT==='a'));
 $('tb').setAttribute('aria-pressed',String(TREAT==='b'));
 $('tz').setAttribute('aria-pressed',String(DARK));
 document.body.style.background=DARK?'#221d18':'';
 document.body.style.color=DARK?'#f0e9dd':'';
}
$('ta').onclick=function(){TREAT='a';render()};
$('tb').onclick=function(){TREAT='b';render()};
$('tz').onclick=function(){DARK=!DARK;render()};
window.addEventListener('hashchange',function(){
 var m=location.hash.match(/t=([ab])/);if(m&&m[1]!==TREAT){TREAT=m[1];render()}});
if(location.hash.indexOf('t=b')>=0){TREAT='b'}
render();
var hs = document.documentElement.scrollWidth>390?'overflow':'ok';
document.body.setAttribute('data-hscroll',hs);
console.log('HSCROLL:'+hs);
</script>
</body>
</html>
"""


# ---------- main ----------

def main():
    assert ISSUE.name == 'issue-462-horse-icons', ISSUE
    assert (ROOT / 'ios/tools/herd-art/horsesvg.py').exists()
    for m in ('Image', 'ImageDraw'):
        assert hasattr(Image, m) or hasattr(ImageDraw, m)
    assert SHELL_GLOB, 'chrome-headless-shell not found'

    for sub in ('masters/treatment-a', 'masters/treatment-b', 'previews',
                'contexts', 'html/stage'):
        (ISSUE / sub).mkdir(parents=True, exist_ok=True)

    canonical_sha = sha256(ROOT / 'ios/tools/herd-art/horsesvg.py')
    original_sha = sha256(ORIGINAL_SRC)

    # geometry: identical across coats (assert), windows derived once
    prims_by_coat = {}
    for coat in COATS:
        svg = horsesvg.horse_svg(identity(coat), POSE, False, FACING, False)
        prims = svgmini.parse_primitives(svg)
        for p in prims:                       # no text baked into icons
            assert 'text' not in p.get('kind', '')
        prims_by_coat[coat] = prims
    ref = prims_by_coat['bay']
    a_win, b_win, a_bbox, b_bbox = crop_windows(ref)
    for coat in COATS[1:]:
        assert crop_windows(prims_by_coat[coat])[:2] == (a_win, b_win)

    summary = {
        'canonical_source': 'ios/tools/herd-art/horsesvg.py',
        'canonical_sha256': canonical_sha,
        'original_source': str(ORIGINAL_SRC.relative_to(ROOT)),
        'original_sha256': original_sha,
        'pose': POSE, 'facing': FACING, 'breed': BREED, 'mane': MANE,
        'tack': TACK, 'accessory': ACC,
        'crops': {'A': {'window': list(a_win), 'neck_bbox': list(a_bbox),
                        'rule': 'side = neck-bbox max-dim * %.2f' % A_PAD},
                  'B': {'window': list(b_win), 'full_bbox': list(b_bbox),
                        'rule': 'side = full-bbox max-dim * %.2f' % B_PAD}},
        'backgrounds': {c: {'hex': BG[c][0], 'name': BG[c][1]} for c in COATS},
        'masters': [], 'previews': [], 'contexts': [],
        'contrast': {}, 'counts': {},
    }

    # contrast table
    part_colors = {'body': None, 'mane': None, 'hoof': horsesvg.HOOF,
                   'muzzle': horsesvg.MUC, 'eye': horsesvg.EYE}
    for coat in COATS:
        body, dark = horsesvg.COAT[coat]
        mane = horsesvg.MANE_C[coat]
        bg = BG[coat][0]
        summary['contrast'][coat] = {
            'bg': bg,
            'bg_vs_body': round(contrast(bg, body), 2),
            'bg_vs_shade': round(contrast(bg, dark), 2),
            'bg_vs_mane': round(contrast(bg, mane), 2),
            'bg_vs_hoof': round(contrast(bg, horsesvg.HOOF), 2),
            'bg_vs_muzzle': round(contrast(bg, horsesvg.MUC), 2),
        }

    # masters + previews
    for treat, win in (('a', a_win), ('b', b_win)):
        for coat in COATS:
            out = ISSUE / 'masters' / f'treatment-{treat}' / f'{coat}-1024.png'
            render_window(prims_by_coat[coat], win, BG[coat][0], out)
            summary['masters'].append(str(out.relative_to(ISSUE)))
        orig = ISSUE / 'masters' / f'treatment-{treat}' / 'original-1024.png'
        shutil.copyfile(ORIGINAL_SRC, orig)
        assert sha256(orig) == original_sha, 'original must stay byte-exact'
        summary['masters'].append(str(orig.relative_to(ISSUE)))
        for coat, _ in CHOICES:
            src = ISSUE / 'masters' / f'treatment-{treat}' / f'{coat}-1024.png'
            for kind, sz in PREVIEW_SPECS:
                po = ISSUE / 'previews' / f'{kind}{sz}-treatment-{treat}-{coat}.png'
                if kind == 'mask':
                    mask_preview(src, po, sz)
                else:
                    plain_preview(src, po, sz)
                summary['previews'].append(str(po.relative_to(ISSUE)))

    # stage HTML + rendered contexts
    screens = []
    for treat in ('a', 'b'):
        for theme in ('light', 'dark'):
            for kind, fn in (('home', home_html), ('picker', picker_html)):
                hp = ISSUE / 'html' / 'stage' / f'{kind}-{treat}-{theme}.html'
                hp.write_text(fn(treat, theme))
                png = ISSUE / 'contexts' / f'{kind}-{treat}-{theme}-390x844.png'
                render_stage(hp, png)
                screens.append(str(hp.relative_to(ISSUE)))
                summary['contexts'].append(str(png.relative_to(ISSUE)))

    # index.html + build summary (index.html sits at the bundle root so its
    # masters/previews/contexts references are correct siblings)
    (ISSUE / 'index.html').write_text(index_html({}))
    summary['counts'] = {
        'masters': len(summary['masters']),
        'previews': len(summary['previews']),
        'contexts': len(summary['contexts']),
        'stage_html': len(screens),
        'treatments': 2, 'coats': len(CHOICES),
    }
    summary['stage_html'] = screens
    (ISSUE / 'tools' / 'build-summary.json').write_text(
        json.dumps(summary, indent=2, sort_keys=True) + '\n')
    print(json.dumps(summary['counts'], sort_keys=True))
    print('A window', [round(v, 2) for v in a_win])
    print('B window', [round(v, 2) for v in b_win])
    print('GENERATE OK')


if __name__ == '__main__':
    main()
