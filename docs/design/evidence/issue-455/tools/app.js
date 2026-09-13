'use strict';
const $=s=>document.querySelector(s),$$=s=>[...document.querySelectorAll(s)];
const q=new URLSearchParams(location.search), variant=document.body.dataset.variant;
const KEY='corral455.prototype.savedMode';
const read=k=>{try{return localStorage.getItem(k)}catch{return null}};
const store=(k,v)=>{try{localStorage.setItem(k,v)}catch{$('#lighting-note').textContent='Storage unavailable'}};
const validMode=v=>['Board','Herd'].includes(v)?v:'Board';
let saved=validMode(read(KEY)), mode=saved;
// Gallery preview is not a persisted preference; a cold reload tests real restoration.
if(q.get('preview')==='herd'&&performance.getEntriesByType('navigation')[0]?.type!=='reload')mode='Herd';
let environment=q.get('env')||'day', host='',repo='',paddock=0,fixture=q.get('state')||'normal',theme=read('corral455.prototype.theme')||'mocha';
let sheetKind='',recovery=false,appReduce=q.has('reduce'),lowPower=false,thermal=false,simBackground=false,pageActive=true;
let frozen=q.has('still'),elapsed=q.has('still')?4:0,ticks=0,raf=0,last=0,painted=0;
let dataset=FIXTURES.map(x=>({...x}));
const states=['blocked','working','idle','done','unknown'],marks={blocked:'!',working:'○',idle:'◦',done:'✓',unknown:'?'};
const esc=s=>String(s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const media=matchMedia('(prefers-reduced-motion: reduce)');
const reduced=()=>appReduce||media.matches;
const isOutage=()=>['offline','key-mismatch','connecting'].includes(fixture);
const status=a=>isOutage()?'unknown':a.state;
const rows=()=>dataset.filter(a=>(!host||a.host===host)&&(!repo||a.repo===repo));
const repositories=()=>[...new Set(rows().map(a=>a.repo))].sort();
const totals=()=>Object.fromEntries(states.map(s=>[s,rows().filter(a=>status(a)===s).length]));
function setDataset(){dataset=FIXTURES.map(a=>({...a}));if(fixture==='dense'){for(let i=0;i<18;i++){const a=FIXTURES[4+i%8];dataset.push({...a,id:`${a.host}::extra-${i}`,name:`${a.name}-${i+1}`,art:a.name,repo:'atlas-vector',state:i%5===0?'blocked':i%3===0?'idle':'working'})}}if(fixture==='empty')dataset=[];}
function healthText(){if(fixture==='offline')return'Offline · last known 6m ago';if(fixture==='key-mismatch')return'Key mismatch · connection paused';if(fixture==='connecting')return'Connecting · retained snapshot';return `${host?host+' · ':''}${host?'1 host':'2 hosts'} connected · synthetic`;}
function scopeText(){return `${host||'All hosts'} · ${repo||'All repositories'}`}
function countMarkup(){const c=totals();return states.map(s=>`<span>${marks[s]} <b>${c[s]}</b> ${s}</span>`).join('');}
function horseButton(a,rail=false){const sprite=SPRITES[a.art||a.name],st=status(a);return `<button class="horse-button" data-horse="${esc(a.id)}" ${isOutage()?'disabled':''} aria-label="${esc(a.name+', '+st+', '+a.repo+' on '+a.host+(isOutage()?'. Source unavailable':'. Opens synthetic Recent Output'))}"><span class="horse-art"><span class="normal">${sprite.normal}</span><span class="static">${sprite.static}</span></span><span class="horse-caption glass"><span class="horse-name">${esc(a.name)}</span><span class="horse-state">${marks[st]} ${st}${isOutage()?' · was '+a.state:''}</span><span class="horse-host">${esc(a.host)}${rail?' · '+esc(a.repo):''}</span></span></button>`}
function render(){
 document.body.dataset.mode=mode;document.body.dataset.theme=theme;document.body.dataset.large=q.has('large')?'true':'false';
 const night=environment==='night'||environment==='auto'&&q.get('auto')==='night';document.body.dataset.environment=night?'night':'day';
 document.body.classList.toggle('offline',isOutage());document.body.classList.toggle('motion-still',reduced());
 $('#filter-count').textContent=(host?1:0)+(repo?1:0)?'· '+((host?1:0)+(repo?1:0)):'';
 $('#scope-text').textContent=scopeText();$('#scope').setAttribute('aria-label','Filters, '+scopeText());
 const c=totals();$('#compact-status').innerHTML=`<b>${c.blocked} blocked</b>${c.working} working<br>${rows().length} total · ⌄`;
 $('#compact-status').setAttribute('aria-label','Status counts. '+states.map(s=>`${c[s]} ${s}`).join(', ')+'. '+healthText());
 $('#health').textContent=healthText();$('#counts').innerHTML=countMarkup();
 $$('[data-light]').forEach(b=>b.setAttribute('aria-pressed',b.dataset.light===environment));
 $('#lighting-note').hidden=environment!=='auto'&&!isOutage()&&!reduced();
 $('#lighting-note').textContent=environment==='auto'?'Auto · demo '+(night?'night':'day'):isOutage()?'Scene paused':reduced()?'Motion off':`${variant.toUpperCase()} · ${night?'Night':'Day'} proof`;
 $('#herd-content').hidden=mode!=='Herd';$('#board').hidden=mode!=='Board';
 const arr=rows(),rail=arr.filter(a=>a.state==='blocked');
 $('#rail-title').textContent=`! FRONT RAIL · ${rail.length} ${isOutage()?'LAST KNOWN':'BLOCKED'}${fixture==='dense'||q.has('large')?' · scroll →':''}`;
 $('#rail-horses').innerHTML=rail.map(a=>horseButton(a,true)).join('')||'<div class="section-note glass" style="padding:12px">'+(isOutage()?'Blocked status cannot be confirmed':'No blocked agents in this scope')+'</div>';
 const ps=repositories();paddock=Math.min(paddock,Math.max(0,ps.length-1));const current=ps[paddock];
 $('#repo-title').textContent=current||'No paddocks';$('#repo-count').innerHTML=current?`${arr.filter(a=>a.repo===current&&a.state!=='blocked').length} here<br>${arr.filter(a=>a.repo===current&&a.state==='blocked').length} at rail`:'0 agents';
 $('#field').innerHTML=current?arr.filter(a=>a.repo===current&&a.state!=='blocked').map(a=>horseButton(a)).join('')||'<div class="empty-message glass">All agents are at the front rail.</div>':'<div class="empty-message glass"><h2>No agents in this scope</h2><p>No activity is being inferred. Filters and Board remain available.</p><button data-action="reset">Reset all filters</button></div>';
 $('#page-label').textContent=current?'Repository paddocks':'No repository paddocks';$('#page-count').textContent=ps.length?`${paddock+1} / ${ps.length} paddocks`:'0 / 0 paddocks';$('#previous').disabled=paddock===0;$('#next').disabled=paddock>=ps.length-1;
 $('#outage').hidden=!isOutage();$('#outage').innerHTML=`<b>${esc(healthText())}</b><p>Unknown · blocked status cannot be confirmed. No live motion or output.</p><div class="actions"><button data-action="recovery">Open Board</button><button data-action="retry">Retry</button></div><p id="retry-note"></p>`;
 $('#recovery-note').textContent=recovery?`Temporary recovery. Saved mode: ${saved}. Foregrounding keeps Board; a cold reload restores the preference.`:`Saved mode: ${saved}. Choose Herd in Settings to apply and save.`;
 $('#board-rows').innerHTML=states.map(s=>{const list=arr.filter(a=>status(a)===s);return `<h2 class="board-section">${marks[s]} ${s} · ${list.length}</h2>`+list.map(a=>`<button class="board-row" data-horse="${esc(a.id)}" ${isOutage()?'disabled':''}><b>${esc(a.name)}</b><small>${esc(a.repo)} · ${esc(a.host)}${isOutage()?' · last known '+a.state:''}</small></button>`).join('')}).join('');
 layout();syncMotion();
}
function layout(){const h=$('#hud').getBoundingClientRect();const gap=16;document.body.style.setProperty('--content-top',`${mode==='Board'?h.bottom+12:Math.max(h.bottom+gap,innerHeight*.329)}px`)}
function setMode(value){saved=validMode(value);store(KEY,saved);mode=saved;recovery=false;render();if(sheetKind==='settings')settingsContents();}
function openBoard(){mode='Board';recovery=true;render();}
function openSheet(kind,title,subtitle=''){sheetKind=kind;const d=$('#sheet');d.className=kind==='settings'?'settings-sheet':kind==='output'?'output-sheet':'';$('#sheet-title').textContent=title;$('#sheet-subtitle').textContent=subtitle;$('#sheet-content').innerHTML='';$('#sheet-footer').innerHTML='';if(!d.open)d.showModal();syncMotion();}
function option(label,sub,selected,action,value){return `<button class="option-row" data-action="${action}" data-value="${esc(value)}" aria-pressed="${selected}"><span>${esc(label)}<small>${esc(sub)}</small></span></button>`}
function filters(){openSheet('filters','Filters',scopeText()+' · selections apply immediately');filterContents();}
function filterContents(){
 $('#sheet-subtitle').textContent=scopeText()+' · selections apply immediately';
 let html='<div class="section-heading"><h2>Host scope</h2><button data-action="host" data-value="">Clear host</button></div>';
 html+=option('All hosts',`${dataset.length} agents`,!host,'host','');
 for(const h of ['Meadow','Orchard'])html+=option(h,`${dataset.filter(a=>a.host===h).length} agents · ${isOutage()?healthText():'connected · synthetic'}`,host===h,'host',h);
 html+='<div class="section-heading"><h2>Repository scope</h2><button data-action="repo" data-value="">Clear repository</button></div>';
 const hostRows=dataset.filter(a=>!host||a.host===host);
 html+=option('All repositories',`${hostRows.length} agents`,!repo,'repo','');
 for(const r of [...new Set(FIXTURES.map(a=>a.repo))].sort())html+=option(r,`${hostRows.filter(a=>a.repo===r).length} agents`,repo===r,'repo',r);
 html+='<p class="section-note">Host and repository are independent filters shared with Board. Fictional fixture counts only.</p>';
 $('#sheet-content').innerHTML=html;$('#sheet-footer').innerHTML='<button data-action="reset">Reset all filters</button>';
}
function settings(){openSheet('settings','Settings','Prototype settings · no live connection');settingsContents();}
function settingsContents(){
 $('#sheet-content').innerHTML='<h2>Default view</h2><div class="picker-row">'+['Board','Herd'].map(m=>`<button data-action="mode" data-value="${m}" aria-pressed="${saved===m}">${m}</button>`).join('')+`</div><p class="section-note" id="saved-note">Saved: ${saved} · Showing: ${mode}. Applies immediately and restores on cold reload.</p><h2>Appearance</h2><div class="picker-row">`+['latte','frappe','macchiato','mocha'].map(t=>`<button data-action="theme" data-value="${t}" aria-pressed="${theme===t}">${t==='frappe'?'Frappé':t[0].toUpperCase()+t.slice(1)}</button>`).join('')+'</div><p class="section-note">Board and general Settings use Catppuccin. Ranch lighting is independent.</p><h2>Ranch environment</h2><div class="picker-row">'+['day','night','auto'].map(e=>`<button data-action="environment" data-value="${e}" aria-pressed="${environment===e}">${e[0].toUpperCase()+e.slice(1)}</button>`).join('')+'</div><p class="section-note">Auto is a deterministic demo '+(q.get('auto')==='night'?'night':'day')+'—not location or solar calculation.</p>'+option('Reduce Motion',media.matches?'System setting is on':'Intentional still scene',reduced(),'reduce','')+'<p class="section-note">Ambient motion also pauses behind sheets, on Board, in background, during source loss, and in simulated Low Power / thermal fallback.</p><h2>Proposed · awaiting approval</h2><p class="section-note">No saved mode → Board. Unknown value → Board. Open Board is temporary, does not save, and is not reversed by foregrounding. Cold reload restores preference. Explicit Settings selection wins immediately.</p>';
}
function statusSheet(){openSheet('status','Status counts',scopeText());$('#sheet-content').innerHTML='<p class="section-note">'+healthText()+'</p>'+states.map(s=>`<div class="option-row"><span>${marks[s]} ${s}</span><b>${totals()[s]}</b></div>`).join('')+'<p class="section-note">Counts use the same fictional dataset as the front rail, paddocks and Board.</p>'}
function proofControls(){openSheet('proof','Prototype controls',`Variant ${variant.toUpperCase()} · synthetic, browser-only`);$('#sheet-content').innerHTML='<div class="proof-controls"><a href="index.html">← Comparison gallery</a><p class="section-note">These controls are review tools, not proposed app UI.</p>'+['normal','dense','offline','connecting','key-mismatch','empty'].map(s=>`<button data-action="fixture" data-value="${s}" aria-pressed="${fixture===s}">${s[0].toUpperCase()+s.slice(1)} fixture</button>`).join('')+`<button data-action="large" aria-pressed="${q.has('large')}">Large text · 145% equivalent</button><button data-action="low" aria-pressed="${lowPower}">Simulate Low Power</button><button data-action="thermal" aria-pressed="${thermal}">Simulate serious thermal state</button><button data-action="background" aria-pressed="${simBackground}">Simulate background / foreground</button><button data-action="opaque" aria-pressed="${document.body.classList.contains('no-transparency')}">Opaque material fallback</button><button data-action="reload">Cold reload · restore saved preference</button><button data-action="no-saved">Test no saved mode → Board</button><button data-action="invalid">Test unknown value → Board</button></div>`;}
function output(id){const a=dataset.find(x=>x.id===id);if(!a||isOutage())return;openSheet('output','Recent Output',`${a.name} · ${a.repo} · ${a.host}`);$('#sheet-content').innerHTML='<span class="proof-label">SYNTHETIC PLACEHOLDER · NOT LIVE OUTPUT</span><div class="output-placeholder">No host was contacted.\n\nThis interaction proves the shared horse / Board-row destination only. A native implementation must use the existing composite-identity Recent Output route.</div><p class="section-note">No terminal commands, live logs, credentials or model responses are represented here.</p>';}
$('#scope').onclick=filters;$('#settings').onclick=settings;$('#compact-status').onclick=statusSheet;$('#open-board').onclick=openBoard;$('#proof-controls').onclick=proofControls;
$('#previous').onclick=()=>{paddock--;render()};$('#next').onclick=()=>{paddock++;render()};
$('#close-sheet').onclick=()=>$('#sheet').close();$('#sheet').addEventListener('close',()=>{sheetKind='';syncMotion()});
$$('[data-light]').forEach(b=>b.onclick=()=>{environment=b.dataset.light;render()});
document.addEventListener('click',e=>{const horse=e.target.closest('[data-horse]');if(horse){output(horse.dataset.horse);return}const b=e.target.closest('[data-action]');if(!b)return;const value=b.dataset.value,action=b.dataset.action;
 if(action==='mode')setMode(value);
 if(action==='host'){host=value;paddock=0;render();filterContents()}
 if(action==='repo'){repo=value;paddock=0;render();filterContents()}
 if(action==='reset'){host='';repo='';paddock=0;render();if(sheetKind==='filters')filterContents()}
 if(action==='theme'){theme=value;store('corral455.prototype.theme',theme);render();settingsContents()}
 if(action==='environment'){environment=value;render();settingsContents()}
 if(action==='reduce'){appReduce=!appReduce;render();settingsContents()}
 if(action==='recovery')openBoard();
 if(action==='retry')$('#retry-note').textContent='Synthetic retry only. Source remains unavailable; nothing was fetched.';
 if(action==='fixture'){fixture=value;paddock=0;setDataset();render();proofControls()}
 if(action==='large'){q.has('large')?q.delete('large'):q.set('large','1');render();proofControls()}
 if(action==='low'){lowPower=!lowPower;syncMotion();proofControls()}
 if(action==='thermal'){thermal=!thermal;syncMotion();proofControls()}
 if(action==='background'){simBackground=!simBackground;syncMotion();proofControls()}
 if(action==='opaque'){document.body.classList.toggle('no-transparency');proofControls()}
 if(action==='reload')location.reload();
 if(action==='no-saved'){localStorage.removeItem(KEY);location.reload()}
 if(action==='invalid'){store(KEY,'not-a-mode');location.reload()}
});
let startX=0;$('#field').addEventListener('touchstart',e=>startX=e.changedTouches[0].clientX,{passive:true});$('#field').addEventListener('touchend',e=>{const delta=e.changedTouches[0].clientX-startX;if(Math.abs(delta)>70){paddock=Math.max(0,Math.min(repositories().length-1,paddock+(delta<0?1:-1)));render()}},{passive:true});
const motionNodes={};for(const env of ['day','night'])motionNodes[env]={canopies:$$(`.${env} .canopy`),grass:$$(`.${env} .grass-wind`),clouds:$$(`.${env} .cloud-drift`),sky:$$(`.${env} .sky-drift`)};
function paint(t){const nodes=motionNodes[document.body.dataset.environment],wind=Math.sin(t*Math.PI/4.8);nodes.canopies.forEach((e,i)=>e.style.transform=`rotate(${(wind*3.8+Math.sin(t*Math.PI/4.8+i*.6)*.6).toFixed(4)}deg)`);nodes.grass.forEach((e,i)=>e.style.transform=`skewX(${(wind*4+Math.sin(t*Math.PI/4.8+i*.12)*.9).toFixed(4)}deg)`);nodes.clouds.forEach((e,i)=>e.style.transform=`translateX(${(Math.sin(t/65)*50*(1-i*.08)).toFixed(4)}px)`);nodes.sky.forEach(e=>e.style.transform=`translate(${(Math.sin(t/140)*70).toFixed(4)}px,${(Math.sin(t/180)*16).toFixed(4)}px)`);}
function allowed(){return mode==='Herd'&&!$('#sheet').open&&!reduced()&&!isOutage()&&dataset.length>0&&!document.hidden&&pageActive&&!simBackground&&!lowPower&&!thermal&&!frozen;}
function tick(now){raf=0;if(!allowed())return;if(!last)last=now;const dt=Math.min((now-last)/1000,.12);last=now;elapsed+=dt;if(now-painted>=33){paint(elapsed);ticks++;painted=now}raf=requestAnimationFrame(tick);}
function syncMotion(){if(raf)cancelAnimationFrame(raf);raf=0;last=0;document.body.classList.toggle('paused',!allowed());if(reduced()){elapsed=0;paint(0)}else paint(elapsed);if(allowed())raf=requestAnimationFrame(tick);}
media.addEventListener('change',render);document.addEventListener('visibilitychange',syncMotion);addEventListener('pagehide',()=>{pageActive=false;syncMotion()});addEventListener('pageshow',()=>{pageActive=true;syncMotion()});addEventListener('resize',layout);
window.PROOF={state:()=>({variant,mode,saved:read(KEY),recovery,environment,theme,host,repo,fixture,counts:totals(),rows:rows().length,paddock,elapsed,ticks,running:!!raf,reduced:reduced(),documentHidden:document.hidden,lowPower,thermal}),freeze:(phase=4)=>{frozen=true;elapsed=phase;syncMotion()},resume:()=>{frozen=false;syncMotion()},phase:paint};
setDataset();render();
