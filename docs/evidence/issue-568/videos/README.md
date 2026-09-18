# #568 gesture video set (committed, size-bounded)

These are compressed transcodes of real-gesture simulator recordings made by the #568 lane (XCUITest
drag/flick/reversal/overscroll + `xcrun simctl io <udid> recordVideo`, two lane-owned simulators: iPhone SE 3rd
gen for `short`, iPhone 16 Pro Max for `tall`, iOS 26.5).

Raw clips (durable, outside the repo by design because of size) live at
`/Users/jirathip/.config/fleet-operations/run/corral/evidence/issue-568/videos/{short,tall}/`; every raw clip's
sha256 is recorded in `manifest.json` (and in the lane report). Each committed clip derives from exactly that raw
file (hashed at transcode time), not from a separate capture.

Transcode (per clip, ffmpeg 9.0.1):

    ffmpeg -v error -y -i <raw.mov> -an -vf "scale=430:-2,fps=15" -c:v libx264 -preset medium -crf 28 \
        -pix_fmt yuv420p -movflags +faststart <run>-<case>.mp4

Included (10 clips, total 5.94 MB):

| viewport | case | committed size | raw size | raw sha256 |
|---|---|---|---|---|
| short | `day-empty` | 0.485 MB | 32.6 MB | `d33d983ef0358700a72a2afc740e0f246e7872bc9680c668cbb272c95ed86401` |
| short | `day-blocked` | 0.538 MB | 27.8 MB | `c48912c80d644bac30f66367c7c3ded4441ad48dd5aea3f8410c135c3ba87609` |
| short | `night-empty` | 0.521 MB | 33.8 MB | `7d119b804777637fb6737ab6a3310770328b196bafa389260ea936265f83a579` |
| short | `night-blocked` | 0.535 MB | 32.0 MB | `27aca59301404854ef6a8ef1e5dc4ccdcc278e6a4067d0be74a9cab9414dd903` |
| short | `day-empty-taps` | 0.143 MB | 7.4 MB | `6f5ce58f60cbd07a3ef4be0edf771e16a51a4caa8f6b637893eaf52d8739b614` |
| short | `day-blocked-accessibility` | 0.081 MB | 2.6 MB | `bfc756f9f040ef7a0e4ab4c724288c77f294cc3c4ecd8fb050d57eca9d6eb4a7` |
| tall | `day-empty` | 0.809 MB | 93.3 MB | `11c6f3571f786ef67244fee74f0b9842d0c480bc7aab117e7365a16534d7b223` |
| tall | `day-blocked` | 0.963 MB | 98.2 MB | `9d33eed1a33c9b19b17942e44a73a4e9ba770068a2352ec6ba425142c4ae678b` |
| tall | `night-empty` | 0.929 MB | 93.7 MB | `27e234709c91361b9c5f3a66f57e927ce68629d198624521e1723a13f852046e` |
| tall | `night-blocked` | 0.935 MB | 90.9 MB | `bf499a88028388ee0d2ad3fc9728d77df6583b24a46d8c1349c57d9be70f17ba` |

Coverage: both vertical edges (each edge case runs top and bottom overscroll/reversal/settle), Day and Night,
blocked and empty rail, short and tall viewports, plus the tap-exclusion case and the oversized/Reduce-Motion case.

Legibility check: `legibility-check-tall-day-empty-fade.png` is a strip from `tall-day-empty.mp4` (t=23.5..27.0 s)
showing the last group translucent/faded at the bottom edge (t=24.5, 25.0) before it settles complete and opaque.

Omitted clips and why: `day-empty-reduced` (a Reduce Motion repeat of `day-empty` on the same viewport), `day-single`
and `day-no-field` (static single-row / empty-paddock fixtures, least information per byte). Their raw clips remain
in the durable raw directory with hashes recorded in the lane report.

Limitation (unchanged, applies to every clip): the gestures are synthetic XCUITest input; timing and velocity are
real but kinematic identity with a human drag is not proven. Simulator-only; the physical-device gate is the owner's.
