#!/usr/bin/env python3
"""Contact sheet for inspection only; the numbered extracted PNGs stay native-size."""
import argparse
import hashlib
import json
from pathlib import Path
from PIL import Image, ImageDraw

parser = argparse.ArgumentParser()
parser.add_argument('directory', type=Path)
parser.add_argument('output', type=Path)
parser.add_argument('--frames', nargs='*', type=int)
parser.add_argument('--count', type=int, default=24)
parser.add_argument('--columns', type=int, default=6)
args = parser.parse_args()
files = sorted(args.directory.glob('[0-9][0-9][0-9][0-9].png'))
assert files, args.directory
selected = [args.directory / f'{number:04d}.png' for number in args.frames] if args.frames else [
    files[round(index * (len(files)-1) / (min(args.count,len(files))-1))]
    for index in range(min(args.count,len(files)))]
width = 220
with Image.open(selected[0]) as original:
    native_size = original.size
height = round(width * native_size[1] / native_size[0])
cell = (width, height+24)
sheet = Image.new('RGB', (cell[0]*args.columns, cell[1]*((len(selected)+args.columns-1)//args.columns)), 'white')
draw = ImageDraw.Draw(sheet)
manifest = []
for index, path in enumerate(selected):
    with Image.open(path) as original:
        assert original.size == native_size
        image = original.convert('RGB').resize((width,height))
    x, y = index % args.columns * cell[0], index // args.columns * cell[1]
    sheet.paste(image, (x,y+24))
    draw.text((x+3,y+5), path.name+' (4 fps sample)', fill='black')
    manifest.append({'path':str(path),'sha256':hashlib.sha256(path.read_bytes()).hexdigest()})
args.output.parent.mkdir(parents=True,exist_ok=True)
sheet.save(args.output)
args.output.with_suffix('.json').write_text(json.dumps({'native_size':native_size,'frames':manifest},indent=2)+'\n')
print(json.dumps({'sheet':str(args.output),'native_size':native_size,'count':len(selected)}))
