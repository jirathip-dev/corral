# Issue 551 — owner physical-iPhone capture protocol

NOT EXECUTED. On this lane `xcrun devicectl list devices` returned `No devices found.`
The tables below are intentionally empty. Simulator URLProtocol tests do not fill
this gate, and do not measure loaded Bazzite or the Mac daemon.

## Capture setup

1. Record the exact app source SHA, version/build, iPhone model/iOS version,
   daemon revision and host OS for each block. No release, TestFlight upload,
   daemon modification, key rotation, grant change or deployment is authorized
   by this protocol.
2. Use the same physical iPhone, network/VPN route, board display mode, expanded
   sections, scroll position and host population for all scenarios in a block.
   Pair through the normal supported flow. Keep at least one row visible.
   Measure Mac and loaded Bazzite in separate blocks with only the measured
   host configured: the logger's random attempt ID deliberately does not name
   a host. Do not try to identify mixed-host attempts by their timing order.
3. Record the owner-approved Bazzite workload command/configuration, keep it
   running unchanged throughout its block, and save its load telemetry. Record
   Mac workload/load too. No workload command was supplied to this lane; the
   owner must supply it rather than an implementer inventing a stress workload.
4. Connect the unlocked iPhone to the Mac. In Console select the iPhone, enable
   Info messages and start streaming. Filter subsystem `com.corral.fleetnotifier`
   and category `foreground-reconnect`. Save a private log capture for the whole
   block. Export only allowlisted timing messages, not personal board contents
   or unrelated device logs, into review evidence.
5. Logger records are `attempt=<UUID> stage=<name> uptime=<seconds>`. Existing
   release-active stage names are `path_ready`, `key_request`, `key_response`,
   `sse_200`, `frame_received`, `frame_applied`, `row_visible`, and the separate
   `retained_row_visible`. Group by attempt UUID, never combine host attempts.
   Save an independently recorded foreground/launch trigger time if reporting
   trigger-to-apply latency: `path_ready` is a path-monitor observation, not the
   foreground trigger, and can arrive after requests have already started.

## Sample sequence (repeat separately for each host)

Take five complete samples for each of the three scenarios. Retain every attempt,
failure, timeout and missing stage; do not replace failures silently. For each
sample start from a verified, populated board and confirm the measured host is
reachable. Use a repeatable owner-controlled board update to cause a data frame
if the daemon is idle; record the stimulus and apply it identically to every
scenario. An idle resumed stream can legitimately emit no frame, so a missing
frame is not a zero-duration result.

- Cold: force-quit Corral from the app switcher, reopen it, and record the first
  complete new attempt. This is a process cold launch, not a new AppModel inside
  an already-running XCTest host.
- Warm 30 s: from the verified board, switch to another app (actual background,
  not a transient inactive interruption), remain there for 30 elapsed seconds,
  return to Corral, and record its new attempt.
- Warm 5 min: repeat with 300 elapsed seconds in the other app.

Counterbalance the scenario order across repetitions. Record actual background
duration and any OS termination during the wait; a terminated process is not a
warm-return sample. For warm samples record the board just before background,
immediately after return while verification is pending, and after the first
verified frame. The first two boards must retain last-known agent states and
metadata; reconnecting/last-known is not Live. Note any `N unknown` flash and
whether unknown state arrived in a verified frame or appeared before one.

After single-host timings, separately check both hosts configured: delay or
make Bazzite unreachable and confirm Mac can apply independently. Do not pool
that check into the single-host timing medians.

## Analysis

For each sample, compute durations from its own monotonic timestamps, then use
`statistics.median` over the five per-sample durations for each metric. Do not
subtract medians of absolute timestamps. The key and SSE branches overlap:
these intervals are not a serial chain and must not be added together.

- Path observed -> key request (dispatch; retain negative values).
- Key request -> key response (key RTT).
- Path observed -> SSE 200 (stream establishment; retain negative values).
- SSE 200 -> first frame received.
- First frame received -> first accepted frame applied (includes verification).
- Key response -> first accepted frame applied (can include data wait).
- First accepted frame applied -> corresponding row visible.
- Path observed -> first accepted frame applied (explicitly a path-relative
  comparison, not launch-relative latency).
- Independently timed foreground/launch trigger -> first applied frame, only
  when the trigger timestamp exists in the same clock domain.

`row_visible` is the actual SwiftUI row appearance/revision-update hook, not GPU
scan-out. A collapsed/offscreen/coalesced row may not mark it. A retained row
already mounted may not emit another `retained_row_visible` mark at foreground;
record that as missing, not as blank UI. Verify retention separately in captures.
Do not invent timestamps for missing marks or exclude them without reporting
attempted and valid sample counts per metric.

The owner acceptance comparison is warm-30 median first-applied-frame <= cold
median * 1.20 on BOTH hosts. Report the 5-minute comparison separately. Identify
which measured stage explains any delta before authorizing a fix.

## Owner metadata (empty)

| Host | App SHA/build | iPhone/iOS | Daemon/OS | Network route | Workload command/config | Load evidence |
| --- | --- | --- | --- | --- | --- | --- |
| Mac | | | | | | |
| Loaded Bazzite | | | | | | |

## Owner results (empty; milliseconds)

| Host | Scenario | Attempted/valid n | Dispatch | Key RTT | Path->200 | 200->frame | Frame->apply | Key response->apply | Apply->row | Path->apply | Trigger->apply | Failures/missing |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Mac | Cold | | | | | | | | | | | |
| Mac | Warm 30 s | | | | | | | | | | | |
| Mac | Warm 300 s | | | | | | | | | | | |
| Loaded Bazzite | Cold | | | | | | | | | | | |
| Loaded Bazzite | Warm 30 s | | | | | | | | | | | |
| Loaded Bazzite | Warm 300 s | | | | | | | | | | | |

| Host | Warm-30 / cold ratio | <= 1.20 | Implicated stage | Retention before verification | Never Live while pending | Capture paths |
| --- | --- | --- | --- | --- | --- | --- |
| Mac | | | | | | |
| Loaded Bazzite | | | | | | |
