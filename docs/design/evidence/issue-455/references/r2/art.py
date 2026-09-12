"""Original sculpted-vector ranch illustration; deterministic, no external art."""
import random, math
import fixtures as F
MANES=['#2e2019','#5d3317','#17161a','#7d8089','#e8dcc0','#41321f','#4a342c','#241a10']
def mix(a,b,t):
    return '#'+''.join(f'{round(int(a[k:k+2],16)*(1-t)+int(b[k:k+2],16)*t):02x}' for k in (1,3,5))
def path(d,fill,**attrs):
    return '<path d="'+d+'" fill="'+fill+'" '+' '.join(f'{k.replace("_","-")}="{v}"' for k,v in attrs.items())+'/>'
def horse(i,state='idle',pose='stand',uid='h',facing=1):
    """Continuous anatomical silhouette; light is clipped, never joint plates."""
    import re
    c=F.COAT_HEX[i['coat']]['body']; mane=MANES[F.COATS.index(i['coat'])]
    dark=mix(c,'#15212b',.56); light=mix(c,'#ffe6bd',.45)
    is_shift=pose=='shift'; grazing=pose=='graze'
    def warp(d):
        # Weight shift moves the ribcage over the support line and drops the
        # resting hip. Transform CONTINUOUS body, not a detached thigh piece.
        if not is_shift:return d
        def point(m):
            x,y=map(float,m.groups())
            weight=max(0,min(1,(78-y)/25))
            return f'{x+weight*3.2:.2f} {y+weight*max(0,(82-x)/55)*4.8:.2f}'
        return re.sub(r'(-?\d+(?:\.\d+)?)\s+(-?\d+(?:\.\d+)?)',point,d)
    if grazing:
        crown='C91 35 100 43 105 54 Q108 60 110 67 L106 63 L103 57 Q101 59 105 67 L111 70 L111 60 Q114 59 114 70 Q119 73 122 82 L130 95 Q132 100 126 101 L120 99 L113 87 Q106 87 105 79 Q98 72 94 63'
        crest='M84 38 C97 37 102 49 108 66'
        eye=(115,78); nostril=(127,97)
        blaze='M116 73 Q120 80 122 87 L126 95 L124 96 Q118 85 113 77Z'
    else:
        crown='Q91 31 98 21 Q101 15 105 17 L103 9 Q105 7 108 17 L111 17 L113 8 Q116 9 114 20 Q118 23 120 29 L132 40 Q136 45 130 47 L124 45 L116 37 Q111 40 107 36 Q104 45 103 54 Q102 61 98 64'
        crest='M84 39 Q94 27 98 21 Q101 16 106 18'
        eye=(116,27);nostril=(131,43)
        blaze='M115 22 Q119 29 124 34 L130 41 L128 43 L121 35 Q117 29 113 24Z'
    top='M24 44 C30 39 41 37 51 41 C62 44 71 43 80 39 L85 35 '
    front=' C98 71 93 75 92 80 Q92 83 90 85 L89 97 Q91 99 91 101 L97 104 L97 107 L85 107 L83 104 L85 100 L86 85 Q83 82 84 78 Q86 71 83 66 '
    belly={'draft':'Q68 76 44 67','stock':'Q66 69 47 64','light':'Q66 66 47 62'}[i['breed']]
    hind=(' C45 72 39 76 36 82 Q39 88 46 96 L49 101 L55 105 L54 107 L49 106 L44 100 L31 85 Q29 81 31 77 L34 71 ' if is_shift else ' C45 71 38 76 34 82 L34 96 Q36 99 36 101 L41 104 L40 107 L28 107 L27 104 L29 100 L30 85 Q27 82 28 79 L32 71 ')
    outline=warp(top+crown+front+belly+hind+'C21 65 19 53 24 44 Z')
    farhind=('M43 56 C53 64 51 72 46 80 L46 98 L51 102 L51 106 L42 106 L41 102 L42 82 L41 73 L36 62 Z' if is_shift else 'M43 56 Q52 64 47 74 L41 83 L43 99 L49 102 L48 105 L40 105 L38 101 L37 82 L39 72 L35 62Z')
    farfront='M94 53 Q105 60 102 70 L99 81 L100 98 L106 102 L105 105 L97 105 L96 101 L95 82 L94 68Z'
    s=f'<svg class="horse-svg" viewBox="0 0 148 112" xmlns="http://www.w3.org/2000/svg" data-pose="{pose}" data-identity="{i}"><defs><clipPath id="{uid}-silhouette">'+path(outline,'white')+f'</clipPath><linearGradient id="{uid}-coat" x1=".2" y1="0" x2=".6" y2="1"><stop stop-color="{light}"/><stop offset=".32" stop-color="{c}"/><stop offset=".69" stop-color="{c}"/><stop offset="1" stop-color="{dark}"/></linearGradient><radialGradient id="{uid}-light"><stop stop-color="{light}" stop-opacity=".32"/><stop offset=".48" stop-color="{light}" stop-opacity=".15"/><stop offset="1" stop-color="{light}" stop-opacity="0"/></radialGradient><radialGradient id="{uid}-shade"><stop stop-color="{dark}" stop-opacity=".8"/><stop offset="1" stop-color="{dark}" stop-opacity="0"/></radialGradient><linearGradient id="{uid}-leather" x2="0" y2="1"><stop stop-color="#c36b4a"/><stop offset="1" stop-color="#683a30"/></linearGradient><filter id="{uid}-soft" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="1.4"/></filter></defs>'
    s+=f'<g transform="translate({148 if facing==-1 else 0} 0) scale({facing} 1)">'
    s+=f'<path class="cast-shadow" d="M29 106 Q50 95 93 101 L143 109 Q100 114 37 110Z" fill="#142924" opacity=".43" filter="url(#{uid}-soft)"/>'
    contacts=[(47 if is_shift else 44,105),(101,105),(91,107)]+([] if is_shift else [(34,107)])
    for x,y in contacts:s+=f'<ellipse cx="{x}" cy="{y}" rx="6.2" ry="1.1" fill="#132521" opacity=".57"/>'
    if is_shift:s+='<ellipse cx="54" cy="106" rx="2" ry=".7" fill="#132521" opacity=".36"/>'
    s+=path(warp(farhind),mix(c,'#1e2929',.34))+path(farfront,mix(c,'#1e2929',.34))
    # Tail grows from the top of the pelvis, with a continuous dock and hairs.
    s+=path(warp('M25 43 C18 45 22 62 18 74 Q17 84 11 90 Q24 88 27 71 L31 50Z'),mane)
    for j in range(5):s+=path(warp(f'M{27+j*.4} 46 Q{23+j*.5} 69 {18+j} 84'),'none',stroke=mix(mane,light,.28),stroke_width='.45',opacity='.55')
    s+=path(outline,f'url(#{uid}-coat)')
    s+=f'<g clip-path="url(#{uid}-silhouette)">'
    s+=f'<image x="0" y="0" width="148" height="112" href="{paint_surface(c,pose=pose)}"/>'
    # Broad feathered illumination crosses anatomical regions rather than
    # tracing round hip/shoulder pieces. No component seams, caps or bevels.
    s+=f'<ellipse cx="57" cy="43" rx="42" ry="18" fill="url(#{uid}-light)" transform="rotate(6 57 43)"/><ellipse cx="73" cy="67" rx="37" ry="12" fill="url(#{uid}-shade)"/><ellipse cx="95" cy="51" rx="19" ry="24" fill="url(#{uid}-light)" transform="rotate(28 95 51)"/>'
    s+=path(warp('M23 52 C32 66 39 62 47 65 Q66 73 85 63 Q90 66 86 77 L85 98 L94 103 L87 110 L81 96 L80 72 Q64 77 45 69 L37 82 L34 106 L26 107 L25 80Z'),dark,opacity='.28',filter=f'url(#{uid}-soft)')
    s+=path(warp('M29 44 Q32 61 37 68 M89 40 Q92 53 86 62'),'none',stroke=dark,stroke_width='1.4',opacity='.32',filter=f'url(#{uid}-soft)')
    # Tendon planes are narrow diffuse strokes, not circular joint drawings.
    s+=path(warp('M32 73 L30 80 L32 94 M88 68 Q90 76 88 83 L87 96'),'none',stroke=light,stroke_width='1.15',opacity='.45')
    s+=path('M85 100 Q90 100 93 103 L98 105 L98 109 L83 109Z','#3b3931')
    s+=path('M85 102 Q91 102 96 105','none',stroke='#b1a28c',stroke_width='.65')
    if is_shift:s+=path('M49 100 L54 104 L56 106 L53 108 L49 104Z','#403c32')
    else:s+=path('M28 101 Q33 100 38 104 L42 105 L42 110 L26 110Z','#3b3931')
    if i['breed']=='draft':
        s+=path('M85 96 L88 100 L90 97 L92 101 L84 102Z',mix(mane,c,.48),opacity='.7')
    # Original roan/tactile coat marks follow the ribcage; deterministic.
    rng=random.Random(442)
    for _ in range(90):
        x=rng.uniform(27,102); y=rng.uniform(39,67)
        s+=path(f'M{x:.2f} {y:.2f}q1 .1 1.7 .7','none',stroke=light if i['coat']=='roan' or rng.random()<.6 else dark,stroke_width='.55',opacity='.19')
    s+='</g>'
    # Cheek/throatlatch are low-contrast planes within the continuous head.
    if grazing:
        s+=path('M110 74 Q118 77 117 84 L113 85 Q108 84 107 79Z',dark,opacity='.22')
        s+=path('M124 94 Q130 94 131 98 L127 100 L122 98Z',mix(c,'#393330',.38))
        rim='M86 36 Q100 42 105 56 L110 67 M112 61 L114 70 Q120 74 122 82 L130 96'
    else:
        s+=path('M109 26 Q117 27 118 33 Q112 39 108 33Z',dark,opacity='.23')
        s+=path('M128 38 Q135 41 133 45 L129 47 L125 44Z',mix(c,'#393330',.38))
        rim='M105 10 L108 18 M114 10 L114 20 Q118 23 121 30 L133 41 M107 38 Q104 45 103 54'
    s+=f'<ellipse cx="{eye[0]}" cy="{eye[1]}" rx="1.05" ry=".75" fill="#1b211f"/><ellipse cx="{nostril[0]}" cy="{nostril[1]}" rx=".9" ry=".65" fill="#252925"/>'
    if i['coat'] in ['palomino','chestnut','dun','buckskin']:s+=path(blaze,'#e8dfc8',opacity='.9')
    if i['mane']=='braided':s+=path(crest,'none',stroke=mane,stroke_width='3',stroke_dasharray='1.5 1')
    else:
        s+=path(crest,'none',stroke=mane,stroke_width='2.4')
        if i['mane']=='flowing':s+=path('M99 21 Q94 37 84 44 L82 51 Q96 45 103 24Z' if not grazing else 'M91 39 Q104 48 107 64 L103 65 Q98 50 88 43Z',mane)
    if i['tack']!='none':
        s+=path(warp('M49 42 Q61 46 75 42 L77 54 Q62 58 49 52Z'),f'url(#{uid}-leather)')
        s+=path(warp('M50 43 Q64 48 74 44'),'none',stroke='#d19a72',stroke_width='.7')
        if i['tack']=='saddle':
            s+=path(warp('M54 43 Q62 40 71 44 L70 48 L55 47Z'),'#68442e')
            s+=path('M68 48 L69 60 L73 61 L73 58','none',stroke='#aa9a80',stroke_width='.8')
    if i['accessory']=='bandana':s+=path('M92 43 L102 47 L95 53Z','#b14b3c')
    if i['accessory']=='hat':
        s+=path('M99 14 Q110 10 119 17 L115 19 L100 17Z' if not grazing else 'M103 65 Q112 62 121 69 L119 72 L104 68Z','#caa96e')
    s+=path(warp('M28 43 Q37 36 50 41 Q65 45 80 39 L85 35'),'none',stroke='#d3dfeb',stroke_width='1',opacity='.68',**{'class':'moon-rim'})
    s+=path(rim,'none',stroke='#d3dfeb',stroke_width='1.05',opacity='.8',**{'class':'moon-rim'})
    # Tufts in front of planted feet integrate the terrain, not a green oval.
    for x in [31,49,87,104]:s+=path(f'M{x} 108 l-1 -2 m1 2 l2 -1.6','none',stroke='#7e8b60',stroke_width='.5',opacity='.65')
    s+='</g>'
    if state=='blocked':s+=path('M140 79V105','none',stroke='#bda980',stroke_width='1.3')+path('M140 79H148L145 83L148 87H140Z','#ae5b61')
    return s+'</svg>'

def world(night):
    r=random.Random(442)
    sky=('#121a32','#667991') if night else ('#388fc4','#e5e8c9')
    s=f'<svg class="world" viewBox="0 0 390 640" preserveAspectRatio="none" xmlns="http://www.w3.org/2000/svg"><defs><linearGradient id="sky" x2="0" y2="1"><stop stop-color="{sky[0]}"/><stop offset="1" stop-color="{sky[1]}"/></linearGradient><linearGradient id="field" x1=".1" y1="0" x2=".8" y2="1"><stop stop-color="{"#63746a" if night else "#bcc17b"}"/><stop offset=".4" stop-color="{"#415d55" if night else "#8e9c58"}"/><stop offset="1" stop-color="{"#273f3e" if night else "#485e3b"}"/></linearGradient><radialGradient id="light"><stop stop-color="{"#c4d4ea" if night else "#fff3c7"}" stop-opacity=".15"/><stop offset="1" stop-color="#fff3c7" stop-opacity="0"/></radialGradient><filter id="haze"><feGaussianBlur stdDeviation="6"/></filter><linearGradient id="wood" x2="0" y2="1"><stop stop-color="{"#a4a99d" if night else "#d6bd85"}"/><stop offset=".25" stop-color="{"#7e867d" if night else "#b09662"}"/><stop offset="1" stop-color="{"#444e4c" if night else "#665537"}"/></linearGradient><filter id="grain"><feTurbulence type="fractalNoise" baseFrequency=".8" numOctaves="3" seed="442"/><feColorMatrix type="saturate" values="0"/><feComponentTransfer><feFuncA type="linear" slope=".07"/></feComponentTransfer><feBlend in="SourceGraphic" mode="soft-light"/></filter></defs>'
    s+=path('M0 0H390V640H0Z','url(#sky)')
    if night:
        s+='<defs><linearGradient id="galactic" x1="0" y1="0" x2="1" y2="1"><stop stop-color="#93a4c3" stop-opacity=".12"/><stop offset=".35" stop-color="#cec5d5" stop-opacity=".32"/><stop offset=".65" stop-color="#b6c8d2" stop-opacity=".26"/><stop offset="1" stop-color="#8f99b9" stop-opacity=".08"/></linearGradient><filter id="stellar-cloud"><feTurbulence type="fractalNoise" baseFrequency=".045 .05" numOctaves="4" seed="844"/><feColorMatrix type="matrix" values="0 0 0 0 .76 0 0 0 0 .79 0 0 0 0 .92 0 0 0 1.9 -.65"/><feComposite in2="SourceGraphic" operator="in"/></filter></defs>'
        band='M-8 -20 C63 -7 103 39 168 70 C240 107 316 147 398 195 L404 232 C311 191 235 145 156 113 C86 83 41 31 -8 21Z'
        s+=path(band,'url(#galactic)',filter='url(#haze)')
        s+='<g filter="url(#haze)">'+path(band,'#bdc9d8',filter='url(#stellar-cloud)',opacity='.42')+'</g>'
        s+=path('M87 29 Q112 61 155 75 L145 78 Q104 64 98 48 L83 43Z','#222a45',opacity='.3',filter='url(#haze)')
        s+=path('M172 92 Q189 108 215 112 L203 118 L178 108 L165 105Z','#222a45',opacity='.38')
        cloud=random.Random(884)
        for _ in range(95):
            yy=cloud.uniform(-15,210);xx=45+yy*1.48+cloud.gauss(0,14)
            s+=f'<ellipse cx="{xx:.1f}" cy="{yy:.1f}" rx="{cloud.uniform(3,13):.1f}" ry="{cloud.uniform(2,8):.1f}" fill="{cloud.choice(["#ccd7dd","#c8bed3","#ded1bf"])}" opacity=".06" filter="url(#haze)"/>'
        s+=path('M102 49Q151 83 213 109','none',stroke='#e3d9d2',stroke_width='11',opacity='.10',filter='url(#haze)')
        # Granular astronomical band: core points and offset dust lane, not fog.
        star=random.Random(844)
        for _ in range(1200):
            y=star.uniform(-10,206); center=45+y*1.48
            x=center+star.gauss(0,19)
            if 0<x<390:
                s+=f'<circle cx="{x:.2f}" cy="{y:.2f}" r="{star.uniform(.15,.48):.2f}" fill="{star.choice(["#b7c8df","#d6cbd6","#f4e4d0"])}" opacity="{star.uniform(.15,.48):.2f}"/>'
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
    for idx,(d,c) in enumerate(layers):
        s+=f'<defs><linearGradient id="ridge-volume{idx}" x1="0" y1="0" x2=".65" y2="1"><stop stop-color="{mix(c,"#cad3d0",.23)}"/><stop offset=".24" stop-color="{c}"/><stop offset=".42" stop-color="{mix(c,"#284d43",.24)}"/><stop offset="1" stop-color="{c}"/></linearGradient></defs>'
        s+=path(d,f'url(#ridge-volume{idx})')
        s+=f'<defs><clipPath id="ridge{idx}">'+path(d,'white')+'</clipPath></defs>'
        brush=random.Random(442+idx)
        s+=f'<g clip-path="url(#ridge{idx})">'
        for _ in range(75):
            x=brush.uniform(-20,390);y=brush.uniform(163+idx*20,235+idx*16);w=brush.uniform(9,43)
            s+=path(f'M{x:.1f} {y:.1f}q{w*.4:.1f} -8 {w:.1f} -3l{-w*.6:.1f} 8Z',mix(c,'#dde0c0' if not night else '#9baac0',.3),opacity=f'{brush.uniform(.03,.12):.2f}')
        s+='</g>'
    s+='<ellipse cx="157" cy="227" rx="245" ry="18" fill="'+('#adbac5' if night else '#e8e4c4')+'" opacity=".13" filter="url(#haze)"/>'
    s+=path('M76 193L117 186L150 208L179 214L127 202Z','#c0c6ab' if not night else '#72818b',opacity='.27')
    s+=path('M239 210L272 204L296 214L335 228L290 219Z','#cad0a4' if not night else '#7b8b8b',opacity='.3')
    s+=path('M0 259Q80 228 163 255Q258 278 390 249V640H0Z','url(#field)')
    s+='<defs><clipPath id="ground-paint">'+path('M0 259Q80 228 163 255Q258 278 390 249V640H0Z','white')+'</clipPath></defs>'
    s+=f'<image x="0" y="250" width="390" height="390" opacity=".75" clip-path="url(#ground-paint)" href="{paint_surface("#516757" if night else "#809253",kind="ground",night=night)}"/>'
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
            gid=f'leaf{x}-{y}-{_}'
            t+=f'<defs><linearGradient id="{gid}" x1="0" y1="0" x2=".7" y2="1"><stop stop-color="{mix(color,"#ccd2a0" if not night else "#9aaeb1",.18)}"/><stop offset=".5" stop-color="{color}"/><stop offset="1" stop-color="{mix(color,"#203c37",.28)}"/></linearGradient></defs>'
            t+=path(f'M{xx-size:.1f} {yy:.1f}q2 {-size:.1f} {size:.1f} {-size*.8:.1f}q{size:.1f} -1 {size*1.3:.1f} {size*.7:.1f}q-1 {size:.1f} {-size:.1f} {size*.8:.1f}q{-size:.1f} 2 {-size*1.3:.1f} {-size*.7:.1f}Z',f'url(#{gid})')
        return t+'</g>'
    for x,y,k in [(46,223,.36),(98,233,.25),(258,223,.4),(317,230,.32),(381,200,1.2),(-7,199,1.1)]:
        s+=f'<ellipse cx="{x+20*k}" cy="{y+54*k}" rx="{39*k}" ry="{4*k}" fill="#183c33" opacity=".2"/>'
        s+=tree(x,y,k)
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

# Procedural paint is embedded in SVG; no downloaded/generated-service assets.
# Analytic depth provides one connected body surface, never joint discs.
def paint_surface(color, kind='coat', pose='stand', night=False):
    import base64, io
    from PIL import Image
    key=(color,kind,pose,night)
    if key in _PAINT_CACHE:return _PAINT_CACHE[key]
    rgb=tuple(int(color[k:k+2],16) for k in (1,3,5))
    w,h=(148,112) if kind=='coat' else (390,390)
    im=Image.new('RGBA',(w,h));pix=im.load()
    def noise(x,y):
        ix,iy=math.floor(x),math.floor(y);fx=x-ix;fy=y-iy
        fx=fx*fx*(3-2*fx);fy=fy*fy*(3-2*fy)
        def v(a,b):
            z=(a*374761393+b*668265263+442)&0xffffffff
            z=((z^(z>>13))*1274126177)&0xffffffff
            return (z^(z>>16))/4294967295
        return (v(ix,iy)*(1-fx)+v(ix+1,iy)*fx)*(1-fy)+(v(ix,iy+1)*(1-fx)+v(ix+1,iy+1)*fx)*fy
    def depth(x,y):
        # Elliptic ribcage, smooth withers/neck union and narrow tendon volumes.
        fields=[]
        for cx,cy,rx,ry,z in [(56,52,43,19,17),(91,49,14,24,10),(29,54,14,18,10)]:
            q=1-((x-cx)/rx)**2-((y-cy)/ry)**2
            fields.append(z*math.sqrt(max(0,q)))
        if pose=='graze':a,b,c,d=91,47,111,76
        else:a,b,c,d=93,45,106,24
        vx,vy=c-a,d-b;t=max(0,min(1,((x-a)*vx+(y-b)*vy)/(vx*vx+vy*vy)))
        distance=((x-a-t*vx)**2+(y-b-t*vy)**2)**.5
        fields.append(8*math.sqrt(max(0,1-(distance/10)**2)))
        if y>66:
            leg=88 if x>65 else (34+(y-82)*.8 if pose=='shift' else 32)
            fields.append(3*math.sqrt(max(0,1-((x-leg)/5)**2)))
        # Smooth union, so intersecting muscles do not acquire separate edges.
        z=0
        for f in fields:
            k=5;blend=max(k-abs(z-f),0)/k
            z=max(z,f)+blend*blend*k*.25
        return z
    for y in range(h):
        for x in range(w):
            if kind=='coat':
                dx=(depth(x+1,y)-depth(x-1,y))*.5;dy=(depth(x,y+1)-depth(x,y-1))*.5
                inv=1/math.sqrt(dx*dx+dy*dy+1)
                diffuse=max(0,(-dx*-.44-dy*-.67+.6)*inv)
                broad=noise(x*.075,y*.11)-.5
                bristle=noise(x*.35+y*.08,y*.72)-.5
                value=.46+.68*diffuse+.10*broad+.05*bristle
                warm=max(0,diffuse-.58)*.22
                shadow=max(0,.6-diffuse)*.15
                out=[rgb[j]*value+(245,210,160)[j]*warm+(31,47,58)[j]*shadow for j in range(3)]
            else:
                # Domain-warped pasture: large light/shade masses, then broken
                # grass strokes. Distance compression gives material recession.
                t=y/(h-1);scale=1.5+3*t
                u=x/(25*scale);v=y/(11*scale)
                n=noise(u+noise(u*.4,v*.5)*2,v)
                fine=noise(x*.42,y*.85)
                ridge=noise(x*.027,y*.016)
                value=.65+.48*n+.18*ridge+(fine-.5)*(.08+.13*t)
                out=[rgb[j]*value for j in range(3)]
            pix[x,y]=tuple(max(0,min(255,round(a))) for a in out)+(255,)
    bio=io.BytesIO();im.save(bio,format='PNG',optimize=False)
    uri='data:image/png;base64,'+base64.b64encode(bio.getvalue()).decode()
    _PAINT_CACHE[key]=uri
    return uri
_PAINT_CACHE={}
