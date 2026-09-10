# #459 wind/foliage evidence — coherent tree canopy + grass motion

Real-time, phone-scale captures of the actual app (Debug build) on an owned
iPhone 16 simulator (UDID `59DDC0C5-891E-4EC0-91AF-4F50DF68D793`, iOS 26.5),
herd scene, Day environment, fictional `-demoMode` fleet — no live daemon, no
physical device, no TestFlight claim.

The captures answer the owner's "I could not see motion" requirement: the
recorded scene is the SHIPPING renderer path, and the same sampled window is
measured on the base build (`8671cfc`, static canopy) and on the candidate.

## Artifacts

| File | What it is |
|---|---|
| `wind-candidate.mp4` | candidate herd scene, 20 s real time (2 full wind cycles visible) |
| `wind-base-static.mp4` | base build (static canopy), same scene/args, 21 s |
| `wind-reduce-motion-static.mp4` | candidate + forced Reduce Motion (still scene), 13 s |
| `screens/wind-candidate-1.png`, `-2.png` | full-res 1178×2556 stills 2.6 s apart (one gust half-cycle) |
| `wind-candidate-diff.png` | per-pixel |Δ| map of the two stills (×3 gain) |
| `wind-base-diff.png`, `wind-reduce-motion-diff.png` | same map for the base / Reduce-Motion controls |
| `measurements/*.json` | the exact numeric outputs quoted below |

The `.mp4` files are CRF-20 H.264 re-encodes (preset slow) at the native
1178×2556 capture size; the raw `simctl recordVideo` captures are preserved
under `/tmp/g459-{candidate,base,reduce}-clean.mp4`. Re-encoding does not
change the verdict: the candidate still shows the canopy band moving, the
base still shows 1 changed pixel there, Reduce Motion still shows 0.

Capture command (one build, one lock hold, sim boot→shutdown):

```
xcrun simctl io <udid> recordVideo --codec h264 --force out.mp4 &
xcrun simctl launch <udid> com.corral.fleetnotifier -demoMode \
    -fleetnotifier.fleetPresentation Herd -herdEnvironment Day
```

Two launch facts are load-bearing (both are pre-existing product behavior):

* `-fleetnotifier.fleetPresentation Herd` + `-herdEnvironment Day` are real
  UserDefaults keys; the argument domain selects the Herd scene and Day light
  without editing any source.
* An unanswered notification-permission alert (left in the simulator by an
  earlier **test-host** run; not by the app launches themselves) makes the
  scene `inactive`, which correctly stops the clock and freezes the scene.
  The #415/#458 demo-evidence carve-out
  (`-corral458BoardReloadScenario` → `suppressOSNotificationPromptForDemoEvidence`)
  prevents it; captures were taken with that arg and the frame set contains no
  alert.

## Measured results

Full-resolution still pair, 2.6 s apart (screen rows are 3× device pixels):

| metric | candidate | base (static) | Reduce Motion |
|---|---:|---:|---:|
| changed pixels (whole screen) | 20 675 / 3 013 524 | 14 713 / 3 013 524 | **0** |
| changed pixels, canopy band rows 1080–1440 | **8 173** / 424 440 | **0** / 424 440 | **0** |
| changed pixels, rows ≥1740 (horse grid area) | 12 502 | 14 713 | 0 |

The candidate's extra motion is exactly the tree-canopy band (art y≈219–262
→ screen rows ≈1095–1216), with zero changed pixels in rows 1440–1740 and in
the base's canopy band: the base has no canopy motion at all. The horse-grid
rows move in both builds (live demo fleet; the per-run counts differ only
because the two runs sample different points of the horses' roam cycles),
and nothing moves anywhere under Reduce Motion.

Video window (12 s, frames extracted at 2.5 fps and analysed at 294×639):

| metric | candidate | base | Reduce Motion |
|---|---:|---:|---:|
| mean changed pixels between consecutive frames | 558.6 | 274.1 | **0.0** |
| changed pixels over one half wind cycle | 1 602 | 1 109 | 0 |
| changed pixels, canopy band rows 260–380 | **629** / 35 280 | **1** / 35 280 | **0** |

CPU / cadence (host `ps %cpu` of the app process; dense 60-agent evidence
scene plus a steady demo window; see `measurements/cpu-cadence.json`):

| window | candidate | base |
|---|---:|---:|
| steady active demo scene (~17 s) | 16.70 % | 16.16 % |
| live phase 01–03 (dense fleet, clock on) | 10.64 % | 10.31 % |
| phase 07 live dense (~2.2 s, 3–4 samples) | 19.27 % | 27.05 % |
| frozen dense phase 06 (reduce-motion posture) | 9.37 % | 9.70 % |

The wind adds ≈0.3–0.5 percentage points of one app process (~3 %) in the
steady live windows — inside sample noise for the simulator; the 2.2 s live
dense window is too short to separate signal (its spread is dominated by the
scene transition). Simulator CPU is not device energy.

Clock cadence and shutdown, read from the app's own phase markers
(`measurements/markers-{cand,base}.json`): live phases measure 9.56–10.05 Hz
(the existing single 100 ms `HerdClock`); Reduce-Motion phases hold ticks
frozen (344); after dismissal `clockRunning=false` with ticks frozen at 382 —
no persistent timer survives the scene.

## Reproduce

```
ios/evidence/issue-459-wind/drivers/capture-wind.sh <udid> <FleetNotifier.app> out.mp4 20
ios/evidence/issue-459-wind/drivers/measure-wind.py pair <a.png> <b.png> --label cand --outdir out/
ios/evidence/issue-459-wind/drivers/measure-wind.py clip out.mp4 --start 4 --duration 12 --label cand --outdir out/
```

`measure-wind.py` is stdlib+ffmpeg only; its synthetic self-test output was
validated before use.

## Honest limitations

* Simulator only: no physical-iPhone smoothness, thermal or battery claim.
  Physical review remains the human gate recorded in the issue.
* Ambient updates ride the one 100 ms scene clock (~9.6–10 Hz measured), the
  point the issue names as "the starting point": raising it needs the
  HerdView-level invalidation isolation the #459 file fence excludes; the
  recorded motion is driven by amplitude/period, not by frame rate.
* The measured windows are short real-time slices of a demo fleet; they are
  not a device performance certification.
