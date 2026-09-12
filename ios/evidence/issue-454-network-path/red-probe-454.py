#!/usr/bin/env python3
"""#454 RED probe: revert the lane to the EXACT pre-#454 behavior — no path
integration is wired — while keeping the new seam + tests compiling.

The probe swaps the `pathMonitor.start { ... }` wiring in
`AppModel.startPathMonitor()` for a comment. That is the original reviewed
state of the source (issue #454: "the reviewed iOS source contains no
NWPathMonitor integration"): tests still compile and drive the seam, but no
path signal can ever reach the model, so every acceleration assertion must go
RED.

Usage:
  apply   -> writes the probe (refuses if already applied)
  revert  -> restores the exact GREEN bytes and verifies the sha256
  check   -> prints which variant is on disk
"""
from __future__ import annotations

import hashlib
import sys
from pathlib import Path

WORKTREE = Path("/Users/jirathip/.herdr/worktrees/corral/prep454-path-contract")
APP_MODEL = WORKTREE / "ios/FleetNotifier/App/AppModel.swift"

GREEN_WIRING = """        pathMonitor.start { [weak self] satisfied in
            Task { @MainActor in self?.handlePathStatus(satisfied) }
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
    has_green = GREEN_WIRING in text
    has_red = RED_WIRING in text
    if action == "check":
        state = "GREEN" if has_green else ("RED" if has_red else "UNKNOWN")
        print(f"{state} {sha256(APP_MODEL)}")
        return 0
    if action == "apply":
        if has_red:
            print("already RED")
            return 0
        if not has_green:
            print("FAIL: green wiring not found — refusing to probe")
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
            print("FAIL: red probe not found — refusing to rewrite")
            return 1
        APP_MODEL.write_text(text.replace(RED_WIRING, GREEN_WIRING, 1))
        print(f"reverted sha256={sha256(APP_MODEL)}")
        return 0
    print(f"unknown action: {action}")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
