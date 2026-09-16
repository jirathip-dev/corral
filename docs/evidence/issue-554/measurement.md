# Issue 554 measurement scope and reproduction

Lane base: `97ed9e03cb4d265da02ac28bad1685000b159f80` (`g554-live-session-preflight`).
The #551 measurement harness is re-used **unchanged**:
`FleetNotifierTests/ForegroundReconnectTests.testWarmReturnStageMeasurements`
(env-gated by `G551_MEASURE=1`), five fresh XCTest processes, each one cold model
followed by 30 s and 300 s of real elapsed `.background` waiting through the
production scene seam. Only the driver (`measure.py`), the destination simulator
and the derived-data path are #554-owned; the measured method is not modified.

Run from the worktree root:

```
python3 docs/evidence/issue-547/bounded-run.py g554-measure-outer 3000 \
  flock /tmp/n.lock python3 docs/evidence/issue-554/measure.py
python3 docs/evidence/issue-554/timings.py /tmp/g554-measure.log > /tmp/g554-timings.json
```

The reducer computes per-sample intervals first, preserves negative path-ready
differences, then medians, and additionally compares each scenario's medians with
the #551 baseline recorded in
`docs/evidence/issue-551/simulator-timings.json`; the cold-launch comparison is
reported separately as the non-regression check this lane must show.

## What this can and cannot show

- The fixture is a local `URLProtocol` with immediate responses: **no transport
  delay is injected**, so this harness cannot reproduce or falsify the stale
  connection-pool warm return. It only shows the staged production timings are
  still recorded and that the cold-launch model stages did not regress.
- Warm paths exercise the real seam (`.background` → `stopLive`,
  `.active` → `startLive`) but the XCTest process is not suspended by iOS, so
  real OS background networking and socket reuse are out of scope.
- `cold_model` is a new `AppModel` in a fresh XCTest host, not the ordinary
  app's force-quit/reopen initialization.
- `row_visible` is a SwiftUI lifecycle/update hook, not GPU scan-out; a mounted
  retained row may not emit `retained_row_visible`, and a missing mark is not
  proof of blanking.
- No physical iPhone, no loaded Bazzite host and no Tailscale path is involved.
  The owner device gate stays unexecuted: `docs/evidence/issue-554/device-protocol.md`.

## Raw artifacts

| Artifact | What it holds |
| --- | --- |
| `/tmp/g554-measure.log` | five repetitions of the opt-in method, raw `G551_SAMPLE` lines |
| `/tmp/g554-timings.json` | reducer output incl. the #551 baseline comparison |
| `docs/evidence/issue-554/timings.json` | committed copy of the reducer output |
| `docs/evidence/issue-554/baseline-551-medians.json` | the #551 lane's medians, TRANSCRIBED from `docs/evidence/issue-551/lane-report.md` with provenance (not re-derived here) |
| `docs/evidence/issue-554/measure.py` | the driver this lane ran |
| `docs/evidence/issue-554/timings.py` | the reducer + baseline comparison |

## Measured result at this head

Sample counts: exactly `{cold_model: 5, warm_30: 5, warm_300: 5}` — staged
timings are still recorded; no sample was dropped or invented.

| Cold metric | #551 baseline | #554 head | delta |
| --- | ---: | ---: | ---: |
| `seam_entry_to_apply` | 111.149 | 42.087 | −69.062 |
| `apply_to_row` | 48.948 | 21.873 | −27.075 |
| `key_rtt` | 5.656 | 2.603 | −3.053 |
| `path_to_apply` | 5.015 | 2.363 | −2.652 |

All four cold-launch medians are BETTER than the baseline, i.e. **no cold-launch
regression**. Warm staging is flat: warm_30 medians moved sub-millisecond in both
directions (`key_rtt` 0.886 vs 0.928, `path_to_sse_200` 0.476 vs 0.514,
`frame_to_apply` 3.926 vs 3.779, `key_response_to_apply` 4.000 vs 3.840,
`path_to_apply` 4.619 vs 4.542, `seam_entry_to_apply` 8.450 vs 8.338 ms).
