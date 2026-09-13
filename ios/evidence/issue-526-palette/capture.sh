#!/bin/bash
# #526 evidence capture: launch the Debug app with a deterministic
# -corral526*Evidence driver and screenshot each Documents/ux-evidence
# marker phase, stopping at the sentinel marker.
#
# The driver writes each marker AFTER the phase state settles and holds
# >= 9 s (the #449/#528 convention); this script shoots 2 s after a NEW
# marker first appears, so a capture can never drift into the next phase.
#
# usage: bash capture.sh <sim-udid> <prefix> <sentinel-marker> [launch args...]
set -u
DEV="$1"; PREFIX="$2"; SENTINEL="$3"; shift 3
APP=com.corral.fleetnotifier
OUT=/tmp/g526-raw-$PREFIX
rm -rf "$OUT"
mkdir -p "$OUT"

xcrun simctl terminate "$DEV" "$APP" >/dev/null 2>&1
APPDIR=$(xcrun simctl get_app_container "$DEV" "$APP" data 2>/dev/null)
if [ -n "$APPDIR" ]; then rm -rf "$APPDIR/Documents/ux-evidence"; fi

xcrun simctl launch "$DEV" "$APP" "$@" \
  || { echo "LAUNCH_FAILED"; exit 1; }

deadline=$((SECONDS+600))
while [ $SECONDS -lt $deadline ]; do
  APPDIR=$(xcrun simctl get_app_container "$DEV" "$APP" data 2>/dev/null)
  if [ -n "$APPDIR" ] && [ -d "$APPDIR/Documents/ux-evidence" ]; then
    for m in "$APPDIR"/Documents/ux-evidence/*.marker; do
      [ -e "$m" ] || continue
      name=$(basename "$m" .marker)
      if [ ! -f "$OUT/$name.png" ]; then
        sleep 2
        xcrun simctl io "$DEV" screenshot "$OUT/$name.png" >/dev/null 2>&1 \
          && echo "SHOT $name"
      fi
      if [ "$name" = "$SENTINEL" ]; then
        echo "SENTINEL_REACHED $name"
        exit 0
      fi
    done
  fi
  sleep 0.5
done
echo "TIMEOUT_WITHOUT_SENTINEL"
exit 1
