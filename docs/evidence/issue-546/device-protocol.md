# #546 physical-iPhone acceptance protocol — NOT RUN

This is the isolated client slice, not a delivered push service. #544 is an
unmet dependency. No device results, latency measurements, APNs dispatch,
relay deployment, or sender/device isolation proof are claimed here.
`xcrun devicectl list devices` on the implementation host returned
`No devices found.` Simulator URLProtocol tests are client tests, not a relay.

## Before an owner-run acceptance session

1. Obtain separate authorization for #544 and finish its authenticated
   host/device binding, opaque routing, revocation, opt-out and send ceilings.
   Never use a fake relay as acceptance evidence. No APNs credentials belong
   in this document, the app's hint payload, test fixtures, or captured logs.
2. Use a physical iPhone with a compatible signed build, its genuine APNs
   registration and Tailscale/private host path. Record build SHA, iOS version,
   device model (not serial/UDID/token), and scenario settings. The existing
   remote-notification background mode is necessary, not a delivery guarantee.
3. Pair and confirm the host fingerprint normally. Enable the separate
   Background snapshot refresh preference explicitly (default OFF). Visible
   alerts remain a separate preference and permission chain. This client-only
   preference does not enroll APNs or update a relay; #544 must finish that
   independent consent/binding path before delivered-hint acceptance can run.
4. Establish a board and its epoch in foreground, then background the app.
   Do not leave a debugger attached for the delivery/force-quit scenarios.
   Record whether this is an in-memory resume, normal terminated relaunch, or
   user force-quit. The client fails closed if a notification reaches the
   delegate before the restored model is installed. Measure this case rather
   than assuming SwiftUI launch ordering.

## Relay v1 contract (unimplemented here)

APNs request headers: `apns-push-type: background`, `apns-priority: 5`, and the
correct topic/device token for the authenticated device. Headers are not part
of iOS `userInfo` and the client cannot verify their values from that callback.
Use a bounded expiry and coalesce pending changes for the authorized binding.
Do not fan out token/output events or send a polling heartbeat.

The complete JSON body, with no additional keys:

    {
      "v": 1,
      "type": "board_changed",
      "host_id": "<canonical base64 of the locally pinned 32-byte X25519 public key>",
      "aps": { "content-available": 1 }
    }

The placeholder is a public opaque host identity, never a hostname or URL.
Both `1` values are JSON numbers, not booleans or strings. No alert, sound,
badge, prompt, output, repository, branch, agent, title, body, URL, enrollment,
or sender-supplied device routing field is allowed. Unknown/extra fields are
rejected. Do not widen the existing visible-alert payload to carry hints.
Failed/delayed hints must never suppress a separately authorized visible alert.

The relay must enforce authenticated sender/device isolation and revocation,
host/device and app opt-outs, one attempted hint per host/device per 30 minutes,
and two attempted hints per device per rolling hour, before dispatch. The
client only sees delivered callbacks: its persisted counters cannot prove the
relay's attempted-send ceilings or count hints Apple delays/discards.

## Privacy and retention

The relay must see only binding/routing and bounded operational metadata, not
fleet content. On receipt the client resolves an already enrolled local pin,
performs one `/host-key` check and at most one `/snapshot` GET through the
existing CorraldClient/URLSession private path. It never subscribes to SSE,
fetches transcripts, enrolls a host, prompts, or retries internally. The
existing snapshot GET is not request-signed; do not describe it as such. The
existing HTTPS/private-path and pinned host-key comparison are reused.

On-device attempt metadata: at most two `{hostID, attemptedAt}` records under
`fleetnotifier.backgroundHintAttempts.v1` in UserDefaults. Each consumes quota
for one rolling hour; host matching consumes the first 30 minutes. Pruning is
lazy on next model launch or admitted hint, so expired bytes can remain while
the app never runs. Clock rollback conservatively retains future-dated entries.
Opt-out does not erase quota (off/on must not defeat ceilings). No new payload,
transcript or full-agent document is persisted.

Existing BoardCacheRow fields only: composite identity, profile UUID, agent ID,
state, timestamps/state-entered-at, display name/title/reason/tool, pane ref,
repo/branch/worktree basename and last-seen timestamp. Existing cache retention
is until replacement or host removal, using BoardCacheStore's atomic protected
file. Revision and epoch persist together through HostProfileStore; the active
store also maintains its existing legacy cursor mirror. Preference, attempts,
metadata and cursor survive normal process termination, subject to existing
best-effort persistence and iOS file protection; full agents, tails, stream
owners, trust verification and pending requests are memory-only. A cold board
still needs a foreground stream snapshot to rebuild its full delta base.

Only a freshly checked, non-streaming store may accept the hint snapshot via
applyRefresh. It cannot cross an established epoch, roll a cursor backwards,
release a speculative foreground verification inbox or mark the connection
Live. Foreground, removal, newer data, epoch changes and expiry win. Failure
leaves retained data stale. A local 15-second deadline cancels the request and
finishes once without waiting for the remote host.

## Capture procedure and success criteria

For each scenario, use a session-local label such as H1/D1, not host URLs,
pinned keys, tokens, account IDs, agent IDs, repo names or content. Synchronize
clocks or record their uncertainty; distinguish host and iPhone clocks.

Record only UTC/monotonic timestamps for:

- host board change (host-side observation),
- relay attempt and Apple acceptance, if any (#544 evidence),
- iOS handler receipt,
- pin-check start/end and snapshot fetch start/end,
- snapshot application or refusal/cancellation/expiry and completion.

Receipt/fetch/apply timestamp capture needs an owner-authorized physical test
instrumentation session; this slice does not add production content logging or
pretend its simulator timestamps measure APNs. A missing receipt is missing
receipt, not a measured network timeout. Check completion count, no residual
hint owner, and the stale/not-Live posture in each negative case.

Positive criterion: a genuinely delivered hint fetches over the private
Tailscale path and updates only that host's allowlisted state inside budget;
a later foreground stream still verifies/reconnects and supersedes it. Compare
return-to-app freshness with refresh OFF using matched workloads and observed
runs, reporting sample size and missing/delayed deliveries. Never extrapolate
to always-on sync or guaranteed delivery.

Exercise each case below, independently. Restore settings/path/host after each
case and prove ordinary foreground reconnect without any hint still works.
Do not defeat quotas to manufacture delivery. Use genuine elapsed windows or
an independently authorized test build and label any altered limits explicitly.

## Results — intentionally unfilled (owner/device/#544 gate)

| Scenario | Build / iOS / device | Host-change / receipt / fetch / apply times | Outcome / completion count / freshness | Evidence |
|---|---|---|---|---|
| Delivered hint; app backgrounded; private path available | | | | |
| Normal process termination and restored-model timing | | | | |
| Offline host (local deadline, no retry) | | | | |
| Tailscale/private path unavailable | | | | |
| App refresh preference OFF (no work) | | | | |
| iOS Background App Refresh disabled | | | | |
| Low Power Mode enabled | | | | |
| User force-quit (delivery may not occur until manual launch) | | | | |
| Host removed/revoked/key changed before receipt | | | | |
| Foreground / newer stream data / epoch race during fetch | | | | |
| Duplicate hints and both rolling ceilings | | | | |
| Authorized H1/D1 vs unrelated sender/device binding | | | | |
| Visible alert enabled, hint disabled/delayed/failed | | | | |
| Visible alert denied/OFF, hint independently enabled | | | | |
| Foreground reconnect without hints; OFF/ON freshness comparison | | | | |
