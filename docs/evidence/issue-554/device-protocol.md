# Issue 554 — owner warm-return gate (physical iPhone, NOT EXECUTED)

NOT EXECUTED. On this lane `xcrun devicectl list devices` reported no device, and
this lane made no device, TestFlight, release, daemon or Tailscale action. The
tables below are intentionally EMPTY. Every number in this repository's #554 lane
report is a simulator or host measurement, which cannot fill this gate.

## Owner gate, as specified in the issue

10 warm returns after >= 60 s in the background, over Tailscale to the loaded
Bazzite host; **none** may exceed 2 s to the first *verified* frame, and the
"many seconds" outliers must be gone. A verified frame means a frame whose host
identity matched the pinned key and that was applied after `keyContinuityState`
reached `.verified` — a retained/last-known board does not count.

## Capture setup

1. Record the exact app source SHA, version/build, iPhone model/iOS version,
   daemon revision, host OS and Tailscale route for every block. This protocol
   authorizes no release, TestFlight upload, daemon modification, key rotation,
   grant change or deployment.
2. Use the same physical iPhone, network/VPN route, board presentation and host
   population for all 10 returns. Pair through the normal supported flow.
3. Connect the unlocked iPhone to the Mac; in Console select the iPhone, enable
   Info messages, filter subsystem `com.corral.fleetnotifier` and category
   `foreground-reconnect`, and keep one capture for the whole block. Export only
   allowlisted timing messages — never personal board contents.
4. Logger records are `attempt=<UUID> stage=<name> uptime=<seconds>`. Group by
   attempt UUID; never combine attempts. Record the actual foreground trigger
   time independently if trigger-to-apply latency is reported (`path_ready` is
   an asynchronous path-monitor observation, not the trigger).
5. Background the app by switching to another app for >= 60 elapsed seconds
   (a real background, not a transient inactive interruption). Record the actual
   background duration and any OS termination: a terminated process is a cold
   launch, not a warm return.
6. Per return, record: warm-return ordinal, background duration, `key_request`
   -> `key_response`, `key_response` -> `frame_applied` (first *verified* frame),
   and whether the board was retained-then-verified or blank.

## Analysis

For each return compute durations from its own monotonic timestamps, then report
`statistics.median` and the maximum over the 10 per-return durations. Do not
subtract medians of absolute timestamps. Keep failures, timeouts and missing
stages visible; do not exclude them without reporting attempted and valid
counts per metric.

Pass condition: `max(key_response -> frame_applied) <= 2 s` over 10 valid warm
returns, with no warm return whose first verified frame required more than one
`/host-key` attempt as a *result* of a hung attempt (a single retried attempt is
acceptable but must be reported).

## Owner metadata (empty)

| Host | App SHA/build | iPhone/iOS | Daemon/OS | Network route | Workload command/config | Load evidence |
| --- | --- | --- | --- | --- | --- | --- |
| Loaded Bazzite over Tailscale | | | | | | |

## Owner results (empty; milliseconds)

| # | Background s | key_request -> key_response | key_response -> verified frame | Retained-then-verified? | `/host-key` attempts | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | | | | | | |
| 2 | | | | | | |
| 3 | | | | | | |
| 4 | | | | | | |
| 5 | | | | | | |
| 6 | | | | | | |
| 7 | | | | | | |
| 8 | | | | | | |
| 9 | | | | | | |
| 10 | | | | | | |

| Median | Max | n valid | n attempted | <= 2 s max | Outliers ("many seconds") still present? | Capture paths |
| --- | --- | --- | --- | --- | --- | --- |
| | | | | | | |

## Non-claims this lane makes explicit

- No physical iPhone, no TestFlight build, no loaded Bazzite host and no
  Tailscale path was exercised here.
- The simulator harness (`docs/evidence/issue-554/measurement.md`) uses a local
  `URLProtocol`, which never reaches a socket pool, so it can neither reproduce
  nor falsify the stale-pool warm return; it only shows that the staged
  production timings and the cold-launch path did not regress.
- The stale-pool hypothesis itself remains an owner-approved hypothesis, not a
  measured finding: no device-side capture of a hung attempt exists in this lane.
