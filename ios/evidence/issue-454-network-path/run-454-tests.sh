#!/usr/bin/env bash
# #454 lane test runner (private simulator + private DerivedData, serialized on
# the shared heavy-gate lock). Usage:
#   run-454-tests.sh <label> <result-bundle.xcresult> [only-testing-spec ...]
# Every call is one xcodebuild invocation; the raw exit status is the gate
# status (never a downstream grep). Logs are captured by the caller.
set -uo pipefail

WORKTREE="${IMPL454_WORKTREE:-/Users/jirathip/.herdr/worktrees/corral/prep454-path-contract}"
UDID="${IMPL454_UDID:-57E9A1BE-2617-40CC-9F59-FADAB666FD0C}"
DD="${IMPL454_DD:-/tmp/fn-dd-454}"
LOCK="${IMPL454_LOCK:-/tmp/corral-heavy-gate.lock}"

LABEL="${1:?usage: run-454-tests.sh <label> <result-bundle.xcresult> [only-testing-spec ...]}"
BUNDLE="${2:?usage: run-454-tests.sh <label> <result-bundle.xcresult> [only-testing-spec ...]}"
shift 2

ONLY=()
for spec in "$@"; do
  ONLY+=("-only-testing:$spec")
done

cd "$WORKTREE/ios" || exit 99
echo "#454 runner: label=$LABEL udid=$UDID dd=$DD bundle=$BUNDLE only=${*:-<full suite>}"
/Users/jirathip/.local/bin/flock "$LOCK" env \
  HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 \
  xcodebuild test \
  -project FleetNotifier.xcodeproj \
  -scheme FleetNotifier \
  -destination "platform=iOS Simulator,id=$UDID" \
  -derivedDataPath "$DD" \
  -resultBundlePath "$BUNDLE" \
  "${ONLY[@]+"${ONLY[@]}"}"
