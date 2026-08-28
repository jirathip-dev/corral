# Issue #210 — fleet-health status strip (HEALTH ONLY, no spend)

> Superseded by issue #289. This is historical evidence only; the strip and
> its snapshot aggregation were removed from production clients and daemon.

Evidence for the compact per-fleet health strip on both clients. The strip
shows, per fleet: **orch alive?**, **live worker count**, **last heartbeat
age**, plus a **warning indicator** when a fleet is degraded/stale (missing
orch, stalled heartbeat, declared-but-absent workers) that reads as HEALTH,
never as a stall accusation. No spend/balance numbers appear anywhere.

## Data source

Read-only aggregation over existing herdr/fleet signals, computed at
snapshot-assembly time and carried on the snapshot as `fleet_health`:

- fleet roster: the fleet-ops CLI validated identity catalog
  (`herdr-fleet list`) — already Corral's only identity path (#237);
- live agent state: herdr's trusted catalog (`agent.list` 2s refresh) and
  pane events — already Corral's read model;
- heartbeat: the daemon's per-agent presence observation
  (`Adapter::last_seen_millis`, stamped by the herdr adapter whenever an
  agent is seen; epoch-millis anchor so clients render a ticking age
  locally). No new backend state is persisted; no controller changes.

## Files

| File | What it shows |
|---|---|
| `board-demo-strip.png` (1400x900) | egui web board (WASM demo, bundled fixture): strip above the master list — `● corral orch ✓ 4w ♥4s`, `● synergy orch ✓ 1w ♥12s`, `⏸ plush orch ✗ 0w ♥— paused` |
| `ios-demo-strip-390x844.png` | iPhone 16 simulator demo (`-demoMode`): same strip at the top of the Fleet screen — healthy teal pill + orange degraded `⚠ sendmeter` pill (horizontal scroll shows the rest) |
| `live-fleet-health.json` | Real `GET /snapshot` from a scratch corrald built from this worktree, reading the live herdr socket (`agent.list`) + the real fleet registry: 9 rows, live orch/workers/heartbeat anchors, paused fleets (plush, synergy-website, agent-fleet-doctrine, morsel) suppressed-warning |

## Capture commands (exact)

```
# Board (WASM demo): fixture ships pre-aggregated fleet_health; the demo
# board renders it with no daemon anywhere.
cd clients/egui && wasm-pack build --target web --out-dir pkg --out-name corral-web
cp web/index.html pkg/ ; serve pkg/ on :8155
chrome-headless-shell --remote-debugging-port=9333 ... (CDP daemon, ui-screenshot skill)
cdp_demo.py: seed localStorage corral_web_setup_v1 {"mode":"demo",...}, navigate, settle 9s,
Page.captureScreenshot -> board-demo-strip.png (2800x1800 @2x, shown 1400x900)

# iOS: same build path as the gate
HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 xcodebuild test \
  -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier \
  -destination "platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793" \
  -derivedDataPath /tmp/fn-dd -only-testing:FleetNotifierTests
xcrun simctl boot 59DDC0C5-891E-4EC0-91AF-4F50DF68D793
xcrun simctl install <udid> /tmp/fn-dd/Build/Products/Debug-iphonesimulator/FleetNotifier.app
xcrun simctl launch <udid> com.corral.fleetnotifier -demoMode ; sleep 8
xcrun simctl io <udid> screenshot ios-raw.png
sips -z 844 390 ios-raw.png --out ios-demo-strip-390x844.png   # 0.18% aspect distortion (documented convention)

# Live aggregation proof (scratch daemon reads the LIVE herdr socket; the
# production corrald on 8474 was never touched — established lane pattern):
CORRAL_CONFIG_DIR=$(mktemp -d) ./target/debug/corrald \
  --socket ~/.config/herdr/herdr.sock --bind 127.0.0.1 --port 8599 &
curl -s http://127.0.0.1:8599/snapshot > live-fleet-health.json
```

## SHA-256

```
e305bc7ba76f08c4c7ac7274c0d872e0a71ea655783fe0fbbb7540e670cbf35c  board-demo-strip.png
3195a8f6eeffb0791f14d880088a6d28dc6b149c99be132b788645d1c4aeb97e  ios-demo-strip-390x844.png
abcae48f96d3ca2436b06f9fee8c325d79208803a4989281c439f658ae4556c9  live-fleet-health.json
```

## Design notes / trade-offs

- The strip's data is snapshot-carried (`fleet_health` on `/snapshot` and
  SSE full resnapshots). Deltas do not carry health: between snapshots the
  clients age the heartbeat locally from the epoch anchor, so the strip
  ticks in place without SSE churn (no per-agent delta at 2s cadence).
- Warning semantics: `orch_missing`, `heartbeat_stale` (>60s without an
  observation — the trusted catalog refreshes every 2s), `workers_missing`
  (registry declares workers but none live). Paused fleets never read as
  degraded — they are parked by design (⏸ muted).
- Worker membership: an agent belongs to a fleet via its orch's repo group
  (exact even where the fleet name differs from the checkout dir, e.g.
  `synergy` -> `synergy-costing`); with a missing orch the fallback repo
  spellings are the CLI-validated name + gh_repo basename, so a missing
  orch degrades conservatively (workers may read 0 until it returns).
- The aggregation NEVER carries spend/balance state; the wire shape is
  pinned by `tests/http.rs` `snapshot_carries_fleet_health_aggregation`.
