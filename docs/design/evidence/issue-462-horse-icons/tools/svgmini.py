#!/usr/bin/env python3
"""issue-462 — minimal deterministic renderer for the CANONICAL horsesvg output.

Imports ios/tools/herd-art/horsesvg.py (the canonical generator, unmodified)
and renders its SVG string to Pillow primitives. No art is re-authored here:
this file only interprets the exact path/ellipse/rect data the canonical
generator emits (same parse contract as the shipping export.py).

Supported (superset asserted): path M/L/Q/C/Z absolute, ellipse, circle, rect
(rx), group transform translate/rotate/scale, fill/stroke/stroke-width,
opacity, per-leg style --a rotation. Renders with 24-segment Bezier
flattening at 2x supersampling — deterministic output.
"""
from __future__ import annotations

import math
import re
import xml.etree.ElementTree as ET

BEZIER_SEGS = 24          # subdivisions per Q/C segment
ARC_SEGS = 8              # per rounded-rect corner


# ---------- affine matrices (a b c d e f; x' = a*x + c*y + e) ----------

def m_id():
    return (1.0, 0.0, 0.0, 1.0, 0.0, 0.0)


def m_mul(m, n):
    a1, b1, c1, d1, e1, f1 = m
    a2, b2, c2, d2, e2, f2 = n
    return (a1 * a2 + c1 * b2,
            b1 * a2 + d1 * b2,
            a1 * c2 + c1 * d2,
            b1 * c2 + d1 * d2,
            a1 * e2 + c1 * f2 + e1,
            b1 * e2 + d1 * f2 + f1)


def m_apply(m, x, y):
    a, b, c, d, e, f = m
    return (a * x + c * y + e, b * x + d * y + f)


def parse_transforms(text):
    out = []
    for kind, values in re.findall(r'(\w+)\(([^)]+)\)', text):
        p = [float(v) for v in re.findall(r'[-+]?(?:\d*\.\d+|\d+)(?:[eE][-+]?\d+)?', values)]
        if kind == 'translate':
            ty = p[1] if len(p) > 1 else 0.0
            out.append((1.0, 0.0, 0.0, 1.0, p[0], ty))
        elif kind == 'scale':
            sx = p[0]
            sy = p[1] if len(p) > 1 else sx
            out.append((sx, 0.0, 0.0, sy, 0.0, 0.0))
        elif kind == 'rotate':
            t = math.radians(p[0])
            ct, st = math.cos(t), math.sin(t)
            rot = (ct, st, -st, ct, 0.0, 0.0)
            if len(p) == 3:
                out.append(m_mul(m_mul(_t(p[1], p[2]), rot), _t(-p[1], -p[2])))
            else:
                out.append(rot)
        else:
            raise AssertionError(f'unsupported transform {kind}')
    return out


def _t(x, y):
    return (1.0, 0.0, 0.0, 1.0, x, y)


# ---------- path flattening ----------

def _tokens(d):
    return re.findall(r'[A-Za-z]|[-+]?(?:\d*\.\d+|\d+)(?:[eE][-+]?\d+)?', d)


def flatten_path(d):
    """Absolute M/L/Q/C/Z only (canonical horsesvg emits no relatives, no
    arcs — arcs exist solely on the hat accessory, asserted absent)."""
    tokens = _tokens(d)
    pts, subpaths = [], []
    i, x, y = 0, 0.0, 0.0
    start = (0.0, 0.0)
    while i < len(tokens):
        op = tokens[i]
        i += 1
        if op == 'M':
            x, y = float(tokens[i]), float(tokens[i + 1]); i += 2
            start = (x, y)
            if pts:
                subpaths.append(pts)
            pts = [(x, y)]
        elif op == 'L':
            x, y = float(tokens[i]), float(tokens[i + 1]); i += 2
            pts.append((x, y))
        elif op == 'Q':
            cx, cy, px, py = (float(tokens[i + k]) for k in range(4)); i += 4
            for s in range(1, BEZIER_SEGS + 1):
                t = s / BEZIER_SEGS
                mt = 1 - t
                pts.append((mt * mt * x + 2 * mt * t * cx + t * t * px,
                            mt * mt * y + 2 * mt * t * cy + t * t * py))
            x, y = px, py
        elif op == 'C':
            c1x, c1y, c2x, c2y, px, py = (float(tokens[i + k]) for k in range(6)); i += 6
            for s in range(1, BEZIER_SEGS + 1):
                t = s / BEZIER_SEGS
                mt = 1 - t
                pts.append((mt ** 3 * x + 3 * mt * mt * t * c1x + 3 * mt * t * t * c2x + t ** 3 * px,
                            mt ** 3 * y + 3 * mt * mt * t * c1y + 3 * mt * t * t * c2y + t ** 3 * py))
            x, y = px, py
        elif op in ('Z', 'z'):
            pts.append(start)
            x, y = start
        else:
            raise AssertionError(f'unsupported path op {op!r} (relative/arc data must not appear)')
    if pts:
        subpaths.append(pts)
    return subpaths


def _rounded_rect(x, y, w, h, rx):
    rx = min(rx, w / 2, h / 2)
    pts = []
    corners = [(x + w - rx, y + rx, 0.0), (x + w - rx, y + h - rx, math.pi / 2),
               (x + rx, y + h - rx, math.pi), (x + rx, y + rx, 1.5 * math.pi)]
    for cx, cy, a0 in corners:
        for s in range(ARC_SEGS + 1):
            a = a0 + (ARC_SEGS and (s / ARC_SEGS) * (math.pi / 2))
            pts.append((cx + rx * math.cos(a), cy + rx * math.sin(a)))
    return [pts]


# ---------- SVG walk ----------

def parse_primitives(svg_text):
    """Return [{'group', 'kind', 'pts'|..., 'fill', 'stroke', 'width',
    'opacity'}] in canonical 132x100 user units, transforms applied."""
    root = ET.fromstring(svg_text)
    prims = []

    def visit(node, matrix, opacity, group):
        a = node.attrib
        matrix = matrix
        t = a.get('transform')
        if t:
            for tm in parse_transforms(t):
                matrix = m_mul(matrix, tm)
        cls = a.get('class', '')
        if 'neck' in cls:
            group = 'neck'
        elif 'leg' in cls:
            style = a.get('style', '')
            ang = float(re.search(r'--a:([-+\d.]+)', style).group(1))
            matrix = m_mul(matrix, parse_transforms(f'rotate({ang})')[0])
        opacity = opacity * float(a.get('opacity', 1))
        tag = node.tag.split('}')[-1]
        if tag in ('svg', 'g'):
            for child in node:
                visit(child, matrix, opacity, group)
            return
        prim = {'group': group, 'fill': a.get('fill', 'none'),
                'stroke': a.get('stroke', 'none'),
                'width': float(a.get('stroke-width', 1.0)),
                'opacity': opacity}
        if tag == 'path':
            subs = flatten_path(a['d'])
            prim['kind'] = 'poly'
            prim['subpaths'] = [[m_apply(matrix, px, py) for px, py in sp] for sp in subs]
        elif tag in ('ellipse', 'circle'):
            if tag == 'circle':
                cx, cy, rx, ry = float(a['cx']), float(a['cy']), float(a['r']), float(a['r'])
            else:
                cx, cy = float(a['cx']), float(a['cy'])
                rx, ry = float(a['rx']), float(a['ry'])
            pts = [(cx + rx * math.cos(2 * math.pi * s / 64),
                    cy + ry * math.sin(2 * math.pi * s / 64)) for s in range(64)]
            prim['kind'] = 'poly'
            prim['subpaths'] = [[m_apply(matrix, px, py) for px, py in pts]]
        elif tag == 'rect':
            x, y = float(a['x']), float(a['y'])
            w, h = float(a['width']), float(a['height'])
            rx = float(a.get('rx', 0))
            subs = _rounded_rect(x, y, w, h, rx)
            prim['kind'] = 'poly'
            prim['subpaths'] = [[m_apply(matrix, px, py) for px, py in sp] for sp in subs]
        elif tag == 'text':
            raise AssertionError('text primitive outside expected corpus')
        else:
            raise AssertionError(f'unexpected tag {tag}')
        prims.append(prim)

    visit(root, m_id(), 1.0, 'body')
    return prims


def bbox(prims, exclude_groups=()):
    xs, ys = [], []
    for p in prims:
        if p['group'] in exclude_groups:
            continue
        for sp in p['subpaths']:
            for px, py in sp:
                xs.append(px); ys.append(py)
    return min(xs), min(ys), max(xs), max(ys)
