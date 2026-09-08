#!/usr/bin/env python3
"""issue-442 build — emits every HTML artifact for the Herd-view prototype.

    PYTHONDONTWRITEBYTECODE=1 python3 scripts/build.py

Outputs (all under docs/design/evidence/issue-442/):
  prototype-v1-pasture-panorama.html   interactive, all frames + palettes
  prototype-v2-ranch-map.html          interactive, all frames + palettes
  sprite-sheet.html                    identity axes x state poses
  index.html                           phone-usable gallery / comparison
  stage/<variant>-<palette>-<frame>.html   fixed 390x844 capture stages
                                            (the capture inputs; tracked)

ONE CSS string; every color is a Catppuccin token verbatim from
AppTheme.swift or a documented mix of tokens (fixtures.mix_hex — the
ThemeStore.mixedHex port). Horse art colors are the fixed natural set in
horsesvg.py (identity axis, deliberately palette-independent).
"""
from __future__ import annotations

import html
import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent           # .../issue-442/scripts
EVID = HERE.parent                                # .../issue-442  (leaf ONLY)
assert EVID.name == "issue-442", EVID
sys.path.insert(0, str(HERE))

import fixtures as F   # noqa: E402
import horsesvg as H   # noqa: E402

VARIANTS = {
    "v1": {"slug": "v1-pasture-panorama", "title": "V1 · Pasture Panorama",
           "short": "Pasture Panorama"},
    "v2": {"slug": "v2-ranch-map", "title": "V2 · Ranch Map",
           "short": "Ranch Map"},
}
PALETTES = ("mocha", "latte")
FRAMES = ("blocked-heavy", "dense-fleet", "disconnected")
EXTRA_FRAMES = ("detail-sheet", "filter-sheet", "reduce-motion", "board")

STATE_LABEL = {"working": "working", "blocked": "blocked", "idle": "idle",
               "done": "done", "unknown": "unknown"}
STATE_RANK = {"blocked": 0, "working": 1, "idle": 2, "done": 2, "unknown": 3}
TICK = '<span class="tick">\u2713</span>'
MIDDOT = " \u00b7 "
SAFE_CLOSE = "<\\/"
CHEV_DOWN = "\u25be"
CHEV_RIGHT = "\u25b8"


# ------------------------------------------------------------------ CSS ----

def palette_vars(flavor: str) -> str:
    p = F.PALETTES[flavor]
    lines = [f"  --{k}: {v};" for k, v in p.items()]
    sc = F.state_colors(p)
    for s, c in sc.items():
        f_, b_ = F.state_chip(p, s)
        lines += [f"  --st-{s}: {c};", f"  --st-{s}-fill: {f_};",
                  f"  --st-{s}-border: {b_};"]
    hues = F.repo_hues(F.REPOS_ALL)
    for repo, hue in hues.items():
        key = repo.replace("-", "")
        f_, b_ = F.repo_chip(p, hue)
        lines += [f"  --repo-{key}: {p[hue]};",
                  f"  --repo-{key}-fill: {f_};",
                  f"  --repo-{key}-border: {b_};",
                  f"  --repo-{key}-band: {F.repo_band(p, hue)};",
                  f"  --repo-{key}-ink: {F.repo_ink(p, flavor, hue)};"]
    # Other (surface2) chip ink lock = subtext1
    lines += [f"  --other-ink: {p['subtext1']};"]
    # environment: sky/grass are palette-derived mixes (documented)
    lines += [
        f"  --sky: {F.mix_hex(p['blue'], 0.10, p['mantle'])};",
        f"  --sky2: {F.mix_hex(p['sky'], 0.16, p['base'])};",
        f"  --grass: {F.mix_hex(p['green'], 0.14, p['base'])};",
        f"  --grass2: {F.mix_hex(p['green'], 0.22, p['surface0'])};",
        f"  --fence: {F.mix_hex(p['peach'], 0.35, p['surface2'])};",
        f"  --rail-alert: {p['red']};",
        f"  --tint-sheet: {F.mix_hex(p['base'], 0.80, p['crust'])};",
    ]
    return "\n".join(lines)


CSS_BASE = """
*{box-sizing:border-box;margin:0;padding:0}
html,body{height:100%}
body{font-family:-apple-system,BlinkMacSystemFont,"SF Pro Text","Helvetica Neue",Helvetica,Arial,sans-serif;background:var(--crust);color:var(--text);-webkit-font-smoothing:antialiased;line-height:1.3}
:root{--mono:ui-monospace,"SF Mono",Menlo,monospace;--accent:var(--mauve);--tap:44px}
.mono{font-family:var(--mono)}
button{font:inherit;color:inherit;background:none;border:0;cursor:pointer}
button:focus-visible,[tabindex]:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
.sr{position:absolute!important;width:1px;height:1px;overflow:hidden;clip:rect(0 0 0 0);white-space:nowrap}

/* --- phone stage ------------------------------------------------------ */
.phone{position:relative;width:390px;height:844px;overflow:hidden;background:var(--base);color:var(--text);display:flex;flex-direction:column}
.stage .phone{margin:0}
.chrome{flex:0 0 auto;background:var(--mantle);border-bottom:1px solid var(--surface0);padding:52px 12px 8px}
.chrome-row{display:flex;align-items:center;gap:8px;min-height:var(--tap)}
.title{font-size:22px;font-weight:700;letter-spacing:-.01em;flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.pill-btn{display:inline-flex;align-items:center;gap:6px;min-height:var(--tap);min-width:var(--tap);padding:0 12px;border-radius:12px;background:var(--surface0);color:var(--text);font-size:15px;font-weight:600;border:1px solid var(--surface1)}
.pill-btn .n{font-family:var(--mono);font-size:12px;color:var(--subtext0)}
.seg{display:inline-flex;background:var(--surface0);border:1px solid var(--surface1);border-radius:12px;padding:0;gap:0;overflow:hidden}
.seg button{min-height:44px;min-width:64px;padding:0 12px;border-radius:9px;font-size:15px;font-weight:600;color:var(--subtext0)}
.seg button[aria-pressed="true"]{background:var(--accent);color:var(--crust)}
.seg{min-height:var(--tap);align-items:center}
.gear{width:var(--tap);height:var(--tap);display:inline-flex;align-items:center;justify-content:center;font-size:20px;color:var(--subtext1);border-radius:12px}
.scope{display:flex;align-items:center;gap:6px;margin-top:6px;font-size:13px;color:var(--subtext0);min-height:20px;overflow:hidden;white-space:nowrap;text-overflow:ellipsis}
.scope b{color:var(--text);font-weight:600}
.scope .sync{font-family:var(--mono);font-size:11px;padding:2px 6px;border-radius:6px;background:var(--surface0);border:1px solid var(--surface1);color:var(--subtext1)}

/* --- status bar (front-rail summary; text + glyph, never color alone) --- */
.rail-summary{display:flex;gap:8px;align-items:center;padding:8px 12px;background:var(--mantle);border-bottom:1px solid var(--surface0);font-size:13px;min-height:var(--tap);flex:0 0 auto}
.rail-summary .ct{display:inline-flex;align-items:center;gap:4px;padding:3px 8px;border-radius:7px;border:1px solid;font-weight:600;font-family:var(--mono);font-size:12px}
.ct-blocked{color:var(--st-blocked);background:var(--st-blocked-fill);border-color:var(--st-blocked-border)}
.ct-working{color:var(--st-working);background:var(--st-working-fill);border-color:var(--st-working-border)}
.ct-idle{color:var(--st-idle);background:var(--st-idle-fill);border-color:var(--st-idle-border)}
.ct-done{color:var(--st-done);background:var(--st-done-fill);border-color:var(--st-done-border)}
.ct-unknown{color:var(--subtext1);background:var(--st-unknown-fill);border-color:var(--st-unknown-border)}
.rail-summary .lead{color:var(--subtext0);flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}

/* --- horse tokens ------------------------------------------------------ */
.horse-btn{position:relative;display:flex;flex-direction:column;align-items:center;justify-content:flex-end;min-width:var(--tap);min-height:var(--tap);border-radius:10px;padding:24px 2px 0;-webkit-tap-highlight-color:transparent}
.horse-btn:active{background:color-mix(in srgb,var(--accent) 16%,transparent)}
.horse-svg{display:block;overflow:visible}
.nameplate{font-family:var(--mono);font-size:11px;line-height:1.1;color:var(--subtext1);max-width:110px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;background:color-mix(in srgb,var(--base) 78%,transparent);padding:2px 5px;border-radius:5px;margin-top:-2px;border:1px solid color-mix(in srgb,var(--surface1) 60%,transparent)}
.horse-btn.is-selected .nameplate{outline:2px solid var(--accent)}
.state-flag{position:absolute;top:0;left:50%;transform:translateX(-50%);font-family:var(--mono);font-size:11px;font-weight:700;line-height:1;padding:3px 6px;border-radius:6px;border:1px solid;white-space:nowrap;display:inline-flex;align-items:center;gap:4px}
.state-flag.blocked{color:var(--st-blocked);background:var(--st-blocked-fill);border-color:var(--st-blocked);font-size:12px;padding:4px 7px}
.state-flag.unknown{color:var(--subtext1);background:var(--st-unknown-fill);border-color:var(--st-unknown-border)}
.state-flag.done{color:var(--st-done);background:var(--st-done-fill);border-color:var(--st-done-border)}
.state-flag.working{color:var(--st-working);background:var(--st-working-fill);border-color:var(--st-working-border)}
.state-flag.idle{color:var(--st-idle);background:var(--st-idle-fill);border-color:var(--st-idle-border)}
.alert-mark{display:inline-block;width:16px;height:16px;border-radius:50%;background:var(--st-blocked);color:var(--crust);font-weight:900;font-size:13px;line-height:16px;text-align:center;font-family:var(--mono)}

/* heartbeat (#371 WorkingMotion port: 1.2s cycle, peak 42%, 3 squares) */
.hb{display:inline-flex;gap:3px;align-items:center}
.hb i{width:4px;height:4px;border-radius:1px;background:var(--st-working);opacity:.34;transform:scale(.78);animation:breathe 1.2s infinite}
.hb i:nth-child(2){animation-delay:.16s}.hb i:nth-child(3){animation-delay:.32s}
@keyframes breathe{0%{opacity:.34;transform:scale(.78)}42%{opacity:1;transform:scale(1)}100%{opacity:.34;transform:scale(.78)}}

/* leg swing: CSS variable --a is the pose angle; trot animation sweeps it */
.leg{transform-box:fill-box;transform-origin:50% 0;transform:rotate(var(--a))}
.horse-working:not(.horse-rm) .leg-nf{animation:swingA 0.62s ease-in-out infinite alternate}
.horse-working:not(.horse-rm) .leg-fh{animation:swingA 0.62s ease-in-out infinite alternate}
.horse-working:not(.horse-rm) .leg-ff{animation:swingB 0.62s ease-in-out infinite alternate}
.horse-working:not(.horse-rm) .leg-nh{animation:swingB 0.62s ease-in-out infinite alternate}
.horse-working:not(.horse-rm) .body-g{animation:bob 0.62s ease-in-out infinite alternate}
@keyframes swingA{from{transform:rotate(26deg)}to{transform:rotate(-26deg)}}
@keyframes swingB{from{transform:rotate(-26deg)}to{transform:rotate(26deg)}}
@keyframes bob{from{transform:translate(0,-1.5px) rotate(-4deg)}to{transform:translate(0,0.5px) rotate(-4deg)}}
.horse-blocked:not(.horse-rm) .leg-nf{animation:paw 1.1s ease-in-out infinite}
@keyframes paw{0%,100%{transform:rotate(-26deg)}50%{transform:rotate(-8deg)}}
.horse-idle:not(.horse-rm) .neck{transform-box:fill-box;transform-origin:20% 10%;animation:graze 4s ease-in-out infinite}
@keyframes graze{0%,100%{transform:rotate(0deg)}50%{transform:rotate(-3deg)}}
.blocked-pulse:not(.rm) .state-flag.blocked{animation:pulse 1.4s ease-in-out infinite}
@keyframes pulse{0%,100%{transform:translateX(-50%) scale(1)}50%{transform:translateX(-50%) scale(1.08)}}
/* Reduce Motion: every animation removed (static poses), never merely paused */
@media (prefers-reduced-motion: reduce){
  .leg,.body-g,.neck,.hb i,.state-flag{animation:none!important}
  .hb{display:none}.hb-static{display:inline-block!important}
}
.rm .leg,.rm .body-g,.rm .neck,.rm .hb i,.rm .state-flag{animation:none!important}
.rm .hb{display:none}.rm .hb-static{display:inline-block!important}
.hb-static{display:none;width:7px;height:7px;border-radius:50%;background:var(--st-working)}

/* --- V1: Pasture Panorama ----------------------------------------------- */
.v1 .pasture{flex:1;min-height:0;position:relative;display:flex;flex-direction:column;background:linear-gradient(var(--sky),var(--sky2) 42%,var(--grass) 42%,var(--grass2))}
.v1 .rail-row{position:relative;display:flex;align-items:flex-end;gap:10px;padding:40px 12px 6px;min-height:160px;overflow-x:auto;overflow-y:hidden;scroll-snap-type:x mandatory;-webkit-overflow-scrolling:touch;border-bottom:3px solid var(--fence)}
.v1 .rail-row::before{content:"";position:absolute;left:0;right:0;bottom:22px;height:3px;background:var(--fence);opacity:.55}
.v1 .rail-row .horse-btn{position:relative;z-index:1}
.v1 .rail-label{position:absolute;top:8px;left:12px;font-family:var(--mono);font-size:11px;font-weight:700;letter-spacing:.06em;text-transform:uppercase;color:var(--st-blocked);display:flex;align-items:center;gap:6px;padding:4px 8px;border-radius:6px;background:var(--st-blocked-fill);border:1px solid var(--st-blocked-border)}
.v1 .rail-row .horse-btn{scroll-snap-align:start;flex:0 0 auto;width:132px}
.v1 .rail-row .horse-svg{width:112px;height:85px}
.v1 .rail-row .nameplate{max-width:128px}
.v1 .rail-empty{font-size:13px;color:var(--subtext0);padding:56px 12px 24px;display:flex;align-items:center;gap:8px}
.v1 .paddock-strip{flex:1;min-height:0;display:flex;align-items:stretch;overflow-x:auto;overflow-y:hidden;scroll-snap-type:x mandatory;-webkit-overflow-scrolling:touch}
.v1 .paddock{flex:0 0 100%;height:100%;scroll-snap-align:start;display:flex;flex-direction:column;position:relative;padding:8px 12px 12px;min-width:0}
.v1 .paddock + .paddock{border-left:3px dashed var(--fence)}
.v1 .paddock-head{display:flex;align-items:center;gap:8px;min-height:var(--tap)}
.v1 .paddock-name{font-size:17px;font-weight:700;letter-spacing:-.01em;flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;display:flex;align-items:center;gap:7px}
.v1 .paddock-name i{width:8px;height:8px;border-radius:2px;flex:0 0 auto}
.v1 .paddock-count{font-family:var(--mono);font-size:12px;color:var(--subtext0)}
.v1 .paddock-nav{font-family:var(--mono);font-size:12px;color:var(--subtext0);display:flex;gap:6px;align-items:center}
.v1 .paddock-nav b{color:var(--text)}
.v1 .field{flex:1;min-height:0;display:flex;flex-wrap:wrap;align-content:center;justify-content:center;gap:14px 6px;padding:6px 0 8px;overflow-y:auto}
.v1 .field .horse-btn{width:160px}
.v1 .field .horse-svg{width:148px;height:112px}
.v1 .field .nameplate{max-width:170px;font-size:12px}
.v1 .field .state-flag{font-size:12px}
.v1 .dots{display:flex;justify-content:center;gap:6px;padding:6px 0 10px;flex:0 0 auto}
.v1 .dots i{width:8px;height:8px;border-radius:50%;background:var(--surface2)}
.v1 .dots i.on{background:var(--accent);width:20px;border-radius:4px}
.v1 .hint{font-size:12px;color:var(--subtext0);text-align:center;padding-bottom:6px}

/* --- V2: Ranch Map ------------------------------------------------------ */
.v2 .map{flex:1;min-height:0;overflow-y:auto;padding:10px 12px 16px;display:flex;flex-direction:column;gap:10px;background:linear-gradient(var(--grass),var(--grass2))}
.v2 .front-rail{position:relative;border:2px solid var(--st-blocked);border-radius:14px;background:color-mix(in srgb,var(--st-blocked-fill) 70%,var(--base));padding:8px 10px 6px}
.v2 .front-rail .rail-label{font-family:var(--mono);font-size:11px;font-weight:700;letter-spacing:.06em;text-transform:uppercase;color:var(--st-blocked);display:flex;align-items:center;gap:6px;min-height:24px}
.v2 .front-rail .row{display:flex;gap:6px;overflow-x:auto;padding-top:2px}
.v2 .front-rail .horse-btn{flex:0 0 auto;width:128px}
.v2 .front-rail .nameplate{max-width:124px}
.v2 .front-rail .horse-svg{width:96px;height:73px}
.v2 .front-rail .rail-empty{font-size:13px;color:var(--subtext0);padding:8px 0 4px;display:flex;align-items:center;gap:8px}
.v2 .paddock{border:2px solid var(--surface1);border-radius:14px;background:color-mix(in srgb,var(--base) 55%,transparent);padding:8px 10px 8px;position:relative;box-shadow:inset 0 -6px 0 color-mix(in srgb,var(--fence) 35%,transparent)}
.v2 .paddock[data-collapsed="true"] .grid{display:none}
.v2 .paddock-head{display:flex;align-items:center;gap:8px;min-height:var(--tap)}
.v2 .paddock-name{font-size:15px;font-weight:700;flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;display:flex;align-items:center;gap:7px}
.v2 .paddock-name i{width:8px;height:8px;border-radius:2px;flex:0 0 auto}
.v2 .paddock-counts{display:flex;gap:4px;flex:0 0 auto}
.v2 .paddock-counts .ct{padding:2px 6px;font-size:11px;border-radius:6px;border:1px solid;font-family:var(--mono);font-weight:700}
.v2 .chev{width:var(--tap);height:var(--tap);display:inline-flex;align-items:center;justify-content:center;color:var(--subtext0);font-size:13px;margin-right:-10px}
.v2 .grid{display:grid;grid-template-columns:repeat(4,1fr);gap:6px 4px;padding-top:2px}
.v2 .grid .horse-btn{width:100%}
.v2 .grid .horse-svg{width:78px;height:59px}
.v2 .grid .nameplate{max-width:84px;font-size:10px;white-space:normal;overflow-wrap:anywhere;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;text-align:center;line-height:1.15}
.v2 .band{position:absolute;inset:0 0 auto 0;height:5px;border-radius:12px 12px 0 0}
.v2 .paddock .more{grid-column:1/-1;display:flex;align-items:center;justify-content:center;min-height:var(--tap);font-size:13px;color:var(--accent);font-weight:600;border-top:1px dashed var(--surface1)}
.v2 .legend{display:flex;flex-wrap:wrap;gap:6px;font-size:12px;color:var(--subtext0);padding:4px 2px 0}

/* --- outage overlay ------------------------------------------------------ */
.outage{position:absolute;left:12px;right:12px;bottom:16px;background:var(--mantle);border:2px solid var(--st-blocked);border-radius:14px;padding:12px 14px;box-shadow:0 12px 32px rgba(0,0,0,.35);z-index:5}
.outage h3{font-size:16px;font-weight:700;display:flex;align-items:center;gap:8px;margin-bottom:6px}
.outage p{font-size:13px;color:var(--subtext1);line-height:1.35}
.outage .meta{font-family:var(--mono);font-size:11px;color:var(--subtext0);margin-top:8px}
.outage .btns{display:flex;gap:8px;margin-top:10px}
.outage .btns button{min-height:var(--tap);flex:1;border-radius:10px;border:1px solid var(--surface1);background:var(--surface0);font-weight:600;font-size:15px}
.outage .btns button.primary{background:var(--accent);color:var(--crust);border-color:var(--accent)}
.is-disconnected .pasture,.is-disconnected .map{filter:saturate(.3)}
.is-disconnected .horse-btn{pointer-events:none;opacity:.7}
.stale-bar{display:flex;align-items:center;gap:8px;padding:6px 12px;background:var(--st-blocked-fill);border-bottom:1px solid var(--st-blocked-border);color:var(--st-blocked);font-size:13px;font-weight:600;flex:0 0 auto;min-height:32px}

/* --- sheets (native detent + Filters) ------------------------------------- */
.scrim{position:absolute;inset:0;background:rgba(0,0,0,.36);z-index:20}
.sheet{position:absolute;left:0;right:0;bottom:0;background:var(--tint-sheet);border-radius:18px 18px 0 0;padding:8px 16px 28px;z-index:21;max-height:76%;display:flex;flex-direction:column;box-shadow:0 -6px 30px rgba(0,0,0,.3);border-top:1px solid var(--surface1)}
.grabber{width:36px;height:5px;border-radius:3px;background:var(--surface2);margin:0 auto 12px}
.sheet h2{font-size:17px;font-weight:700;letter-spacing:-.01em;display:flex;align-items:center;gap:8px}
.sheet .sub{font-size:12px;color:var(--subtext0);margin-top:3px;font-family:var(--mono);line-height:1.35;overflow-wrap:anywhere}
.sheet .chips{display:flex;gap:6px;margin:10px 0;flex-wrap:wrap}
.chip{display:inline-flex;align-items:center;gap:5px;padding:3px 8px 3px 6px;border-radius:999px;font-size:12px;font-weight:700;border:1px solid}
.chip i{width:6px;height:6px;border-radius:2px}
.st-chip{display:inline-flex;align-items:center;gap:4px;padding:3px 8px;border-radius:7px;border:1px solid;font-family:var(--mono);font-size:12px;font-weight:700}
.tail{flex:1;min-height:0;overflow:auto;background:var(--mantle);border:1px solid var(--surface0);border-radius:10px;padding:10px 12px;font-family:var(--mono);font-size:12px;line-height:1.5;color:var(--text)}
.tail .q{color:var(--overlay0)}.tail .k{color:var(--yellow)}.tail .s{color:var(--blue)}.tail .a{color:var(--green)}.tail .d{color:var(--red)}
.tail .role{display:flex;align-items:center;gap:6px;margin:6px 0 2px;font-weight:700}
.tail .role.agent{color:var(--accent)}.tail .role.tool{color:var(--peach)}
.sheet .same{font-size:12px;color:var(--subtext0);margin-top:10px;display:flex;align-items:center;gap:6px}
.sheet .same b{color:var(--text)}
.opt{display:flex;align-items:center;gap:10px;min-height:var(--tap);padding:0 4px;border-bottom:1px solid var(--surface0);font-size:15px}
.opt .tick{margin-left:auto;color:var(--accent);font-weight:700}
.opt .n{font-family:var(--mono);font-size:12px;color:var(--subtext0);margin-left:8px}
.sheet .grp{font-family:var(--mono);font-size:11px;letter-spacing:.06em;text-transform:uppercase;color:var(--subtext0);margin:12px 0 2px}

/* --- Board (existing mode, for the switch demo) ---------------------------- */
.board{flex:1;min-height:0;overflow-y:auto}
.b-sec{font-family:var(--mono);font-size:11px;letter-spacing:.06em;text-transform:uppercase;color:var(--subtext0);padding:10px 12px 4px;background:var(--base);position:sticky;top:0}
.b-sub{padding:4px 12px;font-size:12px;font-weight:700;display:flex;align-items:center;gap:6px}
.b-row{display:flex;flex-direction:column;gap:3px;padding:8px 12px;min-height:var(--tap);border-bottom:1px solid var(--surface0)}
.b-row.is-idle{opacity:.65}
.b-row .l1{display:flex;align-items:center;gap:6px}
.b-row .nm{font-size:15px;font-weight:600;flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.b-row .l2{font-size:12px;color:var(--subtext0);font-family:var(--mono);display:flex;gap:6px;align-items:center;overflow:hidden;white-space:nowrap}

/* --- label stamp (evidence) ---------------------------------------------- */
.stamp{position:absolute;left:0;right:0;top:0;height:44px;display:flex;align-items:center;justify-content:center;font-family:var(--mono);font-size:11px;color:var(--subtext0);letter-spacing:.02em;pointer-events:none;z-index:30}
.stamp b{color:var(--text);font-weight:700;margin-right:6px}

/* --- interactive shell ---------------------------------------------------- */
.shell{min-height:100%;display:flex;flex-direction:column;align-items:center;gap:14px;padding:16px 12px 40px}
.ctl{display:flex;flex-wrap:wrap;gap:8px;justify-content:center;max-width:390px}
.ctl button{min-height:var(--tap);padding:0 12px;border-radius:10px;background:var(--surface0);border:1px solid var(--surface1);font-size:14px;font-weight:600;color:var(--subtext1)}
.ctl button[aria-pressed="true"]{background:var(--accent);color:var(--crust);border-color:var(--accent)}
.ctl .grp{width:100%;font-family:var(--mono);font-size:11px;letter-spacing:.06em;text-transform:uppercase;color:var(--subtext0);text-align:center;margin-top:4px}
.frame{box-shadow:0 20px 60px rgba(0,0,0,.45);border-radius:34px;overflow:hidden;border:6px solid var(--crust);background:var(--crust)}
.frame .phone{border-radius:28px}
@media (max-width:420px){.frame{border-radius:0;border:0}.frame .phone{border-radius:0}.shell{padding:0 0 40px}}
"""


def css_for(flavor: str, scoped: str = ":root") -> str:
    return f"{scoped}{{\n{palette_vars(flavor)}\n}}"


# ------------------------------------------------------------ fragments ----

def esc(s: str) -> str:
    return html.escape(s, quote=True)


def repo_key(repo: str) -> str:
    return repo.replace("-", "")


def state_glyph_html(state: str, rm: bool = False) -> str:
    if state == "working":
        return ('<span class="hb" aria-hidden="true"><i></i><i></i><i></i>'
                '</span><span class="hb-static" aria-hidden="true"></span>')
    return f'<span aria-hidden="true">{esc(F.STATE_MARK[state])}</span>'


def state_flag(a: F.Agent, rm: bool) -> str:
    s = a.state
    if s == "blocked":
        return (f'<span class="state-flag blocked"><span class="alert-mark">'
                f'!</span>blocked</span>')
    if s == "unknown":
        return '<span class="state-flag unknown">? unknown</span>'
    if s == "done":
        return '<span class="state-flag done">\u2713 done</span>'
    if s == "working":
        return f'<span class="state-flag working">{state_glyph_html(s)}working</span>'
    return '<span class="state-flag idle">\u25e6 idle</span>'


def horse_button(a: F.Agent, rm: bool, facing: int = 1, variant: str = "v1",
                 selected: bool = False, desat: bool = False) -> str:
    ident = F.horse_identity(a.name)
    svg = H.horse_svg(ident, a.state, reduce_motion=rm, facing=facing,
                      desaturated=desat)
    sel = " is-selected" if selected else ""
    return (f'<button class="horse-btn{sel}" type="button" '
            f'data-agent="{esc(a.name)}" data-repo="{esc(a.repo)}" '
            f'data-state="{a.state}" data-host="{esc(a.host)}" '
            f'aria-label="{esc(a.name)}, {a.state}, {esc(a.repo)} on '
            f'{esc(a.host)}. Opens recent output." '
            f'data-opens="detail-sheet">'
            f'{state_flag(a, rm)}{svg}'
            f'<span class="nameplate">{esc(a.name)}</span></button>')


def chrome(mode: str, variant: str, scope_text: str,
           active_filters: int, title: str = "Corral") -> str:
    board_on = "true" if mode == "board" else "false"
    herd_on = "true" if mode == "herd" else "false"
    fl = "Filters" + (f' <span class="n">\u00b7 {active_filters}</span>'
                      if active_filters else "")
    return (
        '<header class="chrome">'
        '<div class="chrome-row">'
        f'<button class="pill-btn" type="button" data-opens="filter-sheet" '
        f'aria-label="Filters">{fl}</button>'
        f'<h1 class="title">{esc(title)}</h1>'
        f'<div class="seg" role="group" aria-label="Mode">'
        f'<button type="button" data-switch="board" aria-pressed="{board_on}">'
        f'Board</button>'
        f'<button type="button" data-switch="herd" aria-pressed="{herd_on}">'
        f'Herd</button></div>'
        '<button class="gear" type="button" aria-label="Settings">'
        '\u2699</button>'
        '</div>'
        f'<div class="scope"><span>Scope</span> <b>{esc(scope_text)}</b>'
        '<span class="sync" title="Filters are shared with Board">shared'
        '</span></div>'
        '</header>')


def rail_summary(fx: F.Fixture) -> str:
    counts = {s: 0 for s in F.STATES}
    for a in fx.agents:
        counts[a.state] += 1
    pieces = []
    for s in ("blocked", "working", "idle", "done", "unknown"):
        if counts[s]:
            pieces.append(f'<span class="ct ct-{s}">'
                          f'{state_glyph_html(s)}{counts[s]}&nbsp;{s}</span>')
    lead = (f"{len(fx.agents)} horses \u00b7 "
            f"{len({a.repo for a in fx.agents})} paddocks")
    return (f'<div class="rail-summary" role="status">'
            f'<span class="lead">{lead}</span>{"".join(pieces)}</div>')


def group_by_repo(agents):
    out: dict[str, list] = {}
    for a in agents:
        out.setdefault(a.repo, []).append(a)
    for k in out:
        out[k].sort(key=lambda a: (STATE_RANK[a.state], a.name))
    return dict(sorted(out.items()))


def hue_var(repo: str) -> str:
    return f"var(--repo-{repo_key(repo)})"


def repo_chip_html(repo: str) -> str:
    k = repo_key(repo)
    return (f'<span class="chip" style="color:var(--repo-{k}-ink);'
            f'background:var(--repo-{k}-fill);border-color:var(--repo-{k}-border)">'
            f'<i style="background:var(--repo-{k})"></i>{esc(repo)}</span>')


# ------------------------------------------------------------------ V1 -----

def v1_body(fx: F.Fixture, rm: bool, opts: dict) -> str:
    """Pasture Panorama: front rail strip on top (blocked at the fence),
    then a horizontally-paged paddock strip, one paddock per page."""
    groups = group_by_repo(fx.agents)
    blocked = [a for a in fx.agents if a.state == "blocked"]
    blocked.sort(key=lambda a: a.name)
    page = opts.get("page", 0)
    selected = opts.get("selected")
    desat = not fx.source_ok
    rail_items = "".join(
        horse_button(a, rm, facing=1, variant="v1",
                     selected=(a.name == selected), desat=desat)
        for a in blocked)
    if blocked:
        rail = (f'<div class="rail-row" role="region" aria-label="Front rail: '
                f'blocked horses">'
                f'<span class="rail-label"><span class="alert-mark">!</span>'
                f'front rail \u00b7 {len(blocked)} blocked</span>{rail_items}'
                f'</div>')
    elif desat:
        rail = ('<div class="rail-row rail-stale"><span class="rail-label" '
                'style="color:var(--subtext0);background:var(--surface0);'
                'border-color:var(--surface1)">'
                '<span aria-hidden="true">?</span>front rail \u00b7 unknown'
                '</span><div class="rail-empty">Blocked state cannot be '
                'confirmed while the source is down.</div></div>')
    else:
        rail = ('<div class="rail-row"><span class="rail-label">'
                '<span aria-hidden="true">\u25e6</span>front rail</span>'
                '<div class="rail-empty">No blocked horses at the rail.'
                '</div></div>')
    pads = []
    repos = list(groups.keys())
    for idx, (repo, agents) in enumerate(groups.items()):
        k = repo_key(repo)
        nonblocked = [a for a in agents if a.state != "blocked"]
        blocked_here = len(agents) - len(nonblocked)
        horses = "".join(
            horse_button(a, rm, facing=(1 if i % 2 == 0 else -1),
                         selected=(a.name == selected), desat=desat)
            for i, a in enumerate(nonblocked))
        note = (f'<span class="paddock-count">+{blocked_here} blocked at rail'
                f' \u2191</span>' if blocked_here else "")
        pads.append(
            f'<section class="paddock" data-repo="{esc(repo)}" '
            f'aria-label="Paddock {esc(repo)}">'
            f'<div class="paddock-head">'
            f'<h2 class="paddock-name"><i style="background:var(--repo-{k})">'
            f'</i>{esc(repo)}</h2>{note}'
            f'<span class="paddock-nav"><b>{idx+1}</b>/{len(repos)}</span>'
            f'</div>'
            f'<div class="field">{horses}</div>'
            f'</section>')
    dots = "".join(f'<i class="{"on" if i == page else ""}"></i>'
                   for i in range(len(repos)))
    return (
        f'<main class="pasture{" is-disconnected" if desat else ""}">'
        f'{rail}'
        f'<div class="paddock-strip" data-page="{page}">{"".join(pads)}</div>'
        f'<div class="dots" aria-hidden="true">{dots}</div>'
        f'<div class="hint">Swipe \u2194 between paddocks \u00b7 tap a horse for'
        f' recent output</div>'
        f'</main>')


# ------------------------------------------------------------------ V2 -----

def v2_body(fx: F.Fixture, rm: bool, opts: dict) -> str:
    """Ranch Map: elevated overview. Front rail card at the top (blocked),
    then every paddock as a bounded card with a 4-up grid; by default one
    row (4) is open and the rest collapses behind a 'Show all' row — dense
    fleets overflow by navigation, never by shrinking labels."""
    groups = group_by_repo(fx.agents)
    blocked = sorted([a for a in fx.agents if a.state == "blocked"],
                     key=lambda a: a.name)
    selected = opts.get("selected")
    desat = not fx.source_ok
    collapsed = set(opts.get("collapsed", ()))
    cap = opts.get("cap", 4)   # one 4-up row open by default; Show all expands
    if blocked:
        rail = (f'<section class="front-rail" aria-label="Front rail: blocked '
                f'horses"><div class="rail-label"><span class="alert-mark">!'
                f'</span>front rail \u00b7 {len(blocked)} blocked</div>'
                f'<div class="row">'
                + "".join(horse_button(a, rm, selected=(a.name == selected),
                                       desat=desat) for a in blocked)
                + '</div></section>')
    elif desat:
        rail = ('<section class="front-rail" style="border-style:dashed;'
                'border-color:var(--surface1);background:transparent">'
                '<div class="rail-label" style="color:var(--subtext0)">'
                '<span aria-hidden="true">?</span>front rail \u00b7 unknown'
                '</div><div class="rail-empty">Blocked state cannot be '
                'confirmed while the source is down.</div></section>')
    else:
        rail = ('<section class="front-rail" style="border-style:dashed;'
                'border-color:var(--surface1);background:transparent">'
                '<div class="rail-label" style="color:var(--subtext0)">'
                '<span aria-hidden="true">\u25e6</span>front rail</div>'
                '<div class="rail-empty">No blocked horses at the rail.'
                '</div></section>')
    cards = []
    for repo, agents in groups.items():
        k = repo_key(repo)
        counts = {}
        for a in agents:
            counts[a.state] = counts.get(a.state, 0) + 1
        ct = "".join(
            f'<span class="ct ct-{s}">{esc(F.STATE_MARK[s])}{n}</span>'
            for s, n in sorted(counts.items(),
                               key=lambda kv: STATE_RANK[kv[0]]))
        nonblocked = [a for a in agents if a.state != "blocked"]
        shown = nonblocked[:cap]
        rest = len(nonblocked) - len(shown)
        horses = "".join(horse_button(a, rm, selected=(a.name == selected),
                                      desat=desat) for a in shown)
        more = (f'<button class="more" type="button" data-more="{esc(repo)}">'
                f'Show all {len(nonblocked)} \u00b7 {rest} more</button>'
                if rest else "")
        is_col = "true" if repo in collapsed else "false"
        chev = CHEV_RIGHT if repo in collapsed else CHEV_DOWN
        cards.append(
            f'<section class="paddock" data-repo="{esc(repo)}" '
            f'data-collapsed="{is_col}" aria-label="Paddock {esc(repo)}">'
            f'<span class="band" style="background:var(--repo-{k}-band)"></span>'
            f'<div class="paddock-head">'
            f'<h2 class="paddock-name"><i style="background:var(--repo-{k})">'
            f'</i>{esc(repo)}</h2>'
            f'<span class="paddock-counts">{ct}</span>'
            f'<button class="chev" type="button" data-toggle="{esc(repo)}" '
            f'aria-expanded="{"false" if repo in collapsed else "true"}" '
            f'aria-label="Collapse {esc(repo)}">'
            f'{chev}</button>'
            f'</div>'
            f'<div class="grid">{horses}{more}</div>'
            f'</section>')
    legend = ('<div class="legend">'
              '<span>! blocked \u00b7 \u25cb working \u00b7 \u25e6 idle \u00b7 '
              '\u2713 done \u00b7 ? unknown</span></div>')
    return (f'<main class="map{" is-disconnected" if desat else ""}">'
            f'{rail}{"".join(cards)}{legend}</main>')


# ------------------------------------------------------------ overlays -----

def outage_card(fx: F.Fixture) -> str:
    return (
        '<div class="outage" role="alert">'
        '<h3><span class="alert-mark">!</span>Source disconnected</h3>'
        '<p>corrald on <b class="mono">mac-mini</b> stopped answering '
        '(SSE closed, 3 reconnects failed). The herd shows the last '
        'snapshot as <b>unknown</b>; nothing here is live.</p>'
        '<div class="meta">last snapshot rev 8,412 \u00b7 42 s ago \u00b7 '
        'GET /events \u2192 connection refused</div>'
        '<div class="btns"><button type="button">Open Board</button>'
        '<button type="button" class="primary">Retry</button></div>'
        '</div>')


def stale_bar() -> str:
    return ('<div class="stale-bar" role="status"><span class="alert-mark">'
            '!</span>Disconnected \u00b7 showing last known state as unknown'
            '</div>')


def detail_sheet(a: F.Agent) -> str:
    st = a.state
    k = repo_key(a.repo)
    return (
        '<div class="scrim" data-closes="detail-sheet"></div>'
        f'<section class="sheet" role="dialog" aria-modal="true" '
        f'aria-label="Recent output for {esc(a.name)}" data-sheet="detail">'
        '<div class="grabber"></div>'
        f'<h2><span class="st-chip" style="color:var(--st-{st});'
        f'background:var(--st-{st}-fill);border-color:var(--st-{st}-border)">'
        f'{state_glyph_html(st)}{st}</span>{esc(a.name)}</h2>'
        f'<div class="sub">{esc(a.repo)} \u00b7 {esc(a.branch)} \u00b7 '
        f'{esc(a.host)} \u00b7 {esc(a.harness)}</div>'
        f'<div class="chips">{repo_chip_html(a.repo)}'
        f'<span class="chip" style="color:var(--other-ink);'
        f'border-color:var(--surface1);background:var(--surface0)">'
        f'<i style="background:var(--surface2)"></i>{esc(a.host)}</span></div>'
        '<div class="tail" aria-label="Recent output tail">'
        '<div class="role agent">\u25cf agent</div>'
        '<div>Reading the failing gate log <span class="q">#412</span>.</div>'
        '<div class="role tool">\u25a0 tool</div>'
        '<div><span class="k">$</span> cargo test --lib ledger::replay</div>'
        '<div><span class="d">- assert_eq!(window.end, 1_024)</span></div>'
        '<div><span class="a">+ assert_eq!(window.end, 1_024 + skew)</span>'
        '</div>'
        '<div class="role agent">\u25cf agent</div>'
        f'<div>{"Waiting on your approval to widen the replay window." if st == "blocked" else "Re-running the focused suite before touching the caller."}</div>'
        '<div class="q">\u2500 20 of 148 lines \u00b7 Show all</div>'
        '</div>'
        '<div class="same"><span aria-hidden="true">\u21c4</span>'
        '<span>Same sheet the Board row opens: <b>Recent Output</b> \u00b7 '
        'read-only</span></div>'
        '</section>')


def filter_sheet(fx: F.Fixture, active_repo: str | None,
                 active_host: str | None) -> str:
    groups = group_by_repo(fx.agents)
    hosts = fx.hosts
    rows = ['<div class="grp">Host</div>',
            f'<div class="opt">All hosts<span class="n">{len(fx.agents)}'
            f'</span>{"" if active_host else TICK}</div>']
    for name, n, health in hosts:
        tick = TICK if name == active_host else ""
        health_note = (MIDDOT + health) if health != "ok" else ""
        rows.append(f'<div class="opt">{esc(name)}<span class="n">{n} lanes'
                    f'{health_note}</span>'
                    f'{tick}</div>')
    rows.append('<div class="grp">Repository</div>')
    tick_all = "" if active_repo else TICK
    rows.append(f'<div class="opt">All repositories<span class="n">'
                f'{len(fx.agents)}</span>{tick_all}</div>')
    for repo, agents in groups.items():
        tick = TICK if repo == active_repo else ""
        rows.append(f'<div class="opt">{repo_chip_html(repo)}<span class="n">'
                    f'{len(agents)}</span>{tick}</div>')
    return (
        '<div class="scrim" data-closes="filter-sheet"></div>'
        '<section class="sheet" role="dialog" aria-modal="true" '
        'aria-label="Filters" data-sheet="filter">'
        '<div class="grabber"></div>'
        '<h2>Filters</h2>'
        '<div class="sub">One filter model \u00b7 applies to Board and Herd</div>'
        f'{"".join(rows)}'
        '<div class="same"><span aria-hidden="true">\u21c4</span>'
        '<span>Changing scope here changes <b>both</b> modes.</span></div>'
        '</section>')


# ---------------------------------------------------------------- board ----

def board_body(fx: F.Fixture) -> str:
    secs = {"blocked": [], "working": [], "idle": [], "unknown": []}
    for a in fx.agents:
        key = "idle" if a.state == "done" else a.state
        secs[key].append(a)
    out = ['<main class="board">']
    for sec, agents in secs.items():
        if not agents:
            continue
        out.append(f'<div class="b-sec">{sec} \u00b7 {len(agents)}</div>')
        for repo, rs in group_by_repo(agents).items():
            k = repo_key(repo)
            out.append(f'<div class="b-sub" style="background:var(--repo-{k}-band);'
                       f'color:var(--repo-{k}-ink)"><i style="width:6px;height:6px;'
                       f'border-radius:2px;background:var(--repo-{k})"></i>'
                       f'{esc(repo)}</div>')
            for a in rs:
                st = a.state
                out.append(
                    f'<button class="b-row{" is-idle" if st == "idle" else ""}" '
                    f'type="button" data-agent="{esc(a.name)}" '
                    f'data-opens="detail-sheet">'
                    f'<div class="l1"><span class="st-chip" style="color:var(--st-{st});'
                    f'background:var(--st-{st}-fill);border-color:var(--st-{st}-border)">'
                    f'{state_glyph_html(st)}{st}</span>'
                    f'<span class="nm">{esc(a.name)}</span></div>'
                    f'<div class="l2">{esc(a.branch)} \u00b7 {esc(a.host)}</div>'
                    f'</button>')
    out.append('</main>')
    return "".join(out)


# ------------------------------------------------------------- screens -----

def screen(variant: str, flavor: str, frame: str, *, mode: str = "herd",
           rm: bool = False, overlay: str | None = None,
           stamp: str | None = None, opts: dict | None = None) -> str:
    """One full phone (390x844) for a variant/palette/frame + overlay."""
    opts = opts or {}
    fx = F.FRAMES[frame]()
    scope = "All hosts \u00b7 All repositories"
    active_filters = 0
    if opts.get("repo"):
        scope = f"All hosts \u00b7 {opts['repo']}"
        active_filters = 1
        fx.agents = [a for a in fx.agents if a.repo == opts["repo"]]
    if not fx.source_ok:
        rm = True   # outage: static last-known poses, never animated
    body_cls = f"phone {variant}{' rm' if rm else ''} blocked-pulse"
    parts = [f'<div class="{body_cls}" data-variant="{variant}" '
             f'data-palette="{flavor}" data-frame="{frame}" '
             f'data-mode="{mode}" data-rm="{"true" if rm else "false"}">']
    if stamp:
        parts.append(f'<div class="stamp"><b>{esc(stamp)}</b></div>')
    parts.append(chrome(mode, variant, scope, active_filters))
    if not fx.source_ok:
        parts.append(stale_bar())
    else:
        parts.append(rail_summary(fx))
    if mode == "board":
        parts.append(board_body(fx))
    elif variant == "v1":
        parts.append(v1_body(fx, rm, opts))
    else:
        parts.append(v2_body(fx, rm, opts))
    if not fx.source_ok:
        parts.append(outage_card(fx))
    if overlay == "detail-sheet":
        who = opts.get("selected") or next(
            a for a in fx.agents if a.state == "blocked").name
        a = next(a for a in fx.agents if a.name == who)
        parts.append(detail_sheet(a))
    elif overlay == "filter-sheet":
        parts.append(filter_sheet(fx, opts.get("repo"), None))
    parts.append('</div>')
    return "".join(parts)


# --------------------------------------------------------------- pages -----

def doc(title: str, body: str, flavor: str, extra_css: str = "",
        extra_js: str = "", stage: bool = False) -> str:
    stage_css = ("html,body{width:390px;height:844px;overflow:hidden;"
                 "background:var(--crust)}" if stage else "")
    txt = f"""<!doctype html>
<html lang="en" data-palette="{flavor}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<title>{esc(title)}</title>
<style>
{css_for(flavor)}
{CSS_BASE}
{stage_css}
{extra_css}
</style>
</head>
<body class="{"stage" if stage else ""}">
{H.DEFS}
{body}
{f"<script>{extra_js}</script>" if extra_js else ""}
</body>
</html>"""
    return "\n".join(line.rstrip() for line in txt.splitlines()) + "\n"


INTERACTIVE_JS = r"""
(function(){
  const root = document.getElementById('app');
  const state = {variant: root.dataset.variant, palette: 'mocha',
                 frame: 'blocked-heavy', mode: 'herd', rm: false,
                 overlay: null, selected: null, repo: null, page: 0,
                 collapsed: []};
  const screens = JSON.parse(document.getElementById('screens').textContent);
  function key(){
    // selection only matters for the detail sheet; the filter sheet and the
    // plain screen are keyed without it so a stale selection never misses.
    const sel = state.overlay === 'detail-sheet' ? (state.selected || '-') : '-';
    return [state.variant, state.palette, state.frame, state.mode,
            state.rm ? 'rm' : 'motion', state.overlay || 'none',
            state.repo || 'all', sel].join('|');
  }
  function render(){
    const k = key();
    let html = screens[k];
    if (!html) {
      console.warn('screen miss', k);
      // fall back: same without selection / overlay specifics
      const fb = [state.variant, state.palette, state.frame, state.mode,
                  state.rm ? 'rm' : 'motion', 'none', state.repo || 'all', '-'].join('|');
      html = screens[fb] || '<div class="phone"><p style="padding:80px 20px">frame not prebuilt: ' + k + '</p></div>';
    }
    root.innerHTML = html;
    document.documentElement.dataset.palette = state.palette;
    document.body.classList.toggle('latte', state.palette === 'latte');
    document.querySelectorAll('.ctl [data-set]').forEach(b => {
      const [k2, v] = b.dataset.set.split('=');
      const cur = String(state[k2]);
      b.setAttribute('aria-pressed', cur === v ? 'true' : 'false');
    });
    document.querySelectorAll('.ctl [data-palette-css]').forEach(s => {});
    const strip = root.querySelector('.paddock-strip');
    if (strip) strip.scrollLeft = state.page * strip.clientWidth;
  }
  document.addEventListener('click', e => {
    const ctl = e.target.closest('[data-set]');
    if (ctl) {
      const [k2, v] = ctl.dataset.set.split('=');
      if (k2 === 'rm') state.rm = (v === 'true');
      else if (k2 === 'repo') state.repo = (v === 'null' ? null : v);
      else state[k2] = v;
      if (k2 === 'frame' || k2 === 'mode') { state.overlay = null; state.selected = null; state.page = 0; }
      render(); return;
    }
    const sw = e.target.closest('[data-switch]');
    if (sw && root.contains(sw)) { state.mode = sw.dataset.switch; state.overlay = null; render(); return; }
    const opens = e.target.closest('[data-opens]');
    if (opens && root.contains(opens)) {
      state.overlay = opens.dataset.opens;
      if (opens.dataset.agent) state.selected = opens.dataset.agent;
      render(); return;
    }
    const closes = e.target.closest('[data-closes]');
    if (closes) { state.overlay = null; state.selected = null; render(); return; }
    const opt = e.target.closest('.opt');
    if (opt && root.contains(opt)) {
      const chip = opt.querySelector('.chip');
      if (opt.textContent.startsWith('All repositories')) state.repo = null;
      else if (chip) state.repo = chip.textContent.trim();
      state.overlay = null; render(); return;
    }
  });
  // swipe / scroll paging for V1
  document.addEventListener('scroll', e => {
    const strip = e.target.closest && e.target.closest('.paddock-strip');
    if (!strip) return;
    const p = Math.round(strip.scrollLeft / strip.clientWidth);
    if (p !== state.page) { state.page = p;
      root.querySelectorAll('.dots i').forEach((d, i) => d.classList.toggle('on', i === p)); }
  }, true);
  // palette CSS swap
  const styles = JSON.parse(document.getElementById('palettes').textContent);
  const pal = document.getElementById('pal');
  const origRender = render;
  render = function(){ pal.textContent = styles[state.palette]; origRender(); };
  render();
})();
"""


def build_interactive(variant: str) -> str:
    """One self-contained interactive page: every frame x palette x mode x
    RM x overlay pre-rendered as HTML strings, swapped by a small script."""
    screens = {}
    for flavor in PALETTES:
        for frame in FRAMES:
            for mode in ("herd", "board"):
                for rm in (False, True):
                    fx = F.FRAMES[frame]()
                    base_key = [variant, flavor, frame, mode,
                                "rm" if rm else "motion"]
                    screens["|".join(base_key + ["none", "all", "-"])] = \
                        screen(variant, flavor, frame, mode=mode, rm=rm)
                    if fx.source_ok:
                        screens["|".join(base_key + ["filter-sheet", "all", "-"])] = \
                            screen(variant, flavor, frame, mode=mode, rm=rm,
                                   overlay="filter-sheet")
                        for a in fx.agents:
                            screens["|".join(base_key + ["detail-sheet", "all", a.name])] = \
                                screen(variant, flavor, frame, mode=mode, rm=rm,
                                       overlay="detail-sheet",
                                       opts={"selected": a.name})
                        for repo in sorted({a.repo for a in fx.agents}):
                            screens["|".join(base_key + ["none", repo, "-"])] = \
                                screen(variant, flavor, frame, mode=mode, rm=rm,
                                       opts={"repo": repo})
                            screens["|".join(base_key + ["filter-sheet", repo, "-"])] = \
                                screen(variant, flavor, frame, mode=mode, rm=rm,
                                       overlay="filter-sheet", opts={"repo": repo})
                            for a in fx.agents:
                                if a.repo != repo:
                                    continue
                                screens["|".join(base_key + ["detail-sheet", repo, a.name])] = \
                                    screen(variant, flavor, frame, mode=mode, rm=rm,
                                           overlay="detail-sheet",
                                           opts={"repo": repo, "selected": a.name})
    v = VARIANTS[variant]
    ctl = (
        '<div class="ctl">'
        '<span class="grp">Palette</span>'
        '<button type="button" data-set="palette=mocha" aria-pressed="true">Mocha</button>'
        '<button type="button" data-set="palette=latte" aria-pressed="false">Latte</button>'
        '<span class="grp">Frame</span>'
        '<button type="button" data-set="frame=blocked-heavy" aria-pressed="true">Blocked-heavy</button>'
        '<button type="button" data-set="frame=dense-fleet" aria-pressed="false">Dense fleet</button>'
        '<button type="button" data-set="frame=disconnected" aria-pressed="false">Disconnected</button>'
        '<span class="grp">Motion</span>'
        '<button type="button" data-set="rm=false" aria-pressed="true">Animated</button>'
        '<button type="button" data-set="rm=true" aria-pressed="false">Reduce Motion</button>'
        '</div>')
    screens_json = json.dumps(screens).replace("</", SAFE_CLOSE)
    palettes_json = json.dumps(
        {p: css_for(p) for p in PALETTES}).replace("</", SAFE_CLOSE)
    body = (
        f'<div class="shell">'
        f'<div style="max-width:390px;text-align:center">'
        f'<h1 style="font-size:20px;font-weight:700">{esc(v["title"])}</h1>'
        f'<p style="font-size:13px;color:var(--subtext0);margin-top:4px">'
        f'Prototype \u00b7 fictional fixtures \u00b7 original horse art \u00b7 '
        f'tap Board/Herd, Filters, any horse</p></div>'
        f'{ctl}'
        f'<div class="frame"><div id="app" data-variant="{variant}"></div></div>'
        f'<p style="font-size:12px;color:var(--subtext0);max-width:390px;'
        f'text-align:center">Every screen is pre-rendered from '
        f'scripts/build.py; the script only swaps them. Stage copies used '
        f'for the PNGs live in stage/.</p>'
        f'</div>'
        f'<script type="application/json" id="screens">'
        f'{screens_json}</script>'
        f'<script type="application/json" id="palettes">'
        f'{palettes_json}'
        f'</script>')
    extra_css = "#pal{}"
    return doc(v["title"], body, "mocha", extra_css=extra_css,
               extra_js=INTERACTIVE_JS).replace(
        "<style>\n", "<style id=\"pal\">\n", 1).replace(
        f"{CSS_BASE}", f"</style><style>{CSS_BASE}", 1)


# -------------------------------------------------------- sprite sheet -----

def sprite_sheet() -> str:
    names = sorted({a.name for fr in F.FRAMES.values() for a in fr().agents})
    rows = []
    for n in names:
        ident = F.horse_identity(n)
        cells = "".join(
            f'<td><div class="cell">{H.horse_svg(ident, s)}</div></td>'
            for s in F.STATES)
        rm_cells = "".join(
            f'<td><div class="cell rm">{H.horse_svg(ident, s, reduce_motion=True)}'
            f'</div></td>' for s in ("working", "blocked"))
        rows.append(
            f'<tr><th><div class="who"><b>{esc(n)}</b>'
            f'<span>{ident["coat"]} \u00b7 {ident["breed"]} \u00b7 '
            f'{ident["mane"]} mane \u00b7 {ident["tack"]} \u00b7 '
            f'{ident["accessory"]}</span></div></th>{cells}{rm_cells}</tr>')
    axes = []
    for coat in F.COATS:
        ident = {"coat": coat, "breed": "light", "mane": "flowing",
                 "tack": "none", "accessory": "none"}
        axes.append(f'<td><div class="cell sm">{H.horse_svg(ident, "done")}'
                    f'</div><div class="lab">{coat}</div></td>')
    breeds = "".join(
        f'<td><div class="cell sm">'
        f'{H.horse_svg({"coat": "bay", "breed": b, "mane": "cropped", "tack": "none", "accessory": "none"}, "done")}'
        f'</div><div class="lab">{b}</div></td>' for b in F.BREEDS)
    manes = "".join(
        f'<td><div class="cell sm">'
        f'{H.horse_svg({"coat": "grey", "breed": "light", "mane": m, "tack": "none", "accessory": "none"}, "done")}'
        f'</div><div class="lab">{m}</div></td>' for m in F.MANES)
    tacks = "".join(
        f'<td><div class="cell sm">'
        f'{H.horse_svg({"coat": "chestnut", "breed": "stock", "mane": "braided", "tack": t, "accessory": "none"}, "done")}'
        f'</div><div class="lab">{t}</div></td>' for t in F.TACKS)
    accs = "".join(
        f'<td><div class="cell sm">'
        f'{H.horse_svg({"coat": "dun", "breed": "light", "mane": "flowing", "tack": "pad", "accessory": a}, "done")}'
        f'</div><div class="lab">{a}</div></td>' for a in F.ACCESSORIES)
    body = f"""
<div class="wrap">
<h1>Corral herd \u2014 original horse sprite sheet</h1>
<p class="lede">Every horse is authored as flat vector SVG by
<code>scripts/horsesvg.py</code> in this lane (no borrowed mascots, no
kitchen sprites, no provider logos). Identity = sha256(agent name) \u2192
coat \u00d7 breed silhouette \u00d7 mane \u00d7 tack \u00d7 accessory, so the
same lane is always the same horse. State changes pose only. Coat colors
are fixed natural horse colors, deliberately outside the Catppuccin
palettes so identity survives Mocha \u2194 Latte.</p>

<h2>Identity axes</h2>
<table class="axes"><tr><th>coat (8)</th>{"".join(axes)}</tr>
<tr><th>breed silhouette (3)</th>{breeds}</tr>
<tr><th>mane (3)</th>{manes}</tr>
<tr><th>tack (3)</th>{tacks}</tr>
<tr><th>accessory (3)</th>{accs}</tr></table>

<h2>State poses \u00b7 one row per fixture lane (identity constant across columns)</h2>
<p class="lede">Columns: working (trot loop / leg swing), blocked (head high, near hoof
paws, frontal alert flag), idle (grazing), done (settled stand, never active),
unknown (uneasy stance + ? badge), then the Reduce Motion statics for working
(planted stand, heartbeat \u2192 static dot) and blocked (hoof planted, no pulse).</p>
<table class="poses">
<thead><tr><th>lane</th><th>working</th><th>blocked</th><th>idle</th><th>done</th><th>unknown</th><th>working \u00b7 RM</th><th>blocked \u00b7 RM</th></tr></thead>
<tbody>{"".join(rows)}</tbody></table>
<p class="lede">Provenance: see PROVENANCE.md. Generator SHA is in the manifest.</p>
</div>"""
    extra = """
.wrap{max-width:1180px;margin:0 auto;padding:24px 16px 60px}
h1{font-size:24px;margin-bottom:8px}h2{font-size:17px;margin:26px 0 8px}
.lede{font-size:14px;color:var(--subtext1);max-width:820px;line-height:1.45}
code{font-family:var(--mono);font-size:12px;background:var(--surface0);padding:1px 5px;border-radius:4px}
table{border-collapse:separate;border-spacing:6px}
th{font-family:var(--mono);font-size:11px;color:var(--subtext0);text-align:left;font-weight:600;vertical-align:middle}
.cell{width:132px;height:100px;background:var(--grass);border-radius:10px;border:1px solid var(--surface1)}
.cell.sm{width:110px;height:84px}
.cell.rm{outline:2px dashed var(--surface2)}
.cell .horse-svg{width:100%;height:100%}
.lab{font-family:var(--mono);font-size:11px;color:var(--subtext1);text-align:center;margin-top:3px}
.who{min-width:180px}.who b{display:block;font-family:var(--mono);font-size:12px;color:var(--text)}
.who span{display:block;font-size:11px;color:var(--subtext0);margin-top:2px}
"""
    return doc("Corral herd \u2014 sprite sheet (issue-442)", body, "mocha",
               extra_css=extra)


# ------------------------------------------------------------- gallery -----

def gallery(png_names: list[str]) -> str:
    cards = []
    for v in ("v1", "v2"):
        cards.append(
            f'<a class="card big" href="prototype-{VARIANTS[v]["slug"]}.html">'
            f'<b>{esc(VARIANTS[v]["title"])}</b><span>interactive prototype '
            f'\u00b7 Board \u2194 Herd \u00b7 filters \u00b7 detail sheet</span></a>')
    cards.append('<a class="card" href="sprite-sheet.html"><b>Sprite sheet</b>'
                 '<span>identity axes \u00d7 state poses \u00d7 Reduce Motion</span></a>')
    cards.append('<a class="card" href="README.md"><b>README</b>'
                 '<span>hierarchy \u00b7 measured comparison \u00b7 recommendation</span></a>')
    thumbs = []
    for n in png_names:
        label = n.replace("herd-", "").replace("-390x844.png", "")
        thumbs.append(f'<a class="th" href="{n}"><img src="{n}" alt="{esc(label)}" '
                      f'loading="lazy" width="195" height="422"><span>{esc(label)}'
                      f'</span></a>')
    body = f"""
<div class="wrap">
<h1>Corral #442 \u2014 Herd view</h1>
<p class="lede">Two information architectures for the optional full-screen
Herd mode over the same live fleet state as the Board. Fictional fixtures,
original horse art. <b>PROTOTYPE \u2014 awaiting Guy approval.</b></p>
<div class="cards">{"".join(cards)}</div>
<h2>Recommendation</h2>
<p class="lede"><b>V2 Ranch Map</b> for the first implementation: it keeps
all four paddocks and every blocked horse on one screen at 390\u00d7844
(V1 shows one paddock per page), scales to the 18-horse dense fixture with
a Show-all row instead of a swipe hunt, and its front-rail card is a bounded
control the impl lane can port directly. V1 is the stronger story and
should return as the per-paddock zoom inside V2. Full measurements in the
README. This recommendation does not clear the gate.</p>
<h2>Frames \u00b7 390\u00d7844</h2>
<div class="thumbs">{"".join(thumbs)}</div>
</div>"""
    extra = """
.wrap{max-width:900px;margin:0 auto;padding:22px 14px 60px}
h1{font-size:24px;margin-bottom:6px}h2{font-size:17px;margin:24px 0 8px}
.lede{font-size:14px;color:var(--subtext1);line-height:1.45}
.cards{display:grid;grid-template-columns:1fr 1fr;gap:10px;margin-top:14px}
.card{display:flex;flex-direction:column;gap:4px;padding:14px;border-radius:14px;background:var(--surface0);border:1px solid var(--surface1);color:var(--text);text-decoration:none;min-height:var(--tap)}
.card.big{background:color-mix(in srgb,var(--mauve) 14%,var(--surface0));border-color:color-mix(in srgb,var(--mauve) 38%,var(--surface1))}
.card b{font-size:15px}.card span{font-size:12px;color:var(--subtext0)}
.thumbs{display:grid;grid-template-columns:repeat(auto-fill,minmax(160px,1fr));gap:10px}
.th{display:flex;flex-direction:column;gap:4px;text-decoration:none;color:var(--subtext1)}
.th img{width:100%;height:auto;border-radius:12px;border:1px solid var(--surface1);background:var(--crust)}
.th span{font-family:var(--mono);font-size:10px;line-height:1.25;word-break:break-all}
@media (max-width:480px){.cards{grid-template-columns:1fr}}
"""
    return doc("Corral #442 \u2014 Herd view gallery", body, "mocha",
               extra_css=extra)


# ---------------------------------------------------------------- stages ---

def stage_specs() -> list[dict]:
    """The exact frames captured as PNGs. Name = herd-<variant>-<palette>-
    <frame>-390x844.png. Twelve required + proof extras."""
    specs = []
    for v in ("v1", "v2"):
        for p in PALETTES:
            for fr in FRAMES:
                specs.append({"name": f"herd-{v}-{p}-{fr}", "variant": v,
                              "palette": p, "frame": fr, "mode": "herd",
                              "rm": False, "overlay": None, "opts": {}})
        # proof extras (mocha): detail sheet from a blocked horse, filter
        # sheet with a repo scope applied, Reduce Motion statics, Board mode
        specs += [
            {"name": f"herd-{v}-mocha-detail-sheet", "variant": v,
             "palette": "mocha", "frame": "blocked-heavy", "mode": "herd",
             "rm": False, "overlay": "detail-sheet",
             "opts": {"selected": "oak-before-dark"}},
            {"name": f"herd-{v}-mocha-filter-sheet", "variant": v,
             "palette": "mocha", "frame": "blocked-heavy", "mode": "herd",
             "rm": False, "overlay": "filter-sheet", "opts": {}},
            {"name": f"herd-{v}-mocha-reduce-motion", "variant": v,
             "palette": "mocha", "frame": "blocked-heavy", "mode": "herd",
             "rm": True, "overlay": None, "opts": {}},
            {"name": f"herd-{v}-latte-repo-scoped", "variant": v,
             "palette": "latte", "frame": "dense-fleet", "mode": "herd",
             "rm": False, "overlay": None,
             "opts": {"repo": F.REPO_LONG}},
        ]
    specs.append({"name": "board-mocha-blocked-heavy", "variant": "v2",
                  "palette": "mocha", "frame": "blocked-heavy",
                  "mode": "board", "rm": False, "overlay": None, "opts": {}})
    return specs


def build_stage(spec: dict) -> str:
    stamp = spec["name"].replace("herd-", "").replace("-", " \u00b7 ", 2)
    body = screen(spec["variant"], spec["palette"], spec["frame"],
                  mode=spec["mode"], rm=spec["rm"], overlay=spec["overlay"],
                  stamp=spec["name"], opts=spec["opts"])
    return doc(spec["name"], body, spec["palette"], stage=True)


# ---------------------------------------------------------------- main -----

def main() -> int:
    os.chdir(EVID)
    (EVID / "stage").mkdir(exist_ok=True)
    written = []
    for v in ("v1", "v2"):
        p = EVID / f"prototype-{VARIANTS[v]['slug']}.html"
        p.write_text(build_interactive(v), encoding="utf-8")
        written.append(p)
    p = EVID / "sprite-sheet.html"
    p.write_text(sprite_sheet(), encoding="utf-8")
    written.append(p)
    specs = stage_specs()
    for spec in specs:
        sp = EVID / "stage" / f"{spec['name']}.html"
        sp.write_text(build_stage(spec), encoding="utf-8")
        written.append(sp)
    pngs = [f"{s['name']}-390x844.png" for s in specs]
    p = EVID / "index.html"
    p.write_text(gallery(pngs), encoding="utf-8")
    written.append(p)
    (EVID / "stage" / "specs.json").write_text(
        json.dumps(specs, indent=1) + "\n", encoding="utf-8")
    written.append(EVID / "stage" / "specs.json")
    for w in written:
        print(f"wrote {w.relative_to(EVID)}  {w.stat().st_size} B")
    print(f"BUILD OK: {len(written)} files, {len(specs)} stages")
    return 0


if __name__ == "__main__":
    sys.exit(main())
