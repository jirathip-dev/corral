#!/usr/bin/env python3
"""#454 RED probe: revert the lane to the EXACT pre-#454 behavior — no path
integration is wired — while keeping the new seam + tests compiling.

The probe swaps the `pathMonitor.start { ... }` wiring in
`AppModel.startPathMonitor()` — together with the generation-guard setup that
wiring carries — for a comment. That is the original reviewed state of the
source (issue #454: "the reviewed iOS source contains no NWPathMonitor
integration"): tests still compile and drive the seam, but no path signal can
ever reach the model, so every acceleration assertion must go RED.

MAINTENANCE (#523): GREEN_WIRING tracks the LIVE wiring in
`startPathMonitor()`. When that wiring legitimately changes (new guard, new
callback shape), re-derive GREEN_WIRING from the current `AppModel.swift` and
keep RED_WIRING a compiling stand-in for "no path integration is wired" — a
~30-second edit. A stale anchor makes the probe fail closed (exit 1, nothing
written) instead of probing the wrong thing; never loosen the anchor to a
non-unique fragment (a single match is asserted before splicing). The sibling
`pre-review-fixes-454.py` anchors (`F2_START_FIXED`) track the same wiring and
must move with it.

The worktree is overridable with IMPL454_WORKTREE (same convention as
`run-454-tests.sh`), so the probe runs against any checkout, not only the
original lane worktree.

Usage:
  apply   -> writes the probe (refuses if already applied)
  revert  -> restores the exact GREEN bytes and verifies the sha256
  check   -> prints which variant is on disk
"""
from __future__ import annotations

import hashlib
import os
import sys
from pathlib import Path

WORKTREE = Path(
    os.environ.get(
        "IMPL454_WORKTREE",
        "/Users/jirathip/.herdr/worktrees/corral/prep454-path-contract",
    )
)
APP_MODEL = WORKTREE / "ios/FleetNotifier/App/AppModel.swift"

# #523: the LIVE `startPathMonitor()` wiring, INCLUDING the #454 pre-review
# generation guard. Must match `AppModel.swift` byte-for-byte and exactly once
# (see MAINTENANCE above).
GREEN_WIRING = """        // #454 pre-review fix 2: this logical start is a NEW generation; the
        // callback carries it so a retired monitor's queued updates are
        // refused instead of driving the restarted session.
        pathMonitorGeneration &+= 1
        let generation = pathMonitorGeneration
        pathMonitor.start { [weak self] satisfied in
            Task { @MainActor in
                guard let self, self.pathMonitorGeneration == generation else { return }
                self.handlePathStatus(satisfied)
            }
        }
"""

RED_WIRING = """        // #454 RED PROBE (temporary, reverted before commit): the pre-#454
        // state has NO path integration — the monitor is never wired, so no
        // path signal can reach the model. This is the exact behavior the
        // issue describes at the reviewed head.
"""


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    action = sys.argv[1] if len(sys.argv) > 1 else "check"
    text = APP_MODEL.read_text()
    # Exact-and-unique anchors: a single match IS the state; anything else
    # (duplicate or absent) refuses to splice — fail closed.
    green_matches = text.count(GREEN_WIRING)
    red_matches = text.count(RED_WIRING)
    has_green = green_matches == 1
    has_red = red_matches == 1
    if action == "check":
        state = "GREEN" if has_green else ("RED" if has_red else "UNKNOWN")
        print(f"{state} {sha256(APP_MODEL)}")
        return 0
    if action == "apply":
        if has_red:
            print("already RED")
            return 0
        if not has_green:
            print(
                "FAIL: green wiring not found"
                f" (anchor matches={green_matches}, want 1) — refusing to probe"
            )
            return 1
        before = sha256(APP_MODEL)
        APP_MODEL.write_text(text.replace(GREEN_WIRING, RED_WIRING, 1))
        print(f"applied  green_sha256={before} red_sha256={sha256(APP_MODEL)}")
        return 0
    if action == "revert":
        if has_green:
            print("already GREEN")
            return 0
        if not has_red:
            print(
                "FAIL: red probe not found"
                f" (anchor matches={red_matches}, want 1) — refusing to rewrite"
            )
            return 1
        APP_MODEL.write_text(text.replace(RED_WIRING, GREEN_WIRING, 1))
        print(f"reverted sha256={sha256(APP_MODEL)}")
        return 0
    print(f"unknown action: {action}")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
