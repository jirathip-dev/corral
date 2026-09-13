"""Contact sheets from actual verified PNGs; no resynthesized UI."""
from pathlib import Path
from PIL import Image,ImageDraw,ImageFont
ROOT=Path(__file__).resolve().parents[1];E=ROOT/'evidence'
groups={'comparison':['a-day-390x844.png','b-day-390x844.png','a-night-390x844.png','b-night-390x844.png'],'sheets':['b-filter-day-390x844.png','b-filter-night-390x844.png','b-settings-390x844.png','b-filter-type-end-390x844.png'],'edges':['b-offline-390x844.png','b-empty-390x844.png','b-type-390x844.png','b-dense-390x844.png']}
for name,files in groups.items():
 sheet=Image.new('RGB',(390*len(files),844),'#181825')
 for i,file in enumerate(files):sheet.paste(Image.open(E/file).convert('RGB'),(390*i,0))
 sheet.save(E/(name+'-contact.png'))
# View identical actual scene regions at 2x; PNG labels carry real elapsed times.
for env in ['day','night']:
 sheet=Image.new('RGB',(780,400),'#181825');d=ImageDraw.Draw(sheet)
 for i,key in enumerate(['start','end']):
  im=Image.open(E/f'{env}-motion-{key}.png').convert('RGB');crop=im.crop((195,260,390,450)).resize((390,380))
  sheet.paste(crop,(i*390,20));d.text((i*390+8,4),f'{env.upper()} real-time {key} | scene crop, uniform 2x',fill='#cdd6f4')
 sheet.save(E/f'{env}-motion-contact.png')
print('Contact sheets built from verified image paths')
