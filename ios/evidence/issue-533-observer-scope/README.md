# 533 evidence — HerdThemeStore defaults observer scoped to its own suite

Provenance: lane `impl533-observer-scope`, code/tests/pin commit
`77bcee5b62fac7a06a53cda0ce7ba5ed2d51f58a` over base `origin/integration`
`ed24e6f57075acf1e5b082c7cd29a931dfbfbd62`. Every probe here ran on the exact
bytes of `77bcee5` (file sha256s below; reverified with
`git show 77bcee5:<path> | shasum -a 256`). This evidence directory itself is
committed in the lane's follow-up commit; the head commit is
`git log -1 --format=%H -- ios/evidence/issue-533-observer-scope/` and the
lane report `.report-533-observer.md` names it literally.

## What was measured

`ThemeStore` (`ios/FleetNotifier/UI/AppTheme.swift`) observes
`UserDefaults.didChangeNotification`. Before the change the observer was
registered with `object: nil`, so a write to ANY suite woke it (the #526
independent review's condition C2: latent shared-state coupling). The change
registers the observation with `object: defaults` — the store's own suite
instance — while keeping `queue: .main`, `weak self`, the deinit removal and
the untouched Reduce Motion observer.

The focused tests are `ThemeStoreObserverScopeTests` (ThemeTests.swift):

- `testUnrelatedSuiteWriteDoesNotWakeTheStore` — negative leg: writing a
  DIFFERENT `UserDefaults(suiteName:)` must not re-read the store's own keys
  within a bounded window (inverted XCTestExpectation, 1.0 s). RED at the
  mutated head, GREEN at the lane head. A value-only assertion cannot
  discriminate this: the spurious wake re-reads the SAME suite, so the
  resolved palette is byte-identical with and without the fix. The observer's
  RE-READ is the wake's only observable trace, so the test counts reads via a
  read-recording `UserDefaults` subclass (`ReadCountingDefaults`).
- `testSameSuiteWriteStillRefreshesTheResolvedPalette` — positive control: a
  write through the store's OWN suite instance still refreshes the resolved
  palette (`herdEnvironment` night resolves Mocha in Herd mode). Passes at
  BOTH heads, proving the counting mechanism observes a real write; #526's
  `testExternalEnvironmentWriteRefreshesTheResolvedPalette` stays unmodified.

## Mutation proof (raw)

Scratch tree `/tmp/i533-mut` = `git archive 77bcee5` + one anchored
mutation: `object: defaults,` -> `object: nil,` (exactly one site, the
ThemeStore observer; `AppTheme.swift` sha256 moved
`819a9c40…` -> `70e2c5ef…`). Exact commands, mutation site and raw exit codes:
`commands.log`.

- RED: `probe-mutation-red.txt` — `Test Suite 'ThemeStoreObserverScopeTests'
  failed`, `ThemeTests.swift:459 error: Fulfilled inverted expectation "an
  unrelated-suite write must not re-read the store's keys"`,
  `ThemeTests.swift:463 XCTAssertEqual failed: ("6") is not equal to ("2")`,
  `Executed 2 tests, with 2 failures (0 unexpected)`, `EXIT=65`. The
  same-suite positive control passed in the same run. No compile error.
- Restore: `cp` + `cmp -s` byte-identical; sha256 back to `819a9c40…`.
- GREEN after restore: `probe-mutation-restore-green.txt` —
  `Executed 2 tests, with 0 failures (0 unexpected)`, `EXIT=0`.

## Tree identity (sha256)

    819a9c408106da94b9a1944f8dcb81259163f3380d8eb2212696737c32c4d01b  ios/FleetNotifier/UI/AppTheme.swift
    fc30833426bb7ef06f8d0cbcb478da04129b823e9638f9b9308e224753c82bbf  ios/FleetNotifierTests/ThemeTests.swift
    70e2c5ef9f1c9739708e6fea9952ea8dfe9ef80a9c799a815aa604dc4bcca08a  AppTheme.swift in the mutated scratch only

Simulator: `Corral533-Observer`, UDID
`48FCC0F0-DAEB-4AFC-89CA-9D1DDB4D28A4` (iPhone 16, iOS 26.5), created and
booted for this lane and used for every run below.

## Runs (verbatim slices in this directory)

| file | tree | result |
|---|---|---|
| `probe-fixed-head-focused.txt` | lane head `77bcee5` | 2 tests, 0 failures, `EXIT=0` |
| `probe-mutation-red.txt` | `/tmp/i533-mut` (`object: nil`) | 2 tests, 2 failures, `EXIT=65` |
| `probe-mutation-restore-green.txt` | `/tmp/i533-mut` (restored) | 2 tests, 0 failures, `EXIT=0` |

The `probe-*.txt` files are verbatim slices of the full xcodebuild logs: the
run header, then everything from `Test Suite 'Selected tests' started` to the
end of the log (test output, summary, `EXIT=`). The single elided middle is
xcodebuild build/setup noise; each file states its own elision marker and the
full log path it was sliced from. No output was re-typed.

`SHA256SUMS` covers the files in this directory (there are no binary
artifacts in this evidence set).

## Explicitly NOT claimed

- No simulator UI or screenshot evidence: the change is not value-visible
  (see the value-only argument above); the proof is the focused tests plus
  the mutation RED/GREEN, not rendered frames.
- No CI run and no physical-device run: everything ran on the lane host's
  simulator above. The full-suite gate result is reported in the lane report,
  not in this directory.
- No claim about any OTHER `UserDefaults.didChangeNotification` observer; the
  Reduce Motion observer and every other observer are unchanged.
- No claim that a same-suite write through a DIFFERENT `UserDefaults`
  instance (a second `UserDefaults(suiteName:)` for the same suite name)
  wakes the store: Foundation posts the changed instance, so `object:
  defaults` scopes to the store's own instance. The #526-covered writer (the
  #457 evidence plumbing and the store's own setters) writes through that
  instance.
- No daemon, network, pairing, `main`, or TestFlight surface was touched.
