#!/bin/bash
# #551 exact gate driver — the script this lane actually ran for G1-G7.
# Usage: bash docs/evidence/issue-551/run-gates.sh [g1|chain|all]
# Each heavy leg is serialized on the shared heavy lock (/tmp/n.lock) and its
# raw exit is echoed by the leg itself; bounded-run.py adds an outer deadline
# and appends RAW_EXIT= to its own log.
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT" || exit 126
SELECT="${1:-all}"
H='flock /tmp/n.lock env HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1'
DEST='platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793'
PROJECT=(-project ios/FleetNotifier.xcodeproj -scheme FleetNotifier)

echo "HEAD=$(git rev-parse HEAD)"
echo "DF_START: $(df -h / | tail -1)"

if [ "$SELECT" = g1 ] || [ "$SELECT" = all ]; then
  echo "--- G1 focused ---"
  python3 docs/evidence/issue-547/bounded-run.py g551-focused 900 \
    $H xcodebuild test "${PROJECT[@]}" -destination "$DEST" -derivedDataPath /tmp/g551-dd \
    -only-testing:FleetNotifierTests/ScenePhaseLifecycleTests \
    -only-testing:FleetNotifierTests/ForegroundReconnectTests \
    -only-testing:FleetNotifierTests/PreflightRetryCoordinatorTests \
    -only-testing:FleetNotifierTests/PreflightRetryActiveHostTests \
    -only-testing:FleetNotifierTests/EpochRecoveryTests
  echo "FOCUSED_EXIT=$?"
fi

if [ "$SELECT" = chain ] || [ "$SELECT" = all ]; then
  echo "--- measurement (opt-in stage capture, 5x) ---"
  python3 docs/evidence/issue-547/bounded-run.py g551-measure-outer 3000 \
    flock /tmp/n.lock python3 docs/evidence/issue-551/measure.py
  echo "MEASURE_EXIT=$?"
  python3 docs/evidence/issue-551/timings.py /tmp/g551-measure.log > /tmp/g551-timings.json
  echo "TIMINGS_EXIT=$?"

  echo "--- G2 full suite ---"
  $H xcodebuild test "${PROJECT[@]}" -destination "$DEST" -derivedDataPath /tmp/g551-dd \
    -only-testing:FleetNotifierTests > /tmp/g551-full.log 2>&1; echo "FULL_EXIT=$?"

  echo "--- G3 boundary + self-test ---"
  python3 ios/check-release-demo.py > /tmp/g551-check-release.log 2>&1; echo "CHECK_RELEASE_EXIT=$?"
  python3 ios/check-release-demo.py --self-test > /tmp/g551-self-test.log 2>&1; echo "SELF_TEST_EXIT=$?"

  echo "--- G4 Debug + Release builds + binary check ---"
  rm -rf /tmp/g551-dd /tmp/g551-debug-dd /tmp/g551-release-dd
  echo "DF_BEFORE_G4: $(df -h / | tail -1)"
  $H xcodebuild build "${PROJECT[@]}" -configuration Debug \
    -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/g551-debug-dd \
    > /tmp/g551-debug.log 2>&1; echo "DEBUG_EXIT=$?"
  $H xcodebuild build "${PROJECT[@]}" -configuration Release -sdk iphonesimulator \
    -destination 'generic/platform=iOS Simulator' -derivedDataPath /tmp/g551-release-dd \
    CODE_SIGNING_ALLOWED=NO > /tmp/g551-release.log 2>&1; echo "RELEASE_EXIT=$?"
  python3 ios/check-release-demo.py --binary \
    /tmp/g551-release-dd/Build/Products/Release-iphonesimulator/FleetNotifier.app/FleetNotifier \
    > /tmp/g551-binary.log 2>&1; echo "BINARY_EXIT=$?"
  rm -rf /tmp/g551-debug-dd /tmp/g551-release-dd
  echo "DF_AFTER_G4: $(df -h / | tail -1)"

  echo "--- G5 project-generation drift ---"
  (cd ios && xcodegen generate --spec project.yml) > /tmp/g551-xcodegen.log 2>&1
  echo "XCODEGEN_EXIT=$?"
  git diff --exit-code -- ios/ > /tmp/g551-xcodegen-diff.log 2>&1
  echo "XCODEGEN_DIFF_EXIT=$?"

  echo "--- G6 advisory anti-slop (head) ---"
  swift run --package-path ios/tools/anti-slop-swift anti-slop \
    ios/FleetNotifier ios/FleetNotifierTests > /tmp/g551-aslop-head.log 2>&1
  echo "ASLOP_EXIT=$?"

  echo "--- G7 hygiene ---"
  git diff --check > /tmp/g551-diffcheck.log 2>&1; echo "DIFFCHECK_EXIT=$?"
fi

echo "DF_END: $(df -h / | tail -1)"
echo "G551_GATES_DONE"
