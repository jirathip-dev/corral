# #110 tappable-controls verification

This evidence accompanies the iOS board/detail-control work.

## Source and model checks

- Swift frontend parsing passed for the changed app and test sources.
- Swift type-check passed for all `FleetNotifier` app sources against the
  iOS 17 simulator target.
- Swift type-check passed for `FleetNotifierTests` with the checked-in
  XCTest framework path.
- `git diff --check` passed.

The compiled test target includes `FleetViewState` coverage for the Idle / done
disclosure and the actual `NavigationStack` path reconciliation after
deletion. It also includes deterministic URLProtocol-backed drive tests that
await request observation and completion for Prompt, Interrupt, direct
approval, notification approval, duplicate claim replies, deleted-target
refusal, and two simultaneous live drives cancelled at the demo boundary.
Additional held-boundary tests cover two concurrent cold-start notification
snapshot replies cancelled before they can apply or approve, a stale-agent
snapshot refresh cancelled before it can overwrite the demo fleet, and
cancellation during held biometrics with no `/step-up` or `/drive` request.
Registration tests hold `/register` across both demo and reset boundaries,
reject a concurrent registration, and verify that no metadata or `/events`
stream is resurrected. An injected APNs reset race holds the upload before
`/device-token` and verifies that cancellation prevents the retired request.
Exit-demo tests verify persisted host/key/signer restoration, demo row and
cursor clearing, a fresh live snapshot before actions, and a needs-setup
fallback with no live dispatch when the identity is absent.
The deterministic URLProtocol script pointer is lock-protected for every
set/clear/read operation.
The live SSE hop also carries its connection generation so a decoded frame
cannot apply after disconnect/demo. Separate policy tests cover explicit
lifecycle labels, action availability/grant explanations, Tail 200 payload
construction, null Interrupt payload construction, and claim-bound approval
availability. These tests were type-checked but could not execute because
this host has no iOS runtime.

## Runtime limitation

The verification host has the iOS 26.5 SDKs but no installed iOS runtime or
device platform. `xcodebuild` therefore reports no available destinations and
the generic iOS destination is ineligible because “iOS 26.5 is not installed.”
The required Herdr wrapper confirms the same concrete limitation:
`hermes-sim-task: no available iOS runtime`.
Simulator-backed UI tests and screenshots are not claimed until an iOS runtime
is installed; the checks above are the source/type-check evidence available on
this host.
