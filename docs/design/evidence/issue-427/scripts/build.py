#!/usr/bin/env python3
"""Build the deterministic Corral #427 prototype bundle."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

HTML = r'''<!doctype html>
<html lang="en" data-palette="__PALETTE__">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<title>Corral #427 — filter header prototype __DEFAULT_VARIANT__</title>
<style>
[data-palette="latte"]{--rosewater:#dc8a78;--flamingo:#dd7878;--pink:#ea76cb;--mauve:#8839ef;--red:#d20f39;--maroon:#e64553;--peach:#fe640b;--yellow:#df8e1d;--green:#40a02b;--teal:#179299;--sky:#04a5e5;--sapphire:#209fb5;--blue:#1e66f5;--lavender:#7287fd;--text:#4c4f69;--subtext1:#5c5f77;--subtext0:#6c6f85;--overlay2:#7c7f93;--overlay1:#8c8fa1;--overlay0:#9ca0b0;--surface2:#acb0be;--surface1:#bcc0cc;--surface0:#ccd0da;--base:#eff1f5;--mantle:#e6e9ef;--crust:#dce0e8}
[data-palette="frappe"]{--rosewater:#f2d5cf;--flamingo:#eebebe;--pink:#f4b8e4;--mauve:#ca9ee6;--red:#e78284;--maroon:#ea999c;--peach:#ef9f76;--yellow:#e5c890;--green:#a6d189;--teal:#81c8be;--sky:#99d1db;--sapphire:#85c1dc;--blue:#8caaee;--lavender:#babbf1;--text:#c6d0f5;--subtext1:#b5bfe2;--subtext0:#a5adce;--overlay2:#949cbb;--overlay1:#838ba7;--overlay0:#737994;--surface2:#626880;--surface1:#51576d;--surface0:#414559;--base:#303446;--mantle:#292c3c;--crust:#232634}
[data-palette="macchiato"]{--rosewater:#f4dbd6;--flamingo:#f0c6c6;--pink:#f5bde6;--mauve:#c6a0f6;--red:#ed8796;--maroon:#ee99a0;--peach:#f5a97f;--yellow:#eed49f;--green:#a6da95;--teal:#8bd5ca;--sky:#91d7e3;--sapphire:#7dc4e4;--blue:#8aadf4;--lavender:#b7bdf8;--text:#cad3f5;--subtext1:#b8c0e0;--subtext0:#a5adcb;--overlay2:#939ab7;--overlay1:#8087a2;--overlay0:#6e738d;--surface2:#5b6078;--surface1:#494d64;--surface0:#363a4f;--base:#24273a;--mantle:#1e2030;--crust:#181926}
[data-palette="mocha"]{--rosewater:#f5e0dc;--flamingo:#f2cdcd;--pink:#f5c2e7;--mauve:#cba6f7;--red:#f38ba8;--maroon:#eba0ac;--peach:#fab387;--yellow:#f9e2af;--green:#a6e3a1;--teal:#94e2d5;--sky:#89dceb;--sapphire:#74c7ec;--blue:#89b4fa;--lavender:#b4befe;--text:#cdd6f4;--subtext1:#bac2de;--subtext0:#a6adc8;--overlay2:#9399b2;--overlay1:#7f849c;--overlay0:#6c7086;--surface2:#585b70;--surface1:#45475a;--surface0:#313244;--base:#1e1e2e;--mantle:#181825;--crust:#11111b}
:root{--accent:var(--mauve);--hairline:color-mix(in srgb,var(--surface1) 58%,transparent);--status-surface:var(--surface1);--ui-scale:1}
[data-palette="latte"]{--status-surface:var(--surface0)}
*{box-sizing:border-box}
html,body{margin:0;min-width:100%;min-height:100%;background:var(--crust);color:var(--text);font-family:-apple-system,BlinkMacSystemFont,"SF Pro Text","Helvetica Neue",Arial,sans-serif;-webkit-font-smoothing:antialiased}
body.capture{width:100vw;height:100vh;overflow:hidden;background:var(--base)}
button{font:inherit;color:inherit;border:0;background:none;padding:0;cursor:pointer}
button:focus-visible{outline:3px solid var(--accent);outline-offset:2px}
[data-hit-target]{min-width:44px;min-height:44px}
.mono{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-variant-numeric:tabular-nums}
.shell{min-height:100vh;display:grid;grid-template-columns:300px minmax(375px,390px);gap:28px;align-items:start;justify-content:center;padding:28px;background:var(--crust)}
body.capture .shell{display:block;padding:0;min-height:0;background:var(--base)}
.demo-controls{position:sticky;top:28px;padding:18px;border:1px solid var(--surface1);border-radius:18px;background:var(--mantle)}
body.capture .demo-controls{display:none}
.demo-controls .eyebrow{font:700 11px/1.2 ui-monospace,SFMono-Regular,monospace;letter-spacing:.08em;text-transform:uppercase;color:var(--accent)}
.demo-controls h1{font-size:22px;line-height:1.08;margin:8px 0 6px;letter-spacing:-.02em}
.demo-controls p{font-size:13px;line-height:1.45;color:var(--subtext1);margin:0 0 16px}
.control-group{border-top:1px solid var(--surface0);padding-top:12px;margin-top:12px}
.control-group b{display:block;font-size:11px;text-transform:uppercase;letter-spacing:.08em;color:var(--subtext1);margin-bottom:8px}
.control-set{display:flex;flex-wrap:wrap;gap:6px}
.demo-chip{min-height:36px;padding:7px 10px;border:1px solid var(--surface1);border-radius:999px;background:var(--base);font-size:12px;font-weight:650}
.demo-chip[aria-pressed="true"]{background:var(--accent);border-color:var(--accent);color:var(--base)}
.phone{width:min(100vw,390px);height:min(100vh,844px);position:relative;overflow:hidden;display:flex;flex-direction:column;background:var(--base);color:var(--text);font-size:calc(13px * var(--ui-scale));line-height:1.3;isolation:isolate}
body:not(.capture) .phone{width:390px;height:844px;border-radius:34px;box-shadow:0 0 0 1px var(--surface1);overflow:hidden}
body.a11y .phone{--ui-scale:1.25}
.statusbar{height:54px;flex:0 0 54px;position:relative;display:flex;align-items:flex-start;justify-content:space-between;padding:16px 21px 0;background:var(--mantle);font-size:12px;font-weight:650}
.island{position:absolute;top:7px;left:50%;transform:translateX(-50%);width:118px;height:31px;border-radius:18px;background:#000;z-index:2}
.sysicons{display:flex;gap:5px;align-items:center}
.signal{width:13px;height:8px;border:1.5px solid currentColor;border-top:0;border-left:0;transform:skewX(-18deg)}
.battery{width:20px;height:9px;border:1.5px solid currentColor;border-radius:3px;position:relative}
.battery:after{content:"";position:absolute;right:-3px;top:2px;width:2px;height:4px;background:currentColor;border-radius:0 2px 2px 0}
.battery:before{content:"";position:absolute;inset:1.5px 4px 1.5px 1.5px;background:currentColor;border-radius:1px}
.navbar{min-height:58px;flex:0 0 auto;display:grid;grid-template-columns:minmax(0,1fr) 52px;align-items:center;padding:0 12px 0 14px;background:var(--mantle);border-bottom:1px solid var(--crust)}
.filter-trigger{justify-self:start;max-width:280px;min-height:52px;display:grid;grid-template-columns:22px minmax(0,1fr);grid-template-rows:auto auto;column-gap:8px;align-content:center;text-align:left;border-radius:11px;padding:4px 7px 4px 4px}
.filter-trigger .filter-icon{grid-row:1/3;width:22px;height:22px;color:var(--accent);align-self:center}
.filter-trigger .label{font-size:16px;line-height:1.1;font-weight:720;letter-spacing:-.015em}
.filter-trigger .summary{font-size:11px;line-height:1.2;color:var(--subtext1);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;margin-top:3px}
.settings{justify-self:end;width:44px;height:44px;border-radius:50%;display:grid;place-items:center;color:var(--accent)}
.settings svg{width:21px;height:21px}
.refresh-hint{min-height:30px;flex:0 0 auto;display:flex;align-items:center;gap:6px;padding:5px 18px;background:var(--mantle);border-bottom:1px solid var(--crust);font-size:10.5px;color:var(--subtext1)}
.refresh-arrow{font-size:13px;color:var(--accent)}
.health-banner{min-height:36px;display:flex;align-items:center;gap:8px;padding:7px 16px;border-bottom:1px solid var(--hairline);background:color-mix(in srgb,var(--peach) 8%,var(--base));color:var(--text);font-size:11px;font-weight:620}
.health-mark{flex:0 0 9px;width:9px;height:9px;border-radius:3px;background:var(--peach)}
.board-shell{position:relative;flex:1;min-height:0;display:flex;flex-direction:column;overflow:hidden;background:var(--base)}
.board{min-height:0;flex:1;overflow-y:auto;overflow-x:hidden;scrollbar-width:none;background:var(--base);padding-bottom:20px}
.board::-webkit-scrollbar,.panel-scroll::-webkit-scrollbar{display:none}
.status-section{border-bottom:1px solid var(--crust)}
.status-head{position:sticky;top:0;z-index:2;min-height:44px;display:flex;align-items:center;gap:8px;padding:8px 18px;background:var(--status-surface);font-size:16px;font-weight:750;letter-spacing:-.01em}
.status-square{width:10px;height:10px;border-radius:3px;background:var(--state)}
.status-head .chevron{margin-left:auto;color:var(--subtext1)}
.repo-band{min-height:28px;display:flex;align-items:center;gap:7px;padding:5px 18px 5px 16px;border-left:2px solid var(--hue);background:color-mix(in srgb,var(--hue) 9%,var(--mantle));font-size:11px;font-weight:680;color:var(--text)}
.repo-band .repo-dot{width:8px;height:8px;border-radius:3px;background:var(--hue)}
.repo-band .count{margin-left:auto;font-variant-numeric:tabular-nums}
.agent-row{min-height:62px;display:grid;grid-template-columns:39px minmax(0,1fr) auto;gap:10px;align-items:center;padding:8px 16px;border-bottom:1px solid var(--surface0);background:var(--base)}
.state-glyph{width:38px;height:24px;border-radius:8px;display:grid;place-items:center;background:color-mix(in srgb,var(--state) 17%,var(--base));border:1px solid color-mix(in srgb,var(--state) 34%,var(--base));font-size:10px;font-weight:800;color:var(--text)}.workbeat{display:flex;align-items:center;gap:3px}.workbeat i{width:4px;height:4px;border-radius:1px;background:var(--teal)}
.agent-copy{min-width:0}.agent-name{font-size:13px;font-weight:680;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.agent-meta{display:flex;gap:5px;align-items:center;min-width:0;margin-top:4px;color:var(--subtext1);font-size:10px}.agent-meta .branch{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.host-badge{display:inline-flex;align-items:center;min-height:20px;padding:2px 6px;border-radius:6px;background:var(--mantle);border:1px solid var(--surface0);font-size:9px;color:var(--subtext1);white-space:nowrap}.age{font-size:10px;color:var(--subtext1);font-variant-numeric:tabular-nums}.stale-line{grid-column:2/4;color:var(--subtext1);font-size:10px;margin-top:-4px}.empty{min-height:100%;display:flex;flex-direction:column;align-items:center;justify-content:center;text-align:center;padding:36px 30px}.empty .empty-glyph{width:38px;height:38px;border:2px solid var(--surface2);border-radius:12px;display:grid;place-items:center;color:var(--accent);font-size:19px}.empty h2{font-size:17px;margin:13px 0 5px}.empty p{font-size:12px;line-height:1.45;color:var(--subtext1);margin:0 0 12px}.text-action{min-height:44px;padding:7px 8px;color:var(--accent);font-size:13px;font-weight:680;border-radius:10px}
/* Variant B: compact inline rail, closed height 132pt. */
.inline-rail{flex:0 0 auto;background:var(--mantle);border-bottom:1px solid var(--crust);padding:6px 12px 8px}
.rail-row{display:grid;grid-template-columns:minmax(0,1fr) 44px;align-items:stretch;min-height:48px;border-bottom:1px solid var(--surface0)}
.rail-row:last-of-type{border-bottom:0}.rail-select{display:grid;grid-template-columns:74px minmax(0,1fr) 18px;align-items:center;text-align:left;padding:4px 4px;border-radius:9px}.rail-scope{font:720 10px/1 ui-monospace,SFMono-Regular,monospace;letter-spacing:.06em;color:var(--subtext1);text-transform:uppercase}.rail-value{font-size:13px;font-weight:660;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.rail-caret{color:var(--accent);text-align:center}.rail-clear{width:44px;height:44px;align-self:center;border-radius:50%;color:var(--accent);font-size:18px}.rail-clear[disabled]{opacity:.28}.rail-reset{width:100%;min-height:44px;display:flex;align-items:center;justify-content:center;color:var(--accent);font-size:12px;font-weight:680}.inline-menu{margin:4px 0 7px;border:1px solid var(--surface1);border-radius:12px;overflow:hidden;background:var(--base)}
/* Shared filter surface */
.filter-scope{position:relative}.filter-scope+.filter-scope{border-top:8px solid color-mix(in srgb,var(--mantle) 72%,var(--base))}.scope-heading{min-height:48px;display:flex;align-items:center;gap:8px;padding:4px 16px;border-bottom:1px solid var(--hairline)}.scope-heading h3{margin:0;font-size:11px;text-transform:uppercase;letter-spacing:.07em;color:var(--subtext1)}.scope-heading .scope-clear{margin-left:auto;color:var(--accent);font-size:12px;font-weight:670;min-height:44px;padding:0 4px}.scope-heading .scope-clear[disabled]{opacity:.3}.choice{width:100%;min-height:50px;display:grid;grid-template-columns:minmax(0,1fr) 18px;gap:10px;align-items:center;text-align:left;padding:7px 16px;border-bottom:1px solid var(--hairline);background:transparent}.choice:last-child{border-bottom:0}.choice .check{width:16px;color:var(--accent);font-size:15px;font-weight:800;text-align:center}.choice-copy{min-width:0}.choice-name{display:block;font-size:13px;font-weight:660;line-height:1.25;white-space:normal;overflow-wrap:anywhere}.choice-meta{display:block;margin-top:2px;color:var(--subtext1);font-size:10.5px;line-height:1.3;white-space:normal}.choice[aria-pressed="true"]{background:color-mix(in srgb,var(--accent) 12%,var(--base))}.choice[aria-pressed="true"] .choice-name{color:var(--accent)}
/* Variant A: native bottom sheet with visible board context. */
.scrim{position:absolute;inset:0;z-index:7;background:color-mix(in srgb,var(--crust) 64%,transparent);backdrop-filter:blur(2px)}
.sheet{position:absolute;left:0;right:0;bottom:0;z-index:8;height:min(73%,620px);display:flex;flex-direction:column;overflow:hidden;border-radius:22px 22px 0 0;border-top:1px solid var(--surface1);background:color-mix(in srgb,var(--base) 80%,transparent);backdrop-filter:blur(24px) saturate(115%);-webkit-backdrop-filter:blur(24px) saturate(115%)}
.drag-handle{width:37px;height:5px;border-radius:4px;background:var(--surface2);margin:8px auto 3px;flex:0 0 auto}.sheet-head{min-height:56px;display:grid;grid-template-columns:minmax(0,1fr) auto;gap:4px;align-items:center;padding:0 10px 0 16px;border-bottom:1px solid var(--hairline)}.sheet-title{min-width:0}.sheet-title h2{font-size:17px;letter-spacing:-.015em;margin:0}.sheet-title p{font-size:10.5px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;color:var(--subtext1);margin:3px 0 0}.close{width:44px;height:44px;border-radius:50%;display:grid;place-items:center;color:var(--accent);font-size:20px;font-weight:500}.panel-scroll{flex:1;min-height:0;overflow-y:auto;scrollbar-width:none;background:color-mix(in srgb,var(--base) 92%,transparent)}.sheet-footer{flex:0 0 auto;min-height:54px;display:grid;place-items:center;padding:4px 12px 6px;border-top:1px solid var(--surface0);background:color-mix(in srgb,var(--mantle) 88%,transparent)}.sheet-footer .text-action{width:100%}
/* Variant C: full inline Scope Path editor. */
.scope-editor{flex:1;min-height:0;display:flex;flex-direction:column;background:var(--base)}.editor-head{min-height:70px;display:grid;grid-template-columns:minmax(0,1fr) 44px;align-items:center;padding:8px 12px 8px 18px;border-bottom:1px solid var(--surface0);background:var(--mantle)}.editor-head h2{font-size:18px;margin:0;letter-spacing:-.02em}.editor-head p{font-size:10.5px;color:var(--subtext1);margin:3px 0 0;line-height:1.3}.scope-path{position:relative;padding-left:34px}.scope-path:before{content:"";position:absolute;left:25px;top:23px;bottom:23px;width:2px;background:color-mix(in srgb,var(--accent) 38%,var(--surface0))}.scope-node{position:relative}.node-number{position:absolute;left:-23px;top:12px;width:20px;height:20px;z-index:1;display:grid;place-items:center;border-radius:6px;background:var(--accent);color:var(--base);font:800 10px/1 ui-monospace,SFMono-Regular,monospace}.editor-footer{min-height:58px;display:grid;grid-template-columns:minmax(0,1fr) auto;align-items:center;gap:8px;padding:7px 12px 7px 18px;border-top:1px solid var(--surface0);background:var(--mantle)}.editor-footer .current{min-width:0}.editor-footer .current b{display:block;font-size:10px;color:var(--subtext1);text-transform:uppercase;letter-spacing:.07em}.editor-footer .current span{display:block;font-size:13px;font-weight:660;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;margin-top:2px}
.home-indicator{position:absolute;z-index:20;left:50%;bottom:5px;transform:translateX(-50%);width:126px;height:5px;border-radius:4px;background:var(--text);opacity:.82;pointer-events:none}

body.a11y .filter-trigger .label{font-size:18px}body.a11y .filter-trigger .summary{font-size:13px}body.a11y .choice-name{font-size:16px}body.a11y .choice-meta{font-size:13px}body.a11y .choice{min-height:60px}body.a11y .rail-value{font-size:15px}body.a11y .agent-name{font-size:15px;white-space:normal}body.a11y .agent-meta{font-size:12px;flex-wrap:wrap}body.a11y .status-head{font-size:18px}body.a11y .sheet{height:78%}
@media (prefers-reduced-motion:reduce){*{animation:none!important;transition:none!important;scroll-behavior:auto!important}}
@media(max-width:760px){.shell{display:block;padding:0}.demo-controls{display:none}.phone{width:100vw;height:100vh;border-radius:0!important;box-shadow:none!important}}
</style>
</head>
<body>
<div class="shell">
  <aside class="demo-controls" aria-label="Prototype controls">
    <div class="eyebrow">Corral · issue 427</div>
    <h1>Filter / header redesign</h1>
    <p>Monitor surface. Three native-iOS directions over the existing Catppuccin board.</p>
    <div class="control-group"><b>Direction</b><div class="control-set" id="variant-controls"></div></div>
    <div class="control-group"><b>Palette</b><div class="control-set" id="palette-controls"></div></div>
    <div class="control-group"><b>State</b><div class="control-set" id="state-controls"></div></div>
  </aside>
  <div id="phone" class="phone" aria-label="Corral iOS prototype"></div>
</div>
<script>
'use strict';
const DEFAULT_VARIANT='__DEFAULT_VARIANT__';
const PARAMS=new URLSearchParams(location.search);
const VARIANTS=['A','B','C'];
const PALETTES=['latte','frappe','macchiato','mocha'];
const STATES=['populated','host-only','repository-only','both-filters','connecting-host','offline-stale-host','zero-results'];
const STATE_LABELS={'populated':'Populated','host-only':'Host only','repository-only':'Repository only','both-filters':'Both filters','connecting-host':'Connecting','offline-stale-host':'Offline / stale','zero-results':'Zero results'};
const vm={
 variant:(PARAMS.get('variant')||DEFAULT_VARIANT).toUpperCase(),
 palette:PARAMS.get('palette')||'mocha',
 scenario:PARAMS.get('state')||'both-filters',
 capture:PARAMS.get('capture')==='1',
 a11y:PARAMS.get('a11y')==='1',
 forceOpen:PARAMS.get('forceOpen')==='1',
 panelOpen:false,
 inlineMenu:null,
 consoleErrors:[]
};
let interactionRunning=false;
const initial={
 'populated':[null,null],
 'host-only':['Bazzite',null],
 'repository-only':[null,'corral'],
 'both-filters':['Bazzite','corral'],
 'connecting-host':[null,null],
 'offline-stale-host':[null,null],
 'zero-results':['Bazzite','corral']
};
[vm.host,vm.repo]=initial[vm.scenario]||[null,null];
vm.panelOpen=vm.forceOpen||((vm.variant==='A'||vm.variant==='C')&&['populated','host-only','repository-only','connecting-host','offline-stale-host'].includes(vm.scenario));
vm.inlineMenu=PARAMS.get('menu')||(vm.forceOpen?'host':(vm.variant==='B'&&['connecting-host','offline-stale-host'].includes(vm.scenario)?'host':(vm.variant==='B'&&vm.scenario==='repository-only'?'repo':null)));
window.addEventListener('error',e=>vm.consoleErrors.push(String(e.message||e.error||'error')));
window.addEventListener('unhandledrejection',e=>vm.consoleErrors.push(String(e.reason||'rejection')));
function esc(s){return String(s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]))}
function iconFilter(){return `<svg class="filter-icon" aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M4 6h10M18 6h2M4 12h3M11 12h9M4 18h8M16 18h4"/><circle cx="16" cy="6" r="2"/><circle cx="9" cy="12" r="2"/><circle cx="14" cy="18" r="2"/></svg>`}
function iconGear(){return `<svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06-2.83 2.83-.06-.06a1.7 1.7 0 0 0-1.88-.34 1.7 1.7 0 0 0-1.03 1.56V21h-4v-.08A1.7 1.7 0 0 0 8.97 19.4a1.7 1.7 0 0 0-1.88.34l-.06.06-2.83-2.83.06-.06A1.7 1.7 0 0 0 4.6 15.03 1.7 1.7 0 0 0 3.08 14H3v-4h.08A1.7 1.7 0 0 0 4.6 8.97a1.7 1.7 0 0 0-.34-1.88L4.2 7.03 7.03 4.2l.06.06a1.7 1.7 0 0 0 1.88.34A1.7 1.7 0 0 0 10 3.08V3h4v.08a1.7 1.7 0 0 0 1.03 1.52 1.7 1.7 0 0 0 1.88-.34l.06-.06 2.83 2.83-.06.06a1.7 1.7 0 0 0-.34 1.88A1.7 1.7 0 0 0 20.92 10H21v4h-.08A1.7 1.7 0 0 0 19.4 15z"/></svg>`}
function activeCount(){return Number(vm.host!==null)+Number(vm.repo!==null)}
function summary(){return `${vm.host||'All hosts'} · ${vm.repo||'All repositories'}`}
function healthFor(name){
 if(name==='macbook-air') return vm.scenario==='connecting-host'?'connecting':'live';
 if(name==='Bazzite') return vm.scenario==='offline-stale-host'?'offline · stale 6m':'live';
 return 'mixed health';
}
const HOSTS=[['macbook-air','15 lanes'],['Bazzite','2 lanes']];
const REPOS=[['corral','1 lane'],['morsel','3 lanes'],['plush-meadow','']];
function choice(kind,name,meta,selected){
 const all=kind==='host'?name==='All hosts':name==='All repositories';
 const aria=all?name:`${kind==='host'?'Host':'Repository'} ${name}`;
 return `<button class="choice" data-hit-target data-kind="${kind}" data-value="${all?'':esc(name)}" aria-label="${esc(aria)}${meta?', '+esc(meta):''}" aria-pressed="${selected}"><span class="choice-copy"><span class="choice-name">${esc(name)}</span>${meta?`<span class="choice-meta">${esc(meta)}</span>`:''}</span><span class="check" aria-hidden="true">${selected?'✓':''}</span></button>`
}
function hostChoices(){return choice('host','All hosts','17 lanes',vm.host===null)+HOSTS.map(([n,c])=>choice('host',n,`${c} · ${healthFor(n)}`,vm.host===n)).join('')}
function repoChoices(){return choice('repo','All repositories','17 lanes',vm.repo===null)+REPOS.map(([n,c])=>choice('repo',n,c,vm.repo===n)).join('')}
function filterScope(kind,pathNode=false){
 const host=kind==='host';
 const clearDisabled=host?vm.host===null:vm.repo===null;
 return `<section class="filter-scope ${pathNode?'scope-node':''}" data-scope="${kind}">${pathNode?`<span class="node-number" aria-hidden="true">${host?'1':'2'}</span>`:''}<div class="scope-heading"><h3>${host?'Host scope':'Repository scope'}</h3><button class="scope-clear" data-hit-target data-clear="${kind}" aria-label="Clear ${kind==='host'?'host':'repository'} filter" ${clearDisabled?'disabled':''}>Clear ${host?'host':'repository'}</button></div>${host?hostChoices():repoChoices()}</section>`
}
function filterTrigger(){
 const n=activeCount(); const label=n?`Filters · ${n}`:'Filters';
 return `<button class="filter-trigger" data-hit-target id="filter-trigger" aria-label="${esc(label)}, ${esc(summary())}">${iconFilter()}<span class="label">${label}</span><span class="summary">${esc(summary())}</span></button>`
}
function statusbar(){return `<div class="statusbar" aria-hidden="true"><span>5:40</span><span class="island"></span><span class="sysicons"><i class="signal"></i><i class="battery"></i></span></div>`}
function navbar(){return `<header class="navbar">${filterTrigger()}<button class="settings" data-hit-target aria-label="Settings">${iconGear()}</button></header><div class="refresh-hint"><span class="refresh-arrow" aria-hidden="true">↓</span><span>pull to refresh · updates stream in automatically</span></div>`}
function healthBanner(){
 if(vm.scenario==='connecting-host') return `<div class="health-banner" role="status"><span class="health-mark"></span><span>macbook-air connecting · Bazzite live</span></div>`;
 if(vm.scenario==='offline-stale-host') return `<div class="health-banner" role="status"><span class="health-mark"></span><span>1 host offline · retained lanes are stale</span></div>`;
 return ''
}
const boardFixtures={
 blocked:[{repo:'demo-orbit',hue:'green',state:'blocked',name:'demo-orbit-blocked',branch:'demo-payload',host:'macbook-air',age:'29s'},{repo:'demo-orbit',hue:'green',state:'blocked',name:'demo-orbit-blocked',branch:'demo-payload',host:'Bazzite',age:'6m',stale:'stale · last seen 6m ago'}],
 working:[{repo:'corral',hue:'blue',state:'working',name:'g427-filter-header-design',branch:'design evidence',host:'macbook-air',age:'now'},{repo:'demo-atlas',hue:'blue',state:'working',name:'demo-atlas-worker',branch:'demo-recent',host:'Bazzite',age:'51s'}]
};
function rowMarkup(r){
 const sym=r.state==='blocked'?'!':'<span class="workbeat" aria-hidden="true"><i></i><i></i><i></i></span>'; const stateColor=r.state==='blocked'?'var(--red)':'var(--teal)';
 return `<div class="agent-row" style="--state:${stateColor}"><span class="state-glyph" aria-label="${r.state}">${sym}</span><span class="agent-copy"><span class="agent-name">${esc(r.name)}</span><span class="agent-meta"><span class="host-badge">${esc(r.host)}</span><span>·</span><span class="branch mono">${esc(r.branch)}</span></span></span><span class="age">${esc(r.age)}</span>${r.stale?`<span class="stale-line">${esc(r.stale)}</span>`:''}</div>`
}
function sectionMarkup(name,rows){
 const state=name==='blocked'?'var(--red)':'var(--teal)'; const hue=rows[0]?.hue==='green'?'var(--green)':'var(--blue)'; const repo=rows[0]?.repo||'corral';
 return `<section class="status-section" style="--state:${state};--hue:${hue}"><div class="status-head"><span class="status-square"></span><span>${name} (${rows.length})</span><span class="chevron">⌄</span></div><div class="repo-band"><span class="repo-dot"></span><span>${esc(repo)}</span><span class="count">${rows.length}</span></div>${rows.map(rowMarkup).join('')}</section>`
}
function boardRows(){
 if(vm.scenario==='zero-results') return [];
 if(vm.repo==='corral') return [boardFixtures.working[0]];
 if(vm.host==='Bazzite') return [boardFixtures.blocked[1],boardFixtures.working[1]];
 if(vm.scenario==='offline-stale-host') return [boardFixtures.blocked[1]];
 return [...boardFixtures.blocked,...boardFixtures.working]
}
function boardMarkup(){
 const rows=boardRows();
 if(!rows.length) return `<div class="empty"><div class="empty-glyph" aria-hidden="true">⌁</div><h2>No lanes match</h2><p>${esc(summary())}<br>Both scopes remain active and reversible.</p><button class="text-action" data-hit-target data-reset>Reset all filters</button></div>`;
 const blocked=rows.filter(r=>r.state==='blocked'),working=rows.filter(r=>r.state==='working');
 return `${blocked.length?sectionMarkup('blocked',blocked):''}${working.length?sectionMarkup('working',working):''}`
}
function sheetMarkup(){return `<div class="scrim" data-dismiss aria-hidden="true"></div><section class="sheet" role="dialog" aria-modal="true" aria-label="Filters"><div class="drag-handle" aria-hidden="true"></div><header class="sheet-head"><div class="sheet-title"><h2>Filters</h2><p>${esc(summary())}</p></div><button class="close" data-hit-target data-dismiss aria-label="Close filters">×</button></header><div class="panel-scroll" data-scroll-surface>${filterScope('host')}${filterScope('repo')}</div><footer class="sheet-footer"><button class="text-action" data-hit-target data-reset>Reset all filters</button></footer></section>`}
function inlineRail(){
 const menu=vm.inlineMenu==='host'?`<div class="inline-menu" role="group" aria-label="Host choices">${hostChoices()}</div>`:vm.inlineMenu==='repo'?`<div class="inline-menu" role="group" aria-label="Repository choices">${repoChoices()}</div>`:'';
 return `<section class="inline-rail" aria-label="Inline filter rail"><div class="rail-row"><button class="rail-select" data-hit-target data-toggle-menu="host" aria-label="Choose host, ${esc(vm.host||'All hosts')}"><span class="rail-scope">Host</span><span class="rail-value">${esc(vm.host||'All hosts')}</span><span class="rail-caret">⌄</span></button><button class="rail-clear" data-hit-target data-clear="host" aria-label="Clear host filter" ${vm.host===null?'disabled':''}>×</button></div><div class="rail-row"><button class="rail-select" data-hit-target data-toggle-menu="repo" aria-label="Choose repository, ${esc(vm.repo||'All repositories')}"><span class="rail-scope">Repo</span><span class="rail-value">${esc(vm.repo||'All repositories')}</span><span class="rail-caret">⌄</span></button><button class="rail-clear" data-hit-target data-clear="repo" aria-label="Clear repository filter" ${vm.repo===null?'disabled':''}>×</button></div>${menu}<button class="rail-reset" data-hit-target data-reset>Reset all filters</button></section>`
}
function editorMarkup(){return `<section class="scope-editor" role="region" aria-label="Scope path filter editor"><header class="editor-head"><div><h2>Scope path</h2><p>Host first; repository choices rescope immediately.</p></div><button class="close" data-hit-target data-dismiss aria-label="Close filters">×</button></header><div class="panel-scroll scope-path" data-scroll-surface>${filterScope('host',true)}${filterScope('repo',true)}</div><footer class="editor-footer"><div class="current"><b>Viewing</b><span>${esc(summary())}</span></div><button class="text-action" data-hit-target data-reset>Reset all filters</button></footer></section>`}
function renderPhone(){
 const panel=vm.variant==='A'&&vm.panelOpen?sheetMarkup():'';
 const main=vm.variant==='C'&&vm.panelOpen?editorMarkup():`<main class="board-shell">${healthBanner()}<div class="board" aria-label="Agent board">${boardMarkup()}</div></main>`;
 phone.innerHTML=`${statusbar()}${navbar()}${vm.variant==='B'?inlineRail():''}${main}${panel}<div class="home-indicator" aria-hidden="true"></div>`;
 bindPhone();
 document.documentElement.dataset.palette=vm.palette;
 document.body.classList.toggle('capture',vm.capture);
 document.body.classList.toggle('a11y',vm.a11y);
 document.body.dataset.variant=vm.variant;
 document.body.dataset.state=vm.scenario;
 renderControls();
 requestAnimationFrame(runSelfTest);
 if(PARAMS.get('scrollEnd')==='1'){requestAnimationFrame(()=>{const surface=document.querySelector('[data-scroll-surface]');if(surface)surface.scrollTop=surface.scrollHeight})}
 if(PARAMS.get('selftest')==='1'&&!interactionRunning){interactionRunning=true;setTimeout(runInteractionTest,30)}
}
function bindPhone(){
 document.querySelectorAll('[data-kind]').forEach(b=>b.onclick=()=>{const v=b.dataset.value||null;if(b.dataset.kind==='host')vm.host=v;else vm.repo=v;renderPhone()});
 document.querySelectorAll('[data-clear]').forEach(b=>b.onclick=()=>{if(b.dataset.clear==='host')vm.host=null;else vm.repo=null;renderPhone()});
 document.querySelectorAll('[data-reset]').forEach(b=>b.onclick=()=>{vm.host=null;vm.repo=null;renderPhone()});
 document.querySelectorAll('[data-dismiss]').forEach(b=>b.onclick=()=>{vm.panelOpen=false;renderPhone()});
 document.querySelectorAll('[data-toggle-menu]').forEach(b=>b.onclick=()=>{vm.inlineMenu=vm.inlineMenu===b.dataset.toggleMenu?null:b.dataset.toggleMenu;renderPhone()});
 const trigger=document.querySelector('#filter-trigger'); if(trigger)trigger.onclick=()=>{if(vm.variant==='A'||vm.variant==='C'){vm.panelOpen=!vm.panelOpen;renderPhone()}else{vm.inlineMenu=vm.inlineMenu?'': 'host';renderPhone()}};
}
function renderControls(){
 if(vm.capture)return;
 const sets=[['variant-controls',VARIANTS,'variant'],['palette-controls',PALETTES,'palette'],['state-controls',STATES,'scenario']];
 for(const [id,values,key] of sets){const el=document.getElementById(id);if(!el)continue;el.innerHTML=values.map(v=>`<button class="demo-chip" data-demo-key="${key}" data-demo-value="${v}" aria-pressed="${vm[key]===v}">${key==='scenario'?STATE_LABELS[v]:v[0].toUpperCase()+v.slice(1)}</button>`).join('')}
 document.querySelectorAll('[data-demo-key]').forEach(b=>b.onclick=()=>{const k=b.dataset.demoKey,v=b.dataset.demoValue;vm[k]=k==='variant'?v.toUpperCase():v;if(k==='scenario')[vm.host,vm.repo]=initial[v];vm.panelOpen=false;vm.inlineMenu=null;renderPhone()});
}
function runSelfTest(){
 const root=document.documentElement;
 const targets=[...document.querySelectorAll('[data-hit-target]:not([disabled])')];
 const bad=targets.filter(el=>{const r=el.getBoundingClientRect();return r.width<43.5||r.height<43.5});
 const widths=[document.documentElement.scrollWidth,document.body.scrollWidth,phone.scrollWidth];
 const overflow=Math.max(...widths)>window.innerWidth+1;
 const scopes=[...document.querySelectorAll('[data-scope]')].map(e=>e.dataset.scope);
 const scopeOrder=scopes.length===0||scopes.indexOf('host')<=scopes.indexOf('repo');
 const labels=[...document.querySelectorAll('[aria-label]')].map(e=>e.getAttribute('aria-label'));
 const distinct=labels.includes('All hosts, 17 lanes')||labels.some(x=>x&&x.startsWith('Choose host,'));
 const distinctRepo=labels.includes('All repositories, 17 lanes')||labels.some(x=>x&&x.startsWith('Choose repository,'));
 const triggerLabel=document.querySelector('#filter-trigger')?.getAttribute('aria-label')||'';
 const surfaceClosed=scopes.length===0;
 const voiceoverPass=(distinct&&distinctRepo)||(surfaceClosed&&triggerLabel.includes(summary()));
 const scrollSurface=document.querySelector('[data-scroll-surface]');
 let scrollReach=true;
 if(scrollSurface){const last=[...scrollSurface.querySelectorAll('.choice')].at(-1);const old=scrollSurface.scrollTop;scrollSurface.scrollTop=scrollSurface.scrollHeight;if(last){scrollReach=last.getBoundingClientRect().bottom<=scrollSurface.getBoundingClientRect().bottom+1}scrollSurface.scrollTop=old}
 root.dataset.consoleErrors=String(vm.consoleErrors.length);
 root.dataset.hitTargets=bad.length===0?'pass':`fail:${bad.length}`;
 root.dataset.horizontalOverflow=overflow?'fail':'pass';
 root.dataset.scopeOrder=scopeOrder?'pass':'fail';
 root.dataset.voiceoverScopes=voiceoverPass?'pass':'fail';
 root.dataset.resetVisible=(document.querySelector('[data-reset]')||surfaceClosed)?'pass':'fail';
 root.dataset.scrollReach=scrollReach?'pass':'fail';
 root.dataset.summary=summary();
 root.dataset.selftest='ready';
 window.__CORRAL_GATE__={badTargets:bad.map(e=>e.getAttribute('aria-label')||e.textContent.trim()),overflow,scopeOrder,voiceover:voiceoverPass,scrollReach,consoleErrors:[...vm.consoleErrors],variant:vm.variant,palette:vm.palette,state:vm.scenario};
}
function runInteractionTest(){
 const result={variant:vm.variant,steps:[]};
 const press=(selector,label)=>{const el=document.querySelector(selector);if(!el)throw new Error(`missing ${label}`);el.click();result.steps.push(label)};
 try{
   if((vm.variant==='A'||vm.variant==='C')&&!vm.panelOpen)press('#filter-trigger','open surface');
   if(vm.variant==='B'&&vm.inlineMenu!=='host')press('[data-toggle-menu="host"]','open host menu');
   press('[data-kind="host"][data-value="Bazzite"]','select Bazzite');
   if(vm.variant==='B')press('[data-toggle-menu="repo"]','open repository menu');
   press('[data-kind="repo"][data-value="corral"]','select corral');
   const selected=summary()==='Bazzite · corral'&&activeCount()===2;
   press('[data-clear="host"]','clear host only');
   const independent=vm.host===null&&vm.repo==='corral'&&activeCount()===1;
   press('[data-reset]','reset all filters');
   const reset=vm.host===null&&vm.repo===null&&activeCount()===0;
   result.selected=selected;result.independent=independent;result.reset=reset;result.pass=selected&&independent&&reset;
 }catch(error){result.pass=false;result.error=String(error)}
 document.documentElement.dataset.interactionGate=result.pass?'pass':'fail';
 window.__CORRAL_INTERACTION__=result;
 runSelfTest();
}
renderPhone();
</script>
</body></html>
'''

COMPARISON = r'''<!doctype html><html lang="en" data-palette="mocha"><head><meta charset="utf-8"><title>Corral #427 A/B/C comparison</title><style>
*{box-sizing:border-box}html,body{margin:0;background:#11111b;color:#cdd6f4;font-family:-apple-system,BlinkMacSystemFont,"SF Pro Text",sans-serif}body{width:1440px;height:1080px;overflow:hidden;padding:36px 42px}.eyebrow{font:700 12px/1.2 ui-monospace,monospace;letter-spacing:.1em;text-transform:uppercase;color:#cba6f7}h1{font-size:32px;letter-spacing:-.03em;margin:8px 0 6px}.sub{font-size:15px;color:#bac2de;margin:0 0 24px}.grid{display:grid;grid-template-columns:repeat(3,1fr);gap:24px}.card{min-width:0;background:#181825;border:1px solid #45475a;border-radius:18px;padding:16px}.card.recommended{border-color:#cba6f7;box-shadow:inset 0 0 0 1px #cba6f7}.top{display:flex;align-items:flex-start;gap:10px;margin-bottom:12px}.letter{width:34px;height:34px;border-radius:9px;display:grid;place-items:center;background:#313244;color:#cba6f7;font:800 15px/1 ui-monospace,monospace}.top h2{font-size:17px;margin:0}.top p{font-size:12px;color:#bac2de;margin:3px 0 0}.badge{margin-left:auto;border:1px solid #cba6f7;border-radius:999px;padding:5px 8px;color:#cba6f7;font-size:10px;font-weight:750;text-transform:uppercase}.viewport{height:636px;overflow:hidden;border-radius:15px;background:#1e1e2e;position:relative}.viewport iframe{width:390px;height:844px;border:0;transform:scale(.754);transform-origin:top left}.why{margin:12px 2px 0;font-size:12px;line-height:1.45;color:#bac2de}.footer{display:flex;justify-content:space-between;align-items:center;margin-top:22px;padding-top:16px;border-top:1px solid #313244;color:#a6adc8;font-size:12px}.footer b{color:#cdd6f4}
</style></head><body><div class="eyebrow">Corral · issue 427 · design gate</div><h1>Reclaim the header; keep both scopes explicit</h1><p class="sub">Monitor surface · Mocha representative frame · 390×844 · full matrix ships separately</p><div class="grid">
<div class="card recommended"><div class="top"><span class="letter">A</span><div><h2>Native filter sheet</h2><p>Smallest board footprint</p></div><span class="badge">Recommend</span></div><div class="viewport"><iframe title="Direction A" src="index.html?capture=1&variant=A&palette=mocha&state=both-filters&forceOpen=1"></iframe></div><p class="why">Best label reach, health legibility, and Dynamic Type behavior while leaving the board visible behind material.</p></div>
<div class="card"><div class="top"><span class="letter">B</span><div><h2>Inline filter rail</h2><p>Fastest repeat switching</p></div></div><div class="viewport"><iframe title="Direction B" src="index.html?capture=1&variant=B&palette=mocha&state=both-filters"></iframe></div><p class="why">Immediate and spatially stable, but permanently taxes vertical board density and expands further for long lists.</p></div>
<div class="card"><div class="top"><span class="letter">C</span><div><h2>Scope Path</h2><p>Corral-specific editor</p></div></div><div class="viewport"><iframe title="Direction C" src="index.html?capture=1&variant=C&palette=mocha&state=both-filters&forceOpen=1"></iframe></div><p class="why">Makes host→repository recomputation legible, but interrupts monitoring and over-weights an occasional task.</p></div>
</div><div class="footer"><span><b>Recommendation:</b> A — native sheet</span><span>All controls ≥44 pt · textual health · independent scoped clear + reset</span></div></body></html>'''

README_SEED = """# Corral #427 — filter/header redesign prototype\n\nGenerated by `scripts/build.py`. Read the complete design decision, coverage map, and proof results in this bundle after running the capture and verification scripts.\n"""


def write():
    ROOT.mkdir(parents=True, exist_ok=True)
    for variant in ["A", "B", "C"]:
        text = HTML.replace("__DEFAULT_VARIANT__", variant).replace("__PALETTE__", "mocha")
        name = "index.html" if variant == "A" else f"variant-{variant.lower()}.html"
        (ROOT / name).write_text(text, encoding="utf-8")
    (ROOT / "variant-a.html").write_text(HTML.replace("__DEFAULT_VARIANT__", "A").replace("__PALETTE__", "mocha"), encoding="utf-8")
    (ROOT / "comparison.html").write_text(COMPARISON, encoding="utf-8")
    if not (ROOT / "README.md").exists():
        (ROOT / "README.md").write_text(README_SEED, encoding="utf-8")
    print(f"built {ROOT}")


if __name__ == "__main__":
    write()
