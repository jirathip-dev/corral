# 530 evidence — crop / downscale provenance record

Every committed image is derived **only** from app screenshots of this lane's own simulator
(`082082C2-4B1E-4396-8374-25EE0CA51EB8`, iPhone 16, iOS 26.5, 1179x2556 px = 393x852 pt @3x). Raw
capture commands and exit codes are in `capture.log`. The raw fleet-side frames live (uncommitted)
at `/Users/jirathip/.config/fleet-operations/run/corral/evidence/impl530-hoof/`.

No frame, still, or derivative of the owner's video appears here; the video was inspected read-only
on the owning machine for diagnosis only.

## Downscaled phone-scale frames (`sips -z 844 390 <raw> --out <file>`, exit 0 each)

| committed file | raw source | raw source sha256 (first 16) | committed sha256 |
| --- | --- | --- | --- |
| `base-rail-390x844.png` | `herd-base-demo-1.png` | `efbcee3b15c622f7` | `aa4c3b7c33c36639` |
| `head-rail-390x844.png` | `herd-head-demo-1.png` | `b989e2c466ce076a` | `331a5c16613965db` |
| `base-working-draft-paddock-390x844.png` | `draft-base-1.png` | `01ae9b6d312d56fd` | `c0798b86dcba5076` |
| `head-working-draft-paddock-390x844.png` | `draft-head-1.png` | `f983e91f5b28bc72` | `026447632e615113` |

(The full sha256 of every committed file is in `SHA256SUMS.txt`.)

## Comparison panels (Pillow; left = base frame, right = head frame)

| committed file | sources | transform | sha256 |
| --- | --- | --- | --- |
| `compare-blocked-rail-overview.png` | `herd-base-demo-1.png` + `herd-head-demo-1.png` | whole-frame, divisor 2 (LANCZOS), pasted side by side with a 30 px white gap | `2fc21b5ff00e641ed273ca79b0246daccf909a867b71c3947d3ee1bd7ee4f279` |
| `compare-blocked-rail-feet-4x.png` | `herd-base-demo-1.png` + `herd-head-demo-1.png` | crop box `(160, 1050, 540, 1200)`, 4x NEAREST, 30 px gap | `9f84ac87a1a79c62b07f4512a69c4ddd6bcba078a8b772506507fbb6855ff0e6` |
| `compare-blocked-rail-leg-7x.png` | `herd-base-demo-1.png` + `herd-head-demo-1.png` | crop box `(280, 1040, 420, 1120)`, 7x NEAREST, 30 px gap | `b29ed2dd1654ad3b7697724fbba4b4cc126f36da70c5047377c6f2ffeb9f55f9` |
| `compare-working-draft-legs-4x.png` | `draft-base-2.png` + `draft-head-2.png` | crop box `(210, 1980, 510, 2130)`, 4x NEAREST, 30 px gap | `df029c502459fa017e3b123576282f4fb617e9549b20aa0c7c0285fba889409e` |

Implementation of the panels (the same loop the lane ran; Pillow is host-side tooling, not app
code):

```python
left, right = Image.open(base), Image.open(head)
left, right = left.crop(box), right.crop(box)
left  = left.resize((left.width * scale, left.height * scale), Image.NEAREST)   # LANCZOS + //2 for the overview
right = right.resize((right.width * scale, right.height * scale), Image.NEAREST)
combo = Image.new('RGB', (left.width * 2 + 30, max(left.height, right.height)), (255, 255, 255))
combo.paste(left, (0, 0)); combo.paste(right, (left.width + 30, 0)); combo.save(name)
```

Pixel coordinates are in the raw 1179x2556 frames. `NEAREST` is deliberate: it preserves the exact
painted pixels so a reviewer can compare band geometry without resampling artefacts; the two panels
are not phase-matched (the shared clock advances between launches), which is why every quantity in
`README.md` is a phase-invariant rotation/cosine invariant.
