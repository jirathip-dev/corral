#!/usr/bin/env python3
"""#454 pre-review fixes (items 2 and 3) applied as an auditable patch pair.

Item 1 (real adapter restart) is NOT applied here: the ORCH Darwin probe and
this lane's iOS actual-adapter test both decide whether a change is warranted
("if premise disproved by actual raw evidence report it, do not implement a
speculative change").

apply   -> writes the fixes (refuses if already applied)
revert  -> restores the exact committed bytes and verifies the sha256
check   -> prints which variant is on disk + hashes
"""
from __future__ import annotations

import hashlib
import sys
from pathlib import Path

WORKTREE = Path("/Users/jirathip/.herdr/worktrees/corral/prep454-path-contract")
APP_MODEL = WORKTREE / "ios/FleetNotifier/App/AppModel.swift"
COORDINATOR = WORKTREE / "ios/FleetNotifier/Profiles/HostStreamCoordinator.swift"

# Committed (base) bytes at 4d3f0e0 — the pre-review state.
BASE_SHA = {
    APP_MODEL: "0c5188db0cb2ce1e2f13bb6cc1612018c381a3d8adac66acfbdacb6cbb421dd0",
    COORDINATOR: "ccf2ff1e310173f082c7c9f058d9cdc9938b004beb58b8c78d1a29a706f21c20",
}

# ---------------------------------------------------------------- fix 1 (adapter)
F1_BASE = """final class SystemNetworkPathMonitor: NetworkPathMonitoring {
    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "corral.network-path-monitor")

    func start(_ handler: @escaping @Sendable (Bool) -> Void) {
        monitor.pathUpdateHandler = { path in
            let satisfied = path.status == .satisfied
            Task { @MainActor in handler(satisfied) }
        }
        monitor.start(queue: queue)
    }

    func stop() {
        monitor.cancel()
    }
}
"""
F1_FIXED = """final class SystemNetworkPathMonitor: NetworkPathMonitoring {
    private let queue = DispatchQueue(label: "corral.network-path-monitor")
    /// #454 pre-review fix 1: the underlying monitor is created FRESH on every
    /// logical start. The lane's actual-adapter test OBSERVED, for this Swift
    /// `NWPathMonitor` in-process on iOS, that a cancelled instance delivers NO
    /// further path update — so reusing one would silently stop path
    /// observation after the first background → foreground cycle.
    private var monitor: NWPathMonitor?

    func start(_ handler: @escaping @Sendable (Bool) -> Void) {
        // A start while one is already live replaces it (the owner is
        // idempotent, so this is not a normal path).
        monitor?.cancel()
        let monitor = NWPathMonitor()
        self.monitor = monitor
        monitor.pathUpdateHandler = { path in
            let satisfied = path.status == .satisfied
            Task { @MainActor in handler(satisfied) }
        }
        monitor.start(queue: queue)
    }

    func stop() {
        monitor?.cancel()
        monitor = nil
    }
}
"""

# ---------------------------------------------------------------- fix 2 (AppModel)
F2_BASE = """    /// #454: true only while the live session owns the monitor — every
    /// callback is refused after stopLive (background/removal boundary).
    private var pathMonitorStarted = false
"""
F2_FIXED = """    /// #454: true only while the live session owns the monitor — every
    /// callback is refused after stopLive (background/removal boundary).
    private var pathMonitorStarted = false
    /// #454 pre-review fix 2: monotonic generation of the logical monitor
    /// session. A callback already QUEUED from a retired generation must never
    /// be applied to its successor (start → stop → start would otherwise let a
    /// stale update drive the restarted session's hint machinery).
    private var pathMonitorGeneration = 0
"""

F2_START_BASE = """    private func startPathMonitor() {
        guard !pathMonitorStarted else { return }
        pathMonitorStarted = true
        pathWasUnsatisfied = false
        pathMonitor.start { [weak self] satisfied in
            Task { @MainActor in self?.handlePathStatus(satisfied) }
        }
    }
"""
F2_START_FIXED = """    private func startPathMonitor() {
        guard !pathMonitorStarted else { return }
        pathMonitorStarted = true
        pathWasUnsatisfied = false
        // #454 pre-review fix 2: this logical start is a NEW generation; the
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
    }
"""

# ------------------------------------------------------- fix 3 (AppModel defer)
F3_DEFER_BASE = """            defer {
                if self.keyContinuityTaskId == taskId {
                    self.keyContinuityTask = nil
                    self.keyContinuityTaskId = nil
                }
                // #454: a finished/cancelled ladder is never "waiting".
                self.keyContinuityWaiting = false
            }
"""
F3_DEFER_FIXED = """            defer {
                // #454 pre-review fix 3: ONLY the ladder that still owns the
                // handle may clear its own waiting state. A retired ladder's
                // cancellation cleanup must never erase a successor's wait
                // (the successor's re-drive would then silently not happen).
                if self.keyContinuityTaskId == taskId {
                    self.keyContinuityTask = nil
                    self.keyContinuityTaskId = nil
                    self.keyContinuityWaiting = false
                }
            }
"""

F3_STOPLIVE_BASE = """        keyContinuityTask?.cancel()
        keyContinuityTask = nil
        keyContinuityTaskId = nil
        // #399: mirror the active profile's cursor into the profile store
"""
F3_STOPLIVE_FIXED = """        keyContinuityTask?.cancel()
        keyContinuityTask = nil
        keyContinuityTaskId = nil
        // #454 pre-review fix 3: the background boundary resets the ladder's
        // waiting state explicitly (the retired ladder's own cleanup no longer
        // clears it — see beginKeyContinuityCheck).
        keyContinuityWaiting = false
        // #399: mirror the active profile's cursor into the profile store
"""

# ------------------------------------------------- fix 3 (coordinator defer)
F3_COORD_BASE = """            defer {
                if session.continuityGeneration == ladder {
                    session.continuityTask = nil
                }
                // #454: a finished/cancelled ladder is never "waiting".
                session.continuityWaiting = false
            }
"""
F3_COORD_FIXED = """            defer {
                // #454 pre-review fix 3: only the owning ladder clears its own
                // waiting state — a retired ladder's cleanup must never erase
                // its successor's wait.
                if session.continuityGeneration == ladder {
                    session.continuityTask = nil
                    session.continuityWaiting = false
                }
            }
"""

EDITS = [
    (APP_MODEL, F1_BASE, F1_FIXED),
    (APP_MODEL, F2_BASE, F2_FIXED),
    (APP_MODEL, F2_START_BASE, F2_START_FIXED),
    (APP_MODEL, F3_DEFER_BASE, F3_DEFER_FIXED),
    (APP_MODEL, F3_STOPLIVE_BASE, F3_STOPLIVE_FIXED),
    (COORDINATOR, F3_COORD_BASE, F3_COORD_FIXED),
]


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    action = sys.argv[1] if len(sys.argv) > 1 else "check"
    texts = {path: path.read_text() for path in {p for p, _, _ in EDITS}}
    applied = all(fixed in texts[path] for path, _, fixed in EDITS)
    base = all(base_text in texts[path] for path, base_text, _ in EDITS)

    if action == "check":
        state = "FIXED" if applied else ("BASE" if base else "UNKNOWN")
        print(f"{state}")
        for path in sorted(texts, key=str):
            print(f"  {sha256(path)}  {path.relative_to(WORKTREE)}")
        return 0

    if action == "apply":
        if applied:
            print("already FIXED")
            return 0
        if not base:
            print("FAIL: base bytes not found — refusing to patch")
            return 1
        for path, base_text, fixed in EDITS:
            texts[path] = texts[path].replace(base_text, fixed, 1)
        for path, text in texts.items():
            path.write_text(text)
        print("applied")
        for path in sorted(texts, key=str):
            print(f"  {sha256(path)}  {path.relative_to(WORKTREE)}")
        return 0

    if action == "revert":
        if base:
            print("already BASE")
            return 0
        if not applied:
            print("FAIL: neither base nor fixed bytes found — refusing")
            return 1
        for path, base_text, fixed in EDITS:
            texts[path] = texts[path].replace(fixed, base_text, 1)
        for path, text in texts.items():
            path.write_text(text)
        print("reverted")
        for path in sorted(texts, key=str):
            digest = sha256(path)
            expected = BASE_SHA.get(path)
            mark = "OK" if digest == expected else f"MISMATCH expected {expected}"
            print(f"  {digest}  {mark}  {path.relative_to(WORKTREE)}")
        return 0

    print(f"unknown action: {action}")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
