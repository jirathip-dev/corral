"""Capture actual wall-clock browser motion, preserving frame timestamps in MP4."""
import sys
sys.dont_write_bytecode=True
from browser import Browser,ROOT
from pathlib import Path
from PIL import Image,ImageDraw,ImageFont,ImageChops
import time,json,subprocess,shutil,hashlib
DURATION=12.0
report={'scope':'real-time browser playback; not SwiftUI/device evidence','timeCompression':False,'clips':[]}
with Browser() as b:
 for env in ['day','night']:
  b.load('variant-b.html',f'preview=herd&env={env}');b.js('PROOF.freeze(0)');b.js('PROOF.resume()')
  work=ROOT/f'tools/.runtime/clip-{env}';work.mkdir(parents=True,exist_ok=True)
  frames=[];start=time.monotonic();start_state=b.js('PROOF.state()')
  while time.monotonic()-start<DURATION:
   t=time.monotonic()-start;p=b.screenshot(str((work/f'frame-{len(frames):04}.png').relative_to(ROOT)))
   im=Image.open(p).convert('RGB');draw=ImageDraw.Draw(im);draw.rectangle((0,0,390,14),fill='#181825');draw.text((6,2),f'B / {env.upper()} / REAL TIME {t:04.1f}s / PROTOTYPE BROWSER PROOF',fill='#cdd6f4',font=ImageFont.load_default(size=9));im.save(p)
   frames.append((p,t));delay=start+(len(frames)*.1)-time.monotonic()
   if delay>0:time.sleep(delay)
  end=time.monotonic()-start;end_state=b.js('PROOF.state()');assert end_state['ticks']>start_state['ticks']
  concat=[]
  for i,(p,t) in enumerate(frames):
   duration=(frames[i+1][1] if i+1<len(frames) else end)-t
   concat.extend([f"file '{p.name}'",f'duration {duration:.8f}'])
  concat.append(f"file '{frames[-1][0].name}'")
  (work/'frames.txt').write_text('\n'.join(concat)+'\n')
  out=ROOT/f'evidence/{env}-realtime.mp4'
  command=['ffmpeg','-y','-hide_banner','-loglevel','error','-f','concat','-safe','0','-i',str(work/'frames.txt'),'-t',str(end),'-vf','fps=30','-c:v','libx264','-preset','fast','-crf','19','-pix_fmt','yuv420p','-movflags','+faststart','-an',str(out)]
  r=subprocess.run(command,capture_output=True,text=True);assert r.returncode==0,r.stderr
  probe=json.loads(subprocess.check_output(['ffprobe','-v','quiet','-show_streams','-show_format','-of','json',str(out)],text=True))
  duration=float(probe['format']['duration']);assert abs(duration-end)<.15
  # Retain actual first/end sampled frames for motion visual review, not interpolated art.
  shutil.copyfile(frames[0][0],ROOT/f'evidence/{env}-motion-start.png');shutil.copyfile(frames[-1][0],ROOT/f'evidence/{env}-motion-end.png')
  a=Image.open(frames[0][0]).convert('RGB');z=Image.open(frames[-1][0]).convert('RGB');bbox=ImageChops.difference(a.crop((0,15,390,844)),z.crop((0,15,390,844))).getbbox();assert bbox
  report['clips'].append({'environment':env,'file':str(out.relative_to(ROOT)),'duration':duration,'wallSeconds':end,'capturedFrames':len(frames),'captureHz':len(frames)/end,'outputHz':30,'note':'30 fps container holds timestamped browser frames; no interpolation or speed-up','clockStart':start_state,'clockEnd':end_state,'frameTimes':[round(t,4) for p,t in frames],'scenePixelDifferenceBox':bbox,'ffprobe':probe,'sha256':hashlib.sha256(out.read_bytes()).hexdigest()})
  (ROOT/'evidence/motion-verification.json').write_text(json.dumps(report,indent=2)+'\n');shutil.rmtree(work)
print(json.dumps([{k:v for k,v in c.items() if k in ['environment','duration','wallSeconds','capturedFrames','captureHz']} for c in report['clips']],indent=2))
