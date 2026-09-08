#!/usr/bin/env python3
"""Compile the original V1 procedural sprite into native path operations.

Build-time only; stdlib. No SVG/HTML interpreter ships in the app. Interned
operations keep the complete breed/mane/tack/accessory/pose corpus compact.
Colors remain symbolic so all eight original coats share the same geometry.
"""
import argparse
import itertools
import json
import math
import re
from pathlib import Path
import xml.etree.ElementTree as ET
import horsesvg as source

POSES = [('stand', 'stand', False), ('working', 'working', False),
         ('blocked', 'blocked', False), ('done', 'done', False),
         ('unknown', 'unknown', False), ('graze', 'idle', False),
         ('alertStatic', 'blocked', True)]


def commands(d):
    tokens = re.findall(r'[A-Za-z]|[-+]?(?:\d*\.\d+|\d+)', d)
    out, i, x, y = [], 0, 0, 0
    arity = {'M': 2, 'L': 2, 'Q': 4, 'C': 6, 'Z': 0, 'z': 0, 'a': 7}
    while i < len(tokens):
        op = tokens[i]; n = arity[op]
        p = list(map(float, tokens[i+1:i+1+n])); i += n+1
        if op == 'a':
            # The sole original arc is the hat crown: upper semicircle.
            rx, ry, rotation, large, sweep, dx, dy = p
            assert rx == ry and dx == rx*2 and dy == rotation == large == 0 and sweep == 1
            k = rx * 0.5522847498307936
            out += [[3, x, y-k, x+rx-k, y-rx, x+rx, y-rx],
                    [3, x+rx+k, y-rx, x+dx, y-k, x+dx, y]]
            x += dx
        else:
            out.append([{'M':0, 'L':1, 'Q':2, 'C':3, 'Z':4, 'z':4}[op]] + p)
            if p: x, y = p[-2:]
    return out


def transforms(text):
    result = []
    for kind, values in re.findall(r'(\w+)\(([^)]+)\)', text):
        p = list(map(float, values.replace(',', ' ').split()))
        assert kind in ('translate', 'rotate', 'scale')
        result.append([kind, p])
    return result


def drawing(svg, grazing):
    result = []
    def visit(node, matrix, opacity):
        tag = node.tag.split('}')[-1]; a = node.attrib
        matrix = matrix + transforms(a.get('transform', ''))
        opacity *= float(a.get('opacity', 1))
        if 'leg ' in a.get('class', ''):
            angle = float(re.search(r'--a:([-\d]+)', a['style'])[1])
            matrix = matrix + [['rotate', [angle, 0, 0]]]
        if tag in ('svg', 'g'):
            for child in node: visit(child, matrix, opacity)
            return
        primitive = {'part': a.get('id', ''), 'transforms': matrix,
                     'fill': a.get('fill', 'none'), 'stroke': a.get('stroke', 'none'),
                     'width': float(a.get('stroke-width', 1)), 'opacity': opacity}
        if tag == 'path':
            primitive['commands'] = commands(a['d'])
            if grazing and a['d'].startswith('M 74 40'): primitive['part'] = 'grazing-neck'
        elif tag in ('circle', 'ellipse'):
            cx, cy = float(a['cx']), float(a['cy'])
            rx, ry = (float(a['r']),)*2 if tag == 'circle' else (float(a['rx']), float(a['ry']))
            primitive['ellipse'] = [cx-rx, cy-ry, rx*2, ry*2]
        elif tag == 'rect':
            primitive['rect'] = [float(a[k]) for k in ('x', 'y', 'width', 'height')]
            primitive['radius'] = float(a.get('rx', 0))
        elif tag == 'text':
            # Native accessible question cue overlays the original badge.
            primitive['text'] = node.text
            primitive['point'] = [float(a['x']), float(a['y'])-5]
        else:
            raise ValueError(f'unsupported original primitive: {tag}')
        result.append(primitive)
    visit(ET.fromstring(svg), [], 1)
    return result


def build():
    source.COAT = {key: ('body', 'dark') for key in source.COAT}
    source.MANE_C = {key: 'mane' for key in source.MANE_C}
    operations, lookup, scenes = [], {}, {}
    for breed, mane, tack, acc, blaze in itertools.product(range(3), range(3), range(3), range(3), range(2)):
        identity = dict(coat='chestnut' if blaze else 'bay',
                        breed=['light','stock','draft'][breed], mane=['flowing','braided','cropped'][mane],
                        tack=['saddle','pad','none'][tack], accessory=['bandana','hat','none'][acc])
        for pose, state, rm in POSES:
            ids = []
            for op in drawing(source.horse_svg(identity, state, rm), pose == 'graze'):
                key = json.dumps(op, sort_keys=True)
                if key not in lookup:
                    lookup[key] = len(operations); operations.append(op)
                ids.append(lookup[key])
            scenes[f'{breed}-{mane}-{tack}-{acc}-{blaze}-{pose}'] = ids
    return dict(version='original-v1', operations=operations, scenes=scenes)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(); parser.add_argument('output', type=Path)
    args = parser.parse_args(); args.output.parent.mkdir(parents=True, exist_ok=True)
    data = build(); args.output.write_text(json.dumps(data, separators=(',', ':'))+'\n')
    print(f'original-v1: {len(data["scenes"])} scenes, {len(data["operations"])} interned operations')
