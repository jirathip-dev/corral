# #554 transport measurements (host-side probe, no simulator)

Command (raw exit recorded):

```
swift docs/evidence/issue-554/session-probe.swift   # RAW_EXIT=0
```

Raw output — `docs/evidence/issue-554/session-probe.log`:

```
MOCK preserved: true
timeout base/derived: 60.0 60.0
shared timeout/wfc: 60.0 false
derived fetch: 200 ok -
hold-open started: 1 stopped: 0
after invalidateAndCancel stopped: 1 completion: domain=NSURLErrorDomain code=-999
blackhole elapsed=16.0s code=-1001
```

## What each measurement decides

1. `MOCK preserved: true` — a session created from
   `base.configuration` (the derivation `AppModel.installLiveSession` uses)
   keeps the base session's `protocolClasses`, so the tests' injected
   `URLProtocol` stack rides along on the live session. This is what makes the
   injected-session test path survive the per-live-session session.
2. `timeout base/derived: 60.0 60.0` and `shared timeout/wfc: 60.0 false` —
   the derived live session inherits `timeoutIntervalForRequest = 60`, so
   `FleetStore.connect`'s derivation
   (`StreamLiveness(transportTimeout: client.session.configuration.timeoutIntervalForRequest)`)
   still yields the documented 30 s inactivity budget and the `/events` request
   keeps its own 60 s timeout. `waitsForConnectivity` is already `false` on
   `.shared` and is set explicitly on every derived session anyway.
3. `hold-open … stopped: 1` / `completion: domain=NSURLErrorDomain code=-999` —
   `URLSession.invalidateAndCancel()` tears down a task that was STARTED before
   the call (`stopLoading`) and that task's completion carries
   `NSURLErrorCancelled`. This is the observation acceptance criterion (a) uses:
   a task started on the retired live session fails as cancelled, which is
   evidence the model invalidated that exact session.
4. `blackhole elapsed=16.0s code=-1001` — with
   `URLSessionConfiguration.timeoutIntervalForRequest = 2` and
   `URLRequest.timeoutInterval = 15`, the request still took ~15 s
   (`URLError.timedOut`). **The request-level timeout wins**, so the session
   configuration cannot bound a `/host-key` request that sets
   `request.timeoutInterval = 15` in `Network/CorraldClient.swift`.

## Consequence for the contract's fast-fail preflight

`Network/CorraldClient.swift` is outside this lane's fence, so the per-request
timeout cannot be shortened at the request site; and shortening the live
session's `timeoutIntervalForRequest` instead would silently shrink the SSE
liveness budget via (2). The bound is therefore enforced where it *can* be: the
`/host-key` ladder cancels a hung attempt at
`HostPreflightRetryPolicy.attemptTimeout` (default **5 s**), which makes the
attempt fail into the existing #451 retry ladder exactly like any other
transient transport failure. The SSE stream timeout stays 60 s and
`StreamLiveness` stays 30 s.

Non-claims: this probe runs a host `swift` process, not the app; the
black-holed address is an RFC1918 address that never answers, so the 15 s
figure is a real connect/idle timeout, not a simulated delay. No device, no
Bazzite host, no Tailscale path was involved.
