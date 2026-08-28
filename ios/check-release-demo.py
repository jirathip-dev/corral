#!/usr/bin/env python3
"""Prove that the Release iOS app has no Debug demo entrypoint.

The source checks make the intended conditional-compilation boundary explicit;
the binary checks make the check build-derived rather than a substring-only
review of Swift source.  Debug still owns the demo fixtures and tests, while a
Release app must retain the real registration/SSE/drive client and error paths
in every architecture slice.

The source proof is intentionally limited to the supported conditional expressions
``true``, ``false``, ``DEBUG``, and ``!DEBUG``.  Comments and inactive branches
are not evidence, and any other conditional expression fails closed.  Binary
proof requires every architecture slice to be an iOS or iOS Simulator Mach-O
before inspecting that slice's strings and symbols independently.  The
Release source digest is a consistency marker emitted by the declared build
phase; it provides no cryptographic authenticity, code signing, or protection
against a builder deliberately reusing or forging an unkeyed marker.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tempfile
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from release_source_manifest import (
    RELEASE_SOURCE_FILES,
    release_source_digest,
    source_digest_marker,
)


ROOT = Path(__file__).resolve().parent.parent

# This binds an inspected executable to the exact Release app source set used
# for the approved build.  The declared build phase generates the marker from
# its actual inputs; this pinned hash is the expected source set for this
# checkout.  It catches ordinary source drift or a mismatched artifact when
# that phase runs; it is not an authenticity mechanism.
# #209 Devices & Grants surface (grouping, toggles, host admin) updated the
# Manifest-listed Swift sources; the pin follows the new source set.
# #253 read_tail scrub: TUI-furniture divider fallback added to
# RecentOutputModel (isDividerRun) and the Recent row view (Divider).
# #245 Fleet compact spacing: FleetViews.swift updated (manual refresh
# removed, lowercase section headers, tightened spacing); the pin follows.
# #246 Agent sheet redesign (Variant 2): compact bottom toolbar in
# AgentDetailContent + toolbar slot in RecentOutputView/BeforeView.
# #232 read_diff: ± Diff toolbar button + AgentDiffSheet in FleetViews;
# Capability.readDiff + DiffPageWire/DiffPane in Models; readDiff payload in
# CanonicalJSON; diff cache in FleetStore/AppModel; the pin follows.
# #210 fleet-health strip (HEALTH ONLY): FleetHealthEntry in Models +
# Snapshot.fleetHealth decode (tolerant of omitted empty warnings);
# strip in FleetViews chrome; FleetStore fleetHealth projection; BoardModel
# fleetHealthLines + heartbeatAge; demo seedHealth in DemoFleet; the pin
# follows.
# #267: read-only issue browser — Capability.readIssues, GhIssueRef
# body/labels/comment window, IssuesBrowserWire + pane, demo seedIssues,
# IssuesBrowserView (chips + inline detail).
APPROVED_RELEASE_SOURCE_DIGEST = (
    # #290: source-revision tail cursor wiring.
    "f1c1d06c70adf612619f7f235383c6d86399693c7af915ab929a19aba6e1b569"
)
RELEASE_SOURCE_DIGEST_MARKER = source_digest_marker(APPROVED_RELEASE_SOURCE_DIGEST)
RELEASE_BUILD_INPUTS = tuple(
    f"$(SRCROOT)/{relative.removeprefix('ios/')}" for relative in RELEASE_SOURCE_FILES
) + (
    "$(SRCROOT)/embed-release-source-digest.py",
    "$(SRCROOT)/release_source_manifest.py",
)
RELEASE_BUILD_OUTPUT = "$(DERIVED_FILE_DIR)/corral-release-source-digest"

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
        r"\(demo\)",
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
        "func driveKill(",
        "func driveAttach(",
        # #241 removed the transcript surface; #209 added the host-admin
        # grants surface. These markers pin the Release app to the real
        # client paths below.
        "func refreshGrants(",
        "func loadAdminDevices(",
    ),
    "ios/FleetNotifier/UI/FleetViews.swift": (
        "model.driveReadTail(",
        "model.driveInterrupt(",
        "model.driveKill(",
        "model.driveAttach(",
        "model.drivePrompt(",
        "model.loadEarlierOutput(",
        "model.loadAdminDevices(",
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
BINARY_REQUIRED_BASE = (
    "CorraldClient",
    "DriveClient",
    "RegisterResponse",
    "DriveResponse",
    "events failed:",
    "unparseable drive response",
)
BINARY_REQUIRED = (*BINARY_REQUIRED_BASE, RELEASE_SOURCE_DIGEST_MARKER)

SUPPORTED_CONDITIONS = {"true", "false", "DEBUG", "!DEBUG"}

# A decoded line separator must never become a real newline in the string
# view: that would shift source-line alignment with the syntax view.  Keep a
# canonical escaped spelling instead.  It is deliberately not equivalent to
# the decoded character for marker matching, so a malformed or split literal
# cannot accidentally satisfy a marker.
UNICODE_LINE_SEPARATOR_SCALARS = frozenset({0x000A, 0x000D, 0x0085, 0x2028, 0x2029})

# These are intentionally limited to user-facing/argument literals.  Syntax
# markers such as ``enterDemo`` and ``func register(`` must only be found in
# the string-masked code view, so a prose string can never impersonate a
# declaration or call.  Literal demo labels/arguments still need the
# comment-masked string view to prove that they are Debug-only.
SOURCE_STRING_MARKERS = frozenset(
    {
        "-demoMode",
        "-liveVerify",
        "Demo mode",
        "Exit demo",
        "Try demo fleet",
        "Seeded fake fleet",
        "demo-",
        "demo mode",
        r"\(demo\)",
    }
)


class CheckFailure(RuntimeError):
    """A deterministic release-demo assertion failed."""


@dataclass(frozen=True)
class _SourceLine:
    code: str
    strings: str
    debug_active: bool
    release_active: bool


@dataclass(frozen=True)
class _ConditionalDirective:
    kind: str
    expression: str
    parent_debug: bool
    parent_release: bool


@dataclass(frozen=True)
class _SourceAnalysis:
    lines: tuple[_SourceLine, ...]
    directives: tuple[_ConditionalDirective, ...]


@dataclass
class _ConditionalFrame:
    parent_debug: bool
    parent_release: bool
    taken_debug: bool
    taken_release: bool
    current_debug: bool
    current_release: bool
    saw_else: bool = False


def _display_path(path: Path) -> str:
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def _blank(character: str) -> str:
    return character if character in "\r\n" else " "


def _string_escape_width(
    text: str, index: int, string_hashes: int, string_closer: str
) -> int:
    """Return the source width of one Swift string escape, or zero.

    Extended delimiters require exactly the opening delimiter's number of
    pounds after the backslash.  For a multiline delimiter, the escaped unit
    can be the full triple-quote sequence; a trailing raw-string pound belongs
    to the content, not to that escaped sequence.
    """

    prefix = "\\" + ("#" * string_hashes)
    if not text.startswith(prefix, index):
        return 0
    escaped_start = index + len(prefix)
    if escaped_start >= len(text):
        return len(prefix)
    if string_closer.startswith('"""') and text.startswith('"""', escaped_start):
        return len(prefix) + 3
    return len(prefix) + 1


def _unicode_scalar_escape(
    text: str, index: int, string_hashes: int
) -> tuple[int, str] | None:
    """Decode one valid Swift Unicode scalar escape in a string literal."""

    prefix = "\\" + ("#" * string_hashes)
    unicode_prefix = f"{prefix}u"
    if not text.startswith(unicode_prefix, index):
        return None
    braced_prefix = f"{unicode_prefix}{{"
    if not text.startswith(braced_prefix, index):
        raise CheckFailure("unsupported Swift Unicode escape form")
    end = text.find("}", index + len(braced_prefix))
    if end < 0:
        raise CheckFailure("unterminated Swift Unicode scalar escape")
    digits = text[index + len(braced_prefix) : end]
    if not re.fullmatch(r"[0-9A-Fa-f]{1,8}", digits):
        raise CheckFailure("malformed Swift Unicode scalar escape")
    scalar = int(digits, 16)
    if scalar > 0x10FFFF or 0xD800 <= scalar <= 0xDFFF:
        raise CheckFailure("invalid Swift Unicode scalar escape")
    if scalar in UNICODE_LINE_SEPARATOR_SCALARS:
        decoded = f"\\u{{{scalar:X}}}"
    else:
        decoded = chr(scalar)
    return end - index + 1, decoded


def _string_opening_at(text: str, index: int) -> tuple[int, bool] | None:
    hash_index = index
    while hash_index < len(text) and text[hash_index] == "#":
        hash_index += 1
    if hash_index >= len(text) or text[hash_index] != '"':
        return None
    hashes = hash_index - index
    return hashes, text.startswith('"""', hash_index)


def _lex_source(text: str) -> tuple[str, str]:
    """Return syntax- and string-aware views with comments masked.

    The first result masks all Swift string contents as well as comments, so
    declarations/calls in prose cannot satisfy source evidence.  The second
    preserves string contents but masks comments for the small set of
    user-facing literal markers.  Ordinary, escaped, raw, multiline, and raw
    multiline strings are handled; Swift block comments may nest.
    """

    code: list[str] = []
    strings: list[str] = []

    def append_masked(source: str) -> None:
        code.extend(_blank(item) for item in source)
        strings.extend(_blank(item) for item in source)

    def append_code(source: str) -> None:
        code.extend(source)
        strings.extend(source)

    def append_interpolation_code(source: str) -> None:
        """Keep interpolation expressions in code, but not in literal view."""

        code.extend(source)
        strings.extend(_blank(item) for item in source)

    def scan_line_comment(index: int) -> int:
        while index < len(text):
            character = text[index]
            append_masked(character)
            index += 1
            if character in "\r\n":
                return index
        return index

    def scan_block_comment(index: int) -> int:
        depth = 1
        while index < len(text):
            if text.startswith("/*", index):
                append_masked("/*")
                index += 2
                depth += 1
                continue
            if text.startswith("*/", index):
                append_masked("*/")
                index += 2
                depth -= 1
                if depth == 0:
                    return index
                continue
            append_masked(text[index])
            index += 1
        raise CheckFailure("unterminated Swift block comment")

    def scan_interpolation(index: int) -> int:
        depth = 1
        while index < len(text):
            if text.startswith("//", index):
                append_masked("//")
                index = scan_line_comment(index + 2)
                continue
            if text.startswith("/*", index):
                append_masked("/*")
                index = scan_block_comment(index + 2)
                continue
            opening = _string_opening_at(text, index)
            if opening is not None:
                hashes, multiline = opening
                index = scan_string(index, hashes, multiline)
                continue
            character = text[index]
            if character == "(":
                append_interpolation_code(character)
                depth += 1
                index += 1
                continue
            if character == ")":
                append_interpolation_code(character)
                depth -= 1
                index += 1
                if depth == 0:
                    return index
                continue
            append_interpolation_code(character)
            index += 1
        raise CheckFailure("unterminated Swift string interpolation")

    def scan_string(index: int, hashes: int, multiline: bool) -> int:
        quote = '"""' if multiline else '"'
        opening = ("#" * hashes) + quote
        closing = quote + ("#" * hashes)
        append_masked(opening)
        index += len(opening)
        interpolation_prefix = "\\" + ("#" * hashes)

        while index < len(text):
            scalar_escape = _unicode_scalar_escape(text, index, hashes)
            if scalar_escape is not None:
                width, decoded = scalar_escape
                code.extend(_blank(item) for item in text[index : index + width])
                strings.append(decoded)
                index += width
                continue

            interpolation_opening = interpolation_prefix + "("
            if text.startswith(interpolation_opening, index):
                append_masked(interpolation_opening)
                index = scan_interpolation(index + len(interpolation_opening))
                continue

            escape_width = _string_escape_width(text, index, hashes, closing)
            if escape_width:
                escaped_text = text[index : index + escape_width]
                append_masked(escaped_text)
                strings.extend(escaped_text)
                index += escape_width
                continue

            if text.startswith(closing, index):
                append_masked(closing)
                return index + len(closing)
            append_masked(text[index])
            strings[-1] = text[index]
            index += 1
        raise CheckFailure("unterminated Swift string literal")

    def scan_code(index: int) -> int:
        while index < len(text):
            if text.startswith("//", index):
                append_masked("//")
                index = scan_line_comment(index + 2)
                continue
            if text.startswith("/*", index):
                append_masked("/*")
                index = scan_block_comment(index + 2)
                continue
            opening = _string_opening_at(text, index)
            if opening is not None:
                hashes, multiline = opening
                index = scan_string(index, hashes, multiline)
                continue
            append_code(text[index])
            index += 1
        return index

    scan_code(0)
    return "".join(code), "".join(strings)


def _parse_directive(line: str) -> tuple[str, str | None] | None:
    stripped = line.strip()
    if not stripped:
        return None
    if stripped.startswith("#if"):
        match = re.fullmatch(r"#if\s+(.+)", stripped)
        if match is None:
            raise CheckFailure(f"malformed conditional directive {stripped!r}")
        return "if", match.group(1).strip()
    if stripped.startswith("#elseif"):
        match = re.fullmatch(r"#elseif\s+(.+)", stripped)
        if match is None:
            raise CheckFailure(f"malformed conditional directive {stripped!r}")
        return "elseif", match.group(1).strip()
    if stripped == "#else":
        return "else", None
    if stripped == "#endif":
        return "endif", None
    return None


def _condition_value(expression: str) -> tuple[bool, bool]:
    normalized = " ".join(expression.split())
    if normalized not in SUPPORTED_CONDITIONS:
        raise CheckFailure(
            "unsupported conditional-compilation expression "
            f"{expression!r}; refusing to assume it is active"
        )
    if normalized == "true":
        return True, True
    if normalized == "DEBUG":
        return True, False
    if normalized == "!DEBUG":
        return False, True
    return False, False


def _source_analysis(text: str) -> _SourceAnalysis:
    code_text, string_text = _lex_source(text)
    code_lines = code_text.splitlines()
    string_lines = string_text.splitlines()
    if len(code_lines) != len(string_lines):
        raise CheckFailure("source lexer changed the number of lines")

    stack: list[_ConditionalFrame] = []
    lines: list[_SourceLine] = []
    directives: list[_ConditionalDirective] = []

    def current_state() -> tuple[bool, bool]:
        if not stack:
            return True, True
        frame = stack[-1]
        return frame.current_debug, frame.current_release

    for code_line, string_line in zip(code_lines, string_lines):
        directive = _parse_directive(code_line)
        debug_active, release_active = current_state()
        if directive is None:
            lines.append(
                _SourceLine(code_line, string_line, debug_active, release_active)
            )
            continue

        kind, expression = directive
        if kind == "if":
            assert expression is not None
            directives.append(
                _ConditionalDirective(kind, expression, debug_active, release_active)
            )
            branch_debug, branch_release = _condition_value(expression)
            stack.append(
                _ConditionalFrame(
                    parent_debug=debug_active,
                    parent_release=release_active,
                    taken_debug=branch_debug,
                    taken_release=branch_release,
                    current_debug=debug_active and branch_debug,
                    current_release=release_active and branch_release,
                )
            )
        elif kind == "elseif":
            assert expression is not None
            if not stack:
                raise CheckFailure("#elseif without #if")
            frame = stack[-1]
            if frame.saw_else:
                raise CheckFailure("#elseif after #else")
            directives.append(
                _ConditionalDirective(
                    kind,
                    expression,
                    frame.parent_debug,
                    frame.parent_release,
                )
            )
            branch_debug, branch_release = _condition_value(expression)
            frame.current_debug = (
                frame.parent_debug and not frame.taken_debug and branch_debug
            )
            frame.current_release = (
                frame.parent_release and not frame.taken_release and branch_release
            )
            frame.taken_debug = frame.taken_debug or branch_debug
            frame.taken_release = frame.taken_release or branch_release
        elif kind == "else":
            if not stack:
                raise CheckFailure("#else without #if")
            frame = stack[-1]
            if frame.saw_else:
                raise CheckFailure("duplicate #else")
            directives.append(
                _ConditionalDirective(
                    kind,
                    "",
                    frame.parent_debug,
                    frame.parent_release,
                )
            )
            frame.current_debug = frame.parent_debug and not frame.taken_debug
            frame.current_release = frame.parent_release and not frame.taken_release
            frame.taken_debug = True
            frame.taken_release = True
            frame.saw_else = True
        elif kind == "endif":
            if not stack:
                raise CheckFailure("#endif without #if")
            stack.pop()
            directives.append(
                _ConditionalDirective(kind, "", debug_active, release_active)
            )
        else:
            raise AssertionError(f"unknown directive kind {kind!r}")
        lines.append(_SourceLine(code_line, string_line, debug_active, release_active))

    if stack:
        raise CheckFailure("unterminated conditional-compilation block")
    return _SourceAnalysis(tuple(lines), tuple(directives))


def _source_text_for_marker(line: _SourceLine, marker: str) -> str:
    if marker in SOURCE_STRING_MARKERS:
        return line.strings
    return line.code


def _check_source(path: Path, markers: tuple[str, ...]) -> None:
    analysis = _source_analysis(path.read_text(encoding="utf-8"))
    for line_number, line in enumerate(analysis.lines, start=1):
        if not line.release_active:
            continue
        for marker in markers:
            source = _source_text_for_marker(line, marker)
            if re.search(marker, source):
                raise CheckFailure(
                    f"{_display_path(path)}:{line_number}: {marker!r} "
                    "is not behind #if DEBUG"
                )


def _check_required_source(path: Path, required: tuple[str, ...]) -> None:
    analysis = _source_analysis(path.read_text(encoding="utf-8"))
    for marker in required:
        if marker.startswith("#if "):
            expression = marker[4:].strip()
            if not any(
                directive.kind == "if"
                and directive.expression == expression
                and directive.parent_debug
                and directive.parent_release
                for directive in analysis.directives
            ):
                raise CheckFailure(f"{_display_path(path)} is missing {marker!r}")
            continue
        matching = [
            line_number
            for line_number, line in enumerate(analysis.lines, start=1)
            if line.debug_active
            and not line.release_active
            and marker in _source_text_for_marker(line, marker)
        ]
        if not matching:
            raise CheckFailure(
                f"{_display_path(path)} is missing active Debug evidence {marker!r}"
            )


def _check_release_source(path: Path, required: tuple[str, ...]) -> None:
    analysis = _source_analysis(path.read_text(encoding="utf-8"))
    for marker in required:
        matching = [
            line_number
            for line_number, line in enumerate(analysis.lines, start=1)
            if line.release_active and marker in line.code
        ]
        if not matching:
            raise CheckFailure(f"{_display_path(path)} is missing {marker!r}")


def _release_source_digest(root: Path = ROOT) -> str:
    try:
        return release_source_digest(root)
    except (OSError, ValueError) as error:
        raise CheckFailure(str(error)) from error


def _check_release_source_digest(root: Path = ROOT) -> None:
    actual_digest = _release_source_digest(root)
    if actual_digest != APPROVED_RELEASE_SOURCE_DIGEST:
        raise CheckFailure(
            "Release build source digest differs from the expected checkout "
            f"(expected {APPROVED_RELEASE_SOURCE_DIGEST}, got {actual_digest})"
        )


def _check_release_build_phase_configuration() -> None:
    spec_path = ROOT / "ios/project.yml"
    project_path = ROOT / "ios/FleetNotifier.xcodeproj/project.pbxproj"
    try:
        spec = spec_path.read_text(encoding="utf-8")
        project = project_path.read_text(encoding="utf-8")
    except OSError as error:
        raise CheckFailure(
            f"cannot read Release build phase configuration: {error}"
        ) from error

    for input_path in RELEASE_BUILD_INPUTS:
        quoted = f'"{input_path}"'
        if quoted not in spec or quoted not in project:
            raise CheckFailure(
                f"Release build phase is missing explicit input {input_path!r}"
            )
    quoted_output = f'"{RELEASE_BUILD_OUTPUT}"'
    if quoted_output not in spec or quoted_output not in project:
        raise CheckFailure("Release build phase is missing its declared output")
    for quoted_script_path in (
        '"$SRCROOT/embed-release-source-digest.py"',
        '"$SRCROOT/.."',
        '"$DERIVED_FILE_DIR/corral-release-source-digest"',
    ):
        if quoted_script_path not in spec:
            raise CheckFailure(
                f"Release build phase does not quote path {quoted_script_path}"
            )


def _run_checked(command: list[str]) -> str:
    try:
        return subprocess.run(
            command,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError) as error:
        raise CheckFailure(
            f"validation command {' '.join(command)!r} failed: {error}"
        ) from error


def _validate_macho(binary: Path) -> None:
    file_description = _run_checked(["file", "-b", str(binary)])
    if "Mach-O" not in file_description or "executable" not in file_description:
        raise CheckFailure(
            f"{binary} is not a Mach-O executable: {file_description.strip()!r}"
        )

    load_commands = _run_checked(["otool", "-l", str(binary)])
    if "LC_BUILD_VERSION" not in load_commands:
        raise CheckFailure(f"{binary} has no LC_BUILD_VERSION platform metadata")
    platforms = set(
        re.findall(r"^\s*platform\s+(\d+)\s*$", load_commands, re.MULTILINE)
    )
    if not platforms or not platforms.issubset({"2", "7"}):
        raise CheckFailure(
            f"{binary} is not an iOS/iOS Simulator Mach-O (platforms: {sorted(platforms)})"
        )


def _binary_blob(binary: Path) -> str:
    strings = _run_checked(["strings", "-a", str(binary)])
    symbols = _run_checked(["nm", "-a", str(binary)])
    return f"{strings}\n{symbols}"


def _binary_architectures(binary: Path) -> tuple[str, ...]:
    architectures = tuple(_run_checked(["lipo", "-archs", str(binary)]).split())
    if not architectures:
        raise CheckFailure(f"{binary} has no lipo architecture slices")
    return architectures


def _check_binary_slice(binary: Path, architecture: str) -> None:
    try:
        _validate_macho(binary)
        _check_binary_text(_binary_blob(binary))
    except CheckFailure as error:
        raise CheckFailure(f"{binary} [{architecture}]: {error}") from error


def _check_binary(binary: Path) -> None:
    if not binary.is_file():
        raise CheckFailure(f"Release executable does not exist: {binary}")
    architectures = _binary_architectures(binary)
    with tempfile.TemporaryDirectory(prefix="corral-release-demo-slices-") as directory:
        slice_root = Path(directory)
        is_non_fat = _run_checked(["lipo", "-info", str(binary)]).startswith(
            "Non-fat file:"
        )
        for index, architecture in enumerate(architectures):
            slice_path = slice_root / f"slice-{index}"
            if is_non_fat:
                _run_checked(["cp", str(binary), str(slice_path)])
            else:
                _run_checked(
                    [
                        "lipo",
                        "-thin",
                        architecture,
                        str(binary),
                        "-output",
                        str(slice_path),
                    ]
                )
            if not slice_path.is_file():
                raise CheckFailure(
                    f"lipo did not produce {architecture} slice for {binary}"
                )
            _check_binary_slice(slice_path, architecture)


def _expect_failure(action: Callable[[], None], label: str) -> None:
    try:
        action()
    except CheckFailure:
        return
    raise CheckFailure(f"self-test fixture was accepted: {label}")


def _typecheck_swift_fixture(path: Path, debug: bool) -> None:
    command = ["swiftc", "-typecheck", "-parse-as-library"]
    if debug:
        command.extend(("-D", "DEBUG"))
    command.append(str(path))
    _run_checked(command)


def _valid_binary_fixture() -> str:
    return "\n".join(BINARY_REQUIRED) + "\n"


def _compile_macho_fixture(
    root: Path,
    name: str,
    sdk: str,
    architecture: str,
    markers: tuple[str, ...],
) -> Path:
    source = root / f"{name}.c"
    binary = root / name
    evidence = ["fixture-sentinel", *markers]
    literals = ",\n".join(f"    {json.dumps(marker)}" for marker in evidence)
    source.write_text(
        "#include <stddef.h>\n"
        "__attribute__((used)) static const char *evidence[] = {\n"
        f"{literals}\n"
        "};\n"
        "int main(void) { return evidence[0] == NULL; }\n",
        encoding="utf-8",
    )
    sdk_path = _run_checked(["xcrun", "--sdk", sdk, "--show-sdk-path"]).strip()
    if not sdk_path:
        raise CheckFailure(f"xcrun returned no SDK path for {sdk}")
    minimum_version = (
        "-mmacosx-version-min=13.0" if sdk == "macosx" else "-mios-version-min=17.0"
    )
    compiler = _run_checked(["xcrun", "--sdk", sdk, "--find", "clang"]).strip()
    if not compiler:
        raise CheckFailure(f"xcrun returned no clang for {sdk}")
    _run_checked(
        [
            compiler,
            "-isysroot",
            sdk_path,
            "-arch",
            architecture,
            minimum_version,
            str(source),
            "-o",
            str(binary),
        ]
    )
    return binary


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

        elseif_branch = (
            "#if false\n"
            "let hidden = enterDemo()\n"
            "#elseif DEBUG\n"
            "let debug = enterDemo()\n"
            "#else\n"
            "let release = enterDemo()\n"
            "#endif\n"
        )
        analysis = _source_analysis(elseif_branch)
        active_lines = {
            line.code.strip(): (line.debug_active, line.release_active)
            for line in analysis.lines
            if "enterDemo" in line.code
        }
        if active_lines.get("let hidden = enterDemo()") != (False, False):
            raise CheckFailure("#if false branch was treated as active")
        if active_lines.get("let debug = enterDemo()") != (True, False):
            raise CheckFailure("#elseif DEBUG branch was not isolated")
        if active_lines.get("let release = enterDemo()") != (False, True):
            raise CheckFailure("#else branch was not isolated")

        true_branch = (
            "#if true\n"
            "let always = alwaysValue\n"
            "#elseif !DEBUG\n"
            "let release_only = releaseValue\n"
            "#else\n"
            "let unreachable = unreachableValue\n"
            "#endif\n"
        )
        true_analysis = _source_analysis(true_branch)
        true_lines = {
            line.code.strip(): (line.debug_active, line.release_active)
            for line in true_analysis.lines
            if "Value" in line.code
        }
        if true_lines.get("let always = alwaysValue") != (True, True):
            raise CheckFailure("#if true branch was not active for both builds")
        if true_lines.get("let release_only = releaseValue") != (False, False):
            raise CheckFailure("#elseif after #if true was treated as active")
        if true_lines.get("let unreachable = unreachableValue") != (False, False):
            raise CheckFailure("#else after a taken #if true was treated as active")

        not_debug_branch = (
            "#if false\n"
            "let hidden_again = hiddenValue\n"
            "#elseif !DEBUG\n"
            "let release_only = releaseValue\n"
            "#else\n"
            "let debug_only = debugValue\n"
            "#endif\n"
        )
        not_debug_analysis = _source_analysis(not_debug_branch)
        not_debug_lines = {
            line.code.strip(): (line.debug_active, line.release_active)
            for line in not_debug_analysis.lines
            if "Value" in line.code
        }
        if not_debug_lines.get("let release_only = releaseValue") != (False, True):
            raise CheckFailure("#elseif !DEBUG branch was not Release-only")
        if not_debug_lines.get("let debug_only = debugValue") != (True, False):
            raise CheckFailure("#else after #elseif !DEBUG was not Debug-only")

        block_comment = root / "block-comment.swift"
        block_comment.write_text(
            "/* func enterDemo() */\n/* #if DEBUG */\n", encoding="utf-8"
        )
        _check_source(block_comment, (r"enterDemo",))
        _expect_failure(
            lambda: _check_required_source(block_comment, ("func enterDemo",)),
            "marker only in a block comment",
        )
        _expect_failure(
            lambda: _check_required_source(block_comment, ("#if DEBUG",)),
            "conditional marker only in a block comment",
        )

        dead_branch = root / "dead-branch.swift"
        dead_branch.write_text(
            "#if false\nfunc enterDemo() {}\n#endif\n", encoding="utf-8"
        )
        _check_source(dead_branch, (r"enterDemo",))
        _expect_failure(
            lambda: _check_required_source(dead_branch, ("func enterDemo",)),
            "marker only in #if false",
        )

        string_debug = root / "string-debug.swift"
        string_debug.write_text(
            "#if DEBUG\n"
            'let ordinary = "func enterDemo()"\n'
            'let escaped = "func enterDemo() \\"quoted\\""\n'
            'let raw = #"func enterDemo()"#\n'
            'let multiline = """\n'
            "#if DEBUG\n"
            "func enterDemo()\n"
            '"""\n'
            'let rawMultiline = #"""\n'
            "func enterDemo()\n"
            '"""#\n'
            "#endif\n",
            encoding="utf-8",
        )
        _check_source(string_debug, (r"enterDemo",))
        _expect_failure(
            lambda: _check_required_source(string_debug, ("func enterDemo",)),
            "Debug declaration marker only in ordinary/escaped/raw/multiline strings",
        )

        escaped_multiline_debug = root / "escaped-multiline-debug.swift"
        escaped_multiline_debug.write_text(
            r'''#if DEBUG
let spoof = """
prefix \"""
func enterDemo()
middle \"""
suffix
"""
#endif
''',
            encoding="utf-8",
        )
        _typecheck_swift_fixture(escaped_multiline_debug, debug=True)
        _check_source(escaped_multiline_debug, (r"enterDemo",))
        _expect_failure(
            lambda: _check_required_source(
                escaped_multiline_debug, ("func enterDemo",)
            ),
            "escaped ordinary multiline delimiter spoof",
        )

        escaped_raw_debug = root / "escaped-raw-multiline-debug.swift"
        escaped_raw_debug.write_text(
            r'''#if DEBUG
let spoof = #"""
prefix \#"""#
func enterDemo()
middle \#"""#
suffix
"""#
#endif
''',
            encoding="utf-8",
        )
        _typecheck_swift_fixture(escaped_raw_debug, debug=True)
        _check_source(escaped_raw_debug, (r"enterDemo",))
        _expect_failure(
            lambda: _check_required_source(escaped_raw_debug, ("func enterDemo",)),
            "escaped raw multiline delimiter spoof",
        )

        interpolation_debug = root / "interpolation-debug.swift"
        interpolation_debug.write_text(
            r"""#if DEBUG
let fake = "\("func enterDemo()")"
let rawFake = "\(#"func enterDemo()"#)"
let rawPoundFake = "\(##"func enterDemo()"##)"
#endif
""",
            encoding="utf-8",
        )
        _typecheck_swift_fixture(interpolation_debug, debug=True)
        _check_source(interpolation_debug, (r"enterDemo",))
        _expect_failure(
            lambda: _check_required_source(interpolation_debug, ("func enterDemo",)),
            "nested interpolation string Debug spoof",
        )

        interpolation_release = root / "interpolation-release.swift"
        interpolation_release.write_text(
            r"""#if !DEBUG
let fake = "\("model.startLive()")"
let rawFake = "\(#"model.startLive()"#)"
let rawPoundFake = "\(##"model.startLive()"##)"
#endif
""",
            encoding="utf-8",
        )
        _typecheck_swift_fixture(interpolation_release, debug=False)
        _expect_failure(
            lambda: _check_release_source(
                interpolation_release, ("model.startLive()",)
            ),
            "nested interpolation string Release spoof",
        )

        positive_debug = root / "positive-debug.swift"
        positive_debug.write_text(
            "#if DEBUG\nfunc enterDemo() {}\n#endif\n", encoding="utf-8"
        )
        _typecheck_swift_fixture(positive_debug, debug=True)
        _check_required_source(positive_debug, ("#if DEBUG", "func enterDemo"))

        expression_debug = root / "expression-debug.swift"
        expression_debug.write_text(
            r'''#if DEBUG
let rendered = """
\({ () -> String in
    func enterDemo() -> String { "demo" }
    return enterDemo()
}())
"""
#endif
''',
            encoding="utf-8",
        )
        _typecheck_swift_fixture(expression_debug, debug=True)
        _check_required_source(expression_debug, ("func enterDemo",))
        expression_debug_analysis = _source_analysis(
            expression_debug.read_text(encoding="utf-8")
        )
        if not any(
            line.debug_active and not line.release_active and "enterDemo()" in line.code
            for line in expression_debug_analysis.lines
        ):
            raise CheckFailure("Debug interpolation expression code was masked")

        positive_release = root / "positive-release.swift"
        positive_release.write_text(
            "#if !DEBUG\nfunc startLive() {}\n#endif\n", encoding="utf-8"
        )
        _typecheck_swift_fixture(positive_release, debug=False)
        _check_release_source(positive_release, ("func startLive()",))

        expression_release = root / "expression-release.swift"
        expression_release.write_text(
            r"""#if !DEBUG
struct FixtureModel {
    func startLive() -> String { "live" }
}
let model = FixtureModel()
let rendered = "\(model.startLive())"
#endif
""",
            encoding="utf-8",
        )
        _typecheck_swift_fixture(expression_release, debug=False)
        _check_release_source(expression_release, ("model.startLive()",))
        expression_release_analysis = _source_analysis(
            expression_release.read_text(encoding="utf-8")
        )
        if not any(
            line.release_active and "model.startLive()" in line.code
            for line in expression_release_analysis.lines
        ):
            raise CheckFailure("Release interpolation expression code was masked")

        string_release = root / "string-release.swift"
        string_release.write_text(
            "#if !DEBUG\n"
            'let ordinary = "model.startLive()"\n'
            'let escaped = "model.startLive() \\"quoted\\""\n'
            'let raw = #"model.startLive()"#\n'
            'let multiline = """\n'
            "model.startLive()\n"
            '"""\n'
            'let rawMultiline = #"""\n'
            "model.startLive()\n"
            '"""#\n'
            "#endif\n",
            encoding="utf-8",
        )
        _expect_failure(
            lambda: _check_release_source(string_release, ("model.startLive()",)),
            "Release call marker only in ordinary/escaped/raw/multiline strings",
        )

        escaped_multiline_release = root / "escaped-multiline-release.swift"
        escaped_multiline_release.write_text(
            r'''#if !DEBUG
let spoof = """
prefix \"""
model.startLive()
middle \"""
suffix
"""
#endif
''',
            encoding="utf-8",
        )
        _typecheck_swift_fixture(escaped_multiline_release, debug=False)
        _expect_failure(
            lambda: _check_release_source(
                escaped_multiline_release, ("model.startLive()",)
            ),
            "escaped ordinary multiline Release spoof",
        )

        escaped_raw_release = root / "escaped-raw-multiline-release.swift"
        escaped_raw_release.write_text(
            r'''#if !DEBUG
let spoof = #"""
prefix \#"""#
model.startLive()
middle \#"""#
suffix
"""#
#endif
''',
            encoding="utf-8",
        )
        _typecheck_swift_fixture(escaped_raw_release, debug=False)
        _expect_failure(
            lambda: _check_release_source(escaped_raw_release, ("model.startLive()",)),
            "escaped raw multiline Release spoof",
        )

        unguarded_literal = root / "unguarded-demo-literal.swift"
        unguarded_literal.write_text('let banner = "(demo)"\n', encoding="utf-8")
        _expect_failure(
            lambda: _check_source(unguarded_literal, (r"\(demo\)",)),
            "unguarded user-facing demo literal",
        )

        unicode_fixtures = (
            (
                "unicode-ordinary.swift",
                r"""let banner = "\u{28}\u{64}emo)"
""",
            ),
            (
                "unicode-raw.swift",
                r"""let banner = #"\#u{28}\#u{64}emo)"#
""",
            ),
            (
                "unicode-multiline.swift",
                r'''let banner = """
\u{28}\u{64}emo)
"""
''',
            ),
            (
                "unicode-raw-multiline.swift",
                r'''let banner = #"""
\#u{28}\#u{64}emo)
"""#
''',
            ),
        )
        for name, source in unicode_fixtures:
            fixture = root / name
            fixture.write_text(source, encoding="utf-8")
            _typecheck_swift_fixture(fixture, debug=False)
            _expect_failure(
                lambda fixture=fixture: _check_source(fixture, (r"\(demo\)",)),
                f"Unicode-escaped user-facing literal in {name}",
            )

        for scalar in sorted(UNICODE_LINE_SEPARATOR_SCALARS):
            spelling = f"\\u{{{scalar:X}}}"
            fixture = root / f"unicode-line-separator-{scalar:X}.swift"
            fixture.write_text(
                f'#if DEBUG\nlet banner = "{spelling}demo)"\n#endif\n',
                encoding="utf-8",
            )
            _typecheck_swift_fixture(fixture, debug=True)
            analysis = _source_analysis(fixture.read_text(encoding="utf-8"))
            if len(analysis.lines) != 3:
                raise CheckFailure(
                    f"Unicode scalar {spelling} changed source-line alignment"
                )
            if not any(spelling in line.strings for line in analysis.lines):
                raise CheckFailure(
                    f"Unicode scalar {spelling} was not normalized in literal view"
                )
            _check_source(fixture, (r"\(demo\)",))

        malformed_unicode = root / "malformed-unicode.swift"
        malformed_unicode.write_text(
            r"""let banner = "\u{not-a-scalar}"
""",
            encoding="utf-8",
        )
        _expect_failure(
            lambda: _source_analysis(malformed_unicode.read_text(encoding="utf-8")),
            "malformed Unicode scalar escape",
        )

        inactive_guard = root / "inactive-guard.swift"
        inactive_guard.write_text(
            "#if false\n#if DEBUG\nfunc enterDemo() {}\n#endif\n#endif\n",
            encoding="utf-8",
        )
        _check_source(inactive_guard, (r"enterDemo",))
        _expect_failure(
            lambda: _check_required_source(inactive_guard, ("#if DEBUG",)),
            "#if DEBUG nested below inactive #if false",
        )

        _expect_failure(
            lambda: _source_analysis("#if canImport(FakeModule)\n#endif\n"),
            "unsupported conditional expression",
        )
        _expect_failure(
            lambda: _source_analysis("#if UNKNOWN\n#endif\n"),
            "unknown conditional expression",
        )

        modified_source_root = root / "modified checkout with spaces"
        for relative in RELEASE_SOURCE_FILES:
            source = ROOT / relative
            destination = modified_source_root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(source.read_bytes())
        modified_fleet_views = (
            modified_source_root / "ios/FleetNotifier/UI/FleetViews.swift"
        )
        modified_fleet_views.write_text(
            modified_fleet_views.read_text(encoding="utf-8")
            + '\nlet escapedDemoFixture = "\\u{28}demo)"\n',
            encoding="utf-8",
        )
        if (
            _release_source_digest(modified_source_root)
            == APPROVED_RELEASE_SOURCE_DIGEST
        ):
            raise CheckFailure("modified Release source reused the approved digest")
        _expect_failure(
            lambda: _check_release_source_digest(modified_source_root),
            "modified source rejected by the expected Release digest",
        )
        modified_digest = root / "modified release digest with spaces"
        _run_checked(
            [
                sys.executable,
                str(ROOT / "ios/embed-release-source-digest.py"),
                "--source-root",
                str(modified_source_root),
                "--output",
                str(modified_digest),
            ]
        )
        modified_binary = root / "modified release binary with spaces"
        sdk_path = _run_checked(
            ["xcrun", "--sdk", "iphoneos", "--show-sdk-path"]
        ).strip()
        modified_sources = [
            str(modified_source_root / relative) for relative in RELEASE_SOURCE_FILES
        ]
        _run_checked(
            [
                "swiftc",
                "-O",
                "-parse-as-library",
                "-swift-version",
                "5",
                "-target",
                "arm64-apple-ios17.0",
                "-sdk",
                sdk_path,
                *modified_sources,
                "-Xlinker",
                "-sectcreate",
                "-Xlinker",
                "__CORRAL",
                "-Xlinker",
                "__source_digest",
                "-Xlinker",
                str(modified_digest),
                "-o",
                str(modified_binary),
            ]
        )
        public_binary_check = subprocess.run(
            [
                sys.executable,
                str(ROOT / "ios/check-release-demo.py"),
                "--binary",
                str(modified_binary),
            ],
            check=False,
            capture_output=True,
            text=True,
            cwd=ROOT / "ios",
        )
        if public_binary_check.returncode == 0:
            raise CheckFailure(
                "public --binary proof accepted a Mach-O built from modified sources "
                "using the declared generator"
            )
        if "lacks real-path marker" not in public_binary_check.stderr:
            raise CheckFailure(
                "modified-source binary failure did not come from source-digest "
                "marker evidence: "
                f"{public_binary_check.stderr.strip()}"
            )

        for marker in BINARY_REQUIRED:
            fixture = _valid_binary_fixture().replace(marker, "")
            _expect_failure(
                lambda fixture=fixture: _check_binary_text(fixture),
                f"missing real-path marker {marker!r}",
            )

        _expect_failure(
            lambda: _check_binary_text(_valid_binary_fixture() + "Demo mode\n"),
            "release user-facing demo string",
        )
        _check_binary_text(_valid_binary_fixture())

        plain = root / "plain-text"
        plain.write_text(_valid_binary_fixture(), encoding="utf-8")
        _expect_failure(lambda: _check_binary(plain), "non-Mach-O binary input")

        release_slice = _compile_macho_fixture(
            root, "release-slice", "iphoneos", "arm64", BINARY_REQUIRED
        )
        _check_binary(release_slice)

        debug_slice = _compile_macho_fixture(
            root,
            "debug-slice",
            "iphoneos",
            "arm64",
            (*BINARY_REQUIRED, "Demo mode"),
        )
        _expect_failure(lambda: _check_binary(debug_slice), "Debug Mach-O slice")

        macos_slice = _compile_macho_fixture(
            root, "macos-slice", "macosx", "x86_64", BINARY_REQUIRED
        )
        _expect_failure(lambda: _check_binary(macos_slice), "macOS Mach-O slice")

        ios_without_markers = _compile_macho_fixture(
            root, "ios-without-markers", "iphoneos", "arm64", ()
        )
        _expect_failure(
            lambda: _check_binary(ios_without_markers),
            "iOS Mach-O slice missing independent real-path evidence",
        )
        mixed = root / "mixed-macos-ios"
        _run_checked(
            [
                "lipo",
                "-create",
                str(macos_slice),
                str(ios_without_markers),
                "-output",
                str(mixed),
            ]
        )
        if set(_binary_architectures(mixed)) != {"arm64", "x86_64"}:
            raise CheckFailure(
                "mixed self-test artifact is not a two-architecture universal"
            )
        _expect_failure(
            lambda: _check_binary(mixed),
            "universal artifact with real markers only in macOS slice",
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
        _check_release_build_phase_configuration()
        _check_release_source_digest()
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
