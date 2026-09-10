#!/bin/bash
# #459 wind evidence driver — records a real-time, phone-scale clip of the
# production herd scene from an installed Debug build on a booted simulator.
#
# The launch rides the app's existing DEBUG demo route plus real
# UserDefaults keys: -demoMode seeds the fictional demo fleet,
# -fleetnotifier.fleetPresentation Herd opens the actual Herd scene, and
# -herdEnvironment Day pins the lighting so the Day palette is captured.
# No repository source is modified by this driver.
#
# Usage: capture-wind.sh <udid> <FleetNotifier.app> <out.mp4> <seconds> [extra args...]
# Preconditions: the simulator is booted and already booted by the caller;
# the caller owns the /tmp/corral-heavy-gate.lock heavy-gate hold.
set -euo pipefail

UDID="$1"; APP="$2"; OUT="$3"; SECONDS_TO_RECORD="$4"; shift 4

xcrun simctl terminate "$UDID" com.corral.fleetnotifier >/dev/null 2>&1 || true
xcrun simctl install "$UDID" "$APP"
rm -f "$OUT"
xcrun simctl io "$UDID" recordVideo --codec h264 --force "$OUT" &
RECORDER=$!
sleep 1
xcrun simctl launch "$UDID" com.corral.fleetnotifier \
    -demoMode -fleetnotifier.fleetPresentation Herd -herdEnvironment Day "$@"
sleep "$SECONDS_TO_RECORD"
kill -INT "$RECORDER"
wait "$RECORDER" || true
xcrun simctl terminate "$UDID" com.corral.fleetnotifier >/dev/null 2>&1 || true
echo "captured $OUT ($(stat -f%z "$OUT") bytes)"
