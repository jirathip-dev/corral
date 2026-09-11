#!/bin/bash
# #457 evidence capture runner — all chains on the lane's own simulators.
# Each chain: set the environment, launch the Debug app with the
# deterministic driver, shoot every ux-evidence marker phase, stop at the
# sentinel. Raw frames land in /tmp/g457-raw-<prefix>/.
set -u
R1=C5B4EA29-00D4-4250-9F8E-C6E3DBE208CB   # Corral457R1  (iPhone 16, iOS 26.5)
SE=3A0B36C6-75F8-43A8-98B2-5CC4E6FA8C66   # Corral457R1SE (iPhone SE 3rd gen)
APP=/tmp/g457-dd/Build/Products/Debug-iphonesimulator/FleetNotifier.app
STATUS=/tmp/g457-capture-status.txt
: > "$STATUS"

echo "CAP_HEAD=$(git rev-parse HEAD)" >> "$STATUS"

# --- install on both owned sims (serialized boot/install) ---
xcrun simctl boot $R1 2>&1 | grep -v "already booted"
xcrun simctl bootstatus $R1 -b > /dev/null 2>&1; echo "R1_BOOT_EXIT=$?" >> "$STATUS"
xcrun simctl install $R1 "$APP"; echo "R1_INSTALL_EXIT=$?" >> "$STATUS"
xcrun simctl boot $SE 2>&1 | grep -v "already booted"
xcrun simctl bootstatus $SE -b > /dev/null 2>&1; echo "SE_BOOT_EXIT=$?" >> "$STATUS"
xcrun simctl install $SE "$APP"; echo "SE_INSTALL_EXIT=$?" >> "$STATUS"

# --- chain 1: iPhone 16, herd sequence (day/night four-flavor matrix) ---
xcrun simctl ui $R1 appearance light > /dev/null 2>&1
xcrun simctl ui $R1 content_size large > /dev/null 2>&1
bash /tmp/g457-capture.sh $R1 r1-herd 457-12-herd-done -demoMode -corral457ContextEvidence
echo "R1_HERD_CAP_EXIT=$?" >> "$STATUS"

# --- chain 2: iPhone 16, board no-regression shot ---
bash /tmp/g457-capture.sh $R1 r1-board 457-21-board-done -demoMode -corral457BoardShot
echo "R1_BOARD_CAP_EXIT=$?" >> "$STATUS"

# --- chain 3: iPhone 16, forced-opaque chrome (Reduce Transparency path) ---
bash /tmp/g457-capture.sh $R1 r1-opaque 457-12-herd-done -demoMode -corral457ContextEvidence -corral457ForceOpaqueChrome
echo "R1_OPAQUE_CAP_EXIT=$?" >> "$STATUS"

# --- chain 4: iPhone 16, REAL Increase Contrast signal ---
xcrun simctl ui $R1 increase_contrast enabled > /dev/null 2>&1
bash /tmp/g457-capture.sh $R1 r1-hc 457-12-herd-done -demoMode -corral457ContextEvidence
echo "R1_HC_CAP_EXIT=$?" >> "$STATUS"
xcrun simctl ui $R1 increase_contrast disabled > /dev/null 2>&1

# --- chain 5: iPhone 16, AX-XXXL content size ---
xcrun simctl ui $R1 content_size accessibility-extra-extra-extra-large > /dev/null 2>&1
bash /tmp/g457-capture.sh $R1 r1-ax 457-12-herd-done -demoMode -corral457ContextEvidence
echo "R1_AX_CAP_EXIT=$?" >> "$STATUS"
xcrun simctl ui $R1 content_size large > /dev/null 2>&1

# --- chain 6: iPhone SE 3rd gen, herd sequence ---
xcrun simctl ui $SE appearance light > /dev/null 2>&1
xcrun simctl ui $SE content_size large > /dev/null 2>&1
bash /tmp/g457-capture.sh $SE se-herd 457-12-herd-done -demoMode -corral457ContextEvidence
echo "SE_HERD_CAP_EXIT=$?" >> "$STATUS"

# --- chain 7: iPhone SE 3rd gen, AX-XXXL ---
xcrun simctl ui $SE content_size accessibility-extra-extra-extra-large > /dev/null 2>&1
bash /tmp/g457-capture.sh $SE se-ax 457-12-herd-done -demoMode -corral457ContextEvidence
echo "SE_AX_CAP_EXIT=$?" >> "$STATUS"
xcrun simctl ui $SE content_size large > /dev/null 2>&1

# --- shutdown both owned sims (no erase/delete) ---
xcrun simctl shutdown $R1; echo "R1_SHUTDOWN_EXIT=$?" >> "$STATUS"
xcrun simctl shutdown $SE; echo "SE_SHUTDOWN_EXIT=$?" >> "$STATUS"

for d in /tmp/g457-raw-*; do
  echo "FRAMES_$(basename "$d")=$(ls "$d" | wc -l | tr -d ' ')" >> "$STATUS"
done
echo "CAP_CHAIN_DONE=1" >> "$STATUS"
cat "$STATUS"
