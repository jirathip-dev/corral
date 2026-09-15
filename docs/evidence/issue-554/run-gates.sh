#!/bin/bash
# #554 exact gate driver — the script this lane actually ran for G1-G7.
# Usage: bash docs/evidence/issue-554/run-gates.sh [g1|chain|all|measure|aslop]
# Every heavy leg is serialized on the shared heavy lock (/tmp/n.lock) and its
# raw exit is echoed by the leg itself; bounded-run.py adds an outer deadline
# and appends RAW_EXIT= to its own log.
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT" || exit 126
SELECT="${1:-all}"
H='flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1'
DEST='platform=iOS Simulator,id=C8E69C58-66A4-440D-850C-8C1628C720E6'
PROJECT=(-project ios/FleetNotifier.xcodeproj -scheme FleetNotifier)
BOUNDED=docs/evidence/issue-547/bounded-run.py

echo "HEAD=$(git rev-parse HEAD)"
echo "DF_START: $(df -h / | tail -1)"

if [ "$SELECT" = g1 ] || [ "$SELECT" = all ]; then
  echo "--- G1 focused (new #554 class + #451 ladders + scene seam/foreground) ---"
  python3 "$BOUNDED" g554-focused 1500 \
    $H xcodebuild test "${PROJECT[@]}" -destination "$DEST" -derivedDataPath /tmp/g554-dd \
    -only-testing:FleetNotifierTests/LiveSessionTransportTests \
    -only-testing:FleetNotifierTests/PreflightRetryCoordinatorTests \
    -only-testing:FleetNotifierTests/PreflightRetryActiveHostTests \
    -only-testing:FleetNotifierTests/ScenePhaseLifecycleTests \
    -only-testing:FleetNotifierTests/ForegroundReconnectTests
  echo "FOCUSED_EXIT=$?"

  echo "--- G1b contract-item-4 pins: the behaviour that must stay byte-for-byte unchanged ---"
  python3 "$BOUNDED" g554-pins 1200 \
    $H xcodebuild test "${PROJECT[@]}" -destination "$DEST" -derivedDataPath /tmp/g554-dd \
    -only-testing:FleetNotifierTests/HostKeyContinuityModelTests \
    -only-testing:FleetNotifierTests/EpochRecoveryTests \
    -only-testing:FleetNotifierTests/RecentOutputNotGrantedStateTests \
    -only-testing:FleetNotifierTests/GrantsRefreshTests \
    -only-testing:FleetNotifierTests/PerHostGrantsRefreshTests
  echo "PINS_EXIT=$?"
fi

if [ "$SELECT" = chain ] || [ "$SELECT" = all ]; then
  echo "--- G2 full suite ---"
  $H xcodebuild test "${PROJECT[@]}" -destination "$DEST" -derivedDataPath /tmp/g554-dd \
    -only-testing:FleetNotifierTests > /tmp/g554-full.log 2>&1; echo "FULL_EXIT=$?"

  echo "--- G3 boundary + self-test ---"
  python3 ios/check-release-demo.py > /tmp/g554-check-release.log 2>&1; echo "CHECK_RELEASE_EXIT=$?"
  python3 ios/check-release-demo.py --self-test > /tmp/g554-self-test.log 2>&1; echo "SELF_TEST_EXIT=$?"

  echo "--- G4 Debug + Release builds + binary check ---"
  rm -rf /tmp/g554-dd /tmp/g554-debug-dd /tmp/g554-release-dd
  echo "DF_BEFORE_G4: $(df -h / | tail -1)"
  $H xcodebuild build "${PROJECT[@]}" -configuration Debug \
    -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/g554-debug-dd \
    > /tmp/g554-debug.log 2>&1; echo "DEBUG_EXIT=$?"
  $H xcodebuild build "${PROJECT[@]}" -configuration Release -sdk iphonesimulator \
    -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/g554-release-dd \
    CODE_SIGNING_ALLOWED=NO > /tmp/g554-release.log 2>&1; echo "RELEASE_EXIT=$?"
  python3 ios/check-release-demo.py --binary \
    /tmp/g554-release-dd/Build/Products/Release-iphonesimulator/FleetNotifier.app/FleetNotifier \
    > /tmp/g554-binary.log 2>&1; echo "BINARY_EXIT=$?"
  rm -rf /tmp/g554-debug-dd /tmp/g554-release-dd
  echo "DF_AFTER_G4: $(df -h / | tail -1)"

  echo "--- G5 project-generation drift ---"
  (cd ios && xcodegen generate --spec project.yml) > /tmp/g554-xcodegen.log 2>&1
  echo "XCODEGEN_EXIT=$?"
  git diff --exit-code -- ios/ > /tmp/g554-xcodegen-diff.log 2>&1
  echo "XCODEGEN_DIFF_EXIT=$?"

  echo "--- G6 advisory anti-slop (head) ---"
  swift run --package-path ios/tools/anti-slop-swift anti-slop \
    ios/FleetNotifier ios/FleetNotifierTests > /tmp/g554-aslop-head.log 2>&1
  echo "ASLOP_EXIT=$?"

  echo "--- G7 hygiene ---"
  git diff --check > /tmp/g554-diffcheck.log 2>&1; echo "DIFFCHECK_EXIT=$?"
fi

if [ "$SELECT" = aslop ] || [ "$SELECT" = all ]; then
  echo "--- G6b anti-slop base identity compare ---"
  bash docs/evidence/issue-554/aslop-compare.sh
  echo "ASLOP_COMPARE_EXIT=$?"
fi

if [ "$SELECT" = measure ]; then
  echo "--- G8 simulator staged timings (#551 harness, re-run over the #554 fix) ---"
  python3 "$BOUNDED" g554-measure-outer 3000 \
    flock /tmp/n.lock python3 docs/evidence/issue-554/measure.py
  echo "MEASURE_EXIT=$?"
  python3 docs/evidence/issue-554/timings.py /tmp/g554-measure.log > /tmp/g554-timings.json
  echo "TIMINGS_EXIT=$?"
fi

echo "DF_END: $(df -h / | tail -1)"
echo "G554_GATES_DONE"
