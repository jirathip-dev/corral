#!/usr/bin/env python3
"""Prove that the Release iOS app has no Debug demo entrypoint.

The source checks make the intended conditional-compilation boundary explicit;
the binary checks make the check build-derived rather than a substring-only
review of Swift source.  Debug still owns the demo fixtures and tests, while a
Release app must retain the real registration/SSE/drive client and error paths
in every architecture slice.

Source proof is intentionally limited to the supported conditional expressions
``true``, ``false``, ``DEBUG``, and ``!DEBUG``.  Comments and inactive branches
are not evidence, and any other conditional expression fails closed.  Binary
proof requires every architecture slice to be an iOS or iOS Simulator Mach-O
before inspecting that slice's strings and symbols independently.
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

SUPPORTED_CONDITIONS = {"true", "false", "DEBUG", "!DEBUG"}

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
    state = "normal"
    block_depth = 0
    string_closer: str | None = None
    string_hashes = 0
    index = 0

    while index < len(text):
        character = text[index]
        if state == "normal":
            if text.startswith("//", index):
                code.extend((" ", " "))
                strings.extend((" ", " "))
                index += 2
                state = "line-comment"
                continue
            if text.startswith("/*", index):
                code.extend((" ", " "))
                strings.extend((" ", " "))
                index += 2
                block_depth = 1
                state = "block-comment"
                continue

            hash_index = index
            while hash_index < len(text) and text[hash_index] == "#":
                hash_index += 1
            if (
                hash_index < len(text)
                and text[hash_index] == '"'
                and hash_index > index
            ):
                hashes = text[index:hash_index]
                if text.startswith('"""', hash_index):
                    opening = f'{hashes}"""'
                    string_closer = f'"""{hashes}'
                else:
                    opening = f'{hashes}"'
                    string_closer = f'"{hashes}'
                code.extend(_blank(item) for item in opening)
                strings.extend(opening)
                index += len(opening)
                string_hashes = len(hashes)
                state = "string"
                continue
            if text.startswith('"""', index):
                code.extend((" ", " ", " "))
                strings.extend('"""')
                index += 3
                string_closer = '"""'
                string_hashes = 0
                state = "string"
                continue
            if character == '"':
                code.append(" ")
                strings.append(character)
                index += 1
                string_closer = '"'
                string_hashes = 0
                state = "string"
                continue

            code.append(character)
            strings.append(character)
            index += 1
            continue

        if state == "line-comment":
            code.append(_blank(character))
            strings.append(_blank(character))
            index += 1
            if character in "\r\n":
                state = "normal"
            continue

        if state == "block-comment":
            if text.startswith("/*", index):
                code.extend((" ", " "))
                strings.extend((" ", " "))
                index += 2
                block_depth += 1
                continue
            if text.startswith("*/", index):
                code.extend((" ", " "))
                strings.extend((" ", " "))
                index += 2
                block_depth -= 1
                if block_depth == 0:
                    state = "normal"
                continue
            code.append(_blank(character))
            strings.append(_blank(character))
            index += 1
            continue

        if state == "string":
            assert string_closer is not None
            escape_width = _string_escape_width(
                text, index, string_hashes, string_closer
            )
            if escape_width:
                escaped_text = text[index : index + escape_width]
                code.extend(_blank(item) for item in escaped_text)
                strings.extend(escaped_text)
                index += escape_width
                continue
            if text.startswith(string_closer, index):
                closing = string_closer
                code.extend(_blank(item) for item in closing)
                strings.extend(closing)
                index += len(closing)
                string_closer = None
                string_hashes = 0
                state = "normal"
                continue
            code.append(_blank(character))
            strings.append(character)
            index += 1
            continue

        raise AssertionError(f"unknown lexer state {state!r}")

    if state == "block-comment":
        raise CheckFailure("unterminated Swift block comment")
    if state == "string":
        raise CheckFailure("unterminated Swift string literal")
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
