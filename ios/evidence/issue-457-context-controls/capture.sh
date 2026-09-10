#!/bin/bash
# #457 evidence capture: launch the Debug app with the deterministic
# -corral457ContextEvidence driver and screenshot each Documents/ux-evidence
# marker phase (1.5 s after it appears), stopping at the sentinel.
#
# usage: bash capture-457.sh <sim-udid> <prefix> <sentinel-marker> [launch args...]
set -u
DEV="$1"; PREFIX="$2"; SENTINEL="$3"; shift 3
APP=com.corral.fleetnotifier
OUT=/tmp/g457-raw-$PREFIX
mkdir -p "$OUT"

xcrun simctl terminate "$DEV" "$APP" >/dev/null 2>&1
APPDIR=$(xcrun simctl get_app_container "$DEV" "$APP" data 2>/dev/null)
if [ -n "$APPDIR" ]; then rm -rf "$APPDIR/Documents/ux-evidence"; fi

xcrun simctl launch "$DEV" "$APP" "$@" \
  || { echo "LAUNCH_FAILED"; exit 1; }

deadline=$((SECONDS+480))
while [ $SECONDS -lt $deadline ]; do
  APPDIR=$(xcrun simctl get_app_container "$DEV" "$APP" data 2>/dev/null)
  if [ -n "$APPDIR" ] && [ -d "$APPDIR/Documents/ux-evidence" ]; then
    for m in "$APPDIR"/Documents/ux-evidence/*.marker; do
      [ -e "$m" ] || continue
      name=$(basename "$m" .marker)
      if [ ! -f "$OUT/$name.png" ]; then
        sleep 1.5
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
