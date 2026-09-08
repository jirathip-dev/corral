"""Original sculpted-vector ranch illustration; deterministic, no external art."""
import random, math
import fixtures as F
MANES=['#2e2019','#5d3317','#17161a','#7d8089','#e8dcc0','#41321f','#4a342c','#241a10']
def mix(a,b,t):
    return '#'+''.join(f'{round(int(a[k:k+2],16)*(1-t)+int(b[k:k+2],16)*t):02x}' for k in (1,3,5))
def path(d,fill,**attrs):
    return '<path d="'+d+'" fill="'+fill+'" '+' '.join(f'{k.replace("_","-")}="{v}"' for k,v in attrs.items())+'/>'
def horse(i,state='idle',pose='stand',uid='h',facing=1):
    c=F.COAT_HEX[i['coat']]['body']; dark=mix(c,'#161e22',.53); light=mix(c,'#fff0c7',.48); middle=mix(c,'#efd6b1',.16);mane=MANES[F.COATS.index(i['coat'])]
    g=f'url(#{uid}-coat)';muscle=f'url(#{uid}-muscle)'; hoof=f'url(#{uid}-hoof)'
    s=f'<svg class="horse-svg" viewBox="0 0 148 112" xmlns="http://www.w3.org/2000/svg" data-pose="{pose}" data-identity="{i}"><defs><linearGradient id="{uid}-coat" x1=".1" y1="0" x2=".7" y2="1"><stop stop-color="{light}"/><stop offset=".28" stop-color="{middle}"/><stop offset=".55" stop-color="{c}"/><stop offset="1" stop-color="{dark}"/></linearGradient><radialGradient id="{uid}-muscle" cx=".28" cy=".2" r=".85"><stop stop-color="{light}"/><stop offset=".37" stop-color="{c}"/><stop offset="1" stop-color="{dark}"/></radialGradient><linearGradient id="{uid}-hoof"><stop stop-color="#776b5d"/><stop offset=".48" stop-color="#443c34"/><stop offset="1" stop-color="#27282b"/></linearGradient></defs>'
    contacts=[(43,104),(96,105),(89,106)]+([] if pose=='shift' else [(34,105)])
    for x,y in contacts:
        s+=f'<ellipse cx="{x}" cy="{y+.4}" rx="6.5" ry="1.15" fill="#152224" opacity=".5"/>'
    s+='<path class="cast-shadow" d="M26 104 Q81 99 141 108 Q113 113 42 108Z" fill="#162323" opacity=".18"/><ellipse cx="72" cy="105" rx="48" ry="2.4" fill="#152125" opacity=".29"/>'
    s+=f'<g transform="translate({148 if facing==-1 else 0} 0) scale({facing} 1)">'
    # Four separate load paths: hoof/fetlock/cannon/knee or hock, to mass.
    s+=path('M42 57 Q51 65 45 74 L39 82 L42 99 L47 101 L46 104 L39 104 L35 82 L38 72 L34 61Z',dark)
    s+=path('M93 54 Q102 58 100 69 L98 76 L99 80 L96 84 L96 96 Q99 98 98 100 L101 102 L100 105 L92 105 L92 101 L93 97 L94 84 L92 80 L94 76 L91 68Z',dark)
    s+=path('M93 100L99 101L101 105H93Z',hoof)
    s+=path('M39 100L45 100L47 104H39Z',hoof)
    # Tail dock anchored high on croup, flowing mass with strands.
    s+=path('M32 42 C24 41 23 50 23 61 C23 77 17 85 12 90 Q24 88 27 76 Q33 56 34 47Z',mane)
    s+=path('M29 47Q25 67 23 76L18 86','none',stroke=mix(mane,light,.35),stroke_width='.7')
    s+=path('M33 48Q30 72 25 82M25 68Q22 81 15 86','none',stroke=mix(mane,'#101b21',.4),stroke_width='1')
    belly={'draft':65,'stock':63,'light':61}[i['breed']]
    # Withers peak, back saddle, croup and chest form one continuous contour.
    s+=path(f'M29 43 Q35 37 46 39 C57 42 66 45 80 40 L86 35 Q93 34 96 41 C103 44 106 53 101 61 Q96 68 84 {belly+1} Q62 {belly+4} 45 {belly} Q31 64 28 58 Q23 49 29 43Z',g)
    s+=path('M29 45 C32 37 44 39 48 47 Q51 56 42 63 Q28 63 27 53Z',muscle)
    s+=path('M83 40 Q98 36 101 49 Q106 59 96 65 Q86 64 82 56 Q88 49 83 40Z',muscle)
    s+=path(f'M49 49 Q69 44 83 48 Q81 61 67 {belly} Q53 {belly} 45 58Z',muscle,opacity='.52')
    s+=path('M33 42 Q42 39 48 43 M50 44Q66 48 80 42 L86 38','none',stroke=light,stroke_width='1.1',opacity='.74')
    s+=path('M47 58 Q60 65 77 62 M84 46 Q94 48 91 58','none',stroke=dark,stroke_width='.7',opacity='.55')
    # Near hindquarter to stifle, diagonal gaskin, hock, narrow cannon.
    if pose=='shift':
        leg='M32 54 Q45 52 46 64 Q45 71 39 76 L35 82 L44 92 L49 94 L49 98 L45 99 L32 85 Q29 82 32 78 L35 71 Q28 64 32 54Z'
        foot='M44 93L49 94L49 98L45 100L43 98Z'
    else:
        leg='M32 54 Q46 54 46 64 Q45 71 38 76 L34 79 L30 79 L29 83 L33 86 L33 95 Q36 97 35 100 L40 102 L40 106 L28 106 L28 102 L30 97 L29 86 L26 82 L28 78 L33 72 Q28 65 32 54Z'
        foot='M30 101Q35 100 37 102L40 106H28L29 103Z'
    s+=path(leg,g);s+=path(foot,hoof)
    s+=path('M34 57Q40 57 40 64L36 71','none',stroke=light,stroke_width='1.2',opacity='.45')
    # Near foreleg: scapular mass flows into elbow, forearm, knee and fetlock.
    s+=path('M86 53 Q98 54 97 64 Q97 70 93 75 L94 79 L92 83 L90 84 L89 95 Q92 97 92 100 L97 103 L97 107 L84 107 L83 103 L85 100 L86 96 L86 84 L83 81 L83 77 L86 73 L83 65Z',g)
    s+=path('M85 102Q91 101 94 103L97 107H84L83 105Z',hoof)
    s+=path('M88 66 L90 74 M89 82 L87 98','none',stroke=light,stroke_width='.9',opacity='.6')
    s+=path('M84 77Q88 75 93 78L91 81L85 81Z',muscle)
    s+=path('M86 96Q90 95 92 99L88 101L85 100Z',muscle)
    s+=path('M84 102Q90 101 94 103','none',stroke=light,stroke_width='1')
    s+=path('M28 79L32 79L34 82L30 85L27 82Z',muscle)
    if pose!='shift':
        s+=path('M30 96Q34 95 35 98L33 101L29 100Z',muscle)
        s+=path('M29 102Q34 100 38 103','none',stroke=light,stroke_width='1')
    # Distinct cervical mass, poll flexion and jaw. No tubular neck/muzzle.
    if pose=='graze':
        s+=path('M84 37 C95 34 105 42 110 53 Q115 65 119 75 L111 81 Q106 70 99 65 Q91 65 88 58 Q91 47 84 37Z',g)
        s+=path('M90 43 Q102 43 107 55 L114 74 Q105 66 100 58Z',muscle,opacity='.5')
        crest='M87 37Q101 36 109 51L117 72'; head='translate(117 74) rotate(31)'
    else:
        s+=path('M83 42 Q92 32 96 24 Q99 17 105 16 L111 23 Q108 29 105 37 L103 52 Q102 61 96 64 Q87 62 85 56 Q91 48 83 42Z',g)
        s+=path('M100 23Q97 36 92 43L96 58Q104 45 105 30Z',muscle,opacity='.64')
        s+=path('M89 46Q97 41 102 34','none',stroke=light,stroke_width='.9',opacity='.45')
        crest='M85 41Q95 29 97 23Q100 16 105 16';head=f'translate(106 20) rotate({-10 if state=="blocked" else 6})'
    if i['mane']=='braided':
        s+=path(crest,'none',stroke=mane,stroke_width='3.4',stroke_dasharray='2 1.5')
    else:
        s+=path(crest,'none',stroke=mane,stroke_width='3',stroke_linecap='round')
        if i['mane']=='flowing':
            s+=path('M99 19Q95 35 86 45L82 50Q95 46 103 22Z' if pose!='graze' else 'M94 39Q105 48 111 65L108 70Q100 52 90 43Z',mane)
    s+=f'<g transform="{head}">'
    s+=path('M-5 0L-6 -8Q-3 -9 -1 -1Z',g)
    s+=path('M2 -2L4 -9Q7 -8 6 -1Z',g)
    s+=path('M-4 -1 Q2 -5 8 0 L14 9 L23 17 Q26 21 22 24 L16 23 L8 15 Q2 17 -3 11 Q-7 9 -7 3Z',g)
    s+=path('M-3 3Q3 2 7 8Q7 14 2 15Q-5 12 -3 3Z',muscle)
    s+=path('M17 15Q23 16 25 20L23 23L17 22L14 18Z',mix(c,'#302c2d',.4))
    s+=path('M9 2L20 17','none',stroke=light,stroke_width='1',opacity='.6')
    if i['coat'] in ['dun','palomino','buckskin','chestnut']:
        s+=path('M7 0L18 14L17 18L13 12L5 1Z','#eee2c9')
    s+='<ellipse cx="6" cy="4" rx="1.25" ry=".95" fill="#191c1e"/><circle cx="6.3" cy="3.8" r=".3" fill="#e8e3d3"/><ellipse cx="22" cy="20" rx="1" ry=".65" fill="#242322"/>'
    s+=path('M17 22L22 23','none',stroke=dark,stroke_width='.65')
    s+=path('M-5 -7L-3 -1M4 -8L6 -2L8 0L14 9L23 17','none',stroke='#cbddea',stroke_width='1.15',opacity='.85',**{'class':'moon-rim'})
    s+=path('M-4 -2Q1 -4 5 0L2 3Z',mane)
    if i['accessory']=='hat':
        s+=path('M-10 -4Q1 -8 11 -3L9 -1L-10 -1Z','#d8b46a')
        s+=path('M-5 -4L-4 -10H3L6 -4Z','#c5a267')
    s+='</g>'
    if i['tack']!='none':
        s+=path('M49 42Q63 46 76 42L78 53Q64 57 49 53Z','#a74735' if i['tack']=='pad' else '#795031')
        s+=path('M50 43Q63 48 75 43L76 51','none',stroke='#d29b68',stroke_width='.75')
        if i['tack']=='saddle':
            s+=path('M53 42Q61 39 71 43L70 48L55 47Z','#624029')
            s+=path('M67 48L68 61L73 61L73 58','none',stroke='#b2a28b',stroke_width='.9')
    if i['accessory']=='bandana':s+=path('M94 42L104 44L96 51Z','#c2543f')
    if i['breed']=='draft':
        s+=path('M85 96L90 97L92 101L83 101Z',mane,opacity='.8')
        if pose!='shift':s+=path('M29 96L34 97L36 101L28 101Z',mane,opacity='.8')
    s+=path('M28 44Q35 37 46 40M49 43Q65 47 80 41L86 36','none',stroke='#cbddea',stroke_width='1.05',opacity='.82',**{'class':'moon-rim'})
    s+=path('M105 17L111 23Q108 29 105 37L103 52' if pose!='graze' else 'M96 39Q109 44 113 59L119 74','none',stroke='#cbddea',stroke_width='1.15',opacity='.85',**{'class':'moon-rim'})
    # Sparse coat hair catches directional light; deterministic anatomical ROI.
    rng=random.Random(42)
    for _ in range(35):
        x=rng.uniform(48,80);y=rng.uniform(49,59)
        s+=path(f'M{x:.2f} {y:.2f}l1.1 .4','none',stroke=light,stroke_width='.24',opacity='.23')
    s+='</g>'
    if state=='blocked':
        s+=path('M140 79V105','none',stroke='#bda980',stroke_width='1.3')
        s+=path('M140 79H148L145 83L148 87H140Z','#ae5b61')
    return s+'</svg>'

def world(night):
    r=random.Random(442)
    sky=('#121a32','#667991') if night else ('#388fc4','#e5e8c9')
    s=f'<svg class="world" viewBox="0 0 390 640" preserveAspectRatio="none" xmlns="http://www.w3.org/2000/svg"><defs><linearGradient id="sky" x2="0" y2="1"><stop stop-color="{sky[0]}"/><stop offset="1" stop-color="{sky[1]}"/></linearGradient><linearGradient id="field" x1=".1" y1="0" x2=".8" y2="1"><stop stop-color="{"#63746a" if night else "#bcc17b"}"/><stop offset=".4" stop-color="{"#415d55" if night else "#8e9c58"}"/><stop offset="1" stop-color="{"#273f3e" if night else "#485e3b"}"/></linearGradient><radialGradient id="light"><stop stop-color="{"#c4d4ea" if night else "#fff3c7"}" stop-opacity=".35"/><stop offset="1" stop-color="#fff3c7" stop-opacity="0"/></radialGradient><filter id="haze"><feGaussianBlur stdDeviation="6"/></filter><linearGradient id="wood" x2="0" y2="1"><stop stop-color="{"#a4a99d" if night else "#d6bd85"}"/><stop offset=".25" stop-color="{"#7e867d" if night else "#b09662"}"/><stop offset="1" stop-color="{"#444e4c" if night else "#665537"}"/></linearGradient><filter id="grain"><feTurbulence type="fractalNoise" baseFrequency=".8" numOctaves="3" seed="442"/><feColorMatrix type="saturate" values="0"/><feComponentTransfer><feFuncA type="linear" slope=".07"/></feComponentTransfer><feBlend in="SourceGraphic" mode="soft-light"/></filter></defs>'
    s+=path('M0 0H390V640H0Z','url(#sky)')
    if night:
        s+='<path d="M30 -20Q164 70 360 204" fill="none" stroke="#8e9cb2" stroke-width="39" opacity=".12" filter="url(#haze)"/>'
        # Granular astronomical band: core points and offset dust lane, not fog.
        star=random.Random(844)
        for _ in range(1800):
            y=star.uniform(-10,206); center=45+y*1.48
            x=center+star.gauss(0,19)
            if 0<x<390:
                s+=f'<circle cx="{x:.2f}" cy="{y:.2f}" r="{star.uniform(.15,.7):.2f}" fill="{star.choice(["#b7c8df","#d6cbd6","#f4e4d0"])}" opacity="{star.uniform(.15,.7):.2f}"/>'
        for _ in range(130):
            x=star.uniform(0,390);y=star.uniform(0,205)
            s+=f'<circle cx="{x:.2f}" cy="{y:.2f}" r="{star.uniform(.45,1.15):.2f}" fill="#ebedf4" opacity="{star.uniform(.5,1):.2f}"/>'
        s+='<circle cx="327" cy="48" r="32" fill="url(#light)"/><circle cx="327" cy="48" r="7" fill="#dedfd4"/><circle cx="325" cy="46" r="1.8" fill="#bfc7c6" opacity=".4"/>'
    else:
        s+='<ellipse cx="66" cy="66" rx="170" ry="130" fill="url(#light)"/>'
        for x,y,k in [(40,65,1),(257,28,.8),(345,100,.6)]:
            s+=f'<g transform="translate({x} {y}) scale({k})" opacity=".56" filter="url(#haze)"><path d="M-58 5Q-37 -6 -18 0Q-1 -21 14 -11Q27 -16 47 -1Q69 0 85 7Q15 19 -58 5Z" fill="#f8f3da"/></g>'
    # Multiple irregular ridge planes, warm haze and shadowed valleys.
    layers=[('M0 192L20 181L41 185L72 160L91 163L121 147L146 154L176 137L198 144L217 167L238 159L258 166L287 141L309 149L333 171L360 157L390 168V640H0Z','#5f7188' if night else '#9db7b3'),('M0 218Q35 187 65 192L98 209Q131 173 164 185L196 201Q241 166 277 196L310 187Q355 192 390 210V640H0Z','#4e666f' if night else '#819e91'),('M0 243Q46 205 104 234Q159 200 220 235Q278 210 322 233Q358 225 390 234V640H0Z','#425c5d' if night else '#698c72')]
    for d,c in layers:s+=path(d,c)
    s+=path('M76 193L117 186L150 208L179 214L127 202Z','#c0c6ab' if not night else '#72818b',opacity='.27')
    s+=path('M239 210L272 204L296 214L335 228L290 219Z','#cad0a4' if not night else '#7b8b8b',opacity='.3')
    s+=path('M0 259Q80 228 163 255Q258 278 390 249V640H0Z','url(#field)')
    s+=path('M0 338Q121 291 227 320T390 298V326Q302 344 204 343Q86 310 0 354Z','#d9ce92' if not night else '#879182',opacity='.22')
    s+=path('M0 476Q114 437 227 471T390 448V488Q248 511 164 479Q74 471 0 506Z','#253f37',opacity='.16')
    # Receding field tracks: converging width and reduced contrast.
    s+=path('M245 257Q190 357 298 465T362 640H390Q374 514 308 462Q207 352 255 257Z','#bca777' if not night else '#68766c',opacity='.22')
    # Far barn is deliberately left of dark horse, roof and side distinct.
    s+='<g transform="translate(156 207) scale(.68)">'
    s+=path('M0 15L29 -6L58 15V49H0Z','#865744' if not night else '#625651')
    s+=path('M58 15L78 8V40L58 49Z','#614437' if not night else '#434f51')
    s+=path('M-5 16L29 -11L61 12L80 5L78 10L58 20L29 -4L0 21Z','#465452')
    s+=path('M22 26H39V49H22Z','#363e36')
    s+=path('M24 7H33V16H24Z','#f1ce8d' if night else '#a1aaa0')
    for x in range(6,57,7):s+=path(f'M{x} 24V46','none',stroke='#c1936c',stroke_width='.7',opacity='.38')
    s+='</g>'
    def tree(x,y,scale):
        t=f'<g transform="translate({x} {y}) scale({scale})">'
        t+=path('M-4 2L3 53L10 55L5 19L18 -3L14 -7L3 9L0 -5Z','#51503e' if not night else '#33484a')
        t+=path('M2 17L5 53','none',stroke='#c0a776' if not night else '#8b9c94',stroke_width='1.3')
        # Small overlapping leaf clusters with asymmetric lit crowns.
        for _ in range(80):
            a=r.uniform(0,math.tau); rr=math.sqrt(r.random())*32
            xx=math.cos(a)*rr;yy=math.sin(a)*rr*.7-9
            colors=['#3d614a','#547449','#70844e','#91a163'] if not night else ['#263f43','#354e4e','#48625a','#687b67']
            color=colors[min(3,max(0,int((24-xx-yy)/22)))]
            size=r.uniform(4,9)
            t+=path(f'M{xx-size:.1f} {yy:.1f}q2 {-size:.1f} {size:.1f} {-size*.8:.1f}q{size:.1f} -1 {size*1.3:.1f} {size*.7:.1f}q-1 {size:.1f} {-size:.1f} {size*.8:.1f}q{-size:.1f} 2 {-size*1.3:.1f} {-size*.7:.1f}Z',color)
        return t+'</g>'
    for x,y,k in [(46,223,.36),(98,233,.25),(258,223,.4),(317,230,.32),(381,200,1.2),(-7,199,1.1)]:s+=tree(x,y,k)
    # Rear fence is small and recessed; foreground gate is its own DOM overlay.
    for y in [271,535]:
        s+=path(f'M0 {y}Q180 {y-7} 390 {y+2}','none',stroke='#aaa783' if not night else '#818f86',stroke_width='2',opacity='.6')
        for x in range(0,400,35):s+=path(f'M{x} {y-7}V{y+13}','none',stroke='#656c50',stroke_width='2',opacity='.75')
    # Ground texture grows toward viewer, with grass blades grouped in tufts.
    for _ in range(1050):
        x=r.uniform(0,390);y=r.uniform(261,640);scale=(y-240)/400
        h=r.uniform(1,5)*scale
        color=r.choice(['#c4c18a','#819957','#4e703f','#a5ae66'] if not night else ['#8d9a7b','#617a61','#2f5146','#829276'])
        s+=path(f'M{x:.1f} {y:.1f}q-1 {-h:.1f} {-h*.7:.1f} {-h:.1f}m{h*.7:.1f} {h:.1f}q1 {-h*.8:.1f} {h*.6:.1f} {-h*.9:.1f}','none',stroke=color,stroke_width=f'{.35+scale*.45:.2f}',opacity='.55')
    s+='<ellipse cx="'+('332' if night else '25')+'" cy="313" rx="250" ry="230" fill="url(#light)"/>'
    s+='<path d="M0 0H390V640H0Z" fill="transparent" filter="url(#grain)" opacity=".42"/>'
    return s+'</svg>'

def rail():
    return '''<svg class="physical-rail" viewBox="0 0 390 44" preserveAspectRatio="none" aria-hidden="true"><defs><linearGradient id="railwood" x2="0" y2="1"><stop stop-color="#c9b18b"/><stop offset=".2" stop-color="#a58b65"/><stop offset="1" stop-color="#5b5140"/></linearGradient></defs><g fill="#172626" opacity=".4"><path d="M6 40L30 43H42L17 39ZM190 40L214 44H227L201 39ZM372 40L390 44V40L382 39Z"/></g><path d="M0 40H390" stroke="#19282a" stroke-width="5" opacity=".2"/><path d="M0 11L390 14V21L0 18ZM0 28L390 30V36L0 34Z" fill="url(#railwood)"/><path d="M0 17L390 20V22L0 19ZM0 33L390 35V37L0 35Z" fill="#393f35" opacity=".7"/><path d="M6 2L18 0V43H6ZM190 2L201 0V43H190ZM372 2L384 0V43H372Z" fill="url(#railwood)"/><path d="M0 12H390M0 29H390M8 3V41M192 3V41M374 3V41" fill="none" stroke="#dec9a0" stroke-width=".7"/><path d="M20 19L187 29M204 29L369 19" stroke="#74634b" stroke-width="3"/><g fill="#454c46"><circle cx="12" cy="15" r="1.3"/><circle cx="195" cy="15" r="1.3"/><circle cx="378" cy="15" r="1.3"/></g><path d="M194 3H216L210 9L216 15H194Z" fill="#a6545c"/></svg>'''
