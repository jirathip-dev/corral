#!/usr/bin/env python3
"""Prove that the Release iOS app has no Debug demo entrypoint.

The source checks make the intended conditional-compilation boundary explicit;
the binary checks make the check build-derived rather than a substring-only
review of Swift source.  Debug still owns the demo fixtures and tests, while a
Release app must retain the real registration/SSE/drive client and error paths.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tempfile
from collections.abc import Callable
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent

SOURCE_MARKERS: dict[str, tuple[str, ...]] = {
    "ios/FleetNotifier/App/FleetNotifierApp.swift": (
        r"-demoMode",
        r"-liveVerify",
        r"LiveVerifyRunner",
        r"enterDemo",
    ),
    "ios/FleetNotifier/App/AppModel.swift": (
        r"\.demo\b",
        r"enterDemo",
        r"exitDemo",
        r"driveDemo",
        r"DemoFleet",
        r"seedDemo",
        r"upsertDemo",
        r"\(demo\)",
    ),
    "ios/FleetNotifier/App/FleetStore.swift": (
        r"seedDemo",
        r"upsertDemo",
    ),
    "ios/FleetNotifier/App/LiveVerifyRunner.swift": (
        r"-liveVerify",
        r"liveVerify",
        r"LiveVerifyRunner",
    ),
    "ios/FleetNotifier/Demo/DemoFleet.swift": (
        r"DemoFleet",
        r"demo-",
        r"demo mode",
    ),
    "ios/FleetNotifier/UI/FleetViews.swift": (
        r"\.demo\b",
        r"driveDemo",
        r"enterDemo",
        r"exitDemo",
        r"Demo mode",
        r"Exit demo",
        r"Try demo fleet",
        r"Seeded fake fleet",
    ),
}

SOURCE_REQUIRED: dict[str, tuple[str, ...]] = {
    "ios/FleetNotifier/App/FleetNotifierApp.swift": ("#if DEBUG", "-demoMode"),
    "ios/FleetNotifier/App/AppModel.swift": ("#if DEBUG", "func enterDemo"),
    "ios/FleetNotifier/App/FleetStore.swift": ("#if DEBUG", "func seedDemo"),
    "ios/FleetNotifier/App/LiveVerifyRunner.swift": (
        "#if DEBUG",
        "final class LiveVerifyRunner",
    ),
    "ios/FleetNotifier/Demo/DemoFleet.swift": ("#if DEBUG", "enum DemoFleet"),
    "ios/FleetNotifier/UI/FleetViews.swift": ("#if DEBUG", "Demo mode"),
}

RELEASE_SOURCE_REQUIRED: dict[str, tuple[str, ...]] = {
    "ios/FleetNotifier/App/FleetNotifierApp.swift": ("model.startLive()",),
    "ios/FleetNotifier/App/AppModel.swift": (
        "func register(",
        "func startLive()",
    ),
    "ios/FleetNotifier/UI/FleetViews.swift": (
        "model.driveReadTail(",
        "model.driveInterrupt(",
        "model.drivePrompt(",
        "model.driveApprove(",
        "model.handleCannedAction(",
    ),
}

BINARY_FORBIDDEN = (
    "-demoMode",
    "-liveVerify",
    "Demo mode",
    "Exit demo",
    "Try demo fleet",
    "Seeded fake fleet",
    "demo-approve",
    "demo-menu",
    "demo-question",
    "demo-crash",
    "demo-working",
    "demo-idle",
    "demo-done",
    "enterDemo",
    "exitDemo",
    "driveDemo",
    "seedDemo",
    "upsertDemo",
    "DemoFleet",
    "LiveVerifyRunner",
)

# Whole-module Release optimization may fold URL path literals into Foundation
# calls rather than leave the slash-prefixed spelling in a C string section.
# These type/error-path markers are the stable binary evidence, while the
# source checks above require the concrete registration/SSE/drive calls.
BINARY_REQUIRED = (
    "CorraldClient",
    "DriveClient",
    "RegisterResponse",
    "DriveResponse",
    "events failed:",
    "unparseable drive response",
)


class CheckFailure(RuntimeError):
    """A deterministic release-demo assertion failed."""


def _display_path(path: Path) -> str:
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def _code_without_line_comment(line: str) -> str:
    """Remove Swift's single-line comments for source-boundary checks."""

    return line.split("//", 1)[0]


def _source_states(text: str) -> list[tuple[str, bool]]:
    """Return each source line and whether it is DEBUG-only.

    This is deliberately a small conditional-compilation parser, not a Swift
    token search.  Unknown conditional expressions remain active for Debug but
    do not establish a DEBUG-only boundary; #else of DEBUG is never accepted.
    """

    stack: list[tuple[bool, bool]] = []
    states: list[tuple[str, bool]] = []
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("#if "):
            expression = stripped[4:].strip()
            if expression == "DEBUG":
                stack.append((True, True))
            elif expression == "!DEBUG":
                stack.append((True, False))
            else:
                stack.append((False, True))
            states.append((line, False))
            continue
        if stripped.startswith("#elseif "):
            if not stack:
                raise CheckFailure("#elseif without #if")
            expression = stripped[8:].strip()
            is_debug = expression in {"DEBUG", "!DEBUG"}
            active = expression == "DEBUG"
            stack[-1] = (is_debug, active)
            states.append((line, False))
            continue
        if stripped == "#else":
            if not stack:
                raise CheckFailure("#else without #if")
            is_debug, active = stack[-1]
            stack[-1] = (is_debug, not active)
            states.append((line, False))
            continue
        if stripped == "#endif":
            if not stack:
                raise CheckFailure("#endif without #if")
            stack.pop()
            states.append((line, False))
            continue

        active = all(item[1] for item in stack)
        guarded = active and any(item[0] and item[1] for item in stack)
        states.append((line, guarded))

    if stack:
        raise CheckFailure("unterminated conditional-compilation block")
    return states


def _check_source(path: Path, markers: tuple[str, ...]) -> None:
    text = path.read_text(encoding="utf-8")
    states = _source_states(text)
    for line_number, (line, debug_only) in enumerate(states, start=1):
        code = _code_without_line_comment(line)
        for marker in markers:
            if re.search(marker, code) and not debug_only:
                raise CheckFailure(
                    f"{_display_path(path)}:{line_number}: {marker!r} "
                    "is not behind #if DEBUG"
                )


def _check_required_source(path: Path, required: tuple[str, ...]) -> None:
    text = path.read_text(encoding="utf-8")
    for marker in required:
        if marker not in text:
            raise CheckFailure(f"{_display_path(path)} is missing {marker!r}")


def _check_release_source(path: Path, required: tuple[str, ...]) -> None:
    states = _source_states(path.read_text(encoding="utf-8"))
    for marker in required:
        matching = [
            (line_number, debug_only)
            for line_number, (line, debug_only) in enumerate(states, start=1)
            if marker in _code_without_line_comment(line)
        ]
        if not matching:
            raise CheckFailure(f"{_display_path(path)} is missing {marker!r}")
        if all(debug_only for _, debug_only in matching):
            locations = ", ".join(str(number) for number, _ in matching)
            raise CheckFailure(
                f"{_display_path(path)}: {marker!r} is only in DEBUG "
                f"(lines {locations})"
            )


def _binary_blob(binary: Path) -> str:
    strings = subprocess.run(
        ["strings", "-a", str(binary)],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    symbols = subprocess.run(
        ["nm", "-a", str(binary)],
        check=False,
        capture_output=True,
        text=True,
    ).stdout
    return f"{strings}\n{symbols}"


def _check_binary(binary: Path) -> None:
    if not binary.is_file():
        raise CheckFailure(f"Release executable does not exist: {binary}")
    blob = _binary_blob(binary)
    for marker in BINARY_FORBIDDEN:
        if marker in blob:
            raise CheckFailure(f"Release executable contains demo marker {marker!r}")
    for marker in BINARY_REQUIRED:
        if marker not in blob:
            raise CheckFailure(f"Release executable lost real-path marker {marker!r}")


def _expect_failure(action: Callable[[], None], label: str) -> None:
    try:
        action()
    except CheckFailure:
        return
    raise CheckFailure(f"self-test fixture was accepted: {label}")


def _self_test() -> None:
    """Exercise the source and binary assertions with known bad fixtures."""

    with tempfile.TemporaryDirectory(prefix="corral-release-demo-") as directory:
        root = Path(directory)
        unguarded = root / "unguarded.swift"
        unguarded.write_text("model.enterDemo()\n", encoding="utf-8")
        _expect_failure(
            lambda: _check_source(unguarded, (r"enterDemo",)),
            "unguarded demo call",
        )

        release_branch = root / "release-branch.swift"
        release_branch.write_text(
            "#if DEBUG\nlet ok = true\n#else\nmodel.enterDemo()\n#endif\n",
            encoding="utf-8",
        )
        _expect_failure(
            lambda: _check_source(release_branch, (r"enterDemo",)),
            "demo call in the release branch",
        )

        _expect_failure(
            lambda: _check_binary_text(
                "CorraldClient\nDriveClient\nRegisterResponse\nDriveResponse\n"
                "events failed:\nunparseable drive response\nDemo mode\n"
            ),
            "release user-facing demo string",
        )
        _check_binary_text(
            "CorraldClient\nDriveClient\nRegisterResponse\nDriveResponse\n"
            "events failed:\nunparseable drive response\n"
        )


def _check_binary_text(blob: str) -> None:
    for marker in BINARY_FORBIDDEN:
        if marker in blob:
            raise CheckFailure(f"fixture contains demo marker {marker!r}")
    for marker in BINARY_REQUIRED:
        if marker not in blob:
            raise CheckFailure(f"fixture lacks real-path marker {marker!r}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, help="Release app executable to inspect")
    parser.add_argument(
        "--self-test", action="store_true", help="run negative checker fixtures"
    )
    args = parser.parse_args()

    try:
        if args.self_test:
            _self_test()
        for relative, markers in SOURCE_MARKERS.items():
            _check_required_source(ROOT / relative, SOURCE_REQUIRED[relative])
            _check_source(ROOT / relative, markers)
        for relative, markers in RELEASE_SOURCE_REQUIRED.items():
            _check_release_source(ROOT / relative, markers)
        if args.binary:
            _check_binary(args.binary)
    except (CheckFailure, OSError, subprocess.CalledProcessError) as error:
        print(f"release-demo check: FAIL: {error}", file=sys.stderr)
        return 1

    print(
        "release-demo check: PASS (Debug source preserved; Release boundary verified)"
    )
    if args.binary:
        print(f"release-demo check: inspected {args.binary}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
