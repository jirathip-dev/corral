# 530 evidence — hoof-to-leg correspondence (draft fetlock tufts)

Owner-observed defect: with the running gait, some horses' hoof/foot shapes are not aligned with
the ends of their legs. Root cause (issue #530): `HerdArt.drawing` rotated each breed-2
(draft) **fetlock tuft** band by the gait `step` term alone instead of that leg's whole angle
(`rest ± step`), so the pose's rest angle dropped out of the tuft transform and the band sat off
its leg's distal end by up to 11.98 pt (working), 11.18 pt (blocked) and 3.56 pt (unknown) — at
**every** gait phase; the poses with a zero rest angle (stand, done, graze, alert-static) happened
to stay aligned, which is why the owner saw it only on some horses/poses. The real hoof bands
(`leg-N-hoof`) were rigidly attached to their legs in every variant.

Each `compare-*.png` panel is a **pre-fix (left) vs fixed (right)** pair from two app builds of the
same content (see `capture.log` for the build, device, install/launch/screenshot commands and raw
exit codes; `README-derived-crops.md` for crop transforms and sha256s).

## Devices and app/tree identity

* Simulator `082082C2-4B1E-4396-8374-25EE0CA51EB8` — iPhone 16, iOS 26.5, created for this lane,
  booted; **393x852 pt = 1179x2556 px @3x**. Frames downscaled to the repo's evidence convention
  with `sips -z 844 390` (0.18 % aspect distortion).
* **base build** = pre-fix renderer, `ios/FleetNotifier/UI/Herd/HerdArt.swift` sha256
  `08f63618ec4bfe41f5f398f0f5177d9546e7b58f6cf71f7a6eb0b7e4373643e4` (= `origin/integration`
  `1d774d1f5d5270193b028f29f36670eac3fbabfb` at that path).
* **fixed build** = implementation head `dcb7d47c0b9ce950f35132ec81dc1fd339824bf5`
  (`HerdArt.swift` sha256 `36e87d415b4841ecbedbd5db628f96117201efcea738267a7984846398ae23c7`); this
  evidence commit adds files under `ios/evidence/` only, so the app/test/tool sources are
  byte-identical to `dcb7d47`.
* Both builds are DEBUG app products from the lane's own test runs; launches are the app's
  DEBUG-gated demo drivers on the fictional demo fleet — **no live daemon, no real hosts, no
  private rows**, no physical-device or TestFlight claim.

## Frames

| File | What it shows |
| --- | --- |
| `base-rail-390x844.png` | Pre-fix app, `-demoMode -corral458HerdScenario`: phone-scale frame of the global blocked rail (`! FRONT RAIL · 2 BLOCKED`) carrying `demo-garden-agent` (chestnut, **draft**) in the affected **blocked** pose. |
| `head-rail-390x844.png` | Same screen, same launch arguments, fixed build. |
| `base-working-draft-paddock-390x844.png` | Pre-fix app, `-demoMode -corralDemoMultiHostBoardEvidence -corral458HerdScenario`: the `demo-atlas` paddock with `demo-atlas-worker` (dun, **draft**, **working**) drawn unoccluded in the paddock. |
| `head-working-draft-paddock-390x844.png` | Same screen, fixed build. |
| `compare-blocked-rail-overview.png` | Both rail frames side by side (half scale) for context. |
| `compare-blocked-rail-feet-4x.png` | 4x nearest crop of the blocked chestnut draft horse's feet at the rail: pre-fix shows a dark band floating beside each raised leg's end; fixed shows one band per leg end. |
| `compare-blocked-rail-leg-7x.png` | 7x nearest crop of the raised near-front leg: pre-fix band is offset from the leg's end; fixed band sits on it. (The front-rail overlay clips the band's lower edge — see Honest limits.) |
| `compare-working-draft-legs-4x.png` | 4x nearest crop of the working dun draft horse's legs (unoccluded): pre-fix frames show an extra dark band detached from each leg end; fixed frames show exactly one band at each leg end. |

## Measurements

| File | What it quantifies |
| --- | --- |
| `actual-app-tuft-measurement.json` (+ the same text in `measurement.log`) | Blocked chestnut draft rail horse: art→pixel scale derived from the frame is **2.9951 px/pt**; the pre-fix tuft centroid matches the **rest-dropped (angle-0) prediction within 0.1 px** and lies **10.34 pt** from the leg's aligned position; the fixed tuft's band top edge matches the aligned prediction within **0.4 px**. |
| `actual-app-working-draft-tuft.json` | Unoccluded working dun draft horse, per frame: gap between each fetlock band and its leg's paint — pre-fix **7.1 px / 9.2 px** and **0.0 px / 2.2 px** (2.4 / 3.1 and 0.0 / 0.7 pt of visible background), fixed **0.0 px in all four bands** (band touching its leg). |
| `audit-gait-hoof.txt` / `audit-gait-hoof.json` | Pre-fix classification of the whole matrix (3 breeds x 7 poses x 17 gait samples): draft `working` **MISALIGNED 11.98 pt**, `blocked` **11.18 pt**, `unknown` **3.56 pt** (canonical 2.5 pt), aligned for zero-rest poses and `n/a` for light/stock (no tuft ink); hoof bands aligned in every sample. |
| `probe-gait-summary.txt` | The assertion-level mutation battery's arm table (raw exits) and the mutation RED: **1360 XCTest assertion failures** (680 `must sit on its leg's axis` + 680 `must share the hoof's axis`) when the pre-fix swing-only tuft rotation is restored, with `hoof-digest-under-defect-green` = 0 in the same mutated tree and `restored-green` = 0. Raw logs identified by sha256 inside. |
| `measurement.log` | stdout of the three shipped helpers, re-run against the raw fleet-side frames (commands quoted at the top). |
| `capture.log` | Build/install/launch/screenshot/sips commands and every raw exit code, plus the simulator identity. |

## Helpers (evidence-side analysis, not app/tool gates)

* `measure-actual-app.py` — rail-horse measurement: derives the art→pixel mapping from the two
  identical tuft anchors, then compares the observed tuft centroid with both candidate predictions.
* `measure-working-draft-tuft.py` — paddock-horse metric: per frame, the minimum distance from each
  mane-coloured band to coat-coloured leg/hoof paint inside the horse's region.
* `audit-gait-hoof.py` — mirrors the renderer's leg/tuft transforms to classify every
  breed/pose/phase sample (the numbers in `audit-gait-hoof.txt`).

## Honest limits

* These are **simulator** frames; no physical-device or owner acceptance is claimed (conductor
  owned). Touch injection is unavailable on `simctl`, so no frame is an injected tap — the demo
  launch arguments are the app's own DEBUG drivers.
* The blocked rail horse's band is partly occluded by the `RanchFrontRail` overlay, which is why its
  fixed-arm centroid residual is 2.5 pt rather than ~0; the unoccluded paddock measurement
  (0.0 px gap in every band) and the native test matrix carry the precision.
* Screenshot-time gait phase is whatever the shared clock showed (not injectable), so the base and
  fixed panels are not phase-matched; every metric used here is phase-invariant (rotation /
  cosine invariants) and the native matrix samples the phases explicitly.
* **No frame, still, or derivative of the owner's video is used or delivered.** The video was
  inspected read-only on the owning machine for diagnosis only; every image here is a screenshot of
  the app itself.
* The raw fleet-side capture outputs (50 frames + logs) live outside the repo at
  `/Users/jirathip/.config/fleet-operations/run/corral/evidence/impl530-hoof/`; this directory is the
  curated subset (composition notes in `capture.log`).
