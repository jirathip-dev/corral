# #545 physical-iPhone acceptance protocol — NOT RUN

Decision at this lane: **NO-GO; retain immediate disconnect.** The delivered app
sources are byte-identical to baseline `2efa3577086a794ad064898d8af9d69b33a62325`
(#547 and #548 integrated). This branch adds a deterministic feasibility test,
not a background assertion. Comparing these two builds measures a control, not
an experimental background-completion benefit. Do not relabel it an experiment.
No physical results have been collected or entered here.

A future experimental build requires owner authorization and independent review
before this protocol's experimental arm can run. That build must identify one
finite request already on the wire at actual background entry, separate its
apply/persist completion from streaming, and pass the lifecycle/security gates.
There is no authorization here to deploy, re-pair hosts, rotate keys, send APNs,
change a relay/service, or install an experimental build on anyone's device.

## Preparation (owner, physical device only)

1. Record the iPhone model, iOS version, battery level/health, Low Power Mode,
   thermal state, network/VPN route and app build configuration. Use the same
   physical phone and host configuration for both arms. Pin each build's full
   source SHA, version/build number and embedded Release-source digest. Baseline
   is the integration SHA above; an authorized experimental SHA is currently
   **unavailable**. Do not use simulator latency as device latency.
2. Use an owner-approved test host/profile and metadata-only board. Keep the same
   host version, board cardinality, other-host count, active host and agent
   identity in each pair. Do not expose real transcripts, credentials or token
   values in recordings. Never weaken host-key or epoch validation for capture.
3. Capture app/host logs and timestamped screen recording. Use monotonic client
   times within a trial; align host events by a trial label and record clock
   uncertainty, not subtraction of unrelated host/device clocks. Record host
   CPU/load, request/connection latency, network RTT, board revision and epoch,
   cache warmth and background/foreground timestamps. Retain the raw artifacts.
4. No manual pull on return: the first-authoritative-frame endpoint must measure
   the normal scene lifecycle, not a manually accelerated path. If natural
   traffic gives no new frame, record `no frame within 15 s` (censored), never 0.
   A separately approved controlled metadata update may be used consistently in
   both arms; record its exact time and revision. It must not be artificial
   client work started to hold a background assertion.

## Matrix and repetitions

Run matched baseline/experimental trials for EACH cell (when an experimental
build actually exists):

- Actual background absence: 1 s, 3 s, 10 s, 60 s.
- Network: stable Wi-Fi, stable cellular, Wi-Fi -> cellular during absence,
  cellular -> Wi-Fi during absence. Record observed path-change time and actual
  achieved background duration, not merely the requested duration.
- Work at background entry: no finite work; an independently initiated ordinary
  finite refresh confirmed on the wire before entry. Also observe a naturally
  in-flight key check separately; it is not permission to retain its retry ladder.
- Host posture: healthy; owner-approved slow/unreachable test endpoint. For two
  hosts, alternate which host is slow and measure the healthy host separately.

Collect at least 10 matched pairs per applicable cell in two sessions on the
same phone; alternate AB/BA build order to counter cache/thermal/order effects.
Report actual n, exclusions with reasons, failures, and censored trials per cell.
Do not pool short-switch cells with long absences or network-transition cells.
Inactive-only Control Center/app-switcher interruptions are a separate control:
confirm `.background` was NOT delivered and the original healthy streams remain.
Never substitute these for actual app-switch samples.

## One trial

1. Open the same board and wait for a verified authoritative foreground state.
   Record the retained revision/epoch and per-host stream counts. For the finite
   arm initiate the normal refresh in the foreground and confirm its request
   started before leaving. If it finished before background entry, label the
   trial `no eligible work`, not a positive finite-work trial.
2. Switch to another app; confirm the app log records actual `.background`.
   Perform the cell's network transition if applicable. Return after the target
   absence and record both actual background dwell and foreground time.
3. Record two independent endpoints measured from the same foreground timestamp:
   - Return-to-retained-board: first recorded frame with the retained rows visibly
     rendered. A stale board counts here but NOT as authoritative freshness.
   - Return-to-first-authoritative-frame: first post-return accepted stream frame
     after the current key check, with revision/epoch provenance. Also record its
     first visible row separately using the existing reconnect stage timing and
     video; bytes received, frame applied and row visible are different stages.
4. Record `/host-key` request/response, `/events` opened/headers/first data,
   decode/enqueue, frame applied and row-visible stages independently. A 200
   stream acknowledgment or retained task handle is not an authoritative frame.
5. Save logs/video and wait for baseline host load/phone thermal conditions before
   the next trial. Include unsuccessful trials in the result table.

## Bounded execution / security acceptance (experimental arm only)

- Capture Instruments activity/network and assertion begin/end events. Exactly
  zero assertions when no eligible request exists; at most one app-wide assertion
  otherwise, never per host. Record its eligible request ID and generation.
- Five seconds from actual background entry is an **application maximum**, not
  an Apple guarantee. The OS can refuse allowance, expire it earlier, suspend or
  terminate the process. Do not claim guaranteed execution or uninterrupted sync.
- Verify immediate cancellation of SSE/watchdogs/path retry work, no new request
  retries/fanout/polling after background, and no idle CPU/network loop during
  the 10/60 s absences. Inspect activity beyond the five-second cap too.
- Verify exactly one end on completion, failure, local deadline, OS expiration,
  cancellation and foreground return; record event counts and lifetime. Test OS
  refusal/expiry via deterministic injection as well, but do not label an
  injected callback a real OS expiration. If a real OS case cannot be observed,
  mark it unverified; no production approval by assumption.
- Rapid background/foreground cycles must retain no stale owner and create no
  duplicate stream. New connections still re-check the pinned key and preserve
  epoch/cursor rules. No old-generation or removed-host result may alter current
  state. A healthy host must not wait for an unreachable peer.
- Under separately approved test-host controls, cover key mismatch, epoch reset
  and host removal mid-request. Never rotate a live host's key or remove a real
  profile merely for this protocol. Verify privacy hiding and inspect persisted
  artifacts: only existing allowlisted board metadata and cursor+epoch; no new
  transcript, credentials, token, or request-body persistence.

## Empty results templates (owner fills after measurement)

| Trial/pair | Session/order | Build SHA/digest | Phone/iOS/thermal/battery | Host version/load/RTT | Host count/rows | Network + transition time | Requested/actual absence | In-flight request + start time | Retained rev/epoch | Retained-board ms | Authoritative-applied ms | Authoritative-visible ms | Assertion begin/end/reason/lifetime | Requests after background | Artifacts/failure/censor reason |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

| Cell | Baseline n | Experimental n | Excluded/censored/failed | Retained-board median/p95 per arm | Authoritative-frame median/p95 per arm | Paired deltas + spread | Assertion max/end-count violations | Security/lifecycle observations | Repeatable benefit? |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

## Decision rule

Retain the change only with repeatable short-switch benefit and no
security/lifecycle regression. Require paired improvements in the 1/3 s cells
across both sessions larger than recording/timestamp uncertainty; report spread
and tails, not just a fastest sample or an aggregate mean. No regression in
10/60 s, transition, inactive or slow-peer controls. Any leaked assertion,
background retry/stream, trust/epoch failure, privacy change or unexplained
exclusion is NO-GO. Missing physical evidence is NO-GO / keep immediate disconnect,
not approval to ship a speculative keepalive. Independent review and the owner
make the adoption decision; this document makes no physical or performance claim.
