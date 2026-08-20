# #110 tappable-controls verification

This evidence accompanies the iOS board/detail-control work.

## Source and model checks

- Swift frontend parsing passed for the changed app and test sources.
- Swift type-check passed for all `FleetNotifier` app sources against the
  iOS 17 simulator target.
- Swift type-check passed for `FleetNotifierTests` with the checked-in
  XCTest framework path.
- `git diff --check` passed.

The state tests cover the Idle / done disclosure transition, live row-route
reconciliation after deletion, explicit lifecycle labels, action availability
and grant explanations, Tail 200 payload construction, null Interrupt payload
construction, claim-bound approval availability, deleted-target refusal, and
duplicate Tail suppression.

## Runtime limitation

The verification host has the iOS 26.5 SDKs but no installed iOS runtime or
device platform. `xcodebuild` therefore reports no available destinations and
the generic iOS destination is ineligible because “iOS 26.5 is not installed.”
The required Herdr wrapper confirms the same concrete limitation:
`hermes-sim-task: no available iOS runtime`.
Simulator-backed UI tests and screenshots are not claimed until an iOS runtime
is installed; the checks above are the source/type-check evidence available on
this host.
